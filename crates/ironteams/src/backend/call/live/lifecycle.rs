use super::*;

impl Call {
    pub(super) fn start_keep_alive(&mut self, interval_s: Option<u64>) {
        if self.keep_alive.is_some() {
            return;
        }
        let (ngc, s, l) = (self.ngc.clone(), self.s.clone(), self.links.clone());
        self.keep_alive = Some(tokio::spawn(async move {
            if let Err(e) = ngc.run_keep_alive(&s, &l, interval_s).await {
                tracing::warn!(%e, "keepAlive stopped");
            }
        }));
    }

    /// Applies the media answer once; true when it was applied now.
    pub(super) async fn apply_answer(&mut self, answer: &SdpBlob) -> bool {
        if self.answered {
            return false;
        }
        match answer.decode() {
            Ok(sdp) => match self.media.set_answer(&sdp).await {
                Ok(()) => {
                    if !self.meeting {
                        let theirs = calling::media::sdp::directions(&sdp);
                        tracing::info!(%theirs, "1:1 media answer applied");
                    }
                    self.answered = true;
                    self.video
                        .negotiated(&self.media, &sdp, remote_video::Phase::Join)
                        .await;
                    return true;
                }
                Err(e) => tracing::warn!(%e, "media answer refused"),
            },
            Err(e) => tracing::warn!(%e, "undecodable media answer"),
        }
        false
    }

    pub(super) async fn control(&mut self, c: Control, audio: &mut Audio, sink: &EventSink) {
        let (s, links, controller) = (&self.s, &self.links, self.controller.as_str());
        match c {
            Control::Mute(m) => {
                audio.muted.store(m, Ordering::Relaxed);
                if let Err(e) = self
                    .ngc
                    .set_muted(s, links, controller, &mut self.controls, m)
                    .await
                {
                    tracing::warn!(%e, "mute state not published");
                    sink.send(Event::Error(
                        "Others may not see your mute state change.".into(),
                    ));
                }
            }
            Control::RaiseHand => {
                if let Err(e) = self
                    .ngc
                    .raise_hand(s, links, controller, &mut self.controls)
                    .await
                {
                    tracing::warn!(%e, "raise hand failed");
                    sink.send(Event::Error("Couldn't raise your hand.".into()));
                    sink.send(Event::Hand(false));
                }
            }
            Control::LowerHand => {
                let own = self.roster.hand_of(&s.me.mri).map(|h| h.state_id.clone());
                if let Err(e) = self
                    .ngc
                    .lower_hand(s, links, controller, &mut self.controls, own.as_deref())
                    .await
                {
                    tracing::warn!(%e, "lower hand failed");
                    sink.send(Event::Error("Couldn't lower your hand.".into()));
                    sink.send(Event::Hand(true));
                }
            }
            Control::React(name) => match Reaction::parse(name) {
                // The error names the status and the start of the service's answer.
                Some(r) => {
                    if let Err(e) = self.ngc.send_reaction(s, links, controller, r).await {
                        tracing::warn!(%e, "reaction not sent");
                        sink.send(Event::Error("Couldn't send your reaction.".into()));
                    }
                }
                None => tracing::warn!(name, "reaction not sent: not a live reaction"),
            },
            Control::Devices { mic, speaker } => {
                if let Err(e) = audio.start(mic.as_deref(), speaker.as_deref()) {
                    sink.send(Event::Error(format!("Audio device: {e:#}")));
                }
            }
            // Share and Camera are handled by the run loop, which owns their channels.
            Control::Share(_) | Control::Camera(_) | Control::Remote(_) | Control::Leave => {}
            // Before the call runs (an early control): kept for the first subscriptions.
            Control::VideoView(view) => {
                self.video.set_view(view);
            }
            Control::Captions(_) | Control::Recording(_) | Control::Transcription(_) => {
                self.bots.ask(&c);
                self.step_bots(sink).await;
            }
            Control::Hold(on) => self.hold(on, audio, sink).await,
            Control::Sound(off) => calling::audio::set_output_off(off),
            Control::AddPerson { mri, name } => {
                if let Err(e) = self
                    .ngc
                    .add_participant(s, links, controller, &mri, &name)
                    .await
                {
                    tracing::warn!(%e, "add person failed");
                    sink.send(Event::Error(format!("Couldn't add {name} to the call.")));
                }
            }
            Control::Admit { mri, name } => self.admit(&[(mri, name)], sink).await,
            Control::AdmitAll => {
                let waiting: Vec<(String, String)> = self
                    .roster
                    .participants()
                    .iter()
                    .filter(|p| p.presence == Presence::Lobby)
                    .map(|p| (p.mri.clone(), p.display_name.clone()))
                    .collect();
                self.admit(&waiting, sink).await;
            }
            Control::Transfer { mri, name } => match self.ngc.transfer(s, links, &mri).await {
                Ok(()) => self.transferring = Some(name),
                Err(e) => {
                    tracing::warn!(%e, "transfer failed");
                    sink.send(Event::Error(format!(
                        "Couldn't transfer the call to {name}."
                    )));
                }
            },
        }
    }

