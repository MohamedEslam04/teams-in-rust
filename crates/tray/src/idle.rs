//! How long the user has not touched the keyboard or the mouse. Presence uses it to report
//! "not active" after a while, so the service shows Away, as Teams does.

use std::time::Duration;

/// How long since the last keyboard or mouse input, `None` when the desktop cannot say (the caller
/// then assumes the user is present). Linux asks the Wayland compositor's idle notifier first (KDE
/// on Wayland answers nothing else), then the desktop's screen saver service (KDE on X11 and most
/// others), then GNOME's idle monitor.
#[cfg(target_os = "linux")]
pub async fn idle_for() -> Option<Duration> {
    let (idle, source) = linux_idle_for().await;
    note_source(source);
    idle
}

/// Logs which source answers the idle time when it changes: at start, and when the Wayland
/// watcher connects (its first answer comes a moment after the first ask) or ends.
#[cfg(target_os = "linux")]
fn note_source(source: &'static str) {
    static LAST: std::sync::Mutex<&str> = std::sync::Mutex::new("");
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *last != source {
        tracing::info!(source, "idle: input idle time source");
        *last = source;
    }
}

/// The idle time and the source that gave it (`wayland`, `dbus-screensaver`, `mutter` or `none`).
#[cfg(target_os = "linux")]
async fn linux_idle_for() -> (Option<Duration>, &'static str) {
    if let Some(idle) = wayland::idle_for() {
        return (Some(idle), "wayland");
    }
    let Ok(conn) = zbus::Connection::session().await else {
        return (None, "none");
    };
    let saver = conn
        .call_method(
            Some("org.freedesktop.ScreenSaver"),
            "/org/freedesktop/ScreenSaver",
            Some("org.freedesktop.ScreenSaver"),
            "GetSessionIdleTime",
            &(),
        )
        .await;
    if let Ok(reply) = saver
        && let Ok(ms) = reply.body().deserialize::<u32>()
    {
        return (Some(Duration::from_millis(ms.into())), "dbus-screensaver");
    }
    let mutter = conn
        .call_method(
            Some("org.gnome.Mutter.IdleMonitor"),
            "/org/gnome/Mutter/IdleMonitor/Core",
            Some("org.gnome.Mutter.IdleMonitor"),
            "GetIdletime",
            &(),
        )
        .await;
    if let Ok(reply) = mutter
        && let Ok(ms) = reply.body().deserialize::<u64>()
    {
        return (Some(Duration::from_millis(ms)), "mutter");
    }
    (None, "none")
}

