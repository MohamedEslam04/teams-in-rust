//! The theme model: the built-in and custom themes, how a theme's tokens are derived from its 8
//! base colours (in OKLCH), the contrast report and the text adjustment, and `themes.json`.
//!
//! A theme is its [`Base`] colours; the two literal themes (IronTeams Dark and Light) instead
//! carry today's exact token values. [`theme_tokens`] turns either into one [`Tokens`] value per
//! region (content and nav) with a [`Report`]. Nothing here draws or reads the desktop; choosing
//! which theme applies is [`crate::resolve::resolve_look`].

use crate::oklch::{Oklch, best_on, contrast, from_oklch, kind_of, tint, to_oklch, toward};
use crate::prefs::{Rgb, Theme};
use crate::resolve::ResolvedTheme;
use crate::share::{self, NameError};
use crate::store::{self, Error};
use crate::tokens::{DARK, LIGHT, Tokens, from_rgb, literal, nav_of_literal, opaque, to_rgb};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;
use uuid::Uuid;

pub const FILE_NAME: &str = "themes.json";
/// The most custom themes `themes.json` keeps.
pub const MAX_CUSTOM: usize = 50;
pub const DARK_ID: &str = "builtin:dark";
pub const LIGHT_ID: &str = "builtin:light";
const CUSTOM_PREFIX: &str = "custom:";
/// The lightness step of the contrast adjustment.
const STEP: f64 = 0.005;

// ---------------------------------------------------------------------------------------------
// The theme.

/// The 8 colours a theme is made of, as the user picked them: nav background, nav text, nav
/// selected item, content background, content text, accent, mention highlight, unread badge.
/// Stored as 8 `#rrggbb` strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Base(pub [Rgb; 8]);

/// A built-in or custom theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeDef {
    /// `builtin:<name>` or `custom:<uuid>`.
    pub id: String,
    pub name: String,
    pub base: Base,
    /// "Keep my colours": the contrast adjustment moves nothing.
    #[serde(default)]
    pub keep_original: bool,
    /// Set for IronTeams Dark and Light only: the theme is today's literal palette of that kind
    /// and `base` is just for its tile. Never stored.
    #[serde(skip)]
    pub literal: Option<ResolvedTheme>,
}

const fn rgb(v: u32) -> Rgb {
    Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn def(id: &str, name: &str, base: [u32; 8], literal: Option<ResolvedTheme>) -> ThemeDef {
    ThemeDef {
        id: id.to_string(),
        name: name.to_string(),
        base: Base(base.map(rgb)),
        keep_original: false,
        literal,
    }
}

static BUILTINS: LazyLock<Vec<ThemeDef>> = LazyLock::new(|| {
    use ResolvedTheme::{Dark, Light};
    vec![
        def(
            DARK_ID,
            "IronTeams Dark",
            [
                0x1f1f1f, 0xffffff, 0x2e2e2e, 0x141414, 0xffffff, 0x7f85f5, 0xb4b8ff, 0xc4314b,
            ],
            Some(Dark),
        ),
        def(
            LIGHT_ID,
            "IronTeams Light",
            [
                0xf5f5f5, 0x242424, 0xe8ebfa, 0xffffff, 0x242424, 0x5b5fc7, 0x4f52b2, 0xc4314b,
            ],
            Some(Light),
        ),
        def(
            "builtin:dusk",
            "Dusk",
            [
                0x2b1f3a, 0xe9e3f2, 0x4a3566, 0x17141c, 0xf1eef5, 0xb48cf2, 0xd7c2ff, 0xe5484d,
            ],
            None,
        ),
        def(
            "builtin:harbor",
            "Harbor",
            [
                0x1d2b45, 0xe6ecf5, 0x2f4a75, 0xffffff, 0x1c2430, 0x2f6fd6, 0x1f57b8, 0xd93f3f,
            ],
            None,
        ),
        def(
            "builtin:moss",
            "Moss",
            [
                0x1b2620, 0xe3ede6, 0x2c4436, 0x121815, 0xedf3ef, 0x5fbf86, 0x9be0b6, 0xe05252,
            ],
            None,
        ),
        def(
            "builtin:sandstone",
            "Sandstone",
            [
                0xefe6d8, 0x3b3127, 0xe0d0b8, 0xfbf8f3, 0x2b241d, 0xb06025, 0x8f4a17, 0xc8372d,
            ],
            None,
        ),
        def(
            "builtin:ember",
            "Ember",
            [
                0x2a1a14, 0xf3e6df, 0x4b2a1e, 0x181210, 0xf6eee9, 0xf08a4b, 0xffc29e, 0xe5484d,
            ],
            None,
        ),
        def(
            "builtin:glacier",
            "Glacier",
            [
                0xe8f1f8, 0x1b2b3a, 0xcfe2f2, 0xfbfdff, 0x15222e, 0x257bb3, 0x1b5f8e, 0xd64545,
            ],
            None,
        ),
        def(
            "builtin:graphite",
            "Graphite",
            [
                0x1e2022, 0xe6e8ea, 0x33373b, 0x151618, 0xeef0f2, 0x3fb6a8, 0x8ee0d6, 0xe5484d,
            ],
            None,
        ),
        def(
            "builtin:orchard",
            "Orchard",
            [
                0x1f3a2c, 0xe4f0e8, 0x2f5a43, 0xffffff, 0x1d2a22, 0x268551, 0x1f6b41, 0xd13f3f,
            ],
            None,
        ),
    ]
});

/// The ten built-in themes, IronTeams Dark and Light first.
pub fn builtins() -> &'static [ThemeDef] {
    &BUILTINS
}

pub(crate) fn builtin(id: &str) -> &'static ThemeDef {
    builtins()
        .iter()
        .find(|d| d.id == id)
        .unwrap_or(&builtins()[0])
}

/// The theme with this id: a built-in first, then a custom theme of the file.
pub fn find<'a>(file: &'a ThemesFile, id: &str) -> Option<&'a ThemeDef> {
    builtins()
        .iter()
        .find(|d| d.id == id)
        .or_else(|| file.custom.iter().find(|d| d.id == id))
}

/// The kind of the content area: given for the literal themes, else computed from base colour 4.
pub fn theme_kind(def: &ThemeDef) -> ResolvedTheme {
    def.literal.unwrap_or_else(|| kind_of(def.base.0[3]))
}

// ---------------------------------------------------------------------------------------------
// Derivation.

/// The two token sets of a theme. The call set is the content set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionSet {
    pub content: Tokens,
    pub nav: Tokens,
}

fn op(c: Rgb) -> u32 {
    from_rgb(c, 0xff)
}

