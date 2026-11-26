use super::*;

/// A driver with no network or audio: it records what it was asked and reports what the test
/// (or the demo script) tells it to.
#[derive(Clone, Default)]
pub struct FakeDriver {
    /// Everything asked of it, as `join <target> mic=on|off`, `mute on`, `hand up`, `leave`, ...
    pub log: Arc<Mutex<Vec<String>>>,
    pub(super) sink: Arc<Mutex<Option<EventSink>>>,
    /// The handles' leave never finishes (a hung request, for the quit tests).
    pub(super) hang_leave: Arc<AtomicBool>,
    /// Demo script: `Some(delayed)` plays connecting, lobby (when delayed) and connected.
    pub(super) script: Option<bool>,
    /// The pre-join preview: `on <camera>` and `off`, and where its pictures go while it runs.
    #[cfg(feature = "calls")]
    pub(super) previews: Arc<Mutex<Vec<String>>>,
    #[cfg(feature = "calls")]
    pub(super) preview_sink: Arc<Mutex<Option<EventSink>>>,
}

impl FakeDriver {
    /// Reports only what [`Self::emit`] sends.
    #[cfg(test)]
    pub fn manual() -> Self {
        Self::default()
    }

    /// Plays the demo meeting: a few seconds in the lobby when `delayed`, else straight in.
    pub fn scripted(delayed: bool) -> Self {
        FakeDriver {
            script: Some(delayed),
            ..Self::default()
        }
    }

    /// Sends an event as the running call would.
    #[cfg(test)]
    pub fn emit(&self, ev: Event) {
        let sink = self.sink.lock().unwrap().clone();
        if let Some(s) = sink {
            s.send(ev);
        }
    }

    /// Runs a connected 1:1 call as the live driver's `adopt` would (tests).
    #[cfg(test)]
    pub fn adopt(&self, sink: EventSink) -> Box<dyn CallHandle> {
        *self.sink.lock().unwrap() = Some(sink.clone());
        Box::new(FakeHandle {
            log: self.log.clone(),
            sink,
            demo: false,
            hang_leave: self.hang_leave.clone(),
        })
    }

    /// A leave sent to a running call never finishes ([`CallHandle::finished`] waits for ever).
    #[cfg(test)]
    pub fn hang_when_leaving(&self) {
        self.hang_leave.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// The pre-join previews started (`on <camera>`) and stopped (`off`).
    #[cfg(all(test, feature = "calls"))]
    pub fn previews(&self) -> Vec<String> {
        self.previews.lock().unwrap().clone()
    }

    /// A camera picture for the running preview, through the effects and the mirror as the live
    /// camera's are. False when no preview runs.
    #[cfg(all(test, feature = "calls"))]
    pub fn preview_frame(&self, frame: &calling::video::I420Frame) -> bool {
        let Some(sink) = self.preview_sink.lock().unwrap().clone() else {
            return false;
        };
        let mut frame = frame.clone();
        calling::video::effects::Processor::new(sink.effects()).apply(&mut frame);
        let mirror = sink.mirror().load(Ordering::Relaxed);
        sink.video(
            VideoSlot::Preview,
            Some(camera::self_view(&frame, usize::MAX, mirror)),
        );
        true
    }

    pub(super) fn demo_people(name: &str) -> Vec<Person> {
        let p = |n: &str, muted, hand, lobby, role: &str| Person {
            name: n.into(),
            muted,
            hand,
            lobby,
            me: false,
            role: role.into(),
            ..Default::default()
        };
        vec![
            p("Amira Hassan", false, false, false, "organizer"),
            Person {
                name: name.into(),
                me: true,
                role: "attendee".into(),
                ..Default::default()
            },
            p("Daniel Park", true, true, false, "presenter"),
            p("Sara Ali", true, false, false, "attendee"),
            p("Omar Farouk", false, false, false, "attendee"),
            p("Guest (external)", false, false, true, ""),
        ]
    }
}

/// The fake's running preview: stopping it is logged.
#[cfg(feature = "calls")]
pub(super) struct FakePreview {
    pub(super) previews: Arc<Mutex<Vec<String>>>,
    pub(super) sink: Arc<Mutex<Option<EventSink>>>,
}

#[cfg(feature = "calls")]
impl Drop for FakePreview {
    fn drop(&mut self) {
        self.sink.lock().unwrap().take();
        self.previews.lock().unwrap().push("off".into());
    }
}

pub(super) struct FakeHandle {
    pub(super) log: Arc<Mutex<Vec<String>>>,
    pub(super) sink: EventSink,
    /// The demo answers recording, transcription and captions as the service would.
    pub(super) demo: bool,
    pub(super) hang_leave: Arc<AtomicBool>,
}

pub(super) fn start_stop(on: bool) -> &'static str {
    if on { "start" } else { "stop" }
}

