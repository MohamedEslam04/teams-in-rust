use super::*;

/// Holds at most one active call and drives the call window.
#[derive(Clone)]
pub struct CallManager {
    pub(super) shared: Arc<Shared>,
    #[cfg(feature = "calls")]
    pub(super) hub: Arc<live::Hub>,
}

/// Used when IronTeams is built without the `calls` feature: nothing joins natively.
#[cfg(any(test, not(feature = "calls")))]
pub(super) struct Unavailable;

#[cfg(any(test, not(feature = "calls")))]
impl CallDriver for Unavailable {
    fn native(&self) -> bool {
        false
    }

    fn devices(&self) -> Devices {
        Devices::default()
    }

    fn join(&self, _: JoinTarget, _: JoinOptions, sink: EventSink) -> Box<dyn CallHandle> {
        sink.send(Event::Error(
            "Calling isn't built into this version of IronTeams.".into(),
        ));
        sink.send(Event::Ended("Couldn't join.".into()));
        Box::new(NoHandle)
    }
}

/// A handle with nothing behind it.
pub(super) struct NoHandle;

impl CallHandle for NoHandle {
    fn control(&self, _: Control) {}
}

impl CallManager {
    /// The manager with the live driver (or [`Unavailable`] without the `calls` feature).
    pub fn new(ui: UiHandle, client: teams_proto::Client, my_name: String) -> Self {
        #[cfg(not(test))]
        super::super::call_presence::install(client.clone());
        #[cfg(feature = "calls")]
        let hub = live::Hub::new(client);
        // Tests never reach the network or the sound cards.
        #[cfg(test)]
        let driver: Arc<dyn CallDriver> = Arc::new(FakeDriver::manual());
        #[cfg(all(feature = "calls", not(test)))]
        let driver: Arc<dyn CallDriver> = Arc::new(live::LiveDriver::new(hub.clone()));
        #[cfg(all(not(feature = "calls"), not(test)))]
        let driver: Arc<dyn CallDriver> = Arc::new(Unavailable);
        #[cfg(not(feature = "calls"))]
        let _ = client;
        // Tests keep the effects in memory only.
        #[cfg(all(feature = "calls", not(test)))]
        let config_dir = session::Paths::new().ok().map(|p| p.config_dir);
        #[cfg(all(feature = "calls", test))]
        let config_dir = None;
        CallManager {
            shared: Arc::new_cyclic(|_me| Shared {
                model: Mutex::new(CallModel {
                    my_name,
                    ..Default::default()
                }),
                handle: Mutex::new(None),
                held: Mutex::new(None),
                driver: Mutex::new(driver),
                generation: AtomicU64::new(0),
                next_generation: AtomicU64::new(0),
                ticking: AtomicBool::new(false),
                ui,
                photos: Mutex::new(None),
                meeting_start: Mutex::new(None),
                feedback_file: Mutex::new(None),
                #[cfg(feature = "calls")]
                fx: Mutex::new(video_fx::VideoFx::load(config_dir)),
                #[cfg(feature = "calls")]
                preview: Mutex::new(None),
                #[cfg(feature = "calls")]
                me: _me.clone(),
            }),
            #[cfg(feature = "calls")]
            hub,
        }
    }

    /// Starts the calling signalling at sign-in (the calling trouter, so incoming calls arrive).
    pub fn start(&self) {
        #[cfg(feature = "calls")]
        {
            self.hub.start();
            *MANAGER.lock().unwrap() = Some(Arc::new(self.clone()));
            // Teams' background pictures: the thumbnails are fetched in the background, and the
            // Video effects panel redraws as they arrive.
            #[cfg(not(test))]
            {
                if let Ok(paths) = session::Paths::new() {
                    let me = Arc::downgrade(&self.shared);
                    self.shared.fx.lock().unwrap().use_teams(
                        super::teams_backgrounds::dir(&paths.data_dir),
                        Arc::new(move || {
                            if let Some(shared) = me.upgrade() {
                                shared.teams_changed();
                            }
                        }),
                    );
                    self.shared.push_fx();
                }
            }
        }
    }

    /// Where the call window's profile photos come from. The call state is pushed every second
    /// while connected, so a photo still downloading shows up on a later push.
    pub fn set_photos(&self, lookup: impl Fn(&str) -> Option<Pixels> + Send + Sync + 'static) {
        *self.shared.photos.lock().unwrap() = Some(Arc::new(lookup));
    }

    /// Where a meeting's start time comes from (unix ms by the meeting chat's id): the header's
    /// timer counts the meeting's own time, not the time since we joined.
    pub fn set_meeting_start(&self, lookup: impl Fn(&str) -> Option<i64> + Send + Sync + 'static) {
        *self.shared.meeting_start.lock().unwrap() = Some(Arc::new(lookup));
    }

