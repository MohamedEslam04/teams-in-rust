use super::*;

// ---------------------------------------------------------------------------------------------
// Look: what the theme globals get.

/// The colours of one region (ui/tokens/types.slint `Tokens`, same fields in the same order), each
/// as `0xRRGGBBAA`, and whether the region is dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: u32,
    pub surface: u32,
    pub surface_hover: u32,
    pub selected: u32,
    pub border: u32,
    pub panel: u32,
    pub rail: u32,
    pub topbar: u32,
    pub bg_alt: u32,
    pub bg_sunk: u32,
    pub grid_line: u32,
    pub grid_line_soft: u32,
    pub fg: u32,
    pub fg_muted: u32,
    pub fg_subtle: u32,
    pub fg_strong: u32,
    pub accent: u32,
    pub accent_strong: u32,
    pub on_accent: u32,
    pub accent_soft: u32,
    pub accent_subtle: u32,
    pub accent_muted: u32,
    pub accent_faint: u32,
    pub accent_wash: u32,
    pub accent_tint: u32,
    pub bubble_mine: u32,
    pub bubble_other: u32,
    pub bubble_mention: u32,
    pub mention: u32,
    pub code_bg: u32,
    pub danger: u32,
    pub danger_hover: u32,
    pub on_danger: u32,
    pub danger_text: u32,
    pub danger_soft: u32,
    pub warning: u32,
    pub warning_soft: u32,
    pub success: u32,
    pub success_soft: u32,
    pub info_soft: u32,
    pub unread_badge: u32,
    pub on_badge: u32,
    pub presence_available: u32,
    pub presence_busy: u32,
    pub presence_away: u32,
    pub presence_offline: u32,
    pub scrim: u32,
    pub overlay_soft: u32,
    pub shimmer: u32,
    pub dark: bool,
}

/// An opaque colour: `0xRRGGBB` as `0xRRGGBBAA`.
pub const fn opaque(rgb: u32) -> u32 {
    (rgb << 8) | 0xff
}

/// The dark palette IronTeams always had (ui/tokens/theme.slint defaults).
pub const DARK: Palette = Palette {
    bg: opaque(0x141414),
    surface: opaque(0x292929),
    surface_hover: opaque(0x333333),
    selected: opaque(0x2e2e2e),
    border: opaque(0x3d3d3d),
    panel: opaque(0x1f1f1f),
    rail: opaque(0x0f0f0f),
    topbar: opaque(0x0f0f0f),
    bg_alt: opaque(0x181818),
    bg_sunk: opaque(0x101010),
    grid_line: opaque(0x3a3a3a),
    grid_line_soft: opaque(0x262626),
    fg: opaque(0xffffff),
    fg_muted: opaque(0xadadad),
    fg_subtle: opaque(0x8a8a8a),
    fg_strong: opaque(0xffffff),
    accent: opaque(0x7f85f5),
    accent_strong: opaque(0x5b5fc7),
    on_accent: opaque(0xffffff),
    accent_soft: opaque(0x3b3d6b),
    accent_subtle: opaque(0x34365c),
    accent_muted: opaque(0x2b2d52),
    accent_faint: opaque(0x17172a),
    accent_wash: 0x7f85f526,
    accent_tint: 0x7f85f540,
    bubble_mine: opaque(0x2f2f4a),
    bubble_other: opaque(0x292929),
    bubble_mention: opaque(0x3a3a63),
    mention: opaque(0xb4b8ff),
    code_bg: opaque(0x1b1b1b),
    danger: opaque(0xd13438),
    danger_hover: opaque(0xe04a4f),
    on_danger: opaque(0xffffff),
    danger_text: opaque(0xff8a8d),
    danger_soft: opaque(0x3b1f22),
    warning: opaque(0xffb900),
    warning_soft: opaque(0x3a3320),
    success: opaque(0x6bd69b),
    success_soft: opaque(0x1e2f27),
    info_soft: opaque(0x1f2a3b),
    unread_badge: opaque(0xc4314b),
    on_badge: opaque(0xffffff),
    presence_available: opaque(0x6bb700),
    presence_busy: opaque(0xc4314b),
    presence_away: opaque(0xffaa44),
    presence_offline: opaque(0x8a8886),
    scrim: 0x00000080,
    overlay_soft: 0xffffff10,
    shimmer: 0xffffff14,
    dark: true,
};