impl CallHandle for FakeHandle {
    fn finished(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        if self.hang_leave.load(Ordering::SeqCst) {
            Box::pin(std::future::pending())
        } else {
            Box::pin(async {})
        }
    }

    fn control(&self, c: Control) {
        let line = match &c {
            // The window sends it by itself, often: not a call the tests script.
            Control::VideoView(_) => return,
            Control::Mute(m) => format!("mute {}", if *m { "on" } else { "off" }),
            Control::RaiseHand => "hand up".into(),
            Control::LowerHand => "hand down".into(),
            Control::React(name) => format!("react {name}"),
            Control::Share(on) => format!("share {}", if *on { "on" } else { "off" }),
            Control::Camera(on) => format!("camera {}", if *on { "on" } else { "off" }),
            Control::Devices { mic, speaker } => format!(
                "devices {} {}",
                mic.as_deref().unwrap_or("default"),
                speaker.as_deref().unwrap_or("default")
            ),
            Control::Captions(on) => format!("captions {}", if *on { "on" } else { "off" }),
            Control::Recording(on) => format!("recording {}", start_stop(*on)),
            Control::Transcription(on) => format!("transcription {}", start_stop(*on)),
            Control::Hold(h) => format!("hold {}", if *h { "on" } else { "off" }),
            Control::Sound(off) => format!("sound {}", if *off { "off" } else { "on" }),
            Control::AddPerson { mri, .. } => format!("add {mri}"),
            Control::Transfer { mri, .. } => format!("transfer {mri}"),
            Control::Admit { mri, .. } => format!("admit {mri}"),
            Control::AdmitAll => "admit all".into(),
            Control::Remote(a) => format!("control {a:?}"),
            Control::Leave => "leave".into(),
        };
        self.log.lock().unwrap().push(line);
        if self.demo {
            match c {
                Control::Recording(on) => self.sink.send(Event::Recording(on)),
                Control::Transcription(on) => self.sink.send(Event::Transcribing(on)),
                Control::Captions(true) => self.sink.send(Event::Caption(Caption {
                    id: "demo".into(),
                    speaker: "Amira Hassan".into(),
                    text: "Let's start with the design review.".into(),
                })),
                _ => {}
            }
        }
        if c == Control::Leave {
            self.sink.send(Event::Ended(YOU_LEFT_MEETING.into()));
        }
    }
}

impl CallDriver for FakeDriver {
    fn devices(&self) -> Devices {
        let d = |id: &str, name: &str| Device {
            id: Some(id.into()),
            name: name.into(),
        };
        Devices {
            mics: vec![
                d("mic-builtin", "Built-in Microphone"),
                d("mic-usb", "USB Headset Microphone"),
            ],
            speakers: vec![
                d("spk-builtin", "Built-in Speakers"),
                d("spk-usb", "USB Headset"),
            ],
            mic: 0,
            speaker: 0,
            cameras: vec![
                Device {
                    id: None,
                    name: DEFAULT_CAMERA.into(),
                },
                d("cam-builtin", "Integrated Camera"),
                d("cam-usb", "USB Webcam"),
            ],
            camera: 0,
        }
    }

    #[cfg(feature = "calls")]
    fn preview(&self, sink: EventSink, camera: Option<String>) -> Option<Preview> {
        self.previews
            .lock()
            .unwrap()
            .push(format!("on {}", camera.as_deref().unwrap_or("default")));
        *self.preview_sink.lock().unwrap() = Some(sink);
        Some(Box::new(FakePreview {
            previews: self.previews.clone(),
            sink: self.preview_sink.clone(),
        }))
    }

    fn join(&self, target: JoinTarget, opts: JoinOptions, sink: EventSink) -> Box<dyn CallHandle> {
        self.log.lock().unwrap().push(format!(
            "join {} mic={}",
            target.describe(),
            if opts.mic_on { "on" } else { "off" }
        ));
        *self.sink.lock().unwrap() = Some(sink.clone());
        sink.send(Event::Connecting);
        if let Some(delayed) = self.script {
            let (s, people) = (sink.clone(), Self::demo_people(&opts.name));
            let play = async move {
                if delayed {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    s.send(Event::Lobby);
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                }
                s.send(Event::Connected);
                s.send(Event::Roster(people));
                s.send(Event::Recording(true));
            };
            match tokio::runtime::Handle::try_current() {
                Ok(rt) if delayed => {
                    rt.spawn(play);
                }
                _ => {
                    sink.send(Event::Connected);
                    sink.send(Event::Roster(Self::demo_people(&opts.name)));
                    sink.send(Event::Recording(true));
                }
            }
        }
        Box::new(FakeHandle {
            log: self.log.clone(),
            sink,
            demo: self.script.is_some(),
            hang_leave: self.hang_leave.clone(),
        })
    }
}