/// The colours that depend on a region's background, text and kind alone. Fields that depend on
/// more (`selected`, the accent group, the message colours) are set by the caller. The presence,
/// scrim, overlay and shimmer tokens are the fixed values of the kind, which the literal palette
/// of that kind already holds.
fn region(bg: Rgb, fg: Rgb, badge: Rgb, kind: ResolvedTheme) -> Tokens {
    let dark = kind == ResolvedTheme::Dark;
    let black = Rgb::new(0, 0, 0);
    let mut t = if dark { DARK } else { LIGHT };
    let step = |f: f64| op(toward(bg, fg, f));
    t.bg = op(bg);
    t.fg = op(fg);
    t.fg_strong = op(fg);
    t.surface = step(0.06);
    t.bubble_other = t.surface;
    t.surface_hover = step(0.10);
    t.panel = step(0.05);
    t.border = step(0.18);
    t.grid_line = t.border;
    t.grid_line_soft = step(0.09);
    t.bg_alt = step(0.025);
    t.bg_sunk = if dark {
        op(toward(bg, black, 0.15))
    } else {
        step(0.04)
    };
    t.rail = if dark {
        op(toward(bg, black, 0.15))
    } else {
        step(0.08)
    };
    t.topbar = t.rail;
    t.code_bg = step(0.04);
    t.fg_muted = op(toward(fg, bg, 0.25));
    t.fg_subtle = op(toward(fg, bg, 0.38));
    // Status colours: the fixed values of the kind.
    let fixed: [u32; 9] = if dark {
        [
            0xd13438, 0xa4262c, 0xff8a8d, 0x3b1f22, 0xffb900, 0x3a3320, 0x6bd69b, 0x1e2f27,
            0x1f2a3b,
        ]
    } else {
        [
            0xc4314b, 0xa4262c, 0xb10e1c, 0xfde7e9, 0x8a5a00, 0xfff4ce, 0x237b4b, 0xdcf3e5,
            0xe6eefa,
        ]
    };
    let [
        danger,
        hover,
        text,
        soft,
        warning,
        warning_soft,
        success,
        success_soft,
        info_soft,
    ] = fixed.map(opaque);
    t.danger = danger;
    t.danger_hover = hover;
    t.on_danger = opaque(0xffffff);
    t.danger_text = text;
    t.danger_soft = soft;
    t.warning = warning;
    t.warning_soft = warning_soft;
    t.success = success;
    t.success_soft = success_soft;
    t.info_soft = info_soft;
    t.unread_badge = op(badge);
    t.on_badge = op(best_on(&[badge]));
    t.dark = dark;
    t
}

/// Sets `accent` and the tokens computed from it.
fn set_accent(t: &mut Tokens, accent: Rgb) {
    let o = to_oklch(accent);
    let drop = if t.dark { 0.08 } else { 0.10 };
    let strong = from_oklch(Oklch { l: o.l - drop, ..o });
    t.accent = op(accent);
    t.accent_strong = op(strong);
    t.on_accent = op(best_on(&[accent, strong]));
    t.accent_wash = from_rgb(accent, 0x26);
    t.accent_tint = from_rgb(accent, 0x40);
}

/// The accent-tinted backgrounds, from strongest to faintest, over a region's own `bg` and `fg`.
fn set_soft(t: &mut Tokens, bg: Rgb, fg: Rgb, accent: Rgb) {
    t.accent_soft = op(tint(bg, fg, accent, 0.23, 0.48));
    t.accent_subtle = op(tint(bg, fg, accent, 0.19, 0.40));
    t.accent_muted = op(tint(bg, fg, accent, 0.15, 0.40));
    t.accent_faint = op(tint(bg, fg, accent, 0.03, 0.23));
}

/// What the nav copies from the content set: the accent group, mention and the message colours.
fn copy_content_tokens(nav: &mut Tokens, content: &Tokens) {
    nav.accent = content.accent;
    nav.accent_strong = content.accent_strong;
    nav.on_accent = content.on_accent;
    nav.accent_wash = content.accent_wash;
    nav.accent_tint = content.accent_tint;
    nav.mention = content.mention;
    nav.bubble_mine = content.bubble_mine;
    nav.bubble_mention = content.bubble_mention;
    nav.bubble_other = content.bubble_other;
    nav.code_bg = content.code_bg;
}

/// Every token of both regions from the 8 base colours, by proportional lightness steps in OKLCH.
pub fn derive(base: &Base) -> RegionSet {
    let [nav_bg, nav_fg, nav_selected, bg, fg, accent, mention, badge] = base.0;
    let mut content = region(bg, fg, badge, kind_of(bg));
    content.selected = op(tint(bg, fg, accent, 0.08, 0.20));
    set_accent(&mut content, accent);
    set_soft(&mut content, bg, fg, accent);
    content.bubble_mine = op(tint(bg, fg, accent, 0.15, 0.30));
    content.bubble_mention = op(tint(bg, fg, accent, 0.21, 0.43));
    content.mention = op(mention);

    let mut nav = region(nav_bg, nav_fg, badge, kind_of(nav_bg));
    nav.selected = op(nav_selected);
    set_soft(&mut nav, nav_bg, nav_fg, accent);
    copy_content_tokens(&mut nav, &content);
    RegionSet { content, nav }
}

// ---------------------------------------------------------------------------------------------
// Contrast report and adjustment.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Content,
    Nav,
}

/// One checked pair.
#[derive(Debug, Clone, PartialEq)]
pub struct PairResult {
    pub region: Region,
    pub fg: &'static str,
    pub bg: &'static str,
    pub ratio: f64,
    pub min: f64,
}

/// A text token the adjustment moved: the colour derived from what the user picked and the one
/// drawn instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adjusted {
    pub region: Region,
    pub token: &'static str,
    pub picked: u32,
    pub drawn: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Every checked pair of both regions (content first), with the final ratio.
    pub pairs: Vec<PairResult>,
    pub adjusted: Vec<Adjusted>,
    /// A pair still fails after the adjustment (or no adjustment was allowed).
    pub low_contrast: bool,
}

