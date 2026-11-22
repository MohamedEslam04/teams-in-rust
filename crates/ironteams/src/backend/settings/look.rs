use super::*;

// ---------------------------------------------------------------------------------------------
// Look: what the theme globals get.

/// The theme colours, as `0xrrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub rail: u32,
    pub sidebar: u32,
    pub main: u32,
    pub topbar: u32,
    pub surface: u32,
    pub surface_hover: u32,
    pub selected: u32,
    pub border: u32,
    pub accent: u32,
    pub accent_strong: u32,
    pub fg: u32,
    pub fg_muted: u32,
    pub fg_subtle: u32,
    pub bubble_mine: u32,
    pub bubble_other: u32,
    pub mention: u32,
    pub danger: u32,
    pub unread_badge: u32,
}

/// The dark palette IronTeams always had (ui/theme.slint defaults).
pub const DARK: Palette = Palette {
    rail: 0x0f0f0f,
    sidebar: 0x1f1f1f,
    main: 0x141414,
    topbar: 0x0f0f0f,
    surface: 0x292929,
    surface_hover: 0x333333,
    selected: 0x2e2e2e,
    border: 0x3d3d3d,
    accent: 0x7f85f5,
    accent_strong: 0x5b5fc7,
    fg: 0xffffff,
    fg_muted: 0xadadad,
    fg_subtle: 0x8a8a8a,
    bubble_mine: 0x2f2f4a,
    bubble_other: 0x292929,
    mention: 0xb4b8ff,
    danger: 0xd13438,
    unread_badge: 0xc4314b,
};

/// Teams-like light colours.
pub const LIGHT: Palette = Palette {
    rail: 0xebebeb,
    sidebar: 0xf5f5f5,
    main: 0xffffff,
    topbar: 0xebebeb,
    surface: 0xf0f0f0,
    surface_hover: 0xe0e0e0,
    selected: 0xe8ebfa,
    border: 0xd1d1d1,
    accent: 0x5b5fc7,
    accent_strong: 0x4f52b2,
    fg: 0x242424,
    fg_muted: 0x5c5c5c,
    fg_subtle: 0x6e6e6e,
    bubble_mine: 0xe8ebfa,
    bubble_other: 0xf5f5f5,
    mention: 0x4f52b2,
    danger: 0xc4314b,
    unread_badge: 0xc4314b,
};

/// High contrast on black: white text and borders, yellow and cyan highlights.
pub const CONTRAST_DARK: Palette = Palette {
    rail: 0x000000,
    sidebar: 0x000000,
    main: 0x000000,
    topbar: 0x000000,
    surface: 0x000000,
    surface_hover: 0x1f1f1f,
    selected: 0x3d3d00,
    border: 0xffffff,
    accent: 0xffff00,
    accent_strong: 0x1aebff,
    fg: 0xffffff,
    fg_muted: 0xffffff,
    fg_subtle: 0xe6e6e6,
    bubble_mine: 0x000000,
    bubble_other: 0x000000,
    mention: 0xffff00,
    danger: 0xff6b6b,
    unread_badge: 0xffff00,
};

/// High contrast on white: black text and borders, dark blue highlights.
pub const CONTRAST_LIGHT: Palette = Palette {
    rail: 0xffffff,
    sidebar: 0xffffff,
    main: 0xffffff,
    topbar: 0xffffff,
    surface: 0xffffff,
    surface_hover: 0xe6e6e6,
    selected: 0xd6e4ff,
    border: 0x000000,
    accent: 0x00009f,
    accent_strong: 0x00009f,
    fg: 0x000000,
    fg_muted: 0x000000,
    fg_subtle: 0x1a1a1a,
    bubble_mine: 0xffffff,
    bubble_other: 0xffffff,
    mention: 0x00009f,
    danger: 0xa80000,
    unread_badge: 0xa80000,
};

pub(super) fn hex(c: Rgb) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

