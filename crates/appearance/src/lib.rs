//! IronTeams appearance preferences.
//!
//! - [`AppearancePrefs`]: theme, accent, density, font scale and message style, kept in `appearance.json` in the
//!   app config dir (atomic writes; a missing or corrupt file gives the defaults).
//! - [`portal`]: reads and watches the desktop's color scheme and accent over the XDG portal.
//! - [`GeneralPrefs`] (language, formats, start-up) and [`AccessibilityPrefs`] (contrast, motion,
//!   larger text), each in its own file next to `appearance.json`.
//! - [`ShortcutPrefs`]: the keyboard shortcuts the user rebound, in `shortcuts.json`.
//! - [`resolve`]: pure functions that combine the two into what the UI should draw.
//! - The theme engine, not wired into the UI yet: [`themes`] (built-in and custom themes, derived
//!   tokens, contrast report, `themes.json`), [`tokens`] (the token set), [`oklch`] (colour
//!   maths) and [`share`] (the theme share line). [`resolve::resolve_look`] picks the theme.

pub mod accessibility;
pub mod formats;
pub mod general;
pub mod language;
pub mod oklch;
pub mod portal;
pub mod prefs;
pub mod renderer;
pub mod resolve;
pub mod share;
pub mod shortcuts;
pub mod snippets;
mod store;
pub mod themes;
pub mod tokens;

pub use accessibility::{
    AccessibilityPrefs, AutoSwitch, effective_font_scale, resolve_high_contrast,
    resolve_reduced_motion,
};
pub use general::{
    DateFormat, GeneralPrefs, Language, NotificationStyle, TimeFormat, available_languages,
    effective_language, resolve_language, system_language,
};
pub use portal::{PortalChange, PortalError, PortalScheme, PortalSettings, PortalStream};
pub use prefs::{
    AppearancePrefs, Density, FONT_SCALE_MAX, FONT_SCALE_MIN, MessageStyle, Rgb, Theme,
};
pub use renderer::Renderer;
pub use resolve::{
    Fallback, Resolved, ResolvedLook, ResolvedTheme, resolve, resolve_accent, resolve_font_scale,
    resolve_look, resolve_theme,
};
pub use shortcuts::ShortcutPrefs;
pub use snippets::{Snippet, SnippetPrefs};
pub use store::{Error, config_dir};
pub use themes::{Base, CallChoice, Report, Switch, ThemeChoice, ThemeDef, ThemesFile};
pub use tokens::Tokens;