type Pair = (&'static str, &'static str, f64);
type Group = (&'static str, &'static [&'static str], f64);

/// Foreground, the backgrounds it is drawn on, and the lowest ratio (WCAG 4.5 for text, 3 for
/// large text and icons).
const CONTENT_GROUPS: &[Group] = &[
    (
        "fg",
        &[
            "bg",
            "surface",
            "selected",
            "panel",
            "bubble-mine",
            "bubble-other",
            "bubble-mention",
            "accent-soft",
            "accent-subtle",
            "accent-muted",
            "accent-faint",
            "warning-soft",
        ],
        4.5,
    ),
    ("on-accent", &["accent", "accent-strong"], 4.5),
    ("on-badge", &["unread-badge"], 4.5),
    ("on-danger", &["danger", "danger-hover"], 4.5),
    (
        "fg-strong",
        &["danger-soft", "info-soft", "success-soft"],
        4.5,
    ),
    (
        "danger-text",
        &["bg", "surface", "panel", "danger-soft"],
        4.5,
    ),
    (
        "fg-muted",
        &[
            "bg",
            "surface",
            "selected",
            "panel",
            "bubble-mine",
            "bubble-other",
        ],
        3.0,
    ),
    (
        "fg-subtle",
        &[
            "bg",
            "surface",
            "selected",
            "panel",
            "bubble-mine",
            "bubble-other",
        ],
        3.0,
    ),
    ("accent", &["bg", "surface", "panel"], 3.0),
    ("mention", &["bg", "bubble-mine", "bubble-other"], 3.0),
    ("warning", &["bg", "surface", "panel"], 3.0),
    ("success", &["bg", "surface", "panel"], 3.0),
];

/// Only tokens the nav derives for itself: what it copies from the content set is not checked.
const NAV_GROUPS: &[Group] = &[
    ("fg", &["bg", "surface", "selected", "accent-soft"], 4.5),
    ("on-badge", &["unread-badge"], 4.5),
    ("fg-muted", &["bg", "surface", "selected"], 3.0),
    ("fg-subtle", &["bg", "surface", "selected"], 3.0),
];

fn expand(groups: &[Group]) -> Vec<Pair> {
    groups
        .iter()
        .flat_map(|&(fg, bgs, min)| bgs.iter().map(move |&bg| (fg, bg, min)))
        .collect()
}

static CONTENT_PAIRS: LazyLock<Vec<Pair>> = LazyLock::new(|| expand(CONTENT_GROUPS));
static NAV_PAIRS: LazyLock<Vec<Pair>> = LazyLock::new(|| expand(NAV_GROUPS));

/// The pairs a region is checked on, as (foreground token, background token, lowest ratio). The
/// call set is checked on the content list.
pub fn pair_list(region: Region) -> &'static [(&'static str, &'static str, f64)] {
    match region {
        Region::Content => &CONTENT_PAIRS,
        Region::Nav => &NAV_PAIRS,
    }
}

/// The text tokens the adjustment may move, in the order it visits them.
const TEXT_TOKENS: [&str; 9] = [
    "fg",
    "fg-strong",
    "fg-muted",
    "fg-subtle",
    "accent",
    "mention",
    "danger-text",
    "warning",
    "success",
];

fn rgb_of(t: &Tokens, name: &str) -> Rgb {
    to_rgb(t.get(name).unwrap_or(0))
}

fn measure(t: &Tokens, region: Region, out: &mut Vec<PairResult>) {
    for &(fg, bg, min) in pair_list(region) {
        out.push(PairResult {
            region,
            fg,
            bg,
            ratio: contrast(rgb_of(t, fg), rgb_of(t, bg)),
            min,
        });
    }
}

fn pairs_of(set: &RegionSet) -> Vec<PairResult> {
    let mut pairs = Vec::new();
    measure(&set.content, Region::Content, &mut pairs);
    measure(&set.nav, Region::Nav, &mut pairs);
    pairs
}

/// The lowest ratio-to-minimum of `c` over the backgrounds; 1 or more passes every pair.
fn worst(pairs: &[(Rgb, f64)], c: Rgb) -> f64 {
    pairs
        .iter()
        .map(|&(bg, min)| contrast(c, bg) / min)
        .fold(f64::INFINITY, f64::min)
}

/// Moves `picked` in OKLCH lightness, away from its backgrounds first, in steps of 0.005 until
/// every pair passes. When no step passes, the best worst-pair colour of both walks is kept.
fn walk(picked: Rgb, pairs: &[(Rgb, f64)]) -> Rgb {
    let o = to_oklch(picked);
    let mean = pairs.iter().map(|&(bg, _)| to_oklch(bg).l).sum::<f64>() / pairs.len() as f64;
    let up_first = o.l > mean;
    let mut best = (worst(pairs, picked), picked);
    for up in [up_first, !up_first] {
        let mut l = o.l;
        loop {
            let next = if up { l + STEP } else { l - STEP };
            l = next.clamp(0.0, 1.0);
            let c = from_oklch(Oklch { l, ..o });
            let w = worst(pairs, c);
            if w >= 1.0 {
                return c;
            }
            if w > best.0 {
                best = (w, c);
            }
            if l <= 0.0 || l >= 1.0 {
                break;
            }
        }
    }
    best.1
}

fn adjust_region(t: &mut Tokens, region: Region, out: &mut Vec<Adjusted>) {
    for name in TEXT_TOKENS {
        let pairs: Vec<(Rgb, f64)> = pair_list(region)
            .iter()
            .filter(|p| p.0 == name)
            .map(|&(_, bg, min)| (rgb_of(t, bg), min))
            .collect();
        if pairs.is_empty() {
            continue;
        }
        let picked = rgb_of(t, name);
        if worst(&pairs, picked) >= 1.0 {
            continue;
        }
        let drawn = walk(picked, &pairs);
        if drawn == picked {
            continue;
        }
        t.set(name, op(drawn));
        if name == "accent" {
            set_accent(t, drawn);
        }
        out.push(Adjusted {
            region,
            token: name,
            picked: op(picked),
            drawn: op(drawn),
        });
    }
}

/// Moves drawn text tokens until their pairs pass (nothing with `keep_original`), then reports
/// every pair. `set.nav` follows the content accent and mention it copies.
pub fn adjust(set: &mut RegionSet, keep_original: bool) -> Report {
    let mut adjusted = Vec::new();
    if !keep_original {
        adjust_region(&mut set.content, Region::Content, &mut adjusted);
        adjust_region(&mut set.nav, Region::Nav, &mut adjusted);
        let content = set.content;
        copy_content_tokens(&mut set.nav, &content);
    }
    let pairs = pairs_of(set);
    let low_contrast = pairs.iter().any(|p| p.ratio < p.min);
    Report {
        pairs,
        adjusted,
        low_contrast,
    }
}

/// The tokens and the contrast report of a theme. The literal themes give today's palette (with
/// `accent` applied to it) and a report that only measures; every other theme derives from its
/// base and ignores `accent`.
pub fn theme_tokens(def: &ThemeDef, accent: Option<Rgb>) -> (RegionSet, Report) {
    if let Some(kind) = def.literal {
        let content = literal(kind, false, accent);
        let set = RegionSet {
            content,
            nav: nav_of_literal(&content),
        };
        let report = Report {
            pairs: pairs_of(&set),
            adjusted: Vec::new(),
            low_contrast: false,
        };
        return (set, report);
    }
    let mut set = derive(&def.base);
    let report = adjust(&mut set, def.keep_original);
    (set, report)
}

// ---------------------------------------------------------------------------------------------
// Choice and `themes.json`.

/// Which theme the app shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    Fixed(String),
    Pair {
        light: String,
        dark: String,
        switch: Switch,
    },
}

/// What picks the slot of a [`ThemeChoice::Pair`]. Times are minutes after local midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Switch {
    /// Follow the desktop's light or dark setting.
    System,
    Schedule {
        light_from: u16,
        dark_from: u16,
    },
}

/// The theme of the call screen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallChoice {
    #[default]
    SameAsApp,
    Fixed(String),
}

/// The default: IronTeams Light and Dark, following the desktop.
pub fn default_choice() -> ThemeChoice {
    ThemeChoice::Pair {
        light: LIGHT_ID.to_string(),
        dark: DARK_ID.to_string(),
        switch: Switch::System,
    }
}

fn choice_is_valid(c: &ThemeChoice) -> bool {
    match c {
        ThemeChoice::Pair {
            switch:
                Switch::Schedule {
                    light_from,
                    dark_from,
                },
            ..
        } => *light_from <= 1439 && *dark_from <= 1439,
        _ => true,
    }
}

/// `themes.json`: everything new in the theme engine, kept apart from `appearance.json` so that
/// builds without themes never touch it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ThemesFile {
    /// `None` until the user picks: then the old `appearance.json` theme is mapped.
    pub choice: Option<ThemeChoice>,
    /// Account label to one theme id. An override is one fixed theme.
    pub accounts: BTreeMap<String, String>,
    pub call: CallChoice,
    pub font_ui: Option<String>,
    pub font_code: Option<String>,
    pub custom: Vec<ThemeDef>,
}

/// What the loader dropped, for the caller to log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoadNote {
    /// Custom entries left out (unreadable, a bad id or name, a repeated id, or over the limit).
    pub skipped: usize,
    /// The file could not be read as JSON and was renamed `themes.json.bad`.
    pub renamed_bad: bool,
}

/// Why a theme could not be added or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeError {
    /// 50 custom themes already.
    Limit,
    Name(NameError),
    Unknown,
}