/// Teams-like light colours.
pub const LIGHT: Palette = Palette {
    bg: opaque(0xffffff),
    surface: opaque(0xf0f0f0),
    surface_hover: opaque(0xe0e0e0),
    selected: opaque(0xe8ebfa),
    border: opaque(0xd1d1d1),
    panel: opaque(0xf5f5f5),
    rail: opaque(0xebebeb),
    topbar: opaque(0xebebeb),
    bg_alt: opaque(0x181818),
    bg_sunk: opaque(0x101010),
    grid_line: opaque(0x3a3a3a),
    grid_line_soft: opaque(0x262626),
    fg: opaque(0x242424),
    fg_muted: opaque(0x5c5c5c),
    fg_subtle: opaque(0x6e6e6e),
    fg_strong: opaque(0xffffff),
    accent: opaque(0x5b5fc7),
    accent_strong: opaque(0x4f52b2),
    on_accent: opaque(0xffffff),
    accent_soft: opaque(0x3b3d6b),
    accent_subtle: opaque(0x34365c),
    accent_muted: opaque(0x2b2d52),
    accent_faint: opaque(0x17172a),
    accent_wash: 0x7f85f526,
    accent_tint: 0x7f85f540,
    bubble_mine: opaque(0xe8ebfa),
    bubble_other: opaque(0xf5f5f5),
    bubble_mention: opaque(0x3a3a63),
    mention: opaque(0x4f52b2),
    code_bg: opaque(0xebebeb),
    danger: opaque(0xc4314b),
    danger_hover: opaque(0xe04a4f),
    on_danger: opaque(0xffffff),
    danger_text: opaque(0xff8a8d),
    danger_soft: opaque(0x3b1f22),
    warning: opaque(0xffb900),
    warning_soft: opaque(0xfff4ce),
    success: opaque(0x6bd69b),
    success_soft: opaque(0x1e2f27),
    info_soft: opaque(0x1f2a3b),
    unread_badge: opaque(0xc4314b),
    on_badge: opaque(0xffffff),
    presence_available: opaque(0x6bb700),
    presence_busy: opaque(0xc4314b),
    presence_away: opaque(0xffaa44),
    presence_offline: opaque(0x8a8886),
    scrim: 0x00000080,
    overlay_soft: 0x0000000f,
    shimmer: 0xffffffb0,
    dark: false,
};

/// High contrast on black: white text and borders, yellow and cyan highlights.
pub const CONTRAST_DARK: Palette = Palette {
    bg: opaque(0x000000),
    surface: opaque(0x000000),
    surface_hover: opaque(0x1f1f1f),
    selected: opaque(0x3d3d00),
    border: opaque(0xffffff),
    panel: opaque(0x000000),
    rail: opaque(0x000000),
    topbar: opaque(0x000000),
    bg_alt: opaque(0x181818),
    bg_sunk: opaque(0x101010),
    grid_line: opaque(0x3a3a3a),
    grid_line_soft: opaque(0x262626),
    fg: opaque(0xffffff),
    fg_muted: opaque(0xffffff),
    fg_subtle: opaque(0xe6e6e6),
    fg_strong: opaque(0xffffff),
    accent: opaque(0xffff00),
    accent_strong: opaque(0x1aebff),
    on_accent: opaque(0xffffff),
    accent_soft: opaque(0x3b3d6b),
    accent_subtle: opaque(0x34365c),
    accent_muted: opaque(0x2b2d52),
    accent_faint: opaque(0x17172a),
    accent_wash: 0x7f85f526,
    accent_tint: 0x7f85f540,
    bubble_mine: opaque(0x000000),
    bubble_other: opaque(0x000000),
    bubble_mention: opaque(0x3a3a63),
    mention: opaque(0xffff00),
    code_bg: opaque(0x000000),
    danger: opaque(0xff6b6b),
    danger_hover: opaque(0xe04a4f),
    on_danger: opaque(0xffffff),
    danger_text: opaque(0xff8a8d),
    danger_soft: opaque(0x3b1f22),
    warning: opaque(0xffb900),
    warning_soft: opaque(0x3a3320),
    success: opaque(0x6bd69b),
    success_soft: opaque(0x1e2f27),
    info_soft: opaque(0x1f2a3b),
    unread_badge: opaque(0xffff00),
    on_badge: opaque(0xffffff),
    presence_available: opaque(0x6bb700),
    presence_busy: opaque(0xc4314b),
    presence_away: opaque(0xffaa44),
    presence_offline: opaque(0x8a8886),
    scrim: 0x00000080,
    overlay_soft: 0xffffff10,
    shimmer: 0xffffff14,
    dark: true,
};