/// Idle time from the Wayland `ext_idle_notifier_v1` protocol. KDE on Wayland refuses
/// `GetSessionIdleTime` ("not supported on this platform"), so without it the user never goes Away.
/// A thread of its own keeps a Wayland connection with one input-idle notification, and records
/// when input stopped.
#[cfg(target_os = "linux")]
mod wayland {
    use std::sync::{Mutex, Once, PoisonError};
    use std::time::{Duration, Instant};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
    use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
        self, ExtIdleNotificationV1,
    };
    use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

    /// How long without input before the compositor says "idle". Short, so the idle time is known
    /// well before presence's five minutes; the start of idleness is back-dated by it.
    pub(super) const TIMEOUT: Duration = Duration::from_secs(30);

    /// What the input-idle notification has said.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Input {
        /// Not connected (yet), no idle notifier, or the connection ended: Wayland cannot say.
        Unknown,
        /// Input within the last [`TIMEOUT`].
        Active,
        /// No input since this instant.
        IdleSince(Instant),
    }

    impl Input {
        /// The notification went idle at `now`: the last input was [`TIMEOUT`] before.
        pub(super) fn idled(now: Instant) -> Self {
            Self::IdleSince(now.checked_sub(TIMEOUT).unwrap_or(now))
        }

        /// How long without input at `now`, `None` when Wayland cannot say.
        pub(super) fn idle_for(self, now: Instant) -> Option<Duration> {
            match self {
                Self::Unknown => None,
                Self::Active => Some(Duration::ZERO),
                Self::IdleSince(since) => Some(now.saturating_duration_since(since)),
            }
        }
    }

    static INPUT: Mutex<Input> = Mutex::new(Input::Unknown);
    static START: Once = Once::new();

    fn set(input: Input) {
        *INPUT.lock().unwrap_or_else(PoisonError::into_inner) = input;
    }

    /// How long without input, `None` outside Wayland, before the watcher has connected, or when
    /// the compositor has no idle notifier. The first call starts the watcher thread.
    pub(super) fn idle_for() -> Option<Duration> {
        // Without it `connect_to_env` would try `wayland-0`, which an X11 session does not own.
        std::env::var_os("WAYLAND_DISPLAY")?;
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("idle-wayland".into())
                .spawn(|| {
                    if let Err(e) = watch() {
                        tracing::warn!(%e, "idle: the Wayland idle watcher ended");
                    }
                    set(Input::Unknown);
                });
        });
        INPUT
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .idle_for(Instant::now())
    }

    struct Watch;

    /// Connects, asks for an input-idle notification and follows it until the connection ends.
    fn watch() -> Result<(), Box<dyn std::error::Error>> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut queue) = registry_queue_init::<Watch>(&conn)?;
        let qh = queue.handle();
        let notifier: ExtIdleNotifierV1 = globals.bind(&qh, 1..=2, ())?;
        let seat: WlSeat = globals.bind(&qh, 1..=1, ())?;
        let timeout = TIMEOUT.as_millis() as u32;
        // Version 2 counts input only, as Teams does: a playing video's idle inhibitor does not
        // keep the user Available.
        let input_only = notifier.version() >= 2;
        let _notification = if input_only {
            notifier.get_input_idle_notification(timeout, &seat, &qh, ())
        } else {
            notifier.get_idle_notification(timeout, &seat, &qh, ())
        };
        tracing::info!(
            version = notifier.version(),
            input_only,
            "idle: Wayland idle notification requested"
        );
        set(Input::Active);
        loop {
            queue.blocking_dispatch(&mut Watch)?;
        }
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Watch {
        fn event(
            _: &mut Self,
            _: &wl_registry::WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    delegate_noop!(Watch: ignore WlSeat);
    delegate_noop!(Watch: ExtIdleNotifierV1);

    impl Dispatch<ExtIdleNotificationV1, ()> for Watch {
        fn event(
            _: &mut Self,
            _: &ExtIdleNotificationV1,
            event: ext_idle_notification_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_idle_notification_v1::Event::Idled => {
                    tracing::info!("idle: Wayland says idled (no input for 30 s)");
                    set(Input::idled(Instant::now()));
                }
                ext_idle_notification_v1::Event::Resumed => {
                    tracing::info!("idle: Wayland says resumed (input again)");
                    set(Input::Active);
                }
                _ => {}
            }
        }
    }
}

/// How long since the last keyboard or mouse input (`GetLastInputInfo`, in the system's tick
/// count), `None` when Windows does not say.
#[cfg(windows)]
pub async fn idle_for() -> Option<Duration> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a valid `LASTINPUTINFO` with its size set, as the call requires.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // SAFETY: no arguments, no preconditions.
    let now = unsafe { GetTickCount() };
    // Both are 32-bit tick counts that wrap together.
    Some(Duration::from_millis(now.wrapping_sub(info.dwTime).into()))
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
}

/// How long since the last input event of any kind (`CGEventSourceSecondsSinceLastEventType`),
/// `None` when macOS gives no usable number.
#[cfg(target_os = "macos")]
pub async fn idle_for() -> Option<Duration> {
    // The combined session state (0) and any input event type (`~0`).
    // SAFETY: plain value arguments, no pointers.
    let seconds = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// No idle source on this platform: the caller assumes the user is present.
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub async fn idle_for() -> Option<Duration> {
    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::wayland::{Input, TIMEOUT};
    use std::time::{Duration, Instant};

    #[test]
    fn idle_counts_from_the_last_input_not_from_the_idled_event() {
        let start = Instant::now() + Duration::from_secs(3600);
        let idle = Input::idled(start);
        // The compositor says idle after TIMEOUT without input, so the user was already idle that long.
        assert_eq!(idle.idle_for(start), Some(TIMEOUT));
        let five_minutes_on = start + Duration::from_secs(5 * 60) - TIMEOUT;
        assert_eq!(
            idle.idle_for(five_minutes_on),
            Some(Duration::from_secs(5 * 60))
        );
    }

    #[test]
    fn input_makes_the_user_active_and_unknown_says_nothing() {
        assert_eq!(Input::Active.idle_for(Instant::now()), Some(Duration::ZERO));
        assert_eq!(Input::Unknown.idle_for(Instant::now()), None);
    }

    #[test]
    fn an_idle_start_after_now_is_zero_not_a_panic() {
        let now = Instant::now();
        let later = Input::IdleSince(now + Duration::from_secs(10));
        assert_eq!(later.idle_for(now), Some(Duration::ZERO));
    }
}
