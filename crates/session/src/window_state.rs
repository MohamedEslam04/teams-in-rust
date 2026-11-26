//! Window state: size, position, the last open chat and the collapsed sidebar sections.
//!
//! Stored as `window-state.json` in the config dir (next to `config.json`, so `IRONTEAMS_HOME`
//! applies), separate from [`crate::Config`] so a GUI crash or a rewrite never touches account
//! data. Nothing here is secret, but the conversation id is still kept out of `Debug` output and
//! this module never logs.
//!
//! The geometry (`width`, `height`, `x`, `y`, `maximized`) and `sidebar_width` are shared by
//! every account. What an account was looking at (`last_chat`, `last_channel`, the collapsed
//! sections) is an [`AccountView`] kept per account in `accounts`, keyed by the account's
//! [`crate::token_key`] (`label`, or `label@tenant` in a guest organization). A file written
//! before that has these three at the top level; loading moves them into the entry of the
//! account the caller names, and saving never writes them at the top level again.
//!
//! Loading never fails: a missing, unreadable or corrupt file gives the defaults, and values a
//! hand edit or an older build left unreasonable are clamped.

use crate::{Paths, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// File name inside the config dir.
pub const FILE_NAME: &str = "window-state.json";

/// Smallest and largest window edge kept, in logical pixels.
pub const MIN_WIDTH: i32 = 400;
pub const MIN_HEIGHT: i32 = 300;
pub const MAX_SIZE: i32 = 16_384;
/// Positions further than this from the origin on either axis are dropped (the window manager
/// then places the window).
pub const MAX_COORD: i32 = 32_000;

const DEFAULT_WIDTH: i32 = 1100;
const DEFAULT_HEIGHT: i32 = 720;
const MAX_ID_LEN: usize = 512;
const MAX_SECTIONS: usize = 256;
const MAX_SECTION_LEN: usize = 128;
/// Longest account key kept, and most account views kept.
const MAX_KEY_LEN: usize = 256;
const MAX_VIEWS: usize = 64;

/// What one account was looking at: restored when that account starts again.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AccountView {
    /// Id of the conversation that was open.
    pub last_chat: Option<String>,
    /// Id of the last channel that was open: the Teams page opens it again.
    pub last_channel: Option<String>,
    /// Ids of the collapsed sidebar sections.
    pub collapsed_sections: BTreeSet<String>,
}

impl std::fmt::Debug for AccountView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountView")
            .field("last_chat", &self.last_chat.as_ref().map(|_| "<set>"))
            .field("last_channel", &self.last_channel.as_ref().map(|_| "<set>"))
            .field("collapsed_sections", &self.collapsed_sections.len())
            .finish()
    }
}

/// The view of an account that has none yet.
static EMPTY_VIEW: AccountView = AccountView {
    last_chat: None,
    last_channel: None,
    collapsed_sections: BTreeSet::new(),
};

impl AccountView {
    fn is_empty(&self) -> bool {
        *self == AccountView::default()
    }

    fn clamp(&mut self) {
        for id in [&mut self.last_chat, &mut self.last_channel] {
            if id
                .as_deref()
                .is_some_and(|c| c.trim().is_empty() || c.len() > MAX_ID_LEN)
            {
                *id = None;
            }
        }
        self.collapsed_sections
            .retain(|s| !s.is_empty() && s.len() <= MAX_SECTION_LEN);
        while self.collapsed_sections.len() > MAX_SECTIONS {
            self.collapsed_sections.pop_last();
        }
    }

    /// Remembers the open conversation, or forgets it with `None`.
    pub fn set_last_chat(&mut self, id: Option<&str>) {
        self.last_chat = id.map(str::to_owned);
        self.clamp();
    }

    pub fn set_last_channel(&mut self, id: Option<&str>) {
        self.last_channel = id.map(str::to_owned);
        self.clamp();
    }

    pub fn is_collapsed(&self, section: &str) -> bool {
        self.collapsed_sections.contains(section)
    }

