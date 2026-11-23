use super::*;

/// Echo cancellation runs unless `IRONTEAMS_NO_AEC` is set to anything but `0` (the GUI's
/// `call join --no-aec`, for comparing both live) or the user turned it off in Settings ›
/// Devices; noise suppression follows its own switch there.
pub(super) fn processing_wanted() -> ProcessingOptions {
    let prefs = session::Paths::new()
        .map(|p| crate::backend::settings::Local::load(&p.config_dir).devices)
        .unwrap_or_default();
    processing_from(&prefs, compat::var_os("IRONTEAMS_NO_AEC").as_deref())
}

/// The speaker volume kept in Settings (the call window's slider), applied to the call's
/// playback each time the devices open.
pub(super) fn apply_saved_volume() {
    let prefs = session::Paths::new()
        .map(|p| crate::backend::settings::Local::load(&p.config_dir).devices)
        .unwrap_or_default();
    calling::audio::set_output_volume(prefs.speaker_volume);
}

pub(super) fn processing_from(
    prefs: &devices::DevicePrefs,
    no_aec: Option<&std::ffi::OsStr>,
) -> ProcessingOptions {
    ProcessingOptions {
        echo_cancellation: prefs.echo_cancellation && aec_wanted_by(no_aec),
        noise_suppression: prefs.noise_suppression,
    }
}

/// The conversation controller a 1:1 call's conversation push names (`conversationController`).
/// A 1:1 call starts without one, and the controls (mute) need it: `updateEndpointState` sits
/// under it. Only an https URL counts.
pub(super) fn conversation_controller(body: &serde_json::Value) -> Option<String> {
    body.get("conversationController")
        .and_then(serde_json::Value::as_str)
        .filter(|u| u.starts_with("https://"))
        .map(str::to_owned)
}

pub(super) fn aec_wanted_by(no_aec: Option<&std::ffi::OsStr>) -> bool {
    no_aec.is_none_or(|v| v.is_empty() || v == "0")
}

/// The microphone path of every call (meetings and adopted 1:1 calls), as `ironteams-cli call join
/// --mic --speaker` runs it: the capture through AEC3, noise suppression and AGC2, then Opus.
/// With either switch on, the speaker must report each played frame to the returned reference
/// (its delay to the microphone is estimated inside from the devices' measured latency).
pub(super) fn mic_chain<S: calling::audio::FrameSource + 'static>(
    capture: S,
    options: ProcessingOptions,
) -> (
    mpsc::Receiver<OpusPacket>,
    Option<calling::audio::EchoReference>,
) {
    if options.echo_cancellation || options.noise_suppression {
        let (mic, reference) = echo_cancelled(capture, AudioProcessor::with_options(options));
        (spawn_capture_encoder(mic), Some(reference))
    } else {
        (spawn_capture_encoder(capture), None)
    }
}

/// The audio of one call: microphone to the current media sender, incoming packets to the
/// speaker. Restartable for a device change; the media session can be swapped under it.
pub(super) struct Audio {
    pub(super) sender: watch::Receiver<AudioSender>,
    pub(super) muted: Arc<AtomicBool>,
    /// On hold: nothing is sent or played.
    pub(super) held: Arc<AtomicBool>,
    pub(super) play: watch::Sender<Option<mpsc::Sender<OpusPacket>>>,
    pub(super) send_task: Option<JoinHandle<()>>,
    pub(super) playback: Option<JoinHandle<calling::Result<()>>>,
}

impl Audio {
    /// (Re)opens the devices: the mic through the echo canceller, whose reference is what the
    /// speaker plays. Without a microphone the speaker still plays.
    pub(super) fn start(&mut self, mic: Option<&str>, speaker: Option<&str>) -> anyhow::Result<()> {
        self.stop();
        apply_saved_volume();
        let capture = match Capture::start(mic) {
            Ok(c) => c,
            Err(e) => {
                let out = Playback::start(speaker).context("speaker")?;
                self.play_into(out);
                return Err(anyhow::Error::from(e).context("microphone"));
            }
        };
        let (mut encoded, reference) = mic_chain(capture, processing_wanted());
        let out = match reference {
            Some(r) => Playback::start_with_tap(speaker, move |f| r.push(f)),
            None => Playback::start(speaker),
        }
        .context("speaker")?;
        self.play_into(out);
        let (sender, muted, held) = (self.sender.clone(), self.muted.clone(), self.held.clone());
        self.send_task = Some(tokio::spawn(async move {
            while let Some(p) = encoded.recv().await {
                let (m, h) = (muted.load(Ordering::Relaxed), held.load(Ordering::Relaxed));
                let Some(p) = outgoing(p, m, h) else {
                    continue;
                };
                let s = sender.borrow().clone();
                let _ = s.send(p).await;
            }
        }));
        Ok(())
    }

    pub(super) fn play_into(&mut self, out: Playback) {
        let (tx, rx) = mpsc::channel(PLAY_QUEUE);
        self.playback = Some(spawn_decoder_playback(rx, out));
        self.play.send_replace(Some(tx));
    }

    pub(super) fn stop(&mut self) {
        // The playback ends once its queue closes; the encoder once its reader is gone.
        self.play.send_replace(None);
        self.playback.take();
        if let Some(t) = self.send_task.take() {
            t.abort();
        }
    }
}

/// What goes out for one captured packet. Muting is also local: the microphone's packet is
/// replaced by digital silence, so no captured audio leaves while the media keeps flowing (a call
/// that sends no RTP at all is ended by the service as `MediaConnectivityFailure`, and without
/// RTP no RTCP sender report goes out either). On hold every line is `inactive`: nothing goes.
pub(super) fn outgoing(packet: OpusPacket, muted: bool, held: bool) -> Option<OpusPacket> {
    if held {
        None
    } else if muted {
        Some(OPUS_SILENCE.to_vec())
    } else {
        Some(packet)
    }
}

/// Moves one media session's incoming packets to whatever playback is current (none on hold).
pub(super) fn forward(
    play: watch::Receiver<Option<mpsc::Sender<OpusPacket>>>,
    held: Arc<AtomicBool>,
    mut incoming: mpsc::Receiver<OpusPacket>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(p) = incoming.recv().await {
            if held.load(Ordering::Relaxed) {
                continue;
            }
            let tx = play.borrow().clone();
            if let Some(tx) = tx {
                let _ = tx.send(p).await;
            }
        }
    })
}

pub(super) fn link_of(st: MediaState) -> Link {
    match st {
        MediaState::New => Link::New,
        MediaState::Connecting => Link::Connecting,
        MediaState::Connected => Link::Connected,
        MediaState::Disconnected | MediaState::Failed | MediaState::Closed => Link::Disconnected,
    }
}
