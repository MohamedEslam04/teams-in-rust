//! Pure decisions: which theme, accent and text size to show, given the user's choice and what
//! the desktop portal says. No D-Bus here, so all of it is unit-tested.

use crate::general::in_window;
use crate::portal::{PortalScheme, PortalSettings};
use crate::prefs::{AppearancePrefs, Rgb, Theme, clamp_font_scale};
use crate::themes::{
    CallChoice, DARK_ID, LIGHT_ID, Report, Switch, ThemeChoice, ThemeDef, ThemesFile, builtin,
    choice_from_legacy, default_choice, find, theme_kind, theme_tokens,
};
use crate::tokens::{Tokens, literal};

/// The theme to actually draw with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolvedTheme {
    Light,
    Dark,
}

/// `Light` and `Dark` win; `System` follows the portal. With no portal answer, or "no
/// preference", the theme is dark: IronTeams's built-in look, which every view supports (several views
/// still hard-code dark colours, so a light fallback would give a half-light window).
pub fn resolve_theme(prefs: &AppearancePrefs, portal: Option<PortalScheme>) -> ResolvedTheme {
    match prefs.theme {
        Theme::Light => ResolvedTheme::Light,
        Theme::Dark => ResolvedTheme::Dark,
        Theme::System => match portal {
            Some(PortalScheme::Dark) => ResolvedTheme::Dark,
            Some(PortalScheme::Light) => ResolvedTheme::Light,
            Some(PortalScheme::NoPreference) | None => ResolvedTheme::Dark,
        },
    }
}

/// The user's accent if set, else the portal's, else `None` (use IronTeams's built-in accent).
pub fn resolve_accent(prefs: &AppearancePrefs, portal: Option<Rgb>) -> Option<Rgb> {
    prefs.accent.or(portal)
}

/// The font scale, clamped to the supported range.
pub fn resolve_font_scale(prefs: &AppearancePrefs) -> f32 {
    clamp_font_scale(prefs.font_scale)
}

/// Everything the UI needs, in one value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    pub theme: ResolvedTheme,
    pub accent: Option<Rgb>,
    pub density: crate::prefs::Density,
    pub font_scale: f32,
}

/// Combines the preferences with the portal state (`None` when there is no portal).
pub fn resolve(prefs: &AppearancePrefs, portal: Option<&PortalSettings>) -> Resolved {
    Resolved {
        theme: resolve_theme(prefs, portal.map(|p| p.color_scheme)),
        accent: resolve_accent(prefs, portal.and_then(|p| p.accent)),
        density: prefs.density,
        font_scale: resolve_font_scale(prefs),
    }
}

/// A theme id that was not found, and what was used instead. The caller logs each once.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Fallback {
    /// An account's entry: the app choice is used.
    Account { label: String, id: String },
    /// A slot of a pair: the built-in of the slot's kind is used.
    Slot { slot: ResolvedTheme, id: String },
    /// The call theme: the call is drawn with the app theme.
    Call { id: String },
    /// A fixed app choice: the default choice is used.
    Fixed { id: String },
}

/// Everything a window's `Theme` global needs about colour and fonts.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLook {
    pub content: Tokens,
    pub nav: Tokens,
    pub call: Tokens,
    /// The kind of the content area.
    pub kind: ResolvedTheme,
    pub theme_id: String,
    /// The call theme when it is not "same as app".
    pub call_theme_id: Option<String>,
    /// The accent setting applies (IronTeams Dark or Light, without high contrast).
    pub accent_applies: bool,
    /// A schedule picks the slot, so the caller runs its minute timer.
    pub schedule: bool,
    pub font_ui: Option<String>,
    pub font_code: Option<String>,
    pub report: Report,
    pub fallbacks: Vec<Fallback>,
}