    /// Collapses or expands a section.
    pub fn set_section_collapsed(&mut self, section: &str, collapsed: bool) {
        if collapsed {
            self.collapsed_sections.insert(section.to_owned());
            self.clamp();
        } else {
            self.collapsed_sections.remove(section);
        }
    }

    /// Flips a section and returns whether it is collapsed now.
    pub fn toggle_section(&mut self, section: &str) -> bool {
        let now = !self.is_collapsed(section);
        self.set_section_collapsed(section, now);
        self.is_collapsed(section)
    }
}

/// What is restored on start. Every field has a default so old and partial files load.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    /// Width of the window when not maximized.
    pub width: i32,
    /// Height of the window when not maximized.
    pub height: i32,
    /// Position when not maximized; `None` lets the window manager decide (always the case on
    /// Wayland).
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub maximized: bool,
    /// What each account was looking at, by [`crate::token_key`]. An entry that holds nothing is
    /// not kept.
    pub accounts: BTreeMap<String, AccountView>,
    /// Width of the chat and teams list the user dragged it to; `None` keeps the default.
    pub sidebar_width: Option<u32>,
    /// Where the call filmstrip sits beside a shared screen: "right" | "bottom" | "hidden" |
    /// "auto"; `None` (or an unknown value, which `clamp` drops) keeps the "auto" default.
    pub filmstrip_pos: Option<String>,
    /// Top-level fields of a file written before the per-account views. Read only: [`load_from`]
    /// moves them into the loading account's entry, and they are never written.
    ///
    /// [`load_from`]: WindowState::load_from
    #[serde(rename = "last_chat", skip_serializing)]
    legacy_last_chat: Option<String>,
    #[serde(rename = "last_channel", skip_serializing)]
    legacy_last_channel: Option<String>,
    #[serde(rename = "collapsed_sections", skip_serializing)]
    legacy_collapsed_sections: BTreeSet<String>,
    /// The account key the shortcut methods ([`WindowState::set_last_chat`], ...) act on. Not
    /// stored, and not part of equality: it names who is looking, not what is saved.
    #[serde(skip)]
    active: String,
}

impl PartialEq for WindowState {
    fn eq(&self, other: &Self) -> bool {
        // Spelled out so a new field must be added here (or skipped on purpose).
        let WindowState {
            width,
            height,
            x,
            y,
            maximized,
            accounts,
            sidebar_width,
            filmstrip_pos,
            legacy_last_chat,
            legacy_last_channel,
            legacy_collapsed_sections,
            active: _,
        } = self;
        *width == other.width
            && *height == other.height
            && *x == other.x
            && *y == other.y
            && *maximized == other.maximized
            && *accounts == other.accounts
            && *sidebar_width == other.sidebar_width
            && *filmstrip_pos == other.filmstrip_pos
            && *legacy_last_chat == other.legacy_last_chat
            && *legacy_last_channel == other.legacy_last_channel
            && *legacy_collapsed_sections == other.legacy_collapsed_sections
    }
}

impl Eq for WindowState {}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            x: None,
            y: None,
            maximized: false,
            accounts: BTreeMap::new(),
            sidebar_width: None,
            filmstrip_pos: None,
            legacy_last_chat: None,
            legacy_last_channel: None,
            legacy_collapsed_sections: BTreeSet::new(),
            active: String::new(),
        }
    }
}

impl std::fmt::Debug for WindowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowState")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("x", &self.x)
            .field("y", &self.y)
            .field("maximized", &self.maximized)
            .field("accounts", &self.accounts)
            .finish()
    }
}

impl Paths {
    /// `window-state.json` in the config dir.
    pub fn window_state_path(&self) -> PathBuf {
        self.config_dir.join(FILE_NAME)
    }
}

fn clamp_pos(v: Option<i32>) -> Option<i32> {
    v.filter(|v| v.abs() <= MAX_COORD)
}

