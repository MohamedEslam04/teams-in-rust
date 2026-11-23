use super::*;

/// Where a driver reports. Events from a call that is no longer the current one are dropped.
#[derive(Clone)]
pub struct EventSink {
    pub(super) shared: Weak<Shared>,
    pub(super) generation: u64,
}

impl EventSink {
    pub fn send(&self, ev: Event) {
        if let Some(s) = self.shared.upgrade() {
            s.apply(self.generation, ev);
        }
    }

    /// A picture for the call window (`None` clears it). Bypasses the model: frames come many
    /// times a second and change nothing else.
    #[cfg(feature = "calls")]
    pub fn video(&self, slot: VideoSlot, frame: Option<calling::video::RgbFrame>) {
        if let Some(s) = self.shared.upgrade()
            && s.generation.load(Ordering::SeqCst) == self.generation
        {
            s.ui.call_video(slot, frame);
        }
    }

    /// The video effects the camera applies to what it sends (the Video effects panel's).
    #[cfg(feature = "calls")]
    pub fn effects(&self) -> Arc<calling::video::effects::EffectsControl> {
        match self.shared.upgrade() {
            Some(s) => s.fx.lock().unwrap().control(),
            None => calling::video::effects::EffectsControl::new(Default::default()),
        }
    }

    /// Whether our self-view is drawn mirrored ("Mirror my video"), read for every picture.
    #[cfg(feature = "calls")]
    pub fn mirror(&self) -> Arc<AtomicBool> {
        match self.shared.upgrade() {
            Some(s) => s.fx.lock().unwrap().mirror(),
            None => Arc::new(AtomicBool::new(true)),
        }
    }

    /// Why the pre-join preview shows no picture yet ("" once it does).
    #[cfg(feature = "calls")]
    pub fn preview_status(&self, text: &str) {
        if let Some(s) = self.shared.upgrade()
            && s.generation.load(Ordering::SeqCst) == self.generation
        {
            s.ui.preview_status(text.to_owned());
        }
    }

    /// A person's profile photo, when one is known ([`CallManager::set_photos`]).
    #[cfg(feature = "calls")]
    pub fn photo(&self, mri: &str) -> Option<Pixels> {
        let lookup = self.shared.upgrade()?.photos.lock().unwrap().clone()?;
        lookup(mri)
    }

    /// A sink attached to nothing (tests of a driver alone).
    #[cfg(test)]
    pub fn detached() -> Self {
        EventSink {
            shared: Weak::new(),
            generation: 0,
        }
    }
}

/// A call on hold while another one is shown (call waiting). Its driver keeps running and
/// reporting under its own generation.
pub(super) struct Held {
    pub(super) model: CallModel,
    pub(super) handle: Option<Arc<dyn CallHandle>>,
    pub(super) generation: u64,
}

/// Two calls can live together: the shown one (`model`, `handle`, `generation`), which the call
/// window shows and the controls reach, and at most one on hold (`held`). A call's events carry
/// its generation, so each lands on its own model whichever slot it sits in. Lock order: `model`,
/// then `held`, then `handle`.
pub(super) struct Shared {
    pub(super) model: Mutex<CallModel>,
    pub(super) handle: Mutex<Option<Arc<dyn CallHandle>>>,
    pub(super) held: Mutex<Option<Held>>,
    pub(super) driver: Mutex<Arc<dyn CallDriver>>,
    /// The shown call's generation (0: none).
    pub(super) generation: AtomicU64,
    /// The last generation handed out.
    pub(super) next_generation: AtomicU64,
    pub(super) ticking: AtomicBool,
    pub(super) ui: UiHandle,
    pub(super) photos: Mutex<Option<PhotoLookup>>,
    /// Where a meeting's own start time comes from, for the header's timer.
    pub(super) meeting_start: Mutex<Option<StartLookup>>,
    /// Where the call-quality ratings go; none: IronTeams's data dir (and nowhere in tests).
    pub(super) feedback_file: Mutex<Option<std::path::PathBuf>>,
    /// The Video effects panel's choices, which every camera picture goes through.
    #[cfg(feature = "calls")]
    pub(super) fx: Mutex<video_fx::VideoFx>,
    /// The pre-join preview while it runs, and the camera it shows.
    #[cfg(feature = "calls")]
    pub(super) preview: Mutex<Option<(Option<String>, Preview)>>,
    /// This, for the preview's sink.
    #[cfg(feature = "calls")]
    pub(super) me: Weak<Shared>,
}

impl Shared {
    pub(super) fn push(&self) {
        #[cfg(feature = "calls")]
        self.sync_preview();
        let mut d = self.model.lock().unwrap().data(Instant::now());
        if let Some(h) = self.held.lock().unwrap().as_ref() {
            d.held_name = h.model.held_title();
        }
        let lookup = self.photos.lock().unwrap().clone();
        let found = lookup
            .map(|f| photos(&d.people, |m| f(m)))
            .unwrap_or_default();
        self.ui.call_state(d, found);
    }