/// The theme the choice names. A missing `Fixed` falls back to the default choice and a missing
/// slot of a pair to the built-in of its kind. The second value is true for a schedule.
fn pick_theme<'a>(
    choice: &ThemeChoice,
    themes: &'a ThemesFile,
    scheme: Option<PortalScheme>,
    now_min: u16,
    fallbacks: &mut Vec<Fallback>,
) -> (&'a ThemeDef, bool) {
    let choice = match choice {
        ThemeChoice::Fixed(id) => match find(themes, id) {
            Some(def) => return (def, false),
            None => {
                fallbacks.push(Fallback::Fixed { id: id.clone() });
                default_choice()
            }
        },
        pair => pair.clone(),
    };
    let ThemeChoice::Pair {
        light,
        dark,
        switch,
    } = choice
    else {
        return (builtin(DARK_ID), false);
    };
    let is_dark = match switch {
        Switch::System => scheme != Some(PortalScheme::Light),
        Switch::Schedule {
            light_from,
            dark_from,
        } => in_window(dark_from, light_from, now_min),
    };
    let (slot, id, built_in) = if is_dark {
        (ResolvedTheme::Dark, dark, DARK_ID)
    } else {
        (ResolvedTheme::Light, light, LIGHT_ID)
    };
    let def = find(themes, &id).unwrap_or_else(|| {
        fallbacks.push(Fallback::Slot { slot, id });
        builtin(built_in)
    });
    (def, matches!(switch, Switch::Schedule { .. }))
}