impl WindowState {
    /// Brings every value into a sane range. Called on load, on save and by the setters.
    pub fn clamp(&mut self) {
        self.width = self.width.clamp(MIN_WIDTH, MAX_SIZE);
        self.height = self.height.clamp(MIN_HEIGHT, MAX_SIZE);
        // A position needs both axes.
        self.x = clamp_pos(self.x);
        self.y = clamp_pos(self.y);
        if self.x.is_none() || self.y.is_none() {
            self.x = None;
            self.y = None;
        }
        self.accounts.retain(|k, _| k.len() <= MAX_KEY_LEN);
        for v in self.accounts.values_mut() {
            v.clamp();
        }
        self.accounts.retain(|_, v| !v.is_empty());
        // Keep the account in use when a hand edit made the map huge.
        while self.accounts.len() > MAX_VIEWS {
            let victim = self
                .accounts
                .keys()
                .rev()
                .find(|k| **k != self.active)
                .cloned();
            match victim {
                Some(k) => self.accounts.remove(&k),
                None => break,
            };
        }
        // An unknown value from a newer build falls back to the "auto" default.
        if !self
            .filmstrip_pos
            .as_deref()
            .is_some_and(|p| matches!(p, "right" | "bottom" | "hidden" | "auto"))
        {
            self.filmstrip_pos = None;
        }
    }

    /// Loads from the standard config dir for the account the config makes active; defaults
    /// when there is none or it is unreadable.
    pub fn load() -> WindowState {
        let Ok(paths) = Paths::new() else {
            return WindowState::default();
        };
        let key = paths
            .load_config()
            .map(|c| c.active_token_key())
            .unwrap_or_default();
        WindowState::load_from(paths.window_state_path(), &key)
    }

    /// Loads from the standard config dir for the account `key`; defaults when there is none or
    /// it is unreadable.
    pub fn load_for(key: &str) -> WindowState {
        match Paths::new() {
            Ok(p) => WindowState::load_from(p.window_state_path(), key),
            Err(_) => WindowState::default(),
        }
    }

