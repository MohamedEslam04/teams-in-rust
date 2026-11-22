use super::*;

/// Plays the test sound on an output (`None`: the default one).
pub type PlaySound = Arc<dyn Fn(Option<String>) -> Result<(), String> + Send + Sync>;

/// Turns the autostart entry on or off, and says whether it is on.
pub struct Autostart {
    pub set: Box<dyn Fn(bool) -> Result<(), String>>,
    pub get: Box<dyn Fn() -> bool>,
}

impl Autostart {
    /// The desktop autostart entry (`tray::autostart`), in `$XDG_CONFIG_HOME/autostart`, or under
    /// `config_home` when given (tests). IronTeams starts with `--hidden` from it.
    pub fn desktop(config_home: Option<std::path::PathBuf>) -> Autostart {
        let home = config_home.clone();
        Autostart {
            set: Box::new(move |on| {
                let exe = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.to_str().map(str::to_owned))
                    .unwrap_or_else(|| "IronTeams".into());
                let r = match (&home, on) {
                    (Some(h), true) => {
                        tray::autostart::enable_in(h, &exe, Some("--hidden")).map(|_| ())
                    }
                    (Some(h), false) => tray::autostart::disable_in(h),
                    (None, true) => tray::autostart::enable(&exe, Some("--hidden")).map(|_| ()),
                    (None, false) => tray::autostart::disable(),
                };
                r.map_err(|e| e.to_string())
            }),
            get: Box::new(move || match &config_home {
                Some(h) => tray::autostart::is_enabled_in(h),
                None => tray::autostart::is_enabled(),
            }),
        }
    }
}

/// What Settings does outside the window. `system()` has the real ones; tests record calls.
pub struct Hooks {
    pub autostart: Autostart,
    pub test_notification: Box<dyn Fn()>,
    pub open_url: Box<dyn Fn(&str)>,
    /// Starts IronTeams again and quits this one.
    pub restart: Box<dyn Fn()>,
    /// Runs after the tokens were removed: the app closes.
    pub signed_out: Box<dyn Fn()>,
    pub report_problem: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    pub enumerate: Arc<dyn Fn() -> DeviceLists + Send + Sync>,
    pub test_sound: PlaySound,
    /// The Play buttons of the message sound and the ringtone: what to play, which choice, and
    /// the sound file of a Custom one.
    pub play_sound: Box<dyn Fn(SoundKind, RingtoneChoice, Option<String>)>,
    /// The "Choose a file…" buttons: asks for a sound file (`None`: cancelled). Shared, so the
    /// dialog can run with no borrow of the settings held.
    pub pick_sound_file: Rc<dyn Fn() -> Option<std::path::PathBuf>>,
    pub mic_level: OpenMic,
    /// Asks for the latest release (blocking, runs on a thread). Only called when the user has
    /// turned the update check on, in copies that cannot update themselves.
    pub check_update: Arc<dyn Fn() -> Result<Release, String> + Send + Sync>,
    /// Self-updates: `Some` only in a Velopack install (`updater::system`).
    pub updates: Option<Arc<dyn updater::Source>>,
    /// Quits so the updater can install a downloaded update ("Restart to update").
    pub quit: Box<dyn Fn()>,
}

pub struct SettingsEnv {
    /// Config, data and cache dirs (`IRONTEAMS_HOME` applies, as everywhere).
    pub paths: session::Paths,
    /// Where sign-out removes the tokens from.
    pub tokens: Arc<dyn session::TokenStore>,
    /// Read and watch the desktop portal for the system theme, accent and contrast.
    pub portal: bool,
    /// Run slow work (device lists, sign-out, reports) on threads. Off in tests: inline.
    pub background: bool,
    /// Apply the date and time format and "keep running on close" to the process-wide settings
    /// that the time helpers and the tray read. Off in tests, which share one process.
    pub apply_globals: bool,
    /// What the crash-loop guard found at this start.
    pub crash_loop: updater::guard::Verdict,
    pub hooks: Hooks,
}

impl SettingsEnv {
    /// The real environment of the app.
    pub fn system() -> Option<SettingsEnv> {
        let paths = session::Paths::new().ok()?;
        let report_paths = session::Paths::new().ok()?;
        Some(SettingsEnv {
            paths,
            tokens: Arc::new(session::SystemKeyring),
            portal: true,
            background: true,
            apply_globals: true,
            crash_loop: updater::guard::verdict(),
            hooks: Hooks {
                autostart: Autostart::desktop(None),
                test_notification: Box::new(|| {
                    // A Teams card when the cards are on, like the notifications it stands for.
                    std::thread::spawn(|| {
                        use crate::desktop_cards::{client, protocol};
                        let cards = crate::support::quiet::load_prefs().notification_style
                            == appearance::NotificationStyle::Cards;
                        let card = protocol::Card {
                            id: client::next_id(),
                            title: "IronTeams".into(),
                            text: "This is a test notification.".into(),
                            initials: "IT".into(),
                            color: 0x5b5fc7,
                            ..protocol::Card::default()
                        };
                        if cards && client::show(card, Box::new(|_| {})) {
                            return;
                        }
                        let _ = notify_rust::Notification::new()
                            .appname("IronTeams")
                            .summary("IronTeams")
                            .body("This is a test notification.")
                            .show();
                    });
                }),
                open_url: Box::new(|u| {
                    let _ = crate::external::open(u);
                }),
                restart: Box::new(|| {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).spawn();
                    }
                    let _ = slint::quit_event_loop();
                }),
                signed_out: Box::new(|| {
                    let _ = slint::quit_event_loop();
                }),
                report_problem: Arc::new(move || {
                    diag::report_from_args(&report_paths, env!("CARGO_PKG_VERSION"), &[])
                        .map(|p| p.display().to_string())
                        .map_err(|e| e.to_string())
                }),
                enumerate: Arc::new(|| {
                    let mut l = DeviceLists::default();
                    let mut errs = vec![];
                    match devices::audio_inputs() {
                        Ok(v) => l.inputs = v,
                        Err(e) => errs.push(e.to_string()),
                    }
                    match devices::audio_outputs() {
                        Ok(v) => l.outputs = v,
                        Err(e) => errs.push(e.to_string()),
                    }
                    match devices::cameras() {
                        Ok(v) => l.cameras = v,
                        Err(e) => errs.push(e.to_string()),
                    }
                    l.error = (!errs.is_empty()).then(|| errs.join("; "));
                    l
                }),
                test_sound: Arc::new(|out| {
                    devices::test_sound(out.as_deref()).map_err(|e| e.to_string())
                }),
                play_sound: Box::new(|kind, choice, file| {
                    sounds::preview(kind, choice, file.as_deref());
                }),
                pick_sound_file: Rc::new(|| {
                    rfd::FileDialog::new()
                        .set_title("Choose a sound")
                        .add_filter("Sound files", &["wav", "mp3", "m4a", "aac"])
                        .pick_file()
                }),
                mic_level: Box::new(|input| {
                    let s = devices::mic_level(input).map_err(|e| e.to_string())?;
                    Ok(Box::new(move || s.latest().map(|l| l.peak)) as MicPoll)
                }),
                check_update: Arc::new(update_check::fetch_latest),
                updates: updater::system(),
                quit: Box::new(|| {
                    let _ = slint::quit_event_loop();
                }),
            },
        })
    }
}