/// High contrast on white: black text and borders, dark blue highlights.
pub const CONTRAST_LIGHT: Palette = Palette {
    bg: opaque(0xffffff),
    surface: opaque(0xffffff),
    surface_hover: opaque(0xe6e6e6),
    selected: opaque(0xd6e4ff),
    border: opaque(0x000000),
    panel: opaque(0xffffff),
    rail: opaque(0xffffff),
    topbar: opaque(0xffffff),
    bg_alt: opaque(0x181818),
    bg_sunk: opaque(0x101010),
    grid_line: opaque(0x3a3a3a),
    grid_line_soft: opaque(0x262626),
    fg: opaque(0x000000),
    fg_muted: opaque(0x000000),
    fg_subtle: opaque(0x1a1a1a),
    fg_strong: opaque(0xffffff),
    accent: opaque(0x00009f),
    accent_strong: opaque(0x00009f),
    on_accent: opaque(0xffffff),
    accent_soft: opaque(0x3b3d6b),
    accent_subtle: opaque(0x34365c),
    accent_muted: opaque(0x2b2d52),
    accent_faint: opaque(0x17172a),
    accent_wash: 0x7f85f526,
    accent_tint: 0x7f85f540,
    bubble_mine: opaque(0xffffff),
    bubble_other: opaque(0xffffff),
    bubble_mention: opaque(0x3a3a63),
    mention: opaque(0x00009f),
    code_bg: opaque(0xffffff),
    danger: opaque(0xa80000),
    danger_hover: opaque(0xe04a4f),
    on_danger: opaque(0xffffff),
    danger_text: opaque(0xff8a8d),
    danger_soft: opaque(0x3b1f22),
    warning: opaque(0xffb900),
    warning_soft: opaque(0xfff4ce),
    success: opaque(0x6bd69b),
    success_soft: opaque(0x1e2f27),
    info_soft: opaque(0x1f2a3b),
    unread_badge: opaque(0xa80000),
    on_badge: opaque(0xffffff),
    presence_available: opaque(0x6bb700),
    presence_busy: opaque(0xc4314b),
    presence_away: opaque(0xffaa44),
    presence_offline: opaque(0x8a8886),
    scrim: 0x00000080,
    overlay_soft: 0x0000000f,
    shimmer: 0xffffffb0,
    dark: false,
};

pub(super) fn hex(c: Rgb) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

/// Mixes `a` towards `b` by `t` (0 is `a`, 1 is `b`), both `0xRRGGBB`.
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
            p.accent = opaque(mix(a, 0xffffff, 0.3));
            p.accent_strong = opaque(a);
            p.mention = opaque(mix(a, 0xffffff, 0.55));
            p.bubble_mine = opaque(mix(p.bg >> 8, a, 0.25));
        } else {
            p.accent = opaque(a);
            p.accent_strong = opaque(mix(a, 0x000000, 0.15));
            p.mention = opaque(mix(a, 0x000000, 0.2));
            p.bubble_mine = opaque(mix(0xffffff, a, 0.12));
            p.selected = opaque(mix(0xffffff, a, 0.12));
        }
    }
    p
}

/// The nav colours of a literal palette: the content colours on the content's panel.
pub fn nav_of(content: &Palette) -> Palette {
    Palette {
        bg: content.panel,
        ..*content
    }
}

/// Everything the `Theme` global of a window is set to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// The content area.
    pub content: Palette,
    /// The rail, the sidebar and the chat list.
    pub nav: Palette,
    /// The call screen and its menus.
    pub call: Palette,
    pub font_scale: f32,
    pub compact: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    /// 24-hour clock: the time labels drawn by the UI follow it.
    pub h24: bool,
    /// The accent in use, `None` for IronTeams's own.
    pub accent: Option<Rgb>,
}

/// Combines the local preferences with what the desktop portal says (`None`: no portal).
pub fn look(local: &Local, portal: Option<&PortalSettings>) -> Look {
    let r = appearance::resolve(&local.appearance, portal);
    let high_contrast = appearance::resolve_high_contrast(&local.a11y, portal);
    let content = palette(r.theme, high_contrast, r.accent);
    Look {
        content,
        nav: nav_of(&content),
        call: content,
        font_scale: appearance::effective_font_scale(&local.appearance, &local.a11y),
        compact: r.density == appearance::Density::Compact,
        high_contrast,
        reduced_motion: appearance::resolve_reduced_motion(&local.a11y, portal),
        h24: local.general.time_format == TimeFormat::H24,
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
