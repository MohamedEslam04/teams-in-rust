use super::*;

/// The driver the app runs with.
pub struct LiveDriver {
    pub(super) hub: Arc<Hub>,
}

impl LiveDriver {
    pub fn new(hub: Arc<Hub>) -> Self {
        LiveDriver { hub }
    }
}

/// Microphones and speakers from `crates/devices`, the one picked in Settings first (else the
/// system default).
pub(super) fn list_devices() -> Devices {
    let prefs = session::Paths::new()
        .map(|p| crate::backend::settings::Local::load(&p.config_dir).devices)
        .unwrap_or_default();
    let inputs = devices::audio_inputs().unwrap_or_default();
    let outputs = devices::audio_outputs().unwrap_or_default();
    let (mics, mic) = device_rows(&inputs, prefs.find_microphone(&inputs));
    let (speakers, speaker) = device_rows(&outputs, prefs.find_speaker(&outputs));
    let (cameras, camera) = camera::list(prefs.camera.as_deref());
    Devices {
        mic,
        speaker,
        mics,
        speakers,
        cameras,
        camera,
    }
}

/// The picker's rows and the one selected. On Linux the first row is "Default", which follows
/// whatever the sound server's default is (`id: None`) and is selected unless Settings chose a
/// device that is still there; the devices follow, each once. An id saved by an older version
/// is not among them and so selects the default as well.
pub(super) fn device_rows(
    list: &[devices::AudioDevice],
    chosen: Option<&devices::AudioDevice>,
) -> (Vec<Device>, usize) {
    let device = |d: &devices::AudioDevice| Device {
        id: Some(d.id.clone()),
        name: d.name.clone(),
    };
    let at = |wanted: Option<&devices::AudioDevice>| {
        wanted.and_then(|c| list.iter().position(|d| d.id == c.id))
    };
    if cfg!(target_os = "linux") {
        let default = list.iter().find(|d| d.is_default);
        let mut rows = vec![Device {
            id: None,
            name: default.map_or("Default".to_string(), |d| format!("Default ({})", d.name)),
        }];
        rows.extend(list.iter().map(device));
        (rows, at(chosen).map_or(0, |i| i + 1))
    } else {
        let index = at(chosen)
            .or_else(|| list.iter().position(|d| d.is_default))
            .unwrap_or(0);
        (list.iter().map(device).collect(), index)
    }
}

pub(super) struct LiveHandle(pub(super) mpsc::UnboundedSender<Control>);

impl CallHandle for LiveHandle {
    fn control(&self, c: Control) {
        let _ = self.0.send(c);
    }

    /// The call's task owns the receiver and drops it after its leave request.
    fn finished(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.0.closed())
    }
}

impl CallDriver for LiveDriver {
    fn devices(&self) -> Devices {
        list_devices()
    }

    fn join(&self, target: JoinTarget, opts: JoinOptions, sink: EventSink) -> Box<dyn CallHandle> {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run_meeting(self.hub.clone(), target, opts, sink, rx));
        Box::new(LiveHandle(tx))
    }

    fn preview(&self, sink: EventSink, camera: Option<String>) -> Option<super::super::Preview> {
        let p = camera::preview(sink, camera)?;
        Some(Box::new(p))
    }
}