    /// Makes the running call look `by` older, so a test need not wait for it to last.
    #[cfg(test)]
    pub fn backdate_connected(&self, by: Duration) {
        let mut m = self.shared.model.lock().unwrap();
        m.connected_at = m.connected_at.map(|t| t - by);
    }

    /// Makes the floating reactions and our last send `by` older, then shows the window what is
    /// left, so a test need not wait for them to expire.
    #[cfg(test)]
    pub fn backdate_reactions(&self, by: Duration) {
        {
            let mut m = self.shared.model.lock().unwrap();
            for r in &mut m.reactions {
                r.at -= by;
            }
            m.reacted_at = m.reacted_at.map(|t| t - by);
        }
        self.shared.push();
    }

    /// Keeps the call-quality ratings in `path` instead of IronTeams's data dir.
    #[cfg(test)]
    pub fn set_feedback_file(&self, path: std::path::PathBuf) {
        *self.shared.feedback_file.lock().unwrap() = Some(path);
    }

    /// Keeps a call-quality rating (the survey's Send): one JSON line in the data dir, and the log.
    /// Nothing is posted to Microsoft (its feedback endpoint is not known).
    pub(super) fn save_feedback(&self, f: &Feedback, kind: &str, call: Duration) {
        let line = f.line(chrono::Utc::now(), kind, call);
        tracing::info!(feedback = %line, "call quality feedback (kept locally, not sent)");
        let path = self.shared.feedback_file.lock().unwrap().clone();
        #[cfg(not(test))]
        let path = path.or_else(|| {
            Some(
                session::Paths::new()
                    .ok()?
                    .data_dir
                    .join(call_feedback::FILE),
            )
        });
        if let Some(path) = path
            && let Err(e) = call_feedback::append(&path, &line)
        {
            tracing::warn!(error = %e, "could not keep the call quality feedback");
        }
        self.shared.ui.info("Thanks for your feedback".into());
    }

    /// Replaces the driver (tests and the demo use [`FakeDriver`]).
    pub fn set_driver(&self, d: Arc<dyn CallDriver>) {
        *self.shared.driver.lock().unwrap() = d;
    }

    pub(super) fn driver(&self) -> Arc<dyn CallDriver> {
        self.shared.driver.lock().unwrap().clone()
    }

    /// Native calls are available; when not, joins open in the browser.
    pub fn native(&self) -> bool {
        self.driver().native()
    }

    /// The meeting behind this Join link is the one running, being joined or on the pre-join
    /// screen, so the calendar does not offer to join it again.
    pub fn is_in_meeting(&self, join_url: &str) -> bool {
        let m = self.shared.model.lock().unwrap();
        if !(m.phase.active() || m.phase == Phase::PreJoin) {
            return false;
        }
        let same_link = matches!(&m.target, Some(JoinTarget::Link(u)) if u == join_url);
        let same_chat = !m.chat_id.is_empty()
            && ::calendar::meeting_chat_thread_id(join_url).as_deref() == Some(m.chat_id.as_str());
        same_link || same_chat
    }

    /// Who is in `chat_id`'s call while we are in it, as (MRI, name): the lobby and bots left
    /// out. `None` when we are not in that conversation's call (the header's live-call strip).
    pub fn roster_of(&self, chat_id: &str) -> Option<Vec<(String, String)>> {
        let m = self.shared.model.lock().unwrap();
        (m.phase.active() && !chat_id.is_empty() && m.chat_id == chat_id).then(|| {
            m.roster
                .iter()
                .filter(|p| !p.lobby && !p.is_bot())
                .map(|p| (p.mri.clone(), p.name.clone()))
                .collect()
        })
    }

    /// Opens the pre-join screen for a meeting. A running call stays; the window shows why.
    pub fn prejoin(&self, title: String, target: JoinTarget, chat_id: String) {
        let devices = self.driver().devices();
        let _ = self.shared.model.lock().unwrap().prejoin(
            CallKind::Meeting { title },
            target,
            chat_id,
            devices,
        );
        #[cfg(feature = "calls")]
        self.shared.push_fx();
        self.shared.push();
    }

    /// What the camera applies to what it sends (tests).
    #[cfg(all(test, feature = "calls"))]
    pub fn effects(&self) -> calling::video::effects::Effects {
        (*self.shared.fx.lock().unwrap().control().get()).clone()
    }

    /// Keeps the Video effects choices in `dir` from now on, reading what is there (tests).
    #[cfg(all(test, feature = "calls"))]
    pub fn keep_effects_in(&self, dir: std::path::PathBuf) {
        *self.shared.fx.lock().unwrap() = video_fx::VideoFx::load(Some(dir));
        self.shared.push_fx();
    }

