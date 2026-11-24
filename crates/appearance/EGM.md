# appearance

## Overview
IronTeams appearance and general preferences: theme, accent, density, font scale, accessibility switches, text snippets, language and date/time format state, plus a reader for the XDG desktop portal (D-Bus) settings. Each preferences type loads leniently from a JSON file in the config directory and saves atomically.

## Interface
- Modules (all `pub` except `store`): `accessibility`, `formats`, `general`, `language`, `portal`, `prefs`, `resolve`, `snippets` [lib.rs].
- Re-exports at the crate root [lib.rs]: `AccessibilityPrefs, AutoSwitch, effective_font_scale, resolve_high_contrast, resolve_reduced_motion`; `DateFormat, GeneralPrefs, Language, NotificationStyle, TimeFormat, available_languages, effective_language, resolve_language, system_language`; `PortalChange, PortalError, PortalScheme, PortalSettings, PortalStream`; `AppearancePrefs, Density, FONT_SCALE_MAX, FONT_SCALE_MIN, Rgb, Theme`; `Resolved, ResolvedTheme, resolve, resolve_accent, resolve_font_scale, resolve_theme`; `Snippet, SnippetPrefs`; `store::{Error, config_dir}`.
- `prefs`: `Theme {Light, Dark, System}`, `Density {Comfy, Compact}`, `Rgb` (`from_hex`, `to_hex`, `from_unit`), `AppearancePrefs` (`load`, `load_from`, `save`, `save_to`, `path`, `set_font_scale`), `clamp_font_scale`, `FILE_NAME = "appearance.json"` [prefs.rs].
- `accessibility`: `AccessibilityPrefs`, `AutoSwitch {Auto, On, Off}`, `resolve_high_contrast`, `resolve_reduced_motion`, `effective_font_scale`, `LARGER_TEXT_FACTOR = 1.25`, `EFFECTIVE_SCALE_MAX = 2.5`, `FILE_NAME = "accessibility.json"` [accessibility.rs].
- `resolve`: `resolve(&AppearancePrefs, Option<&PortalSettings>) -> Resolved`, `resolve_theme`, `resolve_accent`, `resolve_font_scale` [resolve.rs].
- `snippets`: `Snippet`, `SnippetPrefs` (`effective`), `builtin()`, `FILE_NAME = "snippets.json"` [snippets.rs].
- `general`: `GeneralPrefs`, `QuietHoursPrefs`, `Language`, `DateFormat`, `TimeFormat`, `NotificationStyle`, `FocusDuration`, `FOCUS_FOREVER`, `FALLBACK_LANGUAGE = "en"`, `normalize_tag`, `best_match`, `resolve_language`, `effective_language`, `system_language`, `FILE_NAME = "general.json"` [general.rs].
- `language`: `set_from`, `set_from_with`, `current` (process-wide active language) [language.rs].
- `formats`: `set`, `set_from`, `set_time_zone`, `time_zone`, `current`, `is_h24`, and clock/date helpers (`clock`, `hour`, `meridiem`, `date_pattern`, `local`, `offset_ms` and `_in` / `_for` variants); re-exports `chrono_tz::Tz` [formats.rs].
- `portal`: `read() -> Result<Option<PortalSettings>, PortalError>`, `watch() -> Result<Option<PortalStream>, PortalError>`, `PortalScheme`, `PortalSettings`, `PortalChange`, pure `parse_*` helpers [portal.rs].
- `store`: `Error {NoHome, Io, Encode}` and `config_dir()` [store.rs].

## Invariants
- Font scale is clamped to 0.75..=2.0 on load, set, save and resolve; NaN or infinite becomes 1.0 [prefs.rs:clamp_font_scale]. Effective scale = clamp(base x 1.25 if larger text, 0.75, 2.5) [accessibility.rs:effective_font_scale].
- Loading never fails: a missing, corrupt or wrong-shape file yields defaults, and a bad field resets only that field (`#[serde(default)]` plus lenient deserializers) [store.rs:load_json, lenient].
- Saving is atomic: temp file `.<name>.<pid>.tmp` in the same dir, `sync_all`, rename; the temp is removed on failure [store.rs:save_json].
- Theme resolution: explicit Light/Dark wins; `System` follows the portal; no preference or no portal gives Dark [resolve.rs:resolve_theme]. Accent: user value, else portal accent [resolve.rs:resolve_accent].
- `AutoSwitch` On/Off always win; Auto reduced-motion is true only when the portal reports animations disabled [accessibility.rs:resolve_reduced_motion].
- `SnippetPrefs.items == None` means never saved (built-ins returned); `Some(vec![])` stays empty [snippets.rs:SnippetPrefs::effective].
- Language: `resolve_language` always returns an available language (user choice, system, fallback `en`); `system_language_from` checks `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG` in that order [general.rs:resolve_language, system_language_from].
- `auto_update` stays on unless explicitly `false` in the file [general.rs:lenient_on].
- Portal `read`/`watch` never panic; a missing session bus, portal service or key yields `Ok(None)`, other errors are `Err(PortalError)` [portal.rs:is_absent, read].
- `formats::set_from` must be called at start and on every preference change; the shared format and zone state is process-wide [formats.rs:CHOICE, ZONE].
- `config_dir`: `IRONTEAMS_HOME` overrides to `<home>/config`, else `ProjectDirs("ironteams")`, else `Error::NoHome` [store.rs:config_dir].

## Side Effects
- Disk read and write of `appearance.json`, `accessibility.json`, `snippets.json`, `general.json` in the config dir (create_dir_all, temp write, fsync, rename) [store.rs:save_json].
- Calls `compat::migrate_legacy_dirs()` on every `config_dir()` call when `IRONTEAMS_HOME` is unset [store.rs:config_dir].
- D-Bus: session bus connection to `org.freedesktop.portal.Desktop` (`ReadOne`, fallback `Read`, and `SettingChanged` subscription) [portal.rs:connect, read_key, watch].
- Global state: `static CURRENT` RwLock in `language`, `CHOICE` and `ZONE` RwLocks in `formats`; poisoning is ignored [language.rs, formats.rs].
- Reads env vars `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`, `IRONTEAMS_HOME`; reads local clock and time zone via chrono [general.rs:system_language, formats.rs].

## Verification
- Run: `cargo test -p appearance`