    /// Runs the pre-join preview while the pre-join screen shows with the camera switch on, of
    /// the camera chosen; stops it (the camera goes off) otherwise.
    #[cfg(feature = "calls")]
    pub(super) fn sync_preview(&self) {
        let wanted = {
            let m = self.model.lock().unwrap();
            let on_prejoin = m.phase == Phase::PreJoin && (m.prejoin_camera || m.video_preview);
            let in_call = m.phase.active() && !m.camera && m.can_camera && m.video_preview;
            (on_prejoin || in_call).then(|| m.camera_id())
        };
        let mut preview = self.preview.lock().unwrap();
        if preview.as_ref().map(|(camera, _)| camera) == wanted.as_ref() {
            return;
        }
        if preview.take().is_some() {
            self.ui.call_video(VideoSlot::Preview, None);
            self.ui.preview_status(String::new());
        }
        if let Some(camera) = wanted {
            let sink = EventSink {
                shared: self.me.clone(),
                generation: self.generation.load(Ordering::SeqCst),
            };
            let driver = self.driver.lock().unwrap().clone();
            if let Some(p) = driver.preview(sink, camera.clone()) {
                *preview = Some((camera, p));
            }
        }
    }

    /// The Video effects panel's choice; the panel shows it.
    #[cfg(feature = "calls")]
    pub(super) fn fx_command(&self, c: video_fx::FxCmd) {
        let view = {
            let mut fx = self.fx.lock().unwrap();
            fx.apply(&c);
            fx.view()
        };
        self.ui.video_fx(view);
    }

    /// The panel's state, for the window to show.
    #[cfg(feature = "calls")]
    pub(super) fn push_fx(&self) {
        let view = self.fx.lock().unwrap().view();
        self.ui.video_fx(view);
    }

    /// Something arrived from Teams' background CDN: the panel's tiles are listed again, and a
    /// chosen picture that was missing goes on the camera.
    #[cfg(feature = "calls")]
    pub(super) fn teams_changed(&self) {
        self.fx.lock().unwrap().teams_changed();
        self.push_fx();
    }

    /// Keeps the camera picked (`None`: the default) in Settings' devices, where the camera reads
    /// it when it opens.
    #[cfg(feature = "calls")]
    pub(super) fn camera_chosen(&self, id: Option<String>) {
        let Some(dir) = self
            .fx
            .lock()
            .unwrap()
            .dir()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let mut local = super::super::settings::Local::load(&dir);
        local.devices.camera = id;
        if let Err(e) = local.save_devices() {
            tracing::warn!(error = %e, "the camera choice was not saved");
        }
    }

    /// A new call's generation, which becomes the shown one.
    pub(super) fn new_generation(&self) -> u64 {
        let g = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.generation.store(g, Ordering::SeqCst);
        g
    }

    pub(super) fn apply(self: &Arc<Self>, generation: u64, ev: Event) {
        if generation != self.generation.load(Ordering::SeqCst) {
            return self.apply_held(generation, ev);
        }
        let (mut changed, mut phase, share, joined) = {
            let mut m = self.model.lock().unwrap();
            let first = m.connected_at.is_none();
            let changed = m.apply(ev, Instant::now());
            let joined = first && m.connected_at.is_some();
            (changed, m.phase, m.take_share_when_ready(), joined)
        };
        if joined && self.start_from_server(generation) {
            changed = true;
        }
        if let Some(c) = share {
            self.control(c);
        }
        if phase == Phase::Ended {
            // The driver is done; dropping the handle stops whatever it still runs.
            self.handle.lock().unwrap().take();
            if self.promote_held() {
                changed = true;
                phase = self.model.lock().unwrap().phase;
            }
        }
        self.publish_presence();
        if changed {
            self.push();
        }
        if phase == Phase::Connected {
            self.tick();
        }
    }

    /// A meeting just connected: its timer counts from when the server says the meeting started,
    /// not from our join. True when that moved the timer.
    fn start_from_server(&self, generation: u64) -> bool {
        let Some(chat) = self.model.lock().unwrap().meeting_chat() else {
            return false;
        };
        let Some(lookup) = self.meeting_start.lock().unwrap().clone() else {
            return false;
        };
        let ahead = head_start(lookup(&chat), unix_ms());
        let mut m = self.model.lock().unwrap();
        // Still the same call, and the server's start is before our join.
        if generation != self.generation.load(Ordering::SeqCst) || ahead.is_zero() {
            return false;
        }
        m.head_start = ahead;
        true
    }

    /// "In a call" while connected (or reconnecting), Available once the call is over.
    pub(super) fn publish_presence(&self) {
        let (phase, meeting) = {
            let m = self.model.lock().unwrap();
            (m.phase, matches!(m.kind, CallKind::Meeting { .. }))
        };
        // Connected or reconnecting: active, and past joining.
        let connected =
            phase.active() && !matches!(phase, Phase::PreJoin | Phase::Connecting | Phase::Lobby);
        super::super::call_presence::update(connected, meeting);
    }

