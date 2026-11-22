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
    let index = |list: &[devices::AudioDevice], chosen: Option<&devices::AudioDevice>| {
        chosen
            .and_then(|c| list.iter().position(|d| d.id == c.id))
            .or_else(|| list.iter().position(|d| d.is_default))
            .unwrap_or(0)
    };
    let to = |list: &[devices::AudioDevice]| {
        list.iter()
            .map(|d| Device {
                id: Some(d.id.clone()),
                name: d.name.clone(),
            })
            .collect()
    };
    let (cameras, camera) = camera::list(prefs.camera.as_deref());
    Devices {
        mic: index(&inputs, prefs.find_microphone(&inputs)),
        speaker: index(&outputs, prefs.find_speaker(&outputs)),
        mics: to(&inputs),
        speakers: to(&outputs),
        cameras,
        camera,
    }
}

pub(super) struct LiveHandle(pub(super) mpsc::UnboundedSender<Control>);

impl CallHandle for LiveHandle {
    fn control(&self, c: Control) {
        let _ = self.0.send(c);
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
