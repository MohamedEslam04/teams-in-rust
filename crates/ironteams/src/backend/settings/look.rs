use super::*;
use appearance::Tokens;

// ---------------------------------------------------------------------------------------------
// Look: what the theme globals get.

/// Everything the `Theme` global of a window is set to. The colours come from the theme engine
/// (`appearance::resolve_look`); the token values of the literal themes are in
/// `appearance::tokens`.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    /// The content area.
    pub content: Tokens,
    /// The rail, the sidebar and the chat list.
    pub nav: Tokens,
    /// The call screen and its menus.
    pub call: Tokens,
    /// The theme drawn now (`builtin:dark`, `custom:<uuid>`).
    pub theme_id: String,
    /// The accent setting applies (IronTeams Dark or Light, without high contrast).
    pub accent_applies: bool,
    /// A schedule picks the theme: Settings runs its minute timer.
    pub schedule: bool,
    pub report: appearance::Report,
    /// Theme ids that were not found and what was used instead; Settings logs each once.
    pub fallbacks: Vec<appearance::Fallback>,
    pub font_ui: Option<String>,
    pub font_code: Option<String>,
    pub font_scale: f32,
    pub compact: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    /// 24-hour clock: the time labels drawn by the UI follow it.
    pub h24: bool,
    /// The accent in use, `None` for IronTeams's own.
    pub accent: Option<Rgb>,
}

/// Combines the local preferences with what the desktop portal says (`None`: no portal), the
/// active account (its own theme wins over the app choice) and the local clock (`now_min`,
/// minutes after midnight, for a schedule).
pub fn look(
    local: &Local,
    portal: Option<&PortalSettings>,
    account: Option<&str>,
    now_min: u16,
) -> Look {
    let high_contrast = appearance::resolve_high_contrast(&local.a11y, portal);
    let r = appearance::resolve_look(
        &local.appearance,
        &local.themes,
        account,
        portal,
        high_contrast,
        now_min,
    );
    Look {
        content: r.content,
        nav: r.nav,
        call: r.call,
        theme_id: r.theme_id,
        accent_applies: r.accent_applies,
        schedule: r.schedule,
        report: r.report,
        fallbacks: r.fallbacks,
        font_ui: r.font_ui,
        font_code: r.font_code,
        font_scale: appearance::effective_font_scale(&local.appearance, &local.a11y),
        compact: local.appearance.density == appearance::Density::Compact,
        high_contrast,
        reduced_motion: appearance::resolve_reduced_motion(&local.a11y, portal),
        h24: local.general.time_format == TimeFormat::H24,
        accent: appearance::resolve_accent(&local.appearance, portal.and_then(|p| p.accent)),
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