impl ThemesFile {
    /// Reads the file. A field of the wrong shape takes its default and a bad custom entry is
    /// skipped; a file that is not a JSON object is renamed `.bad` first, so the themes in it
    /// are never overwritten. A missing file gives the defaults.
    pub fn load_from(path: &Path) -> (ThemesFile, LoadNote) {
        let mut note = LoadNote::default();
        let Ok(bytes) = std::fs::read(path) else {
            return (ThemesFile::default(), note);
        };
        let Ok(Value::Object(obj)) = serde_json::from_slice::<Value>(&bytes) else {
            let mut bad = path.as_os_str().to_owned();
            bad.push(".bad");
            note.renamed_bad = std::fs::rename(path, bad).is_ok();
            return (ThemesFile::default(), note);
        };
        let field = |key: &str| obj.get(key).cloned();
        let text = |key: &str| obj.get(key).and_then(Value::as_str).map(str::to_string);
        let accounts = match obj.get("accounts") {
            Some(Value::Object(map)) => map
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect(),
            _ => BTreeMap::new(),
        };
        let mut custom: Vec<ThemeDef> = Vec::new();
        if let Some(Value::Array(items)) = obj.get("custom") {
            let mut seen = HashSet::new();
            for item in items {
                let entry = serde_json::from_value::<ThemeDef>(item.clone())
                    .ok()
                    .filter(|d| d.id.starts_with(CUSTOM_PREFIX) && !seen.contains(&d.id))
                    .and_then(|mut d| {
                        d.name = share::check_name(&d.name).ok()?.to_string();
                        Some(d)
                    });
                match entry {
                    Some(d) if custom.len() < MAX_CUSTOM => {
                        seen.insert(d.id.clone());
                        custom.push(d);
                    }
                    _ => note.skipped += 1,
                }
            }
        }
        let file = ThemesFile {
            choice: field("choice")
                .and_then(|v| serde_json::from_value(v).ok())
                .filter(choice_is_valid),
            accounts,
            call: field("call")
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            font_ui: text("font_ui"),
            font_code: text("font_code"),
            custom,
        };
        (file, note)
    }

    /// Saves atomically (temp file, then rename).
    pub fn save_to(&self, path: &Path) -> Result<(), Error> {
        store::save_json(path, self)
    }

    /// Adds a custom theme and returns its id (`custom:<uuid>`). The name goes through
    /// [`ThemesFile::unique_name`].
    pub fn add(
        &mut self,
        name: &str,
        base: Base,
        keep_original: bool,
    ) -> Result<String, ThemeError> {
        if self.custom.len() >= MAX_CUSTOM {
            return Err(ThemeError::Limit);
        }
        let name = share::check_name(name).map_err(ThemeError::Name)?;
        let name = self.unique_name(name, None);
        let id = format!("{CUSTOM_PREFIX}{}", Uuid::new_v4());
        self.custom.push(ThemeDef {
            id: id.clone(),
            name,
            base,
            keep_original,
            literal: None,
        });
        Ok(id)
    }

    /// Replaces the colours and the "Keep my colours" setting of a custom theme.
    pub fn update(&mut self, id: &str, base: Base, keep_original: bool) -> Result<(), ThemeError> {
        let def = self
            .custom
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or(ThemeError::Unknown)?;
        def.base = base;
        def.keep_original = keep_original;
        Ok(())
    }

    /// Renames a custom theme and returns the name it got (a taken name gets a suffix).
    pub fn rename(&mut self, id: &str, name: &str) -> Result<String, ThemeError> {
        if !self.custom.iter().any(|d| d.id == id) {
            return Err(ThemeError::Unknown);
        }
        let name = share::check_name(name).map_err(ThemeError::Name)?;
        let name = self.unique_name(name, Some(id));
        if let Some(def) = self.custom.iter_mut().find(|d| d.id == id) {
            def.name = name.clone();
        }
        Ok(name)
    }