/// Mixes `a` towards `b` by `t` (0 is `a`, 1 is `b`).
pub(super) fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| -> u32 {
        let (x, y) = (((c >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << s
    };
    ch(a, 16) | ch(a, 8) | ch(a, 0)
}

/// The palette for a theme, contrast mode and accent. High contrast keeps its own highlights.
pub fn palette(theme: ResolvedTheme, high_contrast: bool, accent: Option<Rgb>) -> Palette {
    let dark = theme == ResolvedTheme::Dark;
    if high_contrast {
        return if dark { CONTRAST_DARK } else { CONTRAST_LIGHT };
    }
    let mut p = if dark { DARK } else { LIGHT };
    if let Some(a) = accent.map(hex) {
        if dark {
            p.accent = mix(a, 0xffffff, 0.3);
            p.accent_strong = a;
            p.mention = mix(a, 0xffffff, 0.55);
            p.bubble_mine = mix(p.main, a, 0.25);
        } else {
            p.accent = a;
            p.accent_strong = mix(a, 0x000000, 0.15);
            p.mention = mix(a, 0x000000, 0.2);
            p.bubble_mine = mix(0xffffff, a, 0.12);
            p.selected = mix(0xffffff, a, 0.12);
        }
    }
    p
}

/// Everything the `Theme` global is set to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub palette: Palette,
    pub dark: bool,
    pub font_scale: f32,
    pub compact: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    /// The accent in use, `None` for IronTeams's own.
    pub accent: Option<Rgb>,
}

/// Combines the local preferences with what the desktop portal says (`None`: no portal).
pub fn look(local: &Local, portal: Option<&PortalSettings>) -> Look {
    let r = appearance::resolve(&local.appearance, portal);
    let high_contrast = appearance::resolve_high_contrast(&local.a11y, portal);
    Look {
        palette: palette(r.theme, high_contrast, r.accent),
        dark: r.theme == ResolvedTheme::Dark,
        font_scale: appearance::effective_font_scale(&local.appearance, &local.a11y),
        compact: r.density == appearance::Density::Compact,
        high_contrast,
        reduced_motion: appearance::resolve_reduced_motion(&local.a11y, portal),
        accent: r.accent,
    }
}

/// Font scale steps of the Appearance page.
pub const FONT_STEP: f32 = 0.05;

/// One step up or down, rounded to the step so repeated clicks land on round percentages.
pub fn step_font_scale(current: f32, steps: i32) -> f32 {
    let v = ((current / FONT_STEP).round() + steps as f32) * FONT_STEP;
    appearance::prefs::clamp_font_scale((v * 100.0).round() / 100.0)
}

pub fn switch_slug(s: AutoSwitch) -> &'static str {
    match s {
        AutoSwitch::Auto => "auto",
        AutoSwitch::On => "on",
        AutoSwitch::Off => "off",
    }
}

pub fn switch_from_slug(s: &str) -> Option<AutoSwitch> {
    match s {
        "auto" => Some(AutoSwitch::Auto),
        "on" => Some(AutoSwitch::On),
        "off" => Some(AutoSwitch::Off),
        _ => None,
    }
}

pub fn date_slug(d: DateFormat) -> &'static str {
    match d {
        DateFormat::Auto => "auto",
        DateFormat::Dmy => "dmy",
        DateFormat::Mdy => "mdy",
        DateFormat::Ymd => "ymd",
    }
}

pub fn date_from_slug(s: &str) -> Option<DateFormat> {
    [
        DateFormat::Auto,
        DateFormat::Dmy,
        DateFormat::Mdy,
        DateFormat::Ymd,
    ]
    .into_iter()
    .find(|d| date_slug(*d) == s)
}

pub fn time_slug(t: TimeFormat) -> &'static str {
    match t {
        TimeFormat::Auto | TimeFormat::H12 => "h12",
        TimeFormat::H24 => "h24",
    }
}

pub fn time_from_slug(s: &str) -> Option<TimeFormat> {
    [TimeFormat::H12, TimeFormat::H24]
        .into_iter()
        .find(|t| time_slug(*t) == s)
}
