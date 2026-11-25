# tray

## Overview
IronTeams tray and desktop integration: a StatusNotifierItem (SNI) tray icon with unread badge (Linux, via `ksni`/`zbus`), the Unity launcher badge signal, a screen-lock probe, a keyboard and mouse idle-time probe, and login autostart (XDG `.desktop` file, macOS LaunchAgent plist, Windows Run key). On Windows and macOS the app shows its own tray through Slint, so `Tray::start` there reports `UNSUPPORTED` and does nothing. Icons are drawn in code, with no image assets.

## Interface
- `APP_ID = "io.github.mohamedeslam04.ironteams"`; modules `autostart`, `icon`, `idle`, `launcher`, `menu`, `screen` (all `pub`) [lib.rs].
- `TrayEvent {Open, ToggleMute(bool), ToggleFocus(bool), Quit}`; `TrayOptions {id, title, icon_name, launcher_uri, muted, sync_launcher_badge}` with `Default`; `title_text`, `tooltip_text` [lib.rs].
- `Tray::start(opts) -> TrayHandle` (never fails); `TrayHandle` (`Clone`): `set_unread`, `set_muted`, `set_focus`, `set_launcher_count`, `events() -> crossbeam_channel::Receiver<TrayEvent>`, `tray_error() -> Option<&str>`, `launcher_ready`, `shutdown` [lib.rs].
- `UNSUPPORTED` constant on non-Linux [lib.rs].
- `autostart`: `FILE_NAME = "ironteams.desktop"`, `desktop_entry`, `config_home`, `entry_path_in`, `enable_in`, `disable_in`, `is_enabled_in`; `enable`, `disable`, `is_enabled` in three cfg variants (XDG, macOS, Windows); `LAUNCH_AGENT_LABEL`, `launch_agent`, `launch_agent_path_in`, `launch_agent_enable_in`, `launch_agent_disable_in`, `launch_agent_enabled_in`; `RUN_KEY`, `RUN_VALUE`, `run_command` [autostart.rs].
- `icon`: `Pixmap {width, height, argb}` (`to_rgba`, `pixel`), `SIZES = [22, 32, 48, 64]`, `badge_label`, `app_icon`, `badge_overlay`, `badged_icon` [icon.rs].
- `menu`: `MenuAction`, `MenuEntry`, `menu_model(muted, focus)` [menu.rs].
- `launcher`: `APP_URI`, `OBJECT_PATH`, `INTERFACE`, `SIGNAL`, `LauncherUpdate {app_uri, count, count_visible}`, `launcher_update`; Linux-only `LauncherUpdate::body` and `emit` [launcher.rs].
- `idle::idle_for() -> Option<Duration>` (async; time since the last keyboard or mouse input, `None` when the desktop cannot say or on other platforms) [idle.rs].
- `screen::screen_locked() -> Option<bool>` (always `None` off Linux) [screen.rs].