/// Which theme to draw with and its tokens. Pure: the caller passes the clock (`now_min`, minutes
/// after local midnight) and the high-contrast decision.
///
/// 1. An account's entry wins over the app choice (`themes.choice`, or the old `appearance.json`
///    theme when it is `None`).
/// 2. A pair picks its slot from the portal scheme (dark when there is none or no preference) or
///    from the schedule.
/// 3. High contrast gives all three regions the contrast palette of the theme's kind.
/// 4. Otherwise the theme's tokens, with the accent setting on the literal themes only.
/// 5. The call theme is the app's or its own fixed theme.
pub fn resolve_look(
    appearance: &AppearancePrefs,
    themes: &ThemesFile,
    account: Option<&str>,
    portal: Option<&PortalSettings>,
    high_contrast: bool,
    now_min: u16,
) -> ResolvedLook {
    let mut fallbacks = Vec::new();
    let app_choice = themes
        .choice
        .clone()
        .unwrap_or_else(|| choice_from_legacy(appearance.theme));
    let choice = match account.and_then(|a| Some((a, themes.accounts.get(a)?))) {
        Some((_, id)) if find(themes, id).is_some() => ThemeChoice::Fixed(id.clone()),
        Some((label, id)) => {
            fallbacks.push(Fallback::Account {
                label: label.to_string(),
                id: id.clone(),
            });
            app_choice
        }
        None => app_choice,
    };
    let (def, schedule) = pick_theme(
        &choice,
        themes,
        portal.map(|p| p.color_scheme),
        now_min,
        &mut fallbacks,
    );
    let kind = theme_kind(def);
    let contrast = literal(kind, true, None);
    let mut look = ResolvedLook {
        content: contrast,
        nav: contrast,
        call: contrast,
        kind,
        theme_id: def.id.clone(),
        call_theme_id: None,
        accent_applies: false,
        schedule,
        font_ui: themes.font_ui.clone(),
        font_code: themes.font_code.clone(),
        report: Report::default(),
        fallbacks,
    };
    if high_contrast {
        return look;
    }
    let accent = resolve_accent(appearance, portal.and_then(|p| p.accent));
    let (set, report) = theme_tokens(def, accent);
    look.content = set.content;
    look.nav = set.nav;
    look.call = set.content;
    look.report = report;
    look.accent_applies = def.literal.is_some();
    if let CallChoice::Fixed(id) = &themes.call {
        match find(themes, id) {
            Some(call_def) => {
                look.call = theme_tokens(call_def, accent).0.content;
                look.call_theme_id = Some(call_def.id.clone());
            }
            None => look.fallbacks.push(Fallback::Call { id: id.clone() }),
        }
    }
    look
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Density;

    fn prefs(theme: Theme) -> AppearancePrefs {
        AppearancePrefs {
            theme,
            ..Default::default()
        }
    }

    #[test]
    fn explicit_theme_ignores_the_portal() {
        for p in [
            None,
            Some(PortalScheme::Dark),
            Some(PortalScheme::Light),
            Some(PortalScheme::NoPreference),
        ] {
            assert_eq!(resolve_theme(&prefs(Theme::Light), p), ResolvedTheme::Light);
            assert_eq!(resolve_theme(&prefs(Theme::Dark), p), ResolvedTheme::Dark);
        }
    }

    #[test]
    fn system_theme_follows_the_portal() {
        let s = prefs(Theme::System);
        assert_eq!(
            resolve_theme(&s, Some(PortalScheme::Dark)),
            ResolvedTheme::Dark
        );
        assert_eq!(
            resolve_theme(&s, Some(PortalScheme::Light)),
            ResolvedTheme::Light
        );
        assert_eq!(
            resolve_theme(&s, Some(PortalScheme::NoPreference)),
            ResolvedTheme::Dark
        );
        assert_eq!(resolve_theme(&s, None), ResolvedTheme::Dark);
    }

    #[test]
    fn accent_user_beats_portal() {
        let mine = Rgb::new(1, 2, 3);
        let theirs = Rgb::new(9, 9, 9);
        let mut p = AppearancePrefs::default();
        assert_eq!(resolve_accent(&p, Some(theirs)), Some(theirs));
        assert_eq!(resolve_accent(&p, None), None);
        p.accent = Some(mine);
        assert_eq!(resolve_accent(&p, Some(theirs)), Some(mine));
    }

    #[test]
    fn resolve_combines_everything() {
        let p = AppearancePrefs {
            density: Density::Compact,
            font_scale: 1.25,
            ..Default::default()
        };
        let portal = PortalSettings {
            color_scheme: PortalScheme::Dark,
            accent: Some(Rgb::new(5, 6, 7)),
            ..Default::default()
        };
        let r = resolve(&p, Some(&portal));
        assert_eq!(r.theme, ResolvedTheme::Dark);
        assert_eq!(r.accent, Some(Rgb::new(5, 6, 7)));
        assert_eq!(r.density, Density::Compact);
        assert_eq!(r.font_scale, 1.25);
        assert_eq!(resolve(&p, None).theme, ResolvedTheme::Dark);
    }

    #[test]
    fn font_scale_is_clamped() {
        let p = AppearancePrefs {
            font_scale: 9.0,
            ..Default::default()
        };
        assert_eq!(resolve_font_scale(&p), 2.0);
    }
    // -- resolve_look -------------------------------------------------------------------------

    use crate::themes::{builtins, derive};
    use crate::tokens::{CONTRAST_DARK, CONTRAST_LIGHT, DARK, LIGHT};

    fn themes_with(choice: ThemeChoice) -> ThemesFile {
        ThemesFile {
            choice: Some(choice),
            ..ThemesFile::default()
        }
    }

    fn fixed(id: &str) -> ThemeChoice {
        ThemeChoice::Fixed(id.to_string())
    }

    fn pair(switch: Switch) -> ThemeChoice {
        ThemeChoice::Pair {
            light: LIGHT_ID.to_string(),
            dark: DARK_ID.to_string(),
            switch,
        }
    }

    fn schedule(light_from: u16, dark_from: u16) -> ThemeChoice {
        pair(Switch::Schedule {
            light_from,
            dark_from,
        })
    }

    fn portal_of(scheme: PortalScheme) -> PortalSettings {
        PortalSettings {
            color_scheme: scheme,
            ..Default::default()
        }
    }

    fn look(themes: &ThemesFile, portal: Option<&PortalSettings>, now: u16) -> ResolvedLook {
        resolve_look(
            &AppearancePrefs::default(),
            themes,
            None,
            portal,
            false,
            now,
        )
    }

    #[test]
    fn a_fixed_theme_ignores_the_portal_and_the_clock() {
        let themes = themes_with(fixed("builtin:dusk"));
        let light = portal_of(PortalScheme::Light);
        let l = look(&themes, Some(&light), 700);
        assert_eq!(l.theme_id, "builtin:dusk");
        assert_eq!(l.kind, ResolvedTheme::Dark);
        assert_eq!(l.content, derive(&builtins()[2].base).content);
        assert_eq!(l.nav, derive(&builtins()[2].base).nav);
        assert!(!l.schedule && !l.accent_applies);
        assert!(l.fallbacks.is_empty());
        assert!(!l.report.pairs.is_empty());
        assert_eq!(l.call, l.content);
        assert_eq!(l.call_theme_id, None);
    }

    #[test]
    fn a_system_pair_follows_the_portal() {
        let themes = themes_with(pair(Switch::System));
        for (scheme, id) in [
            (Some(PortalScheme::Dark), DARK_ID),
            (Some(PortalScheme::Light), LIGHT_ID),
            (Some(PortalScheme::NoPreference), DARK_ID),
            (None, DARK_ID),
        ] {
            let portal = scheme.map(portal_of);
            let l = look(&themes, portal.as_ref(), 0);
            assert_eq!(l.theme_id, id, "{scheme:?}");
            assert!(!l.schedule);
        }
        assert_eq!(look(&themes, None, 0).content, DARK);
    }

    #[test]
    fn a_schedule_switches_at_each_boundary() {
        let themes = themes_with(schedule(480, 1140));
        for (now, id) in [
            (0, DARK_ID),
            (479, DARK_ID),
            (480, LIGHT_ID),
            (1139, LIGHT_ID),
            (1140, DARK_ID),
            (1439, DARK_ID),
        ] {
            let l = look(&themes, None, now);
            assert_eq!(l.theme_id, id, "at {now}");
            assert!(l.schedule, "at {now}");
        }
        // The portal does not matter while a schedule is on.
        let dark = portal_of(PortalScheme::Dark);
        assert_eq!(look(&themes, Some(&dark), 600).theme_id, LIGHT_ID);
    }

    #[test]
    fn an_overnight_light_schedule() {
        let themes = themes_with(schedule(1200, 360));
        for (now, id) in [
            (0, LIGHT_ID),
            (359, LIGHT_ID),
            (360, DARK_ID),
            (1199, DARK_ID),
            (1200, LIGHT_ID),
            (1439, LIGHT_ID),
        ] {
            assert_eq!(look(&themes, None, now).theme_id, id, "at {now}");
        }
    }

    #[test]
    fn equal_schedule_times_are_always_light() {
        let themes = themes_with(schedule(600, 600));
        for now in [0, 599, 600, 601, 1439] {
            let l = look(&themes, None, now);
            assert_eq!(l.theme_id, LIGHT_ID, "at {now}");
            assert!(l.schedule);
        }
    }

    #[test]
    fn an_account_entry_wins_and_is_not_scheduled() {
        let mut themes = themes_with(schedule(480, 1140));
        themes
            .accounts
            .insert("work".to_string(), "builtin:moss".to_string());
        let prefs = AppearancePrefs::default();
        for now in [100, 700, 1200] {
            let l = resolve_look(&prefs, &themes, Some("work"), None, false, now);
            assert_eq!(l.theme_id, "builtin:moss", "at {now}");
            assert!(!l.schedule, "at {now}");
            assert!(l.fallbacks.is_empty());
        }
        // Another account, or none, keeps the app choice.
        let l = resolve_look(&prefs, &themes, Some("home"), None, false, 700);
        assert_eq!((l.theme_id.as_str(), l.schedule), (LIGHT_ID, true));
        let l = resolve_look(&prefs, &themes, None, None, false, 700);
        assert_eq!((l.theme_id.as_str(), l.schedule), (LIGHT_ID, true));
    }

    #[test]
    fn a_stale_account_entry_falls_back_to_the_app_choice() {
        let mut themes = themes_with(fixed("builtin:dusk"));
        themes
            .accounts
            .insert("work".to_string(), "custom:gone".to_string());
        let l = resolve_look(
            &AppearancePrefs::default(),
            &themes,
            Some("work"),
            None,
            false,
            0,
        );
        assert_eq!(l.theme_id, "builtin:dusk");
        assert_eq!(
            l.fallbacks,
            [Fallback::Account {
                label: "work".to_string(),
                id: "custom:gone".to_string()
            }]
        );
    }

    #[test]
    fn a_stale_pair_slot_falls_back_to_the_builtin_of_its_kind() {
        let stale = |light: &str, dark: &str| {
            themes_with(ThemeChoice::Pair {
                light: light.to_string(),
                dark: dark.to_string(),
                switch: Switch::System,
            })
        };
        let light = portal_of(PortalScheme::Light);
        let dark = portal_of(PortalScheme::Dark);
        let themes = stale("custom:gone", "builtin:moss");
        let l = look(&themes, Some(&light), 0);
        assert_eq!(l.theme_id, LIGHT_ID);
        assert_eq!(
            l.fallbacks,
            [Fallback::Slot {
                slot: ResolvedTheme::Light,
                id: "custom:gone".to_string()
            }]
        );
        // The slot that is not in use is not looked at.
        assert!(look(&themes, Some(&dark), 0).fallbacks.is_empty());
        let themes = stale("builtin:sandstone", "custom:gone");
        let l = look(&themes, Some(&dark), 0);
        assert_eq!(l.theme_id, DARK_ID);
        assert_eq!(
            l.fallbacks,
            [Fallback::Slot {
                slot: ResolvedTheme::Dark,
                id: "custom:gone".to_string()
            }]
        );
    }

    #[test]
    fn a_stale_fixed_choice_falls_back_to_the_default() {
        let themes = themes_with(fixed("custom:gone"));
        let light = portal_of(PortalScheme::Light);
        let l = look(&themes, Some(&light), 0);
        assert_eq!(l.theme_id, LIGHT_ID);
        assert_eq!(
            l.fallbacks,
            [Fallback::Fixed {
                id: "custom:gone".to_string()
            }]
        );
        assert_eq!(look(&themes, None, 0).theme_id, DARK_ID);
    }

    #[test]
    fn a_stale_call_theme_falls_back_to_the_app_theme() {
        let mut themes = themes_with(fixed("builtin:dusk"));
        themes.call = CallChoice::Fixed("custom:gone".to_string());
        let l = look(&themes, None, 0);
        assert_eq!(l.call, l.content);
        assert_eq!(l.call_theme_id, None);
        assert_eq!(
            l.fallbacks,
            [Fallback::Call {
                id: "custom:gone".to_string()
            }]
        );
    }

    #[test]
    fn high_contrast_uses_the_contrast_palette_of_the_kind_in_all_regions() {
        for (id, palette) in [
            ("builtin:dusk", CONTRAST_DARK),
            ("builtin:dark", CONTRAST_DARK),
            ("builtin:harbor", CONTRAST_LIGHT),
            ("builtin:light", CONTRAST_LIGHT),
        ] {
            let themes = themes_with(fixed(id));
            let prefs = AppearancePrefs {
                accent: Some(Rgb::new(1, 2, 3)),
                ..Default::default()
            };
            let l = resolve_look(&prefs, &themes, None, None, true, 0);
            assert_eq!(
                (l.content, l.nav, l.call),
                (palette, palette, palette),
                "{id}"
            );
            assert_eq!(l.theme_id, id);
            assert!(l.report.pairs.is_empty() && !l.report.low_contrast);
            assert!(!l.accent_applies);
        }
    }

    #[test]
    fn the_accent_applies_only_to_the_literal_themes() {
        let accent = Rgb::from_hex("#0f6cbd").unwrap();
        let prefs = AppearancePrefs {
            accent: Some(accent),
            ..Default::default()
        };
        let resolve_with =
            |id: &str| resolve_look(&prefs, &themes_with(fixed(id)), None, None, false, 0);
        let dark = resolve_with("builtin:dark");
        assert!(dark.accent_applies);
        assert_eq!(dark.content.accent_strong, 0x0f6cbdff);
        assert_eq!(dark.nav.accent, dark.content.accent);
        let light = resolve_with("builtin:light");
        assert!(light.accent_applies);
        assert_eq!(light.content.accent, 0x0f6cbdff);
        let dusk = resolve_with("builtin:dusk");
        assert!(!dusk.accent_applies);
        assert_eq!(dusk.content, derive(&builtins()[2].base).content);
        // The portal's accent counts when the user has none.
        let portal = PortalSettings {
            accent: Some(accent),
            ..Default::default()
        };
        let l = resolve_look(
            &AppearancePrefs::default(),
            &themes_with(fixed("builtin:light")),
            None,
            Some(&portal),
            false,
            0,
        );
        assert_eq!(l.content.accent, 0x0f6cbdff);
    }

    #[test]
    fn the_call_set_is_the_apps_or_its_own_theme() {
        let mut themes = themes_with(fixed("builtin:light"));
        let l = look(&themes, None, 0);
        assert_eq!(l.content, LIGHT);
        assert_eq!(l.call, l.content);
        assert_eq!(l.call_theme_id, None);
        themes.call = CallChoice::Fixed("builtin:dark".to_string());
        let l = look(&themes, None, 0);
        assert_eq!(l.content, LIGHT);
        assert_eq!(l.call, DARK);
        assert_eq!(l.call_theme_id.as_deref(), Some("builtin:dark"));
        assert!(l.fallbacks.is_empty());
    }

    #[test]
    fn no_choice_maps_the_old_appearance_field() {
        let themes = ThemesFile::default();
        assert_eq!(themes.choice, None);
        let resolve_with = |theme: Theme, portal: Option<&PortalSettings>| {
            let prefs = prefs(theme);
            resolve_look(&prefs, &themes, None, portal, false, 0)
        };
        let dark = portal_of(PortalScheme::Dark);
        let light = portal_of(PortalScheme::Light);
        assert_eq!(resolve_with(Theme::Light, Some(&dark)).theme_id, LIGHT_ID);
        assert_eq!(resolve_with(Theme::Dark, Some(&light)).theme_id, DARK_ID);
        assert_eq!(resolve_with(Theme::System, Some(&light)).theme_id, LIGHT_ID);
        assert_eq!(resolve_with(Theme::System, Some(&dark)).theme_id, DARK_ID);
        assert_eq!(resolve_with(Theme::System, None).theme_id, DARK_ID);
        assert!(!resolve_with(Theme::System, None).schedule);
        // Once themes.json has a choice, the old field no longer matters.
        let themes = themes_with(fixed("builtin:moss"));
        let l = resolve_look(&prefs(Theme::Light), &themes, None, None, false, 0);
        assert_eq!(l.theme_id, "builtin:moss");
    }

    #[test]
    fn fonts_are_copied() {
        let themes = ThemesFile {
            font_ui: Some("Inter".to_string()),
            font_code: Some("JetBrains Mono".to_string()),
            ..ThemesFile::default()
        };
        let l = look(&themes, None, 0);
        assert_eq!(l.font_ui.as_deref(), Some("Inter"));
        assert_eq!(l.font_code.as_deref(), Some("JetBrains Mono"));
        let l = look(&ThemesFile::default(), None, 0);
        assert_eq!((l.font_ui, l.font_code), (None, None));
    }
}