    /// Loads from `path` for the account `key` (a [`crate::token_key`]); the shortcut methods act
    /// on its view. A missing or corrupt file gives the defaults; values are clamped. Fields a
    /// file from before the per-account views has at the top level move into `key`'s entry,
    /// without replacing what that entry already holds.
    pub fn load_from(path: impl AsRef<Path>, key: &str) -> WindowState {
        let mut s = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<WindowState>(&raw).ok())
            .unwrap_or_default();
        s.active = key.to_owned();
        s.migrate_legacy();
        s.clamp();
        s
    }

    /// Moves the legacy top-level fields into the active account's view.
    fn migrate_legacy(&mut self) {
        let chat = self.legacy_last_chat.take();
        let channel = self.legacy_last_channel.take();
        let sections = std::mem::take(&mut self.legacy_collapsed_sections);
        if chat.is_none() && channel.is_none() && sections.is_empty() {
            return;
        }
        let key = self.active.clone();
        let v = self.accounts.entry(key).or_default();
        if v.last_chat.is_none() {
            v.last_chat = chat;
        }
        if v.last_channel.is_none() {
            v.last_channel = channel;
        }
        if v.collapsed_sections.is_empty() {
            v.collapsed_sections = sections;
        }
    }

    /// The key of the account the shortcut methods act on (empty before one was named).
    pub fn active_key(&self) -> &str {
        &self.active
    }

    /// Makes `key` the account the shortcut methods act on.
    pub fn set_active_key(&mut self, key: &str) {
        self.active = key.to_owned();
    }

    /// What the account `key` was looking at; empty for an account with no entry.
    pub fn view(&self, key: &str) -> &AccountView {
        self.accounts.get(key).unwrap_or(&EMPTY_VIEW)
    }

    /// The entry of the account `key`, created when it has none. Call [`WindowState::clamp`]
    /// (saving does) after changing it.
    pub fn view_mut(&mut self, key: &str) -> &mut AccountView {
        self.accounts.entry(key.to_owned()).or_default()
    }

    /// The view of the active account.
    pub fn active_view(&self) -> &AccountView {
        self.view(&self.active)
    }

    /// Saves to the standard config dir.
    pub fn save(&self) -> Result<()> {
        self.save_to(Paths::new()?.window_state_path())
    }

    /// Saves to `path` atomically (temp file in the same dir, then rename), creating the dir.
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let mut s = self.clone();
        s.clamp();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let result = (|| -> Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(serde_json::to_string_pretty(&s)?.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Sets the restored (not maximized) size.
    pub fn set_size(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        self.clamp();
    }

    /// Sets the restored (not maximized) position.
    pub fn set_position(&mut self, x: i32, y: i32) {
        self.x = Some(x);
        self.y = Some(y);
        self.clamp();
    }

    pub fn set_maximized(&mut self, maximized: bool) {
        self.maximized = maximized;
    }

    /// Remembers the open conversation of the active account, or forgets it with `None`.
    pub fn set_last_chat(&mut self, id: Option<&str>) {
        let key = self.active.clone();
        self.view_mut(&key).set_last_chat(id);
        self.clamp();
    }

    pub fn set_last_channel(&mut self, id: Option<&str>) {
        let key = self.active.clone();
        self.view_mut(&key).set_last_channel(id);
        self.clamp();
    }

    pub fn is_collapsed(&self, section: &str) -> bool {
        self.active_view().is_collapsed(section)
    }

    /// Collapses or expands a section of the active account.
    pub fn set_section_collapsed(&mut self, section: &str, collapsed: bool) {
        let key = self.active.clone();
        self.view_mut(&key)
            .set_section_collapsed(section, collapsed);
        self.clamp();
    }

    /// Flips a section and returns whether it is collapsed now.
    pub fn toggle_section(&mut self, section: &str) -> bool {
        let now = !self.is_collapsed(section);
        self.set_section_collapsed(section, now);
        self.is_collapsed(section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The account the tests load as.
    const KEY: &str = "work";

    fn sample() -> WindowState {
        let mut s = WindowState::default();
        s.set_active_key(KEY);
        s.set_size(1280, 800);
        s.set_position(-1920, 40);
        s.set_maximized(true);
        s.set_last_chat(Some("19:abc@thread.v2"));
        s.toggle_section("favorites");
        s.toggle_section("teams");
        s
    }

    #[test]
    fn defaults_are_sane() {
        let d = WindowState::default();
        assert_eq!((d.width, d.height), (1100, 720));
        assert_eq!((d.x, d.y, d.maximized), (None, None, false));
        assert_eq!(d.active_view().last_chat, None);
        assert!(d.active_view().collapsed_sections.is_empty());
        assert!(d.accounts.is_empty());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            WindowState::load_from(dir.path().join(FILE_NAME), KEY),
            WindowState::default()
        );
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        let s = sample();
        s.save_to(&p).unwrap();
        assert_eq!(WindowState::load_from(&p, KEY), s);
    }

    #[test]
    fn partial_and_old_files_load_with_defaults_for_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(&p, "{}").unwrap();
        assert_eq!(WindowState::load_from(&p, KEY), WindowState::default());

        // An older build knew only the size; unknown fields are ignored.
        std::fs::write(&p, r#"{"width":1500,"height":900,"future_field":true}"#).unwrap();
        let s = WindowState::load_from(&p, KEY);
        assert_eq!((s.width, s.height), (1500, 900));
        assert_eq!(
            (s.x, s.maximized, s.active_view().last_chat.clone()),
            (None, false, None)
        );
        assert!(s.accounts.is_empty());
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        for junk in ["", "not json", "[1,2", r#"{"width":"wide"}"#, "\u{0}\u{1}"] {
            std::fs::write(&p, junk).unwrap();
            assert_eq!(
                WindowState::load_from(&p, KEY),
                WindowState::default(),
                "{junk:?}"
            );
        }
        std::fs::write(&p, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(WindowState::load_from(&p, KEY), WindowState::default());
    }

    #[test]
    fn unreasonable_values_are_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(&p, r#"{"width":-5,"height":0,"x":2000000000,"y":10}"#).unwrap();
        let s = WindowState::load_from(&p, KEY);
        assert_eq!((s.width, s.height), (MIN_WIDTH, MIN_HEIGHT));
        assert_eq!(
            (s.x, s.y),
            (None, None),
            "an absurd axis drops the position"
        );

        std::fs::write(&p, r#"{"width":99999999,"height":99999,"x":-100,"y":-50}"#).unwrap();
        let s = WindowState::load_from(&p, KEY);
        assert_eq!((s.width, s.height), (MAX_SIZE, MAX_SIZE));
        assert_eq!(
            (s.x, s.y),
            (Some(-100), Some(-50)),
            "a negative position is fine"
        );

        let long = "x".repeat(MAX_ID_LEN + 1);
        std::fs::write(
            &p,
            format!(r#"{{"last_chat":"{long}","collapsed_sections":["", "ok", "{long}"]}}"#),
        )
        .unwrap();
        let s = WindowState::load_from(&p, KEY);
        assert_eq!(s.active_view().last_chat, None);
        assert_eq!(
            s.active_view()
                .collapsed_sections
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["ok".to_string()]
        );

        std::fs::write(&p, r#"{"last_chat":"  "}"#).unwrap();
        assert_eq!(
            WindowState::load_from(&p, KEY).active_view().last_chat,
            None
        );
    }

    #[test]
    fn setters_clamp_and_sections_toggle() {
        let mut s = WindowState::default();
        s.set_size(-1, 1_000_000);
        assert_eq!((s.width, s.height), (MIN_WIDTH, MAX_SIZE));
        s.set_position(i32::MAX, 0);
        assert_eq!((s.x, s.y), (None, None));
        s.set_position(10, 20);
        assert_eq!((s.x, s.y), (Some(10), Some(20)));

        assert!(s.toggle_section("chats"));
        assert!(s.is_collapsed("chats"));
        assert!(!s.toggle_section("chats"));
        assert!(!s.is_collapsed("chats"));
        s.set_section_collapsed("", true);
        assert!(
            s.active_view().collapsed_sections.is_empty(),
            "an empty id is not kept"
        );
        assert!(s.accounts.is_empty(), "an empty view is not kept");

        s.set_last_chat(Some("19:x@thread.v2"));
        assert_eq!(s.active_view().last_chat.as_deref(), Some("19:x@thread.v2"));
        s.set_last_chat(None);
        assert_eq!(s.active_view().last_chat, None);

        s.set_last_channel(Some("19:c@thread.tacv2"));
        assert_eq!(
            s.active_view().last_channel.as_deref(),
            Some("19:c@thread.tacv2")
        );
        s.set_last_channel(Some("  "));
        assert_eq!(s.active_view().last_channel, None, "a blank id is not kept");
    }

    // ---- per-account views ----

    const LEGACY: &str = r#"{
        "width": 1500, "height": 900, "x": 10, "y": 20, "maximized": true,
        "last_chat": "19:legacy@thread.v2",
        "last_channel": "19:chan@thread.tacv2",
        "collapsed_sections": ["favorites", "team:19:t"],
        "sidebar_width": 400,
        "filmstrip_pos": "bottom"
    }"#;

    #[test]
    fn a_legacy_file_moves_into_the_loading_accounts_entry() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(&p, LEGACY).unwrap();
        let s = WindowState::load_from(&p, "work@guest-1");

        let v = s.view("work@guest-1");
        assert_eq!(v.last_chat.as_deref(), Some("19:legacy@thread.v2"));
        assert_eq!(v.last_channel.as_deref(), Some("19:chan@thread.tacv2"));
        assert_eq!(
            v.collapsed_sections.iter().cloned().collect::<Vec<_>>(),
            vec!["favorites".to_string(), "team:19:t".to_string()]
        );
        assert_eq!(s.accounts.len(), 1, "only the loading account got them");
        assert_eq!(s.view("other"), &AccountView::default());
        // The shared fields stay where they were, the filmstrip position included.
        assert_eq!(
            (s.width, s.height, s.x, s.y),
            (1500, 900, Some(10), Some(20))
        );
        assert!(s.maximized);
        assert_eq!(s.sidebar_width, Some(400));
        assert_eq!(s.filmstrip_pos.as_deref(), Some("bottom"));
    }

    #[test]
    fn saving_never_writes_the_legacy_fields_again() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(&p, LEGACY).unwrap();
        WindowState::load_from(&p, KEY).save_to(&p).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let top = v.as_object().unwrap();
        for legacy in ["last_chat", "last_channel", "collapsed_sections"] {
            assert!(!top.contains_key(legacy), "{legacy} was written back");
        }
        assert_eq!(v["accounts"][KEY]["last_chat"], "19:legacy@thread.v2");
        assert_eq!(v["filmstrip_pos"], "bottom");
        // Loading as another account afterwards finds nothing to migrate for it.
        let other = WindowState::load_from(&p, "other");
        assert_eq!(other.active_view(), &AccountView::default());
        assert_eq!(
            WindowState::load_from(&p, KEY)
                .active_view()
                .last_chat
                .as_deref(),
            Some("19:legacy@thread.v2")
        );
    }

    #[test]
    fn two_accounts_keep_separate_views_across_saves() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);

        let mut a = WindowState::load_from(&p, "a");
        a.set_last_chat(Some("19:chat-a@thread.v2"));
        a.toggle_section("favorites");
        a.set_size(1280, 800);
        a.save_to(&p).unwrap();

        // The same window, now running account b: geometry is shared, the view is not.
        let mut b = WindowState::load_from(&p, "b");
        assert_eq!((b.width, b.height), (1280, 800));
        assert_eq!(b.active_view().last_chat, None);
        assert!(!b.is_collapsed("favorites"));
        b.set_last_chat(Some("19:chat-b@thread.v2"));
        b.set_last_channel(Some("19:chan-b@thread.tacv2"));
        b.save_to(&p).unwrap();

        let a = WindowState::load_from(&p, "a");
        assert_eq!(
            a.active_view().last_chat.as_deref(),
            Some("19:chat-a@thread.v2")
        );
        assert_eq!(a.active_view().last_channel, None);
        assert!(a.is_collapsed("favorites"));
        let b = WindowState::load_from(&p, "b");
        assert_eq!(
            b.active_view().last_chat.as_deref(),
            Some("19:chat-b@thread.v2")
        );
        assert_eq!(
            b.active_view().last_channel.as_deref(),
            Some("19:chan-b@thread.tacv2")
        );
        assert!(!b.is_collapsed("favorites"));
    }

    #[test]
    fn a_guest_organization_has_its_own_entry() {
        let mut s = WindowState::default();
        s.set_active_key("work");
        s.set_last_chat(Some("19:home@thread.v2"));
        s.set_active_key("work@guest-1");
        assert_eq!(s.active_view().last_chat, None);
        s.set_last_chat(Some("19:guest@thread.v2"));
        assert_eq!(
            s.view("work").last_chat.as_deref(),
            Some("19:home@thread.v2")
        );
        assert_eq!(
            s.view("work@guest-1").last_chat.as_deref(),
            Some("19:guest@thread.v2")
        );
        let keys: Vec<_> = s.accounts.keys().cloned().collect();
        assert_eq!(keys, ["work", "work@guest-1"]);

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        s.save_to(&p).unwrap();
        let back = WindowState::load_from(&p, "work@guest-1");
        assert_eq!(back, s);
        assert_eq!(
            back.active_view().last_chat.as_deref(),
            Some("19:guest@thread.v2")
        );
    }

    #[test]
    fn a_legacy_field_does_not_replace_what_the_entry_already_holds() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(
            &p,
            r#"{"last_chat":"19:old@thread.v2","last_channel":"19:oldc@thread.tacv2",
                "accounts":{"work":{"last_chat":"19:new@thread.v2"}}}"#,
        )
        .unwrap();
        let v = WindowState::load_from(&p, "work").active_view().clone();
        assert_eq!(v.last_chat.as_deref(), Some("19:new@thread.v2"));
        assert_eq!(
            v.last_channel.as_deref(),
            Some("19:oldc@thread.tacv2"),
            "a field the entry lacks is filled from the legacy one"
        );
    }

    #[test]
    fn an_older_build_reading_the_new_file_ignores_the_views() {
        // What a build from before the per-account views reads: the geometry, and nothing it
        // would take for a last chat.
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Old {
            width: i32,
            last_chat: Option<String>,
            collapsed_sections: BTreeSet<String>,
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        sample().save_to(&p).unwrap();
        let old: Old = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(old.width, 1280);
        assert_eq!(old.last_chat, None);
        assert!(old.collapsed_sections.is_empty());
    }

    #[test]
    fn views_are_clamped_and_empty_ones_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        let long = "x".repeat(MAX_ID_LEN + 1);
        let long_key = "k".repeat(MAX_KEY_LEN + 1);
        std::fs::write(
            &p,
            format!(
                r#"{{"accounts":{{
                    "a":{{"last_chat":"{long}","collapsed_sections":["ok",""]}},
                    "b":{{"last_chat":"  "}},
                    "{long_key}":{{"last_chat":"19:c@thread.v2"}}
                }}}}"#
            ),
        )
        .unwrap();
        let s = WindowState::load_from(&p, "a");
        assert_eq!(s.accounts.keys().collect::<Vec<_>>(), ["a"]);
        assert_eq!(s.view("a").last_chat, None);
        assert_eq!(s.view("a").collapsed_sections.len(), 1);
        assert_eq!(s.view("missing"), &AccountView::default());
    }

    #[test]
    fn the_active_key_is_not_part_of_equality_or_the_file() {
        let mut a = WindowState::default();
        let mut b = WindowState::default();
        a.set_active_key("a");
        b.set_active_key("b");
        assert_eq!(a, b);
        assert_eq!(a.active_key(), "a");
        let v = serde_json::to_value(&a).unwrap();
        assert!(v.get("active").is_none(), "{v}");
    }

    #[test]
    fn save_is_atomic_creates_dirs_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/config").join(FILE_NAME);
        let mut s = sample();
        s.save_to(&p).unwrap();
        // Overwrite: the old content is replaced whole.
        s.set_last_chat(Some("19:other@thread.v2"));
        s.save_to(&p).unwrap();
        assert_eq!(WindowState::load_from(&p, KEY), s);
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![FILE_NAME.to_string()], "no .tmp left behind");
        // The file is plain JSON a user can read.
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["width"], 1280);
    }

    #[test]
    fn failed_save_keeps_the_old_file_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        sample().save_to(&p).unwrap();
        // A directory in the way of the rename target makes the final step fail.
        let blocked = dir.path().join("blocked.json");
        std::fs::create_dir(&blocked).unwrap();
        assert!(WindowState::default().save_to(&blocked).is_err());
        assert!(!dir.path().join("blocked.json.tmp").exists());
        assert_eq!(WindowState::load_from(&p, KEY), sample());
    }

    #[test]
    fn saving_clamps() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        let s = WindowState {
            width: -3,
            ..WindowState::default()
        };
        s.save_to(&p).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["width"], MIN_WIDTH);
    }

    #[test]
    fn debug_does_not_show_the_conversation_id() {
        let s = sample();
        let d = format!("{s:?}");
        assert!(!d.contains("19:abc"), "{d}");
        assert!(!d.contains("favorites"), "{d}");
    }

    #[test]
    fn path_is_next_to_config_json_and_follows_paths() {
        let p = Paths {
            config_dir: PathBuf::from("/h/config"),
            data_dir: PathBuf::from("/h/data"),
            cache_dir: PathBuf::from("/h/cache"),
        };
        assert_eq!(
            p.window_state_path(),
            PathBuf::from("/h/config/window-state.json")
        );
    }
}
