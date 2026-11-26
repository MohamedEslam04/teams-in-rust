//! The token set: every colour one region of the UI draws with, as `0xRRGGBBAA` values, plus the
//! four literal palettes (IronTeams Dark and Light and their high-contrast forms) that keep
//! today's exact colours.
//!
//! The names and order are those of `Tokens` in `ui/tokens/types.slint`; [`NAMES`] has the Slint
//! (hyphenated) spelling of each field.

use crate::prefs::Rgb;
use crate::resolve::ResolvedTheme;

macro_rules! tokens {
    ($($field:ident = $name:literal,)*) => {
        /// The colours of one region. Alpha is the low byte; `dark` is the region's kind.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Tokens {
            $(pub $field: u32,)*
            pub dark: bool,
        }

        /// The Slint name of each colour field, in field order.
        pub const NAMES: [&str; 49] = [$($name),*];

        impl Tokens {
            /// The colour called `name` (its Slint spelling, for example `fg-muted`).
            pub fn get(&self, name: &str) -> Option<u32> {
                match name {
                    $($name => Some(self.$field),)*
                    _ => None,
                }
            }

            /// Sets the colour called `name`; false when there is no such token.
            pub fn set(&mut self, name: &str, v: u32) -> bool {
                match name {
                    $($name => self.$field = v,)*
                    _ => return false,
                }
                true
            }
        }
    };
}

tokens! {
    bg = "bg",
    surface = "surface",
    surface_hover = "surface-hover",
    selected = "selected",
    border = "border",
    panel = "panel",
    rail = "rail",
    topbar = "topbar",
    bg_alt = "bg-alt",
    bg_sunk = "bg-sunk",
    grid_line = "grid-line",
    grid_line_soft = "grid-line-soft",
    fg = "fg",
    fg_muted = "fg-muted",
    fg_subtle = "fg-subtle",
    fg_strong = "fg-strong",
    accent = "accent",
    accent_strong = "accent-strong",
    on_accent = "on-accent",
    accent_soft = "accent-soft",
    accent_subtle = "accent-subtle",
    accent_muted = "accent-muted",
    accent_faint = "accent-faint",
    accent_wash = "accent-wash",
    accent_tint = "accent-tint",
    bubble_mine = "bubble-mine",
    bubble_other = "bubble-other",
    bubble_mention = "bubble-mention",
    mention = "mention",
    code_bg = "code-bg",
    danger = "danger",
    danger_hover = "danger-hover",
    on_danger = "on-danger",
    danger_text = "danger-text",
    danger_soft = "danger-soft",
    warning = "warning",
    warning_soft = "warning-soft",
    success = "success",
    success_soft = "success-soft",
    info_soft = "info-soft",
    unread_badge = "unread-badge",
    on_badge = "on-badge",
    presence_available = "presence-available",
    presence_busy = "presence-busy",
    presence_away = "presence-away",
    presence_offline = "presence-offline",
    scrim = "scrim",
    overlay_soft = "overlay-soft",
    shimmer = "shimmer",
}

/// `0xRRGGBB` as a fully opaque `0xRRGGBBAA`.
pub const fn opaque(rgb: u32) -> u32 {
    (rgb << 8) | 0xff
}

