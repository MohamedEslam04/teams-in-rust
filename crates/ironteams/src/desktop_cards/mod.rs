//! Teams-style notification cards drawn by IronTeams on the desktop, outside the app window:
//! purple cards in the bottom-right corner of the screen, above other windows, also while
//! IronTeams is minimised or in the tray. Teams on Windows does the same: it draws its own
//! toasts instead of the system ones. Settings › Notifications › "Notification style" switches
//! between these cards and the system notifications (`GeneralPrefs::notification_style`).
//!
//! # Why a separate process
//!
//! The main window owns the winit event loop and its Wayland connection, and on Wayland an
//! ordinary window can neither choose its position nor stay on top. The cards therefore live in
//! a small host process: the same executable started as `ironteams --toast-host` ([`HOST_ARG`]),
//! so there is nothing new to install (Flatpak, AppImage, RPM, Velopack). It is started on the
//! first card, kept running, and quits when the app closes its stdin. They talk in JSON lines
//! ([`protocol`]): the app sends cards, the host answers with what the user did; [`client`] is
//! the app's end, [`host`] the host's.
//!
//! # How the host draws ([`select`])
//!
//! 1. **wlr-layer-shell** ([`layer`], Linux) when the compositor advertises
//!    `zwlr_layer_shell_v1` (KWin on Plasma 6, Sway, Hyprland). An overlay surface anchored
//!    bottom-right is placed by the compositor itself, above the panel, on the active output,
//!    and `keyboard_interactivity: on_demand` lets the reply box take the keyboard. Slint has no
//!    layer-shell backend and the maintained bridges are AGPL/GPL (layer-shika, spell-framework),
//!    so the host renders with Slint's software renderer into wl_shm buffers through a minimal
//!    custom platform on smithay-client-toolkit (MIT), which the tree already used.
//! 2. **XWayland / X11** when there is no layer shell (GNOME) but an X server: a borderless,
//!    keep-above window of the notification type, placed in the corner of the primary monitor;
//!    the window manager honours an X11 client's position.
//! 3. **Windows and macOS**: the same borderless always-on-top window, out of the taskbar (an
//!    accessory app without a Dock icon on macOS).
//!
//! Without any of these (a Wayland compositor with neither layer shell nor XWayland, or
//! `IRONTEAMS_CARD_HOST=off`), and whenever the host fails to start or crashes too often, the
//! app shows the system notifications, as before the cards. `IRONTEAMS_CARD_HOST=x11` forces
//! the X11 window on a desktop that has the layer shell, to try the fallback.
//!
//! # What shows where
//!
//! `backend/banner.rs` (`route`) decides, after the notification rules (pause, quiet hours,
//! Focus, muted chats): while the main window is the active one a message is the card inside
//! the window; otherwise it is a desktop card. Calendar reminders are desktop cards even then.
//! While the desktop's Do Not Disturb is on (`org.freedesktop.Notifications` `Inhibited`), the
//! system notification is sent instead, which the desktop keeps quietly in its history; an
//! incoming call still shows its card. `backend/cards.rs` builds the cards and acts on their
//! buttons.

pub mod client;
pub mod geometry;
pub mod host;
#[cfg(target_os = "linux")]
mod layer;
pub mod protocol;
pub mod select;

/// The argument that starts the executable as the card host.
pub const HOST_ARG: &str = "--toast-host";
