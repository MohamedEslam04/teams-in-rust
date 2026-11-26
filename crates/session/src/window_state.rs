//! Window state: size, position, the last open chat and the collapsed sidebar sections.
//!
//! Stored as `window-state.json` in the config dir (next to `config.json`, so `IRONTEAMS_HOME`
//! applies), separate from [`crate::Config`] so a GUI crash or a rewrite never touches account
//! data. Nothing here is secret, but the conversation id is still kept out of `Debug` output and
//! this module never logs.
//!
//! Loading never fails: a missing, unreadable or corrupt file gives the defaults, and values a
//! hand edit or an older build left unreasonable are clamped.

use crate::{Paths, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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

/// What is restored on start. Every field has a default so old and partial files load.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    /// Id of the conversation that was open.
    pub last_chat: Option<String>,
    /// Id of the last channel that was open: the Teams page opens it again.
    pub last_channel: Option<String>,
    /// Ids of the collapsed sidebar sections.
    pub collapsed_sections: BTreeSet<String>,
    /// Width of the chat and teams list the user dragged it to; `None` keeps the default.
    pub sidebar_width: Option<u32>,
    /// Where the call filmstrip sits beside a shared screen: "right" | "bottom" | "hidden" |
    /// "auto"; `None` (or an unknown value, which `clamp` drops) keeps the "auto" default.
    pub filmstrip_pos: Option<String>,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            x: None,
            y: None,
            maximized: false,
            last_chat: None,
            last_channel: None,
            collapsed_sections: BTreeSet::new(),
            sidebar_width: None,
            filmstrip_pos: None,
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
            .field("last_chat", &self.last_chat.as_ref().map(|_| "<set>"))
            .field("last_channel", &self.last_channel.as_ref().map(|_| "<set>"))
            .field("collapsed_sections", &self.collapsed_sections.len())
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
        // An unknown value from a newer build falls back to the "auto" default.
        if !self
            .filmstrip_pos
            .as_deref()
            .is_some_and(|p| matches!(p, "right" | "bottom" | "hidden" | "auto"))
        {
            self.filmstrip_pos = None;
        }
    }

    /// Loads from the standard config dir; defaults when there is none or it is unreadable.
    pub fn load() -> WindowState {
        match Paths::new() {
            Ok(p) => WindowState::load_from(p.window_state_path()),
            Err(_) => WindowState::default(),
        }
    }

    /// Loads from `path`. A missing or corrupt file gives the defaults; values are clamped.
    pub fn load_from(path: impl AsRef<Path>) -> WindowState {
        let mut s = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<WindowState>(&raw).ok())
            .unwrap_or_default();
        s.clamp();
        s
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WindowState {
        let mut s = WindowState::default();
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
        assert_eq!(d.last_chat, None);
        assert!(d.collapsed_sections.is_empty());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            WindowState::load_from(dir.path().join(FILE_NAME)),
            WindowState::default()
        );
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        let s = sample();
        s.save_to(&p).unwrap();
        assert_eq!(WindowState::load_from(&p), s);
    }

    #[test]
    fn partial_and_old_files_load_with_defaults_for_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(&p, "{}").unwrap();
        assert_eq!(WindowState::load_from(&p), WindowState::default());

        // An older build knew only the size; unknown fields are ignored.
        std::fs::write(&p, r#"{"width":1500,"height":900,"future_field":true}"#).unwrap();
        let s = WindowState::load_from(&p);
        assert_eq!((s.width, s.height), (1500, 900));
        assert_eq!((s.x, s.maximized, s.last_chat), (None, false, None));
        assert!(s.collapsed_sections.is_empty());
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        for junk in ["", "not json", "[1,2", r#"{"width":"wide"}"#, "\u{0}\u{1}"] {
            std::fs::write(&p, junk).unwrap();
            assert_eq!(
                WindowState::load_from(&p),
                WindowState::default(),
                "{junk:?}"
            );
        }
        std::fs::write(&p, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(WindowState::load_from(&p), WindowState::default());
    }

    #[test]
    fn unreasonable_values_are_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(&p, r#"{"width":-5,"height":0,"x":2000000000,"y":10}"#).unwrap();
        let s = WindowState::load_from(&p);
        assert_eq!((s.width, s.height), (MIN_WIDTH, MIN_HEIGHT));
        assert_eq!(
            (s.x, s.y),
            (None, None),
            "an absurd axis drops the position"
        );

        std::fs::write(&p, r#"{"width":99999999,"height":99999,"x":-100,"y":-50}"#).unwrap();
        let s = WindowState::load_from(&p);
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
        let s = WindowState::load_from(&p);
        assert_eq!(s.last_chat, None);
        assert_eq!(
            s.collapsed_sections.into_iter().collect::<Vec<_>>(),
            vec!["ok".to_string()]
        );

        std::fs::write(&p, r#"{"last_chat":"  "}"#).unwrap();
        assert_eq!(WindowState::load_from(&p).last_chat, None);
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
        assert!(s.collapsed_sections.is_empty(), "an empty id is not kept");

        s.set_last_chat(Some("19:x@thread.v2"));
        assert_eq!(s.last_chat.as_deref(), Some("19:x@thread.v2"));
        s.set_last_chat(None);
        assert_eq!(s.last_chat, None);

        s.set_last_channel(Some("19:c@thread.tacv2"));
        assert_eq!(s.last_channel.as_deref(), Some("19:c@thread.tacv2"));
        s.set_last_channel(Some("  "));
        assert_eq!(s.last_channel, None, "a blank id is not kept");
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
        assert_eq!(WindowState::load_from(&p), s);
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
        assert_eq!(WindowState::load_from(&p), sample());
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