    /// An event of the call on hold. Its end (the other side hung up) takes the bar away.
    pub(super) fn apply_held(&self, generation: u64, ev: Event) {
        let changed = {
            let mut held = self.held.lock().unwrap();
            let Some(h) = held.as_mut().filter(|h| h.generation == generation) else {
                return;
            };
            let changed = h.model.apply(ev, Instant::now());
            if h.model.phase == Phase::Ended {
                *held = None;
            }
            changed
        };
        if changed {
            self.push();
        }
    }

    /// When the shown call is over and another is on hold, the held one is shown instead, still
    /// on hold (Resume takes it back). True when it did.
    pub(super) fn promote_held(&self) -> bool {
        let mut m = self.model.lock().unwrap();
        if m.phase.active() {
            return false;
        }
        let Some(h) = self.held.lock().unwrap().take() else {
            return false;
        };
        let my_name = std::mem::take(&mut m.my_name);
        *m = h.model;
        m.my_name = my_name;
        m.held = true;
        *self.handle.lock().unwrap() = h.handle;
        self.generation.store(h.generation, Ordering::SeqCst);
        true
    }

    /// Moves the shown call to the hold slot to answer another one, and holds it. False when no
    /// call runs or one is already on hold.
    pub(super) fn hold_current(&self) -> bool {
        let (handle, was_on_hold) = {
            let mut m = self.model.lock().unwrap();
            let mut held = self.held.lock().unwrap();
            if !m.phase.active() || held.is_some() {
                return false;
            }
            let idle = CallModel {
                my_name: m.my_name.clone(),
                ..Default::default()
            };
            let mut model = std::mem::replace(&mut *m, idle);
            let was_on_hold = std::mem::replace(&mut model.held, true);
            let handle = self.handle.lock().unwrap().take();
            // Its events now find it in the hold slot.
            let generation = self.generation.swap(0, Ordering::SeqCst);
            *held = Some(Held {
                model,
                handle: handle.clone(),
                generation,
            });
            (handle, was_on_hold)
        };
        if !was_on_hold && let Some(h) = handle {
            h.control(Control::Hold(true));
        }
        true
    }

    /// Leaves the shown call to answer another one ("End and accept"). The call on hold, if
    /// any, stays on hold.
    pub(super) fn end_current(&self) {
        let handle = {
            let mut m = self.model.lock().unwrap();
            if !m.leave(Instant::now()) {
                return;
            }
            // Whatever it still reports is dropped: it must not bring the held call forward.
            self.generation.store(0, Ordering::SeqCst);
            self.handle.lock().unwrap().take()
        };
        if let Some(h) = handle {
            h.control(Control::Leave);
        }
    }

    /// Resume: the call on hold comes back and the shown one goes on hold. With no other call,
    /// the shown call (on hold) resumes. False when there is nothing to resume.
    pub(super) fn resume(&self) -> bool {
        self.promote_held();
        let (hold, resume) = {
            let mut m = self.model.lock().unwrap();
            let mut held = self.held.lock().unwrap();
            match held.take() {
                Some(mut h) => {
                    h.model.held = false;
                    let mut shown = std::mem::replace(&mut *m, h.model);
                    m.my_name = shown.my_name.clone();
                    shown.held = true;
                    let shown_handle =
                        std::mem::replace(&mut *self.handle.lock().unwrap(), h.handle.clone());
                    let generation = self.generation.swap(h.generation, Ordering::SeqCst);
                    *held = Some(Held {
                        model: shown,
                        handle: shown_handle.clone(),
                        generation,
                    });
                    (shown_handle, h.handle)
                }
                None if m.held && m.phase.active() => {
                    m.held = false;
                    (None, self.handle.lock().unwrap().clone())
                }
                None => return false,
            }
        };
        // Silence the one going on hold before the other takes the devices.
        if let Some(h) = hold {
            h.control(Control::Hold(true));
        }
        if let Some(h) = resume {
            h.control(Control::Hold(false));
        }
        true
    }

    /// The "On hold" bar's End for the other call. False when no other call is on hold.
    pub(super) fn end_held(&self) -> bool {
        let Some(h) = self.held.lock().unwrap().take() else {
            return false;
        };
        if let Some(handle) = h.handle {
            handle.control(Control::Leave);
        }
        true
    }

    /// Pushes the timer once a second while the call runs.
    pub(super) fn tick(self: &Arc<Self>) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.ticking.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        rt.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(s) = weak.upgrade() else { return };
                if !s.model.lock().unwrap().phase.active() {
                    s.ticking.store(false, Ordering::SeqCst);
                    return;
                }
                s.push();
            }
        });
    }

    pub(super) fn control(&self, c: Control) {
        // The camera opens for sending: a running preview of it lets go first.
        #[cfg(feature = "calls")]
        if c == Control::Camera(true) {
            self.sync_preview();
        }
        // Out of the lock: a driver may report (and end the call) from inside `control`.
        let h = self.handle.lock().unwrap().clone();
        if let Some(h) = h {
            h.control(c);
        }
    }
}