## Invariants
- Linux: the tray runs on its own thread `ironteams-tray` with a private current-thread tokio runtime; commands flow UI to worker over an unbounded tokio channel, events flow back over one shared crossbeam receiver (each event goes to exactly one receiver); send errors are ignored [lib.rs:Tray::start, worker].
- `Tray::start` blocks until the worker reports startup; failures are reported through `tray_error`, and the launcher badge still works without an SNI host [lib.rs:Tray::start].
- `shutdown`, `Drop` and `Inner::stop` send `Shutdown` and join the thread once; the worker clears the launcher badge (count 0) on exit [lib.rs:Inner::stop, worker].
- Non-Linux: no thread is spawned, `tray_error = UNSUPPORTED`, `launcher_ready = false` [lib.rs:Tray::start].
- Inside Flatpak (`/.flatpak-info` exists) the SNI well-known D-Bus name is disabled (`disable_dbus_name`); `assume_sni_available(true)` is applied in every case, not only in Flatpak [lib.rs:worker].
- `idle_for` on Linux tries the Wayland `ext_idle_notifier_v1` watcher first (only when `WAYLAND_DISPLAY` is set; KDE on Wayland refuses the D-Bus query), then `org.freedesktop.ScreenSaver.GetSessionIdleTime`, then `org.gnome.Mutter.IdleMonitor.GetIdletime`; `None` if none answers [idle.rs:idle_for].
- Wayland watcher: started once on the first `idle_for` call, on its own thread `idle-wayland` with its own `wayland_client` connection; it asks for one notification with a 30 s timeout (`get_input_idle_notification` on notifier v2, so idle inhibitors such as a playing video do not count as input; `get_idle_notification` on v1). `idled` records the idle start as now minus 30 s, `resumed` means active (idle 0). Until it has bound the notifier, without the protocol, or after the connection ends it says nothing (`Input::Unknown`) and D-Bus is asked instead [idle.rs:wayland].
- Badge label: 0 is hidden, 1..=99 is the number, more than 99 is `99+` [icon.rs:badge_label]. Pixmaps are straight-alpha ARGB; `Pixmap::pixel` panics out of range [icon.rs].
- Launcher: count 0 means `count_visible = false`; count is an `i64` [launcher.rs:launcher_update].
- Menu is fixed: Open, Mute (checked), Focus (checked), separator, Quit [menu.rs:menu_model].
- XDG autostart path is `<config_home>/autostart/ironteams.desktop`; `config_home` is `$XDG_CONFIG_HOME` only if absolute, else `$HOME/.config` [autostart.rs:config_home].
- Autostart writes are atomic (temp file plus rename) and also remove the legacy `kelma` entry (`kelma.desktop`, `...kelma.plist`, Windows value `kelma`) [autostart.rs].
- `is_enabled_in` is false if the entry has `Hidden=true` or `X-GNOME-Autostart-enabled=false` [autostart.rs:is_enabled_in].
- Windows: Run value `ironteams` holds the quoted executable plus an optional flag; macOS: plist has `Label`, `ProgramArguments`, `RunAtLoad`, `ProcessType` [autostart.rs].

## Side Effects
- D-Bus (Linux): session bus connection; SNI registration via `ksni`; broadcast of `com.canonical.Unity.LauncherEntry.Update`; `org.freedesktop.ScreenSaver.GetActive` call on every `screen_locked`; on every `idle_for` call the Wayland watcher cannot answer, a session bus connection and the `GetSessionIdleTime` call, then `GetIdletime` if that fails [lib.rs:worker, launcher.rs, screen.rs, idle.rs].
- Threads and channels created per `Tray::start` [lib.rs].
- Wayland (Linux): one long-lived compositor connection and thread `idle-wayland` for the idle notification [idle.rs:wayland].
- Disk: writes or removes the autostart desktop entry and the macOS LaunchAgent plist (no `launchctl` call in code) [autostart.rs].
- Windows registry: HKCU `...\Run` values written, deleted and read via `windows_sys` FFI [autostart.rs]. `idle_for` calls `GetLastInputInfo` and `GetTickCount` via `windows_sys` (Cargo enables `Win32_System_SystemInformation` and `Win32_UI_Input_KeyboardAndMouse` for it); on macOS it calls `CGEventSourceSecondsSinceLastEventType` from the CoreGraphics framework through `extern "C"` [idle.rs].
- Reads env vars `XDG_CONFIG_HOME`, `HOME` and `WAYLAND_DISPLAY`; probes `/.flatpak-info`; embeds `packaging/io.github.mohamedeslam04.ironteams.desktop` at compile time [autostart.rs, lib.rs].
- Logging (info): the idle source when it changes (`idle: input idle time source`, `source` = `wayland`, `dbus-screensaver`, `mutter` or `none`), the Wayland notification requested (notifier `version`, `input_only`), each `idled` and `resumed` event, and a warn when the watcher ends [idle.rs:note_source, wayland]. One global: the Wayland watcher's last idle state (`static INPUT`, a `Mutex`) and its start-once guard [idle.rs:wayland], plus the last logged source [idle.rs:note_source].

## Verification
- Run: `cargo test -p tray`