    /// "Add new" in the Video effects panel: the picture at `path` becomes a background.
    #[cfg(all(feature = "calls", not(test)))]
    pub fn add_background(&self, path: &std::path::Path) {
        let r = self.shared.fx.lock().unwrap().add_background(path);
        if let Err(e) = r {
            tracing::warn!(error = %e, "the picture was not added as a background");
            self.shared
                .ui
                .info("That picture can't be used as a background.".into());
        }
        self.shared.push_fx();
    }

    /// A Teams background picture is in `dir` (tests): the panel lists and applies it.
    #[cfg(all(test, feature = "calls"))]
    pub fn keep_teams_in(&self, dir: std::path::PathBuf) {
        self.shared
            .fx
            .lock()
            .unwrap()
            .use_teams(dir, Arc::new(|| {}));
        self.shared.push_fx();
    }

    /// Pre-join "Join now".
    pub fn join_now(&self) {
        let Some((target, opts)) = self.shared.model.lock().unwrap().joining() else {
            return;
        };
        let generation = self.shared.new_generation();
        self.shared.push();
        let sink = EventSink {
            shared: Arc::downgrade(&self.shared),
            generation,
        };
        let handle = self.driver().join(target, opts, sink);
        if self.shared.model.lock().unwrap().phase.active() {
            *self.shared.handle.lock().unwrap() = Some(Arc::from(handle));
        }
    }

    /// Runs a call window command.
    pub fn command(&self, c: CallCmd) {
        let now = Instant::now();
        let control = {
            let mut m = self.shared.model.lock().unwrap();
            match c {
                CallCmd::JoinNow => {
                    drop(m);
                    return self.join_now();
                }
                CallCmd::JoinDevice => {
                    if !m.add_device() {
                        return;
                    }
                    drop(m);
                    return self.join_now();
                }
                CallCmd::JoinChat(_) | CallCmd::JoinOngoing(_) => None,
                CallCmd::Cancel => {
                    m.cancel();
                    None
                }
                CallCmd::PrejoinMic(on) => {
                    m.mic_on = on;
                    None
                }
                CallCmd::PrejoinAudio(computer) => {
                    m.prejoin_audio(computer);
                    None
                }
                CallCmd::Mic(i) => m.pick_device(true, i),
                CallCmd::Speaker(i) => m.pick_device(false, i),
                CallCmd::ToggleMute => m.toggle_mute().map(Control::Mute),
                CallCmd::ToggleSound => m.toggle_sound().map(Control::Sound),
                CallCmd::ToggleHand => m.toggle_hand().map(|up| {
                    if up {
                        Control::RaiseHand
                    } else {
                        Control::LowerHand
                    }
                }),
                CallCmd::React(name) => m.react(name, now),
                CallCmd::ToggleHold => m.toggle_hold().map(Control::Hold),
                CallCmd::TransferOpen => m.transfer_opened().map(Control::Hold),
                CallCmd::Find(_) => None,
                // The card is the engine's (`call_person_card`); the call has nothing to do.
                CallCmd::PersonCard(_) => None,
                CallCmd::Invite(mri) => m.invite(&mri),
                CallCmd::Transfer(mri) => m.transfer(&mri),
                CallCmd::ToggleShare => m.toggle_share(),
                CallCmd::ToggleCamera => m.toggle_camera(),
                CallCmd::PrejoinCamera(on) => {
                    m.prejoin_camera = on;
                    None
                }
                CallCmd::Leave => m.leave(now).then_some(Control::Leave),
                CallCmd::Close => {
                    m.close();
                    None
                }
                // Keeps the answer and closes the ended screen in one command, so the answer
                // never depends on the order commands run in (a separate `close` once ran first,
                // before the call commands had their ordered lane, and left nothing to answer).
                CallCmd::Feedback(f) => {
                    let ended = m.close();
                    let (kind, call) = (m.kind.name(), m.duration.unwrap_or_default());
                    drop(m);
                    if ended {
                        self.save_feedback(&f, kind, call);
                        self.shared.push();
                    }
                    return;
                }
                CallCmd::Rejoin => {
                    drop(m);
                    let devices = self.driver().devices();
                    self.shared.model.lock().unwrap().rejoin(devices);
                    self.shared.push();
                    return;
                }
                CallCmd::DismissError => {
                    m.error.clear();
                    None
                }
                CallCmd::Captions(on) => {
                    let active = m.phase.active();
                    (active && m.set_captions(on)).then_some(Control::Captions(on))
                }
                CallCmd::Recording(start) => m.phase.active().then_some(Control::Recording(start)),
                CallCmd::Transcription(start) => {
                    m.phase.active().then_some(Control::Transcription(start))
                }
                CallCmd::DismissNotice => {
                    m.notice.clear();
                    None
                }
                CallCmd::Resume => {
                    drop(m);
                    if self.shared.resume() {
                        self.shared.push();
                        self.shared.tick();
                    }
                    return;
                }
                CallCmd::Control(a) => m.remote_control(a),
                CallCmd::Admit(mri) => m.admit(&mri),
                CallCmd::AdmitAll => m.admit_all(),
                CallCmd::PeopleOpen => {
                    m.people_opened();
                    None
                }
                CallCmd::PeopleFilter(text) => {
                    m.set_people_filter(&text);
                    None
                }
                // The engine copies the link (`Engine::call_cmd`).
                CallCmd::ShareInvite => None,
                CallCmd::CameraDevice(i) => {
                    let changed = m.pick_camera(i);
                    let id = m.camera_id();
                    drop(m);
                    #[cfg(feature = "calls")]
                    if changed {
                        self.shared.camera_chosen(id);
                    }
                    #[cfg(not(feature = "calls"))]
                    let _ = (changed, id);
                    self.shared.push();
                    return;
                }
                CallCmd::VideoPreview(on) => {
                    m.set_video_preview(on);
                    None
                }
                CallCmd::VideoApply => m.apply_video(),
                CallCmd::AudioRefresh => m.refresh_audio(),
                CallCmd::Fx(c) => {
                    drop(m);
                    #[cfg(feature = "calls")]
                    self.shared.fx_command(c);
                    #[cfg(not(feature = "calls"))]
                    let _ = c;
                    return;
                }
                CallCmd::EndHeld => {
                    let shown_on_hold = m.held;
                    drop(m);
                    if self.shared.end_held() {
                        self.shared.push();
                    } else if shown_on_hold {
                        self.command(CallCmd::Leave);
                    }
                    return;
                }
            }
        };
        match control {
            Some(Control::Leave) => {
                // The handle goes first, so a driver reporting the end finds the call gone.
                let h = self.shared.handle.lock().unwrap().take();
                if let Some(h) = h {
                    h.control(Control::Leave);
                }
                // A call on hold stays on hold, now shown.
                if self.shared.promote_held() {
                    self.shared.tick();
                }
                // The driver reports no end for a hang-up made here, so no event brings the
                // presence back from "In a call": say it now (nothing is sent if unchanged).
                self.shared.publish_presence();
            }
            Some(c) => self.shared.control(c),
            None => {}
        }
        self.shared.push();
    }

