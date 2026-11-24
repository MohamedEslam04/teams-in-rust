//! How long the user has not touched the keyboard or the mouse. Presence uses it to report
//! "not active" after a while, so the service shows Away, as Teams does.

use std::time::Duration;

/// How long since the last keyboard or mouse input, `None` when the desktop cannot say (the caller
/// then assumes the user is present). Linux asks the desktop's screen saver service (KDE and most
/// others), then GNOME's idle monitor.
#[cfg(target_os = "linux")]
pub async fn idle_for() -> Option<Duration> {
    let conn = zbus::Connection::session().await.ok()?;
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
        return Some(Duration::from_millis(ms.into()));
    }
    let reply = conn
        .call_method(
            Some("org.gnome.Mutter.IdleMonitor"),
            "/org/gnome/Mutter/IdleMonitor/Core",
            Some("org.gnome.Mutter.IdleMonitor"),
            "GetIdletime",
            &(),
        )
        .await
        .ok()?;
    let ms = reply.body().deserialize::<u64>().ok()?;
    Some(Duration::from_millis(ms))
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