    /// Deletes a custom theme. References to it stay; the resolver falls back.
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.custom.len();
        self.custom.retain(|d| d.id != id);
        self.custom.len() != before
    }

    /// Sets (or with `None` clears) the theme of one account.
    pub fn set_account(&mut self, label: &str, id: Option<&str>) {
        match id {
            Some(id) => {
                self.accounts.insert(label.to_string(), id.to_string());
            }
            None => {
                self.accounts.remove(label);
            }
        }
    }

    /// Removes an account's entry; false when it had none.
    pub fn remove_account(&mut self, label: &str) -> bool {
        self.accounts.remove(label).is_some()
    }

    /// "Reset theme": the default choice, no override for `account`, the call theme as the app's,
    /// no fonts. Custom themes and the other accounts' entries stay.
    pub fn reset(&mut self, account: Option<&str>) {
        self.choice = Some(default_choice());
        if let Some(label) = account {
            self.accounts.remove(label);
        }
        self.call = CallChoice::SameAsApp;
        self.font_ui = None;
        self.font_code = None;
    }

    /// `wanted`, or when a built-in (any case) or another custom theme than `except` has that
    /// name, `wanted` with " (2)", " (3)" and so on. The whole stays within 40 scalar values:
    /// the part before the suffix is cut.
    pub fn unique_name(&self, wanted: &str, except: Option<&str>) -> String {
        let taken = |name: &str| {
            let name = name.to_lowercase();
            builtins().iter().any(|d| d.name.to_lowercase() == name)
                || self
                    .custom
                    .iter()
                    .any(|d| Some(d.id.as_str()) != except && d.name.to_lowercase() == name)
        };
        if !taken(wanted) {
            return wanted.to_string();
        }
        let mut n = 2;
        loop {
            let suffix = format!(" ({n})");
            let keep = share::NAME_MAX.saturating_sub(suffix.chars().count());
            let candidate: String = wanted.chars().take(keep).chain(suffix.chars()).collect();
            if !taken(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }
}

/// The choice an old `appearance.json` `theme` field stands for.
pub fn choice_from_legacy(t: Theme) -> ThemeChoice {
    match t {
        Theme::Light => ThemeChoice::Fixed(LIGHT_ID.to_string()),
        Theme::Dark => ThemeChoice::Fixed(DARK_ID.to_string()),
        Theme::System => default_choice(),
    }
}

/// The `appearance.json` `theme` value to write for a choice: the kind of a fixed theme (`System`
/// when it no longer exists), `System` for every pair.
pub fn legacy_theme(choice: &ThemeChoice, file: &ThemesFile) -> Theme {
    match choice {
        ThemeChoice::Fixed(id) => match find(file, id).map(theme_kind) {
            Some(ResolvedTheme::Light) => Theme::Light,
            Some(ResolvedTheme::Dark) => Theme::Dark,
            None => Theme::System,
        },
        ThemeChoice::Pair { .. } => Theme::System,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hex(s: &str) -> Rgb {
        Rgb::from_hex(s).unwrap()
    }

    fn base(s: &str) -> Base {
        let colours: Vec<Rgb> = s.split_whitespace().map(hex).collect();
        Base(colours.try_into().unwrap())
    }

    fn near(got: Rgb, want: Rgb) -> bool {
        got.r.abs_diff(want.r) <= 1 && got.g.abs_diff(want.g) <= 1 && got.b.abs_diff(want.b) <= 1
    }

    fn assert_hex(t: &Tokens, name: &str, want: &str) {
        let got = to_rgb(t.get(name).unwrap());
        assert!(
            near(got, hex(want)),
            "{name}: got {}, want {want}",
            got.to_hex()
        );
    }

    fn def_of(id: &str) -> &'static ThemeDef {
        builtins().iter().find(|d| d.id == id).unwrap()
    }

    const NIGHT: &str = "#1d2b45 #e6ecf5 #2f4a75 #ffffff #1c2430 #2f6fd6 #1f57b8 #d93f3f";

    // -- built-ins ----------------------------------------------------------------------------

    #[test]
    fn the_ten_builtins_have_the_listed_ids_names_and_kinds() {
        use ResolvedTheme::{Dark, Light};
        let want = [
            ("builtin:dark", "IronTeams Dark", Dark, Dark),
            ("builtin:light", "IronTeams Light", Light, Light),
            ("builtin:dusk", "Dusk", Dark, Dark),
            ("builtin:harbor", "Harbor", Light, Dark),
            ("builtin:moss", "Moss", Dark, Dark),
            ("builtin:sandstone", "Sandstone", Light, Light),
            ("builtin:ember", "Ember", Dark, Dark),
            ("builtin:glacier", "Glacier", Light, Light),
            ("builtin:graphite", "Graphite", Dark, Dark),
            ("builtin:orchard", "Orchard", Light, Dark),
        ];
        assert_eq!(builtins().len(), want.len());
        for (d, (id, name, content, nav)) in builtins().iter().zip(want) {
            assert_eq!((d.id.as_str(), d.name.as_str()), (id, name));
            assert_eq!(theme_kind(d), content, "{id} content kind");
            assert_eq!(kind_of(d.base.0[0]), nav, "{id} nav kind");
            assert_eq!(
                d.literal.is_some(),
                id.ends_with("dark") || id.ends_with("light")
            );
        }
        assert_eq!(builtin("builtin:dark").literal, Some(Dark));
        assert_eq!(builtin("builtin:light").literal, Some(Light));
    }

    #[test]
    fn the_literal_themes_carry_the_tile_base_colours() {
        assert_eq!(
            def_of("builtin:dark").base,
            base("#1f1f1f #ffffff #2e2e2e #141414 #ffffff #7f85f5 #b4b8ff #c4314b")
        );
        assert_eq!(
            def_of("builtin:light").base,
            base("#f5f5f5 #242424 #e8ebfa #ffffff #242424 #5b5fc7 #4f52b2 #c4314b")
        );
    }

    #[test]
    fn find_looks_at_the_builtins_then_the_custom_themes() {
        let mut file = ThemesFile::default();
        let id = file.add("Mine", base(NIGHT), false).unwrap();
        assert_eq!(find(&file, &id).unwrap().name, "Mine");
        assert_eq!(find(&file, "builtin:moss").unwrap().name, "Moss");
        assert!(find(&file, "custom:nope").is_none());
        assert!(find(&ThemesFile::default(), &id).is_none());
    }

    // -- derivation ---------------------------------------------------------------------------

    #[test]
    fn the_pair_lists_name_real_tokens() {
        for region in [Region::Content, Region::Nav] {
            assert!(!pair_list(region).is_empty());
            for &(fg, bg, min) in pair_list(region) {
                assert!(is_token(fg) && is_token(bg), "{fg} on {bg}");
                assert!(min == 4.5 || min == 3.0);
            }
        }
        for name in TEXT_TOKENS {
            assert!(is_token(name), "{name}");
        }
    }

    fn is_token(name: &str) -> bool {
        crate::tokens::NAMES.contains(&name)
    }

    #[test]
    fn derived_builtins_pass_every_pair_without_adjustment() {
        for d in builtins().iter().filter(|d| d.literal.is_none()) {
            let mut set = derive(&d.base);
            let report = adjust(&mut set, false);
            let failing: Vec<_> = report.pairs.iter().filter(|p| p.ratio < p.min).collect();
            assert!(failing.is_empty(), "{}: {failing:?}", d.id);
            assert!(report.adjusted.is_empty(), "{}", d.id);
            assert!(!report.low_contrast, "{}", d.id);
            // And theme_tokens agrees.
            assert_eq!(theme_tokens(d, None).0, set, "{}", d.id);
        }
    }

    #[test]
    fn dusk_goldens() {
        let set = derive(&def_of("builtin:dusk").base);
        let (c, n) = (&set.content, &set.nav);
        for (name, want) in [
            ("bg", "#17141c"),
            ("surface", "#221e27"),
            ("surface-hover", "#29262e"),
            ("selected", "#262030"),
            ("border", "#38353e"),
            ("panel", "#201d25"),
            ("rail", "#100d15"),
            ("bg-alt", "#1b1820"),
            ("bg-sunk", "#100d15"),
            ("grid-line-soft", "#27242c"),
            ("fg-muted", "#b4b1b8"),
            ("fg-subtle", "#959399"),
            ("accent-strong", "#9b73d7"),
            ("on-accent", "#000000"),
            ("accent-soft", "#473760"),
            ("accent-subtle", "#3e3152"),
            ("accent-muted", "#36294a"),
            ("accent-faint", "#1d1728"),
            ("bubble-mine", "#342b44"),
            ("bubble-mention", "#423459"),
            ("code-bg", "#1e1b23"),
            ("on-badge", "#000000"),
        ] {
            assert_hex(c, name, want);
        }
        for (name, want) in [
            ("bg", "#2b1f3a"),
            ("surface", "#352945"),
            ("surface-hover", "#3c2f4c"),
            ("selected", "#4a3566"),
            ("border", "#4a3d5a"),
            ("panel", "#332743"),
            ("rail", "#211530"),
            ("fg-muted", "#b4aebc"),
            ("fg-subtle", "#9994a2"),
            ("accent-soft", "#53436e"),
        ] {
            assert_hex(n, name, want);
        }
    }

    #[test]
    fn harbor_goldens() {
        let set = derive(&def_of("builtin:harbor").base);
        for (name, want) in [
            ("selected", "#e0ecff"),
            ("fg-muted", "#4b5462"),
            ("fg-subtle", "#666f7e"),
            ("accent-strong", "#0d50b4"),
            ("on-accent", "#ffffff"),
            ("bubble-mine", "#c7dcfd"),
            ("accent-soft", "#a8c9fd"),
        ] {
            assert_hex(&set.content, name, want);
        }
        for (name, want) in [
            ("surface", "#263550"),
            ("fg-subtle", "#979da5"),
            ("accent-soft", "#365280"),
        ] {
            assert_hex(&set.nav, name, want);
        }
    }

    #[test]
    fn a_dark_nav_in_a_light_theme_has_the_dark_fixed_colours() {
        for id in ["builtin:harbor", "builtin:orchard"] {
            let set = derive(&def_of(id).base);
            assert!(set.nav.dark && !set.content.dark, "{id}");
            assert_eq!(set.nav.overlay_soft, 0xffffff10, "{id}");
            assert_eq!(set.nav.danger_text, 0xff8a8dff, "{id}");
            assert_eq!(set.content.danger_text, 0xb10e1cff, "{id}");
            for name in ["bubble-mine", "mention", "accent"] {
                assert_eq!(set.nav.get(name), set.content.get(name), "{id} {name}");
            }
        }
    }

    #[test]
    fn the_kind_flag_follows_each_region() {
        let dusk = derive(&def_of("builtin:dusk").base);
        assert!(dusk.content.dark && dusk.nav.dark);
        let sand = derive(&def_of("builtin:sandstone").base);
        assert!(!sand.content.dark && !sand.nav.dark);
    }

    // -- the literal themes -------------------------------------------------------------------

    fn ratio(report: &Report, region: Region, fg: &str, bg: &str) -> f64 {
        report
            .pairs
            .iter()
            .find(|p| p.region == region && p.fg == fg && p.bg == bg)
            .unwrap_or_else(|| panic!("no pair {fg} on {bg}"))
            .ratio
    }

    fn failing(report: &Report, region: Region) -> Vec<(&'static str, &'static str)> {
        let mut v: Vec<_> = report
            .pairs
            .iter()
            .filter(|p| p.region == region && p.ratio < p.min)
            .map(|p| (p.fg, p.bg))
            .collect();
        v.sort_unstable();
        v
    }

    fn sorted(mut v: Vec<(&'static str, &'static str)>) -> Vec<(&'static str, &'static str)> {
        v.sort_unstable();
        v
    }

    fn check_ratios(report: &Report, region: Region, rows: &[(&str, &str, f64)]) {
        for &(fg, bg, want) in rows {
            let got = ratio(report, region, fg, bg);
            assert!(
                (got - want).abs() < 0.01,
                "{fg} on {bg}: {got:.3}, want {want}"
            );
        }
    }

    #[test]
    fn ironteams_dark_reports_the_measured_ratios_and_never_adjusts() {
        let (set, report) = theme_tokens(def_of("builtin:dark"), None);
        assert_eq!(set.content, DARK);
        assert_eq!(set.nav, nav_of_literal(&DARK));
        assert!(report.adjusted.is_empty() && !report.low_contrast);
        use Region::{Content, Nav};
        check_ratios(
            &report,
            Content,
            &[
                ("fg", "accent-soft", 10.17),
                ("fg-muted", "bubble-mine", 5.75),
                ("fg-subtle", "bubble-mine", 3.74),
                ("accent", "surface", 4.56),
                ("mention", "bubble-mine", 6.91),
                ("on-accent", "accent", 3.19),
                ("on-badge", "unread-badge", 5.40),
                ("on-danger", "danger-hover", 3.99),
                ("fg-strong", "success-soft", 14.08),
                ("danger-text", "surface", 6.42),
                ("warning", "surface", 8.45),
                ("success", "surface", 8.12),
            ],
        );
        check_ratios(
            &report,
            Nav,
            &[
                ("fg", "selected", 13.58),
                ("fg-muted", "selected", 6.05),
                ("fg-subtle", "selected", 3.93),
                ("on-badge", "unread-badge", 5.40),
            ],
        );
        assert_eq!(
            failing(&report, Content),
            sorted(vec![("on-accent", "accent"), ("on-danger", "danger-hover")])
        );
        assert!(failing(&report, Nav).is_empty());
    }

    #[test]
    fn ironteams_light_reports_the_measured_ratios_and_never_adjusts() {
        let (set, report) = theme_tokens(def_of("builtin:light"), None);
        assert_eq!(set.content, LIGHT);
        assert!(report.adjusted.is_empty() && !report.low_contrast);
        use Region::{Content, Nav};
        check_ratios(
            &report,
            Content,
            &[
                ("fg", "accent-faint", 1.13),
                ("fg-muted", "selected", 5.63),
                ("fg-subtle", "selected", 4.30),
                ("accent", "surface", 4.72),
                ("mention", "bubble-mine", 5.56),
                ("on-accent", "accent", 5.38),
                ("on-badge", "unread-badge", 5.40),
                ("on-danger", "danger-hover", 3.99),
                ("fg-strong", "success-soft", 14.08),
                ("danger-text", "surface", 1.99),
                ("warning", "surface", 1.51),
                ("success", "surface", 1.57),
            ],
        );
        check_ratios(
            &report,
            Nav,
            &[
                ("fg", "selected", 13.08),
                ("fg-muted", "selected", 5.63),
                ("fg-subtle", "selected", 4.30),
                ("on-badge", "unread-badge", 5.40),
            ],
        );
        assert_eq!(
            failing(&report, Content),
            sorted(vec![
                ("fg", "bubble-mention"),
                ("fg", "accent-soft"),
                ("fg", "accent-subtle"),
                ("fg", "accent-muted"),
                ("fg", "accent-faint"),
                ("on-danger", "danger-hover"),
                ("danger-text", "bg"),
                ("danger-text", "surface"),
                ("danger-text", "panel"),
                ("warning", "bg"),
                ("warning", "surface"),
                ("warning", "panel"),
                ("success", "bg"),
                ("success", "surface"),
                ("success", "panel"),
            ])
        );
    }

    #[test]
    fn a_literal_theme_takes_the_accent_and_a_derived_one_ignores_it() {
        let a = Some(hex("#0f6cbd"));
        let (dark, _) = theme_tokens(def_of("builtin:dark"), a);
        assert_eq!(dark.content.accent, 0x5798d1ff);
        assert_eq!(dark.nav.accent, 0x5798d1ff);
        let dusk = def_of("builtin:dusk");
        assert_eq!(theme_tokens(dusk, a), theme_tokens(dusk, None));
    }

    // -- adjustment ---------------------------------------------------------------------------

    fn picked_of(report: &Report, token: &str) -> Option<(u32, u32)> {
        report
            .adjusted
            .iter()
            .find(|a| a.token == token && a.region == Region::Content)
            .map(|a| (a.picked, a.drawn))
    }

    #[test]
    fn low_contrast_text_moves_and_is_reported() {
        let b = base("#202020 #ffffff #333333 #808080 #777777 #5b5fc7 #aaaaff #cc3344");
        let mut set = derive(&b);
        let picked_fg = set.content.fg;
        let report = adjust(&mut set, false);
        let (picked, drawn) = picked_of(&report, "fg").expect("fg is adjusted");
        assert_eq!(picked, 0x777777ff);
        assert_eq!(picked, picked_fg);
        assert_ne!(drawn, picked);
        assert_eq!(set.content.fg, drawn);
        assert!(contrast(to_rgb(drawn), to_rgb(set.content.bg)) >= 4.5);
        // The base itself is untouched.
        assert_eq!(b.0[4], hex("#777777"));
    }

    #[test]
    fn backgrounds_on_both_sides_of_mid_grey_set_low_contrast() {
        // Black text fails on the darker tints, white text fails on the mid-grey background.
        let b = base("#202020 #ffffff #333333 #808080 #000000 #5b5fc7 #aaaaff #cc3344");
        let mut set = derive(&b);
        let report = adjust(&mut set, false);
        assert!(report.low_contrast);
        assert!(report.pairs.iter().any(|p| p.fg == "fg" && p.ratio < p.min));
    }

    #[test]
    fn keep_original_moves_nothing_and_says_low_contrast() {
        let b = base("#202020 #ffffff #333333 #808080 #777777 #5b5fc7 #aaaaff #cc3344");
        let mut set = derive(&b);
        let before = set;
        let report = adjust(&mut set, true);
        assert_eq!(set, before);
        assert!(report.adjusted.is_empty());
        assert!(report.low_contrast);
        // A passing theme is not low contrast with it either.
        let mut ok = derive(&def_of("builtin:dusk").base);
        assert!(!adjust(&mut ok, true).low_contrast);
    }

    #[test]
    fn an_accent_that_fails_on_the_background_moves_and_its_companions_follow() {
        let b = base("#202020 #ffffff #333333 #141414 #f0f0f0 #1a1a2a #aaaaff #cc3344");
        let mut set = derive(&b);
        let report = adjust(&mut set, false);
        let (picked, drawn) = picked_of(&report, "accent").expect("accent is adjusted");
        assert_eq!(picked, 0x1a1a2aff);
        let accent = to_rgb(drawn);
        let c = &set.content;
        assert_eq!(c.accent, drawn);
        for bg in ["bg", "surface", "panel"] {
            assert!(contrast(accent, rgb_of(c, bg)) >= 3.0, "{bg}");
        }
        let o = to_oklch(accent);
        let strong = from_oklch(Oklch { l: o.l - 0.08, ..o });
        assert_eq!(c.accent_strong, op(strong));
        assert_eq!(c.on_accent, op(best_on(&[accent, strong])));
        assert_eq!(c.accent_wash, from_rgb(accent, 0x26));
        assert_eq!(c.accent_tint, from_rgb(accent, 0x40));
        // The nav copies the drawn accent.
        assert_eq!(set.nav.accent, c.accent);
        assert_eq!(set.nav.accent_strong, c.accent_strong);
        assert!(!report.low_contrast);
    }

    // -- themes.json: shapes ------------------------------------------------------------------

    #[test]
    fn the_json_shapes_are_the_documented_ones() {
        let pair = ThemeChoice::Pair {
            light: "builtin:light".into(),
            dark: "builtin:dark".into(),
            switch: Switch::Schedule {
                light_from: 480,
                dark_from: 1140,
            },
        };
        let v = json!({"pair": {"light": "builtin:light", "dark": "builtin:dark",
            "switch": {"schedule": {"light_from": 480, "dark_from": 1140}}}});
        assert_eq!(serde_json::to_value(&pair).unwrap(), v);
        assert_eq!(serde_json::from_value::<ThemeChoice>(v).unwrap(), pair);
        let fixed = ThemeChoice::Fixed("builtin:dusk".into());
        assert_eq!(
            serde_json::to_value(&fixed).unwrap(),
            json!({"fixed": "builtin:dusk"})
        );
        assert_eq!(
            serde_json::to_value(default_choice()).unwrap(),
            json!({"pair": {"light": "builtin:light", "dark": "builtin:dark", "switch": "system"}})
        );
        assert_eq!(
            serde_json::to_value(CallChoice::SameAsApp).unwrap(),
            json!("same_as_app")
        );
        assert_eq!(
            serde_json::to_value(CallChoice::Fixed("builtin:dark".into())).unwrap(),
            json!({"fixed": "builtin:dark"})
        );
        let def = ThemeDef {
            id: "custom:1".into(),
            name: "Night".into(),
            base: base(NIGHT),
            keep_original: false,
            literal: None,
        };
        assert_eq!(
            serde_json::to_value(&def).unwrap(),
            json!({"id": "custom:1", "name": "Night", "base": NIGHT.split(' ').collect::<Vec<_>>(),
                "keep_original": false})
        );
    }

    #[test]
    fn the_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut file = ThemesFile::default();
        let id = file.add("Night shift", base(NIGHT), true).unwrap();
        file.choice = Some(default_choice());
        file.set_account("personal", Some("builtin:dusk"));
        file.call = CallChoice::Fixed(id);
        file.font_code = Some("JetBrains Mono".into());
        file.save_to(&path).unwrap();
        let (loaded, note) = ThemesFile::load_from(&path);
        assert_eq!(loaded, file);
        assert_eq!(note, LoadNote::default());
        // Colours are written as lower-case #rrggbb strings.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"#1d2b45\""), "{text}");
    }

    // -- themes.json: loader ------------------------------------------------------------------

    fn custom_json(id: &str, name: &str) -> Value {
        json!({"id": id, "name": name, "base": NIGHT.split(' ').collect::<Vec<_>>(), "keep_original": false})
    }

    fn load(value: &Value) -> (ThemesFile, LoadNote, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        let (file, note) = ThemesFile::load_from(&path);
        (file, note, dir)
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (file, note) = ThemesFile::load_from(&dir.path().join(FILE_NAME));
        assert_eq!(file, ThemesFile::default());
        assert_eq!(note, LoadNote::default());
        assert_eq!(file.choice, None);
    }

    #[test]
    fn a_broken_custom_entry_is_skipped_and_the_others_load() {
        let (file, note, _dir) = load(&json!({"custom": [
            custom_json("custom:a", "A"),
            {"id": "custom:b", "name": 5},
            custom_json("custom:c", "C"),
        ]}));
        let ids: Vec<&str> = file.custom.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["custom:a", "custom:c"]);
        assert_eq!(note.skipped, 1);
        assert!(!note.renamed_bad);
    }

    #[test]
    fn custom_entries_need_a_custom_id_a_valid_name_and_a_new_id() {
        let (file, note, _dir) = load(&json!({"custom": [
            custom_json("builtin:dusk", "Not allowed"),
            custom_json("custom:a", "A"),
            custom_json("custom:a", "Again"),
            custom_json("custom:b", "has: colon"),
            custom_json("custom:c", ""),
            custom_json("custom:d", "  Padded "),
        ]}));
        let names: Vec<&str> = file.custom.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["A", "Padded"]);
        assert_eq!(note.skipped, 4);
    }

    #[test]
    fn only_the_first_50_custom_themes_load() {
        let items: Vec<Value> = (0..51)
            .map(|i| custom_json(&format!("custom:{i}"), &format!("Theme {i}")))
            .collect();
        let (file, note, _dir) = load(&json!({"custom": items}));
        assert_eq!(file.custom.len(), MAX_CUSTOM);
        assert_eq!(file.custom.last().unwrap().id, "custom:49");
        assert_eq!(note.skipped, 1);
    }

    #[test]
    fn a_file_that_is_not_json_is_renamed_and_the_defaults_are_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let bad = dir.path().join("themes.json.bad");
        std::fs::write(&path, b"{ not json").unwrap();
        std::fs::write(&bad, b"older").unwrap();
        let (file, note) = ThemesFile::load_from(&path);
        assert_eq!(file, ThemesFile::default());
        assert!(note.renamed_bad);
        assert!(!path.exists());
        assert_eq!(std::fs::read(&bad).unwrap(), b"{ not json");
        // JSON that is not an object counts the same.
        std::fs::write(&path, b"[1, 2]").unwrap();
        let (_, note) = ThemesFile::load_from(&path);
        assert!(note.renamed_bad);
        assert_eq!(std::fs::read(&bad).unwrap(), b"[1, 2]");
    }

    #[test]
    fn a_field_of_the_wrong_shape_resets_only_that_field() {
        let (file, note, _dir) = load(&json!({
            "choice": {"fixed": "builtin:dusk"},
            "call": 5,
            "accounts": {"work": "builtin:moss", "bad": 3},
            "font_ui": 7,
            "font_code": "JetBrains Mono",
        }));
        assert_eq!(file.call, CallChoice::SameAsApp);
        assert_eq!(file.choice, Some(ThemeChoice::Fixed("builtin:dusk".into())));
        assert_eq!(file.font_ui, None);
        assert_eq!(file.font_code.as_deref(), Some("JetBrains Mono"));
        assert_eq!(
            file.accounts,
            BTreeMap::from([("work".to_string(), "builtin:moss".to_string())])
        );
        assert_eq!(note, LoadNote::default());
        let (file, _, _dir) = load(&json!({"accounts": [1], "custom": 4, "choice": "x"}));
        assert_eq!(file, ThemesFile::default());
    }

    #[test]
    fn a_schedule_minute_over_1439_drops_the_choice() {
        let pair = |dark_from: u32| {
            json!({"choice": {"pair": {"light": "builtin:light", "dark": "builtin:dark",
                "switch": {"schedule": {"light_from": 480, "dark_from": dark_from}}}},
                "font_ui": "Inter"})
        };
        let (file, _, _d) = load(&pair(1440));
        assert_eq!(file.choice, None);
        assert_eq!(file.font_ui.as_deref(), Some("Inter"));
        let (file, _, _d) = load(&pair(1439));
        assert!(file.choice.is_some());
        let (file, _, _d) = load(&pair(70_000));
        assert_eq!(file.choice, None);
    }

    // -- themes.json: API ---------------------------------------------------------------------

    #[test]
    fn add_gives_a_custom_uuid_id() {
        let mut file = ThemesFile::default();
        let id = file.add("Night", base(NIGHT), false).unwrap();
        let uuid = id.strip_prefix("custom:").expect("custom: prefix");
        assert!(Uuid::parse_str(uuid).is_ok(), "{id}");
        assert_eq!(file.custom[0].id, id);
        assert_eq!(file.custom[0].literal, None);
    }

    #[test]
    fn names_stay_unique_and_within_40() {
        let mut file = ThemesFile::default();
        let name_of = |file: &ThemesFile, id: &str| find(file, id).unwrap().name.clone();
        let a = file.add("Night", base(NIGHT), false).unwrap();
        let b = file.add("Night", base(NIGHT), false).unwrap();
        let c = file.add("night", base(NIGHT), false).unwrap();
        let d = file.add("Dusk", base(NIGHT), false).unwrap();
        assert_eq!(name_of(&file, &a), "Night");
        assert_eq!(name_of(&file, &b), "Night (2)");
        assert_eq!(name_of(&file, &c), "night (3)");
        assert_eq!(name_of(&file, &d), "Dusk (2)");
        let long = "x".repeat(40);
        file.add(&long, base(NIGHT), false).unwrap();
        let e = file.add(&long, base(NIGHT), false).unwrap();
        let shown = name_of(&file, &e);
        assert_eq!(shown.chars().count(), 40);
        assert_eq!(shown, format!("{} (2)", "x".repeat(36)));
    }

    #[test]
    fn add_refuses_a_bad_name_and_the_51st_theme() {
        let mut file = ThemesFile::default();
        assert_eq!(
            file.add("a:b", base(NIGHT), false),
            Err(ThemeError::Name(NameError::Colon))
        );
        assert_eq!(
            file.add("  ", base(NIGHT), false),
            Err(ThemeError::Name(NameError::Empty))
        );
        for i in 0..MAX_CUSTOM {
            file.add(&format!("T{i}"), base(NIGHT), false).unwrap();
        }
        assert_eq!(
            file.add("One more", base(NIGHT), false),
            Err(ThemeError::Limit)
        );
        assert_eq!(file.custom.len(), MAX_CUSTOM);
    }

    #[test]
    fn update_and_rename_know_only_custom_themes() {
        let mut file = ThemesFile::default();
        let id = file.add("Night", base(NIGHT), false).unwrap();
        let other = base("#000000 #ffffff #111111 #000000 #ffffff #ff0000 #00ff00 #0000ff");
        assert_eq!(file.update(&id, other, true), Ok(()));
        assert_eq!(file.custom[0].base, other);
        assert!(file.custom[0].keep_original);
        assert_eq!(
            file.update("custom:nope", other, true),
            Err(ThemeError::Unknown)
        );
        assert_eq!(
            file.update("builtin:dusk", other, true),
            Err(ThemeError::Unknown)
        );
        assert_eq!(file.rename("custom:nope", "X"), Err(ThemeError::Unknown));
        assert_eq!(
            file.rename(&id, "a:b"),
            Err(ThemeError::Name(NameError::Colon))
        );
        // A theme may keep its own name; a taken one gets a suffix.
        assert_eq!(file.rename(&id, "Night"), Ok("Night".to_string()));
        assert_eq!(file.rename(&id, "Harbor"), Ok("Harbor (2)".to_string()));
        assert_eq!(file.custom[0].name, "Harbor (2)");
    }

    #[test]
    fn delete_leaves_the_references_in_place() {
        let mut file = ThemesFile::default();
        let id = file.add("Night", base(NIGHT), false).unwrap();
        file.choice = Some(ThemeChoice::Fixed(id.clone()));
        file.set_account("work", Some(&id));
        assert!(file.delete(&id));
        assert!(!file.delete(&id));
        assert!(file.custom.is_empty());
        assert_eq!(file.choice, Some(ThemeChoice::Fixed(id.clone())));
        assert_eq!(file.accounts.get("work"), Some(&id));
    }

    #[test]
    fn accounts_can_be_set_cleared_and_removed() {
        let mut file = ThemesFile::default();
        file.set_account("work", Some("builtin:moss"));
        file.set_account("work", Some("builtin:ember"));
        assert_eq!(file.accounts.len(), 1);
        assert_eq!(file.accounts["work"], "builtin:ember");
        file.set_account("work", None);
        assert!(file.accounts.is_empty());
        file.set_account("home", Some("builtin:moss"));
        assert!(file.remove_account("home"));
        assert!(!file.remove_account("home"));
    }

    #[test]
    fn reset_keeps_the_custom_themes_and_the_other_accounts() {
        let mut file = ThemesFile::default();
        let id = file.add("Night", base(NIGHT), false).unwrap();
        file.choice = Some(ThemeChoice::Fixed("builtin:dusk".into()));
        file.set_account("work", Some("builtin:moss"));
        file.set_account("home", Some("builtin:ember"));
        file.call = CallChoice::Fixed(id.clone());
        file.font_ui = Some("Inter".into());
        file.font_code = Some("Mono".into());
        file.reset(Some("work"));
        assert_eq!(file.choice, Some(default_choice()));
        assert_eq!(
            file.accounts,
            BTreeMap::from([("home".to_string(), "builtin:ember".to_string())])
        );
        assert_eq!(file.call, CallChoice::SameAsApp);
        assert_eq!((file.font_ui.clone(), file.font_code.clone()), (None, None));
        assert_eq!(file.custom.len(), 1);
        assert_eq!(file.custom[0].id, id);
        // Without an account, no entry goes.
        file.reset(None);
        assert_eq!(file.accounts.len(), 1);
    }

    // -- legacy field -------------------------------------------------------------------------

    #[test]
    fn the_legacy_field_maps_both_ways() {
        let file = ThemesFile::default();
        assert_eq!(
            choice_from_legacy(Theme::Light),
            ThemeChoice::Fixed("builtin:light".into())
        );
        assert_eq!(
            choice_from_legacy(Theme::Dark),
            ThemeChoice::Fixed("builtin:dark".into())
        );
        assert_eq!(choice_from_legacy(Theme::System), default_choice());
        for t in [Theme::Light, Theme::Dark, Theme::System] {
            assert_eq!(legacy_theme(&choice_from_legacy(t), &file), t);
        }
        let fixed = |id: &str| ThemeChoice::Fixed(id.to_string());
        assert_eq!(legacy_theme(&fixed("builtin:dusk"), &file), Theme::Dark);
        assert_eq!(legacy_theme(&fixed("builtin:harbor"), &file), Theme::Light);
        assert_eq!(legacy_theme(&fixed("custom:gone"), &file), Theme::System);
        assert_eq!(legacy_theme(&default_choice(), &file), Theme::System);
        let mut with_custom = ThemesFile::default();
        let id = with_custom
            .add(
                "Bright",
                base("#ffffff #000000 #eeeeee #ffffff #000000 #0000ff #0000aa #ff0000"),
                false,
            )
            .unwrap();
        assert_eq!(legacy_theme(&fixed(&id), &with_custom), Theme::Light);
    }
}