    /// Lets these people in from the lobby, one `admit` each (the service's `admitAll` leaves
    /// anonymous guests waiting). The first refusal stops and shows as the call's error.
    pub(super) async fn admit(&self, people: &[(String, String)], sink: &EventSink) {
        let (s, links, controller) = (&self.s, &self.links, self.controller.as_str());
        for (mri, name) in people {
            if let Err(e) = self.ngc.admit(s, links, controller, mri, name).await {
                tracing::warn!(%e, "admit failed");
                sink.send(Event::Error(if people.len() > 1 {
                    "Couldn't admit everyone from the lobby.".into()
                } else {
                    format!("Couldn't admit {name}.")
                }));
                return;
            }
        }
    }

    /// Lets the recorder bot's side send what it can now.
    pub(super) async fn step_bots(&mut self, sink: &EventSink) {
        let (s, links, controller) = (&self.s, &self.links, self.controller.as_str());
        self.bots.step(&self.ngc, s, links, controller, sink).await;
    }

    /// Hold (`on`) or resume: silence our side at once, then re-offer the media on the same
    /// peer connection, every line `inactive` to hold (see [`calling::signaling::call_ops`]).
    /// A refused re-offer puts the hold back as it was.
    pub(super) async fn hold(&mut self, on: bool, audio: &mut Audio, sink: &EventSink) {
        // A re-offer now would go on the session about to be replaced (rusty-teams also waits).
        if self.next.is_some() {
            tracing::info!(
                hold = on,
                "hold not sent: the media is moving to a new session"
            );
            sink.send(Event::Held(!on));
            sink.send(Event::Error(
                "The call is reconnecting its media. Try again in a moment.".into(),
            ));
            return;
        }
        audio.held.store(on, Ordering::Relaxed);
        let sent = async {
            let ours = self.media.create_offer().await?;
            let signaled = if on { hold_offer(&ours) } else { ours.clone() };
            let offered = calling::media::sdp::directions(&signaled);
            let transport = calling::media::sdp::transport_summary(&signaled);
            let inline = self
                .ngc
                .renegotiate(&self.s, &self.links, &MODALITIES, &SdpBlob::ngc(signaled))
                .await?;
            tracing::info!(
                hold = on,
                inline = inline.is_some(),
                %offered,
                %transport,
                "1:1 hold re-offer sent"
            );
            anyhow::Ok((ours, inline))
        }
        .await;
        match sent {
            Ok((ours, Some(answer))) => {
                self.apply_reoffer_answer(&answer, &ours, &Links::default(), sink)
                    .await
            }
            Ok((ours, None)) => {
                self.pending_offer = Some(ours);
                self.reoffer = Some(Reoffer::Hold(on));
            }
            Err(e) => {
                tracing::warn!("hold {on} failed: {e:#}");
                self.refused(Reoffer::Hold(on), audio, sink).await;
            }
        }
    }

    /// Puts back what our refused re-offer changed (`call/rejection`, or its request failed):
    /// webrtc-rs's pending offer is rolled back, so the session can answer the other side's
    /// next offer, and the hold, camera or share is undone. The call goes on.
    pub(super) async fn refused(&mut self, reoffer: Reoffer, audio: &Audio, sink: &EventSink) {
        if let Err(e) = self.media.rollback_offer().await {
            tracing::warn!(%e, "refused re-offer not rolled back");
        }
        self.pending_offer = None;
        self.answered = true;
        match reoffer {
            Reoffer::Hold(on) => audio.held.store(!on, Ordering::Relaxed),
            Reoffer::Camera(true) => {
                self.camera.stop(sink).await;
            }
            Reoffer::Share(true) => {
                self.share.stop().await;
                share::leave_session(&self.ngc, &self.s, &mut self.share).await;
            }
            Reoffer::Camera(false) | Reoffer::Share(false) => {}
        }
        for ev in reoffer.refused() {
            sink.send(ev);
        }
    }

    /// Applies the answer to our hold or resume re-offer `ours`, with our directions mirrored
    /// back so the media keeps running locally, and acknowledges it.
    pub(super) async fn apply_reoffer_answer(
        &mut self,
        answer: &SdpBlob,
        ours: &str,
        links: &Links,
        sink: &EventSink,
    ) {
        let applied = match answer.decode() {
            Ok(sdp) => {
                tracing::info!(
                    theirs = %calling::media::sdp::directions(&sdp),
                    transport = %calling::media::sdp::transport_summary(&sdp),
                    "1:1 hold answer"
                );
                self.media
                    .set_answer(&keep_media_running(&sdp, ours))
                    .await
                    .map_err(anyhow::Error::from)
            }
            Err(e) => Err(anyhow::Error::from(e)),
        };
        if let Err(e) = applied {
            tracing::warn!("hold answer not applied: {e:#}");
            sink.send(Event::Error("The call's media update was refused.".into()));
            return;
        }
        if let Some(url) = links.get("mediaAcknowledgement")
            && let Err(e) = self.ngc.acknowledge_media(&self.s, url).await
        {
            tracing::warn!(%e, "media answer not acknowledged");
        }
    }
}
