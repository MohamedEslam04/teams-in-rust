//! Which way the card host draws, from what the session offers. [`choose`] is pure; [`detect`]
//! looks at the environment (and, on Wayland, asks the compositor for its globals).

use super::protocol::Backend;

/// The operating system, as a value so the choice can be tested for every system on any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    Mac,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Other
        }
    }
}

/// What the session offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Caps {
    /// A Wayland session (`WAYLAND_DISPLAY`).
    pub wayland: bool,
    /// The compositor advertises `zwlr_layer_shell_v1` (KWin, Sway, Hyprland; not GNOME).
    pub layer_shell: bool,
    /// An X server is reachable (`DISPLAY`): an X11 session, or XWayland next to Wayland.
    pub x11: bool,
}

/// Forced from the environment (`IRONTEAMS_CARD_HOST`), for testing a fallback on a desktop that
/// would pick another way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Force {
    LayerShell,
    X11,
    /// No cards: system notifications.
    Off,
}

impl Force {
    pub fn parse(v: &str) -> Option<Force> {
        match v.trim().to_ascii_lowercase().as_str() {
            "layer" | "layer-shell" => Some(Force::LayerShell),
            "x11" | "xwayland" => Some(Force::X11),
            "off" | "none" | "system" => Some(Force::Off),
            _ => None,
        }
    }
}

/// The backend for these capabilities, or `None` when cards cannot be drawn (the app then shows
/// system notifications). On Linux the layer shell wins: an overlay surface is placed by the
/// compositor above the panel's exclusive zone and needs no XWayland. Without it (GNOME) a
/// keep-above X11 window is used when an X server is there.
pub fn choose(os: Os, caps: &Caps, force: Option<Force>) -> Option<Backend> {
    match (os, force) {
        (_, Some(Force::Off)) => None,
        (Os::Linux, Some(Force::LayerShell)) => {
            (caps.wayland && caps.layer_shell).then_some(Backend::LayerShell)
        }
        (Os::Linux, Some(Force::X11)) => caps.x11.then_some(Backend::X11),
        (Os::Linux, None) if caps.wayland && caps.layer_shell => Some(Backend::LayerShell),
        (Os::Linux, None) if caps.x11 => Some(Backend::X11),
        (Os::Linux, _) => None,
        (Os::Windows | Os::Mac, _) => Some(Backend::Native),
        (Os::Other, _) => None,
    }
}

fn set(var: &str) -> bool {
    std::env::var_os(var).is_some_and(|v| !v.is_empty())
}

/// Looks at this session. `probe` asks the Wayland compositor whether it has the layer shell
/// (only called in a Wayland session).
pub fn detect(probe: impl FnOnce() -> bool) -> Caps {
    let wayland = set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET");
    Caps {
        wayland,
        layer_shell: wayland && probe(),
        x11: set("DISPLAY"),
    }
}

/// `IRONTEAMS_CARD_HOST`, when set to something known.
pub fn forced() -> Option<Force> {
    compat::var("IRONTEAMS_CARD_HOST")
        .ok()
        .and_then(|v| Force::parse(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KDE_WAYLAND: Caps = Caps {
        wayland: true,
        layer_shell: true,
        x11: true,
    };
    const GNOME_WAYLAND: Caps = Caps {
        wayland: true,
        layer_shell: false,
        x11: true,
    };
    const PURE_WAYLAND_NO_LAYER: Caps = Caps {
        wayland: true,
        layer_shell: false,
        x11: false,
    };
    const X11_SESSION: Caps = Caps {
        wayland: false,
        layer_shell: false,
        x11: true,
    };

    #[test]
    fn linux_prefers_the_layer_shell_then_xwayland_then_nothing() {
        assert_eq!(
            choose(Os::Linux, &KDE_WAYLAND, None),
            Some(Backend::LayerShell)
        );
        assert_eq!(
            choose(Os::Linux, &GNOME_WAYLAND, None),
            Some(Backend::X11),
            "GNOME has no layer shell: XWayland"
        );
        assert_eq!(choose(Os::Linux, &X11_SESSION, None), Some(Backend::X11));
        assert_eq!(choose(Os::Linux, &PURE_WAYLAND_NO_LAYER, None), None);
        assert_eq!(
            choose(Os::Linux, &Caps::default(), None),
            None,
            "no display at all"
        );
        // A layer shell without a Wayland session cannot happen, and is not trusted.
        let odd = Caps {
            wayland: false,
            layer_shell: true,
            x11: false,
        };
        assert_eq!(choose(Os::Linux, &odd, None), None);
    }

    #[test]
    fn windows_and_macos_use_a_native_window() {
        for os in [Os::Windows, Os::Mac] {
            assert_eq!(choose(os, &Caps::default(), None), Some(Backend::Native));
        }
        assert_eq!(choose(Os::Other, &KDE_WAYLAND, None), None);
    }

    #[test]
    fn the_environment_can_force_a_fallback_but_not_an_impossible_backend() {
        assert_eq!(
            choose(Os::Linux, &KDE_WAYLAND, Some(Force::X11)),
            Some(Backend::X11)
        );
        assert_eq!(choose(Os::Linux, &KDE_WAYLAND, Some(Force::Off)), None);
        assert_eq!(
            choose(Os::Windows, &Caps::default(), Some(Force::Off)),
            None
        );
        assert_eq!(
            choose(Os::Linux, &GNOME_WAYLAND, Some(Force::LayerShell)),
            None
        );
        assert_eq!(
            choose(Os::Linux, &PURE_WAYLAND_NO_LAYER, Some(Force::X11)),
            None
        );
        assert_eq!(Force::parse(" X11 "), Some(Force::X11));
        assert_eq!(Force::parse("layer-shell"), Some(Force::LayerShell));
        assert_eq!(Force::parse("off"), Some(Force::Off));
        assert_eq!(Force::parse("bubbles"), None);
    }
}