    /// IronTeams is quitting: leaves the shown call and then the one that was on hold. The
    /// handles come back so the caller can wait for each leave request to go out
    /// ([`CallHandle::finished`]).
    pub fn leave_all(&self) -> Vec<Arc<dyn CallHandle>> {
        let mut handles = Vec::new();
        // A call on hold comes forward once the shown one has left: at most two rounds.
        for _ in 0..2 {
            if !self.shared.model.lock().unwrap().phase.active() {
                break;
            }
            handles.extend(self.shared.handle.lock().unwrap().clone());
            self.command(CallCmd::Leave);
        }
        handles
    }

    /// A people search starts; pass its number to [`Self::found`].
    pub fn find_started(&self) -> u64 {
        self.shared.model.lock().unwrap().find_started()
    }

    /// The people search `seq` answered (dropped when a newer one started).
    pub fn found(&self, seq: u64, found: Vec<Found>) {
        if self.shared.model.lock().unwrap().found(seq, found) {
            self.shared.push();
        }
    }

    /// What the people search would have answered for the latest query (tests).
    #[cfg(test)]
    pub fn test_found(&self, found: Vec<Found>) {
        let seq = self.shared.model.lock().unwrap().find_seq;
        self.found(seq, found);
    }

    /// Demo mode: the fake driver with a timed script; `connected` also starts a connected
    /// meeting right away (`ironteams --call-demo`).
    pub fn demo(&self, connected: bool) {
        let fake = FakeDriver::scripted(!connected);
        self.set_driver(Arc::new(fake));
        if connected {
            self.prejoin(
                "Weekly design review".into(),
                JoinTarget::Code {
                    id: "123456789".into(),
                    passcode: "demo".into(),
                },
                String::new(),
            );
            self.join_now();
        }
    }
}

/// What happens to the call in progress when a waiting call is answered.
#[cfg(any(test, feature = "calls"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentCall {
    /// "Hold and accept": it goes on hold (the "On hold" bar).
    Hold,
    /// "End and accept": we leave it.
    End,
}