/// The colour of a token without its alpha.
pub fn to_rgb(c: u32) -> Rgb {
    Rgb::new((c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8)
}

/// A token colour from an opaque colour and an alpha.
pub fn from_rgb(c: Rgb, alpha: u8) -> u32 {
    (u32::from(c.r) << 24) | (u32::from(c.g) << 16) | (u32::from(c.b) << 8) | u32::from(alpha)
}

/// Mixes `a` towards `b` by `t` (0 is `a`, 1 is `b`). Both are `0xRRGGBB`.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| -> u32 {
        let (x, y) = (((c >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << s
    };
    ch(a, 16) | ch(a, 8) | ch(a, 0)
}

fn hex(c: Rgb) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

/// The dark palette IronTeams always had.
pub const DARK: Tokens = Tokens {
    bg: 0x141414ff,
    surface: 0x292929ff,
    surface_hover: 0x333333ff,
    selected: 0x2e2e2eff,
    border: 0x3d3d3dff,
    panel: 0x1f1f1fff,
    rail: 0x0f0f0fff,
    topbar: 0x0f0f0fff,
    bg_alt: 0x181818ff,
    bg_sunk: 0x101010ff,
    grid_line: 0x3a3a3aff,
    grid_line_soft: 0x262626ff,
    fg: 0xffffffff,
    fg_muted: 0xadadadff,
    fg_subtle: 0x8a8a8aff,
    fg_strong: 0xffffffff,
    accent: 0x7f85f5ff,
    accent_strong: 0x5b5fc7ff,
    on_accent: 0xffffffff,
    accent_soft: 0x3b3d6bff,
    accent_subtle: 0x34365cff,
    accent_muted: 0x2b2d52ff,
    accent_faint: 0x17172aff,
    accent_wash: 0x7f85f526,
    accent_tint: 0x7f85f540,
    bubble_mine: 0x2f2f4aff,
    bubble_other: 0x292929ff,
    bubble_mention: 0x3a3a63ff,
    mention: 0xb4b8ffff,
    code_bg: 0x1b1b1bff,
    danger: 0xd13438ff,
    danger_hover: 0xe04a4fff,
    on_danger: 0xffffffff,
    danger_text: 0xff8a8dff,
    danger_soft: 0x3b1f22ff,
    warning: 0xffb900ff,
    warning_soft: 0x3a3320ff,
    success: 0x6bd69bff,
    success_soft: 0x1e2f27ff,
    info_soft: 0x1f2a3bff,
    unread_badge: 0xc4314bff,
    on_badge: 0xffffffff,
    presence_available: 0x6bb700ff,
    presence_busy: 0xc4314bff,
    presence_away: 0xffaa44ff,
    presence_offline: 0x8a8886ff,
    scrim: 0x00000080,
    overlay_soft: 0xffffff10,
    shimmer: 0xffffff14,
    dark: true,
};

/// Teams-like light colours.
pub const LIGHT: Tokens = Tokens {
    bg: 0xffffffff,
    surface: 0xf0f0f0ff,
    surface_hover: 0xe0e0e0ff,
    selected: 0xe8ebfaff,
    border: 0xd1d1d1ff,
    panel: 0xf5f5f5ff,
    rail: 0xebebebff,
    topbar: 0xebebebff,
    bg_alt: 0x181818ff,
    bg_sunk: 0x101010ff,
    grid_line: 0x3a3a3aff,
    grid_line_soft: 0x262626ff,
    fg: 0x242424ff,
    fg_muted: 0x5c5c5cff,
    fg_subtle: 0x6e6e6eff,
    fg_strong: 0xffffffff,
    accent: 0x5b5fc7ff,
    accent_strong: 0x4f52b2ff,
    on_accent: 0xffffffff,
    accent_soft: 0x3b3d6bff,
    accent_subtle: 0x34365cff,
    accent_muted: 0x2b2d52ff,
    accent_faint: 0x17172aff,
    accent_wash: 0x7f85f526,
    accent_tint: 0x7f85f540,
    bubble_mine: 0xe8ebfaff,
    bubble_other: 0xf5f5f5ff,
    bubble_mention: 0x3a3a63ff,
    mention: 0x4f52b2ff,
    code_bg: 0xebebebff,
    danger: 0xc4314bff,
    danger_hover: 0xe04a4fff,
    on_danger: 0xffffffff,
    danger_text: 0xff8a8dff,
    danger_soft: 0x3b1f22ff,
    warning: 0xffb900ff,
    warning_soft: 0xfff4ceff,
    success: 0x6bd69bff,
    success_soft: 0x1e2f27ff,
    info_soft: 0x1f2a3bff,
    unread_badge: 0xc4314bff,
    on_badge: 0xffffffff,
    presence_available: 0x6bb700ff,
    presence_busy: 0xc4314bff,
    presence_away: 0xffaa44ff,
    presence_offline: 0x8a8886ff,
    scrim: 0x00000080,
    overlay_soft: 0x0000000f,
    shimmer: 0xffffffb0,
    dark: false,
};

/// High contrast on black: white text and borders, yellow and cyan highlights.
pub const CONTRAST_DARK: Tokens = Tokens {
    bg: 0x000000ff,
    surface: 0x000000ff,
    surface_hover: 0x1f1f1fff,
    selected: 0x3d3d00ff,
    border: 0xffffffff,
    panel: 0x000000ff,
    rail: 0x000000ff,
    topbar: 0x000000ff,
    bg_alt: 0x181818ff,
    bg_sunk: 0x101010ff,
    grid_line: 0x3a3a3aff,
    grid_line_soft: 0x262626ff,
    fg: 0xffffffff,
    fg_muted: 0xffffffff,
    fg_subtle: 0xe6e6e6ff,
    fg_strong: 0xffffffff,
    accent: 0xffff00ff,
    accent_strong: 0x1aebffff,
    on_accent: 0xffffffff,
    accent_soft: 0x3b3d6bff,
    accent_subtle: 0x34365cff,
    accent_muted: 0x2b2d52ff,
    accent_faint: 0x17172aff,
    accent_wash: 0x7f85f526,
    accent_tint: 0x7f85f540,
    bubble_mine: 0x000000ff,
    bubble_other: 0x000000ff,
    bubble_mention: 0x3a3a63ff,
    mention: 0xffff00ff,
    code_bg: 0x000000ff,
    danger: 0xff6b6bff,
    danger_hover: 0xe04a4fff,
    on_danger: 0xffffffff,
    danger_text: 0xff8a8dff,
    danger_soft: 0x3b1f22ff,
    warning: 0xffb900ff,
    warning_soft: 0x3a3320ff,
    success: 0x6bd69bff,
    success_soft: 0x1e2f27ff,
    info_soft: 0x1f2a3bff,
    unread_badge: 0xffff00ff,
    on_badge: 0xffffffff,
    presence_available: 0x6bb700ff,
    presence_busy: 0xc4314bff,
    presence_away: 0xffaa44ff,
    presence_offline: 0x8a8886ff,
    scrim: 0x00000080,
    overlay_soft: 0xffffff10,
    shimmer: 0xffffff14,
    dark: true,
};

/// High contrast on white: black text and borders, dark blue highlights.
pub const CONTRAST_LIGHT: Tokens = Tokens {
    bg: 0xffffffff,
    surface: 0xffffffff,
    surface_hover: 0xe6e6e6ff,
    selected: 0xd6e4ffff,
    border: 0x000000ff,
    panel: 0xffffffff,
    rail: 0xffffffff,
    topbar: 0xffffffff,
    bg_alt: 0x181818ff,
    bg_sunk: 0x101010ff,
    grid_line: 0x3a3a3aff,
    grid_line_soft: 0x262626ff,
    fg: 0x000000ff,
    fg_muted: 0x000000ff,
    fg_subtle: 0x1a1a1aff,
    fg_strong: 0xffffffff,
    accent: 0x00009fff,
    accent_strong: 0x00009fff,
    on_accent: 0xffffffff,
    accent_soft: 0x3b3d6bff,
    accent_subtle: 0x34365cff,
    accent_muted: 0x2b2d52ff,
    accent_faint: 0x17172aff,
    accent_wash: 0x7f85f526,
    accent_tint: 0x7f85f540,
    bubble_mine: 0xffffffff,
    bubble_other: 0xffffffff,
    bubble_mention: 0x3a3a63ff,
    mention: 0x00009fff,
    code_bg: 0xffffffff,
    danger: 0xa80000ff,
    danger_hover: 0xe04a4fff,
    on_danger: 0xffffffff,
    danger_text: 0xff8a8dff,
    danger_soft: 0x3b1f22ff,
    warning: 0xffb900ff,
    warning_soft: 0xfff4ceff,
    success: 0x6bd69bff,
    success_soft: 0x1e2f27ff,
    info_soft: 0x1f2a3bff,
    unread_badge: 0xa80000ff,
    on_badge: 0xffffffff,
    presence_available: 0x6bb700ff,
    presence_busy: 0xc4314bff,
    presence_away: 0xffaa44ff,
    presence_offline: 0x8a8886ff,
    scrim: 0x00000080,
    overlay_soft: 0x0000000f,
    shimmer: 0xffffffb0,
    dark: false,
};

/// The literal palette for a kind and contrast mode, with the accent rules IronTeams always had
/// for `DARK` and `LIGHT`. High contrast keeps its own highlights and ignores the accent.
pub fn literal(kind: ResolvedTheme, high_contrast: bool, accent: Option<Rgb>) -> Tokens {
    let dark = kind == ResolvedTheme::Dark;
    if high_contrast {
        return if dark { CONTRAST_DARK } else { CONTRAST_LIGHT };
    }
    let mut p = if dark { DARK } else { LIGHT };
    if let Some(a) = accent.map(hex) {
        let (accent, strong, mention, bubble_mine) = if dark {
            (
                mix(a, 0xffffff, 0.3),
                a,
                mix(a, 0xffffff, 0.55),
                mix(p.bg >> 8, a, 0.25),
            )
        } else {
            p.selected = opaque(mix(0xffffff, a, 0.12));
            (
                a,
                mix(a, 0x000000, 0.15),
                mix(a, 0x000000, 0.2),
                mix(0xffffff, a, 0.12),
            )
        };
        p.accent = opaque(accent);
        p.accent_strong = opaque(strong);
        p.mention = opaque(mention);
        p.bubble_mine = opaque(bubble_mine);
    }
    p
}

/// The nav set of a literal theme: the content set with `bg` replaced by `panel`.
pub fn nav_of_literal(content: &Tokens) -> Tokens {
    Tokens {
        bg: content.panel,
        ..*content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every colour of the four palettes, written out: token, then `DARK`, `LIGHT`,
    /// `CONTRAST_DARK`, `CONTRAST_LIGHT`.
    #[rustfmt::skip]
    const EXPECTED: [(&str, [u32; 4]); 49] = [
        ("bg", [0x141414ff, 0xffffffff, 0x000000ff, 0xffffffff]),
        ("surface", [0x292929ff, 0xf0f0f0ff, 0x000000ff, 0xffffffff]),
        ("surface-hover", [0x333333ff, 0xe0e0e0ff, 0x1f1f1fff, 0xe6e6e6ff]),
        ("selected", [0x2e2e2eff, 0xe8ebfaff, 0x3d3d00ff, 0xd6e4ffff]),
        ("border", [0x3d3d3dff, 0xd1d1d1ff, 0xffffffff, 0x000000ff]),
        ("panel", [0x1f1f1fff, 0xf5f5f5ff, 0x000000ff, 0xffffffff]),
        ("rail", [0x0f0f0fff, 0xebebebff, 0x000000ff, 0xffffffff]),
        ("topbar", [0x0f0f0fff, 0xebebebff, 0x000000ff, 0xffffffff]),
        ("bg-alt", [0x181818ff, 0x181818ff, 0x181818ff, 0x181818ff]),
        ("bg-sunk", [0x101010ff, 0x101010ff, 0x101010ff, 0x101010ff]),
        ("grid-line", [0x3a3a3aff, 0x3a3a3aff, 0x3a3a3aff, 0x3a3a3aff]),
        ("grid-line-soft", [0x262626ff, 0x262626ff, 0x262626ff, 0x262626ff]),
        ("fg", [0xffffffff, 0x242424ff, 0xffffffff, 0x000000ff]),
        ("fg-muted", [0xadadadff, 0x5c5c5cff, 0xffffffff, 0x000000ff]),
        ("fg-subtle", [0x8a8a8aff, 0x6e6e6eff, 0xe6e6e6ff, 0x1a1a1aff]),
        ("fg-strong", [0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff]),
        ("accent", [0x7f85f5ff, 0x5b5fc7ff, 0xffff00ff, 0x00009fff]),
        ("accent-strong", [0x5b5fc7ff, 0x4f52b2ff, 0x1aebffff, 0x00009fff]),
        ("on-accent", [0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff]),
        ("accent-soft", [0x3b3d6bff, 0x3b3d6bff, 0x3b3d6bff, 0x3b3d6bff]),
        ("accent-subtle", [0x34365cff, 0x34365cff, 0x34365cff, 0x34365cff]),
        ("accent-muted", [0x2b2d52ff, 0x2b2d52ff, 0x2b2d52ff, 0x2b2d52ff]),
        ("accent-faint", [0x17172aff, 0x17172aff, 0x17172aff, 0x17172aff]),
        ("accent-wash", [0x7f85f526, 0x7f85f526, 0x7f85f526, 0x7f85f526]),
        ("accent-tint", [0x7f85f540, 0x7f85f540, 0x7f85f540, 0x7f85f540]),
        ("bubble-mine", [0x2f2f4aff, 0xe8ebfaff, 0x000000ff, 0xffffffff]),
        ("bubble-other", [0x292929ff, 0xf5f5f5ff, 0x000000ff, 0xffffffff]),
        ("bubble-mention", [0x3a3a63ff, 0x3a3a63ff, 0x3a3a63ff, 0x3a3a63ff]),
        ("mention", [0xb4b8ffff, 0x4f52b2ff, 0xffff00ff, 0x00009fff]),
        ("code-bg", [0x1b1b1bff, 0xebebebff, 0x000000ff, 0xffffffff]),
        ("danger", [0xd13438ff, 0xc4314bff, 0xff6b6bff, 0xa80000ff]),
        ("danger-hover", [0xe04a4fff, 0xe04a4fff, 0xe04a4fff, 0xe04a4fff]),
        ("on-danger", [0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff]),
        ("danger-text", [0xff8a8dff, 0xff8a8dff, 0xff8a8dff, 0xff8a8dff]),
        ("danger-soft", [0x3b1f22ff, 0x3b1f22ff, 0x3b1f22ff, 0x3b1f22ff]),
        ("warning", [0xffb900ff, 0xffb900ff, 0xffb900ff, 0xffb900ff]),
        ("warning-soft", [0x3a3320ff, 0xfff4ceff, 0x3a3320ff, 0xfff4ceff]),
        ("success", [0x6bd69bff, 0x6bd69bff, 0x6bd69bff, 0x6bd69bff]),
        ("success-soft", [0x1e2f27ff, 0x1e2f27ff, 0x1e2f27ff, 0x1e2f27ff]),
        ("info-soft", [0x1f2a3bff, 0x1f2a3bff, 0x1f2a3bff, 0x1f2a3bff]),
        ("unread-badge", [0xc4314bff, 0xc4314bff, 0xffff00ff, 0xa80000ff]),
        ("on-badge", [0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff]),
        ("presence-available", [0x6bb700ff, 0x6bb700ff, 0x6bb700ff, 0x6bb700ff]),
        ("presence-busy", [0xc4314bff, 0xc4314bff, 0xc4314bff, 0xc4314bff]),
        ("presence-away", [0xffaa44ff, 0xffaa44ff, 0xffaa44ff, 0xffaa44ff]),
        ("presence-offline", [0x8a8886ff, 0x8a8886ff, 0x8a8886ff, 0x8a8886ff]),
        ("scrim", [0x00000080, 0x00000080, 0x00000080, 0x00000080]),
        ("overlay-soft", [0xffffff10, 0x0000000f, 0xffffff10, 0x0000000f]),
        ("shimmer", [0xffffff14, 0xffffffb0, 0xffffff14, 0xffffffb0]),
    ];

    #[test]
    fn the_palettes_hold_every_expected_value() {
        let palettes = [DARK, LIGHT, CONTRAST_DARK, CONTRAST_LIGHT];
        for (name, want) in EXPECTED {
            for (p, w) in palettes.iter().zip(want) {
                assert_eq!(p.get(name), Some(w), "{name}");
            }
        }
        assert_eq!(
            palettes.map(|p| p.dark),
            [true, false, true, false],
            "the kind of each palette"
        );
    }

    #[test]
    fn names_get_and_set_cover_all_49_fields() {
        let expected_names: Vec<&str> = EXPECTED.iter().map(|(n, _)| *n).collect();
        assert_eq!(NAMES.to_vec(), expected_names);
        let mut t = DARK;
        for (i, name) in NAMES.iter().enumerate() {
            let v = 0x0102_0300 + i as u32;
            assert!(t.set(name, v), "{name}");
            assert_eq!(t.get(name), Some(v), "{name}");
        }
        // Every field was written, so no two tokens share a value now and none is left as DARK had it.
        let mut seen = std::collections::BTreeSet::new();
        for name in NAMES {
            assert!(seen.insert(t.get(name)), "{name}");
        }
        assert!(!t.set("no-such-token", 1));
        assert_eq!(t.get("no-such-token"), None);
        assert_eq!(t.get("main"), None, "the old name is not a token");
    }

    #[test]
    fn colour_helpers() {
        assert_eq!(opaque(0x123456), 0x123456ff);
        let c = Rgb::new(0x12, 0x34, 0x56);
        assert_eq!(from_rgb(c, 0x26), 0x12345626);
        assert_eq!(to_rgb(0x12345626), c);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x102030, 0.7), 0x102030);
    }

    #[test]
    fn dark_with_an_accent_follows_the_mix_rules() {
        let t = literal(
            ResolvedTheme::Dark,
            false,
            Some(Rgb::from_hex("#0f6cbd").unwrap()),
        );
        assert_eq!(t.accent, 0x5798d1ff);
        assert_eq!(t.accent_strong, 0x0f6cbdff);
        assert_eq!(t.mention, 0x93bde1ff);
        assert_eq!(t.bubble_mine, 0x132a3eff);
        // Every other field is DARK's.
        let mut rest = t;
        for name in ["accent", "accent-strong", "mention", "bubble-mine"] {
            rest.set(name, DARK.get(name).unwrap());
        }
        assert_eq!(rest, DARK);
    }

    #[test]
    fn light_with_an_accent_follows_the_mix_rules() {
        let t = literal(
            ResolvedTheme::Light,
            false,
            Some(Rgb::from_hex("#0f6cbd").unwrap()),
        );
        assert_eq!(t.accent, 0x0f6cbdff);
        assert_eq!(t.accent_strong, 0x0d5ca1ff);
        assert_eq!(t.mention, 0x0c5697ff);
        assert_eq!(t.bubble_mine, 0xe2edf7ff);
        assert_eq!(t.selected, 0xe2edf7ff);
        let mut rest = t;
        for name in [
            "accent",
            "accent-strong",
            "mention",
            "bubble-mine",
            "selected",
        ] {
            rest.set(name, LIGHT.get(name).unwrap());
        }
        assert_eq!(rest, LIGHT);
    }

    #[test]
    fn no_accent_and_high_contrast_ignore_the_accent() {
        let a = Some(Rgb::new(1, 2, 3));
        assert_eq!(literal(ResolvedTheme::Dark, false, None), DARK);
        assert_eq!(literal(ResolvedTheme::Light, false, None), LIGHT);
        assert_eq!(literal(ResolvedTheme::Dark, true, a), CONTRAST_DARK);
        assert_eq!(literal(ResolvedTheme::Light, true, a), CONTRAST_LIGHT);
    }

    #[test]
    fn the_nav_of_a_literal_theme_has_panel_as_bg() {
        for t in [DARK, LIGHT] {
            let nav = nav_of_literal(&t);
            assert_eq!(nav.bg, t.panel);
            assert_eq!(Tokens { bg: t.bg, ..nav }, t);
        }
    }
}
