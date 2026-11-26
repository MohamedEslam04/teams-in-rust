//! The user's appearance choices (theme, accent, density, font scale, message style) and their file.

use crate::store::{self, Error, lenient};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

pub const FONT_SCALE_MIN: f32 = 0.75;
pub const FONT_SCALE_MAX: f32 = 2.0;
pub const FILE_NAME: &str = "appearance.json";

/// Which theme the user asked for. `System` follows the desktop's light or dark setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

/// Message density in the chat list and message pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfy,
    Compact,
}

/// How a message is drawn in a chat: in a bubble (the default), as a flat line with the photo and
/// name on the left for everyone, or as a flat line with less room around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageStyle {
    #[default]
    Bubbles,
    Flat,
    Compact,
}

impl MessageStyle {
    /// The value that is saved and shown in Settings.
    pub fn slug(self) -> &'static str {
        match self {
            MessageStyle::Bubbles => "bubbles",
            MessageStyle::Flat => "flat",
            MessageStyle::Compact => "compact",
        }
    }

    pub fn from_slug(s: &str) -> Option<MessageStyle> {
        [
            MessageStyle::Bubbles,
            MessageStyle::Flat,
            MessageStyle::Compact,
        ]
        .into_iter()
        .find(|m| m.slug() == s)
    }
}

/// An sRGB color, stored in the file as `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Parses `#rrggbb` or `rrggbb` (any case).
    pub fn from_hex(s: &str) -> Option<Rgb> {
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Rgb::new(byte(0)?, byte(2)?, byte(4)?))
    }

    /// Lower-case `#rrggbb`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// From channels in 0.0..=1.0 (the portal's accent format). Anything outside the range, or
    /// not a number, gives `None`: the portal uses out-of-range values to mean "no accent".
    pub fn from_unit(r: f64, g: f64, b: f64) -> Option<Rgb> {
        let ok = |c: f64| (0.0..=1.0).contains(&c);
        if !(ok(r) && ok(g) && ok(b)) {
            return None;
        }
        let q = |c: f64| (c * 255.0).round() as u8;
        Some(Rgb::new(q(r), q(g), q(b)))
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Rgb, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::from_hex(&s).ok_or_else(|| serde::de::Error::custom("expected a #rrggbb color"))
    }
}

/// Clamps a font scale into the supported range. Not-a-number and infinity give 1.0.
pub fn clamp_font_scale(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(FONT_SCALE_MIN, FONT_SCALE_MAX)
    } else {
        1.0
    }
}

fn default_font_scale() -> f32 {
    1.0
}

fn de_font_scale<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v: f32 = lenient::<D, Option<f32>>(d)?.unwrap_or(1.0);
    Ok(clamp_font_scale(v))
}

/// Appearance preferences. Every field has a default, so an old or partial file still loads.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearancePrefs {
    #[serde(deserialize_with = "lenient")]
    pub theme: Theme,
    /// `None` follows the portal accent.
    #[serde(deserialize_with = "lenient")]
    pub accent: Option<Rgb>,
    #[serde(deserialize_with = "lenient")]
    pub density: Density,
    /// Multiplier for text size, 0.75..=2.0. Use [`AppearancePrefs::set_font_scale`] to change it.
    #[serde(deserialize_with = "de_font_scale")]
    pub font_scale: f32,
    /// Bubbles, flat lines or compact lines in a chat.
    #[serde(deserialize_with = "lenient")]
    pub message_style: MessageStyle,
}

impl Default for AppearancePrefs {
    fn default() -> Self {
        AppearancePrefs {
            theme: Theme::System,
            accent: None,
            density: Density::Comfy,
            font_scale: default_font_scale(),
            message_style: MessageStyle::Bubbles,
        }
    }
}

impl AppearancePrefs {
    /// Sets the font scale, clamped to the supported range.
    pub fn set_font_scale(&mut self, v: f32) {
        self.font_scale = clamp_font_scale(v);
    }

    /// Where the file lives (`<config dir>/appearance.json`).
    pub fn path() -> Result<PathBuf, Error> {
        Ok(store::config_dir()?.join(FILE_NAME))
    }

    /// Loads the preferences; a missing file, an unreadable one or no home gives the defaults.
    pub fn load() -> AppearancePrefs {
        Self::path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> AppearancePrefs {
        store::load_json(path)
    }

    /// Saves atomically (temp file, then rename).
    pub fn save(&self) -> Result<(), Error> {
        self.save_to(&Self::path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), Error> {
        let mut clean = *self;
        clean.font_scale = clamp_font_scale(clean.font_scale);
        store::save_json(path, &clean)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let d = AppearancePrefs::default();
        assert_eq!(d.theme, Theme::System);
        assert_eq!(d.accent, None);
        assert_eq!(d.density, Density::Comfy);
        assert_eq!(d.font_scale, 1.0);
    }

    #[test]
    fn empty_and_partial_files_fill_in_defaults() {
        let d: AppearancePrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(d, AppearancePrefs::default());
        // An older file with only the theme.
        let p: AppearancePrefs = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(p.theme, Theme::Dark);
        assert_eq!(p.density, Density::Comfy);
        assert_eq!(p.font_scale, 1.0);
        // Unknown fields from a newer version are ignored.
        let p: AppearancePrefs =
            serde_json::from_str(r#"{"density":"compact","future":1}"#).unwrap();
        assert_eq!(p.density, Density::Compact);
    }

    #[test]
    fn bad_field_values_fall_back_without_losing_the_rest() {
        let p: AppearancePrefs = serde_json::from_str(
            r##"{"theme":"sepia","accent":"not a color","density":"compact","font_scale":"big"}"##,
        )
        .unwrap();
        assert_eq!(p.theme, Theme::System);
        assert_eq!(p.accent, None);
        assert_eq!(p.density, Density::Compact);
        assert_eq!(p.font_scale, 1.0);
    }

    #[test]
    fn font_scale_clamps_on_load_and_set() {
        let hi: AppearancePrefs = serde_json::from_str(r#"{"font_scale":5.0}"#).unwrap();
        assert_eq!(hi.font_scale, 2.0);
        let lo: AppearancePrefs = serde_json::from_str(r#"{"font_scale":0.1}"#).unwrap();
        assert_eq!(lo.font_scale, 0.75);
        let mut p = AppearancePrefs::default();
        p.set_font_scale(f32::NAN);
        assert_eq!(p.font_scale, 1.0);
        p.set_font_scale(1.5);
        assert_eq!(p.font_scale, 1.5);
        p.set_font_scale(-3.0);
        assert_eq!(p.font_scale, FONT_SCALE_MIN);
    }

    #[test]
    fn color_hex_round_trip() {
        let c = Rgb::new(0x12, 0xab, 0xff);
        assert_eq!(c.to_hex(), "#12abff");
        assert_eq!(Rgb::from_hex("#12ABFF"), Some(c));
        assert_eq!(Rgb::from_hex("12abff"), Some(c));
        assert_eq!(Rgb::from_hex("#12abf"), None);
        assert_eq!(Rgb::from_hex("#gggggg"), None);
        assert_eq!(Rgb::from_hex("#é2abf"), None);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE_NAME);
        let mut p = AppearancePrefs {
            theme: Theme::Dark,
            density: Density::Compact,
            ..Default::default()
        };
        p.accent = Some(Rgb::new(10, 20, 30));
        p.font_scale = 1.5;
        p.save_to(&path).unwrap();
        assert_eq!(AppearancePrefs::load_from(&path), p);
        // The temp file is gone after the rename.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(FILE_NAME)]);
    }

    #[test]
    fn save_overwrites_and_clamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        AppearancePrefs::default().save_to(&path).unwrap();
        let p = AppearancePrefs {
            font_scale: 8.0,
            ..Default::default()
        };
        p.save_to(&path).unwrap();
        assert_eq!(AppearancePrefs::load_from(&path).font_scale, 2.0);
    }

    #[test]
    fn missing_or_corrupt_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        assert_eq!(
            AppearancePrefs::load_from(&path),
            AppearancePrefs::default()
        );
        std::fs::write(&path, b"{ this is not json").unwrap();
        assert_eq!(
            AppearancePrefs::load_from(&path),
            AppearancePrefs::default()
        );
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(
            AppearancePrefs::load_from(&path),
            AppearancePrefs::default()
        );
        // Valid JSON of the wrong shape.
        std::fs::write(&path, b"[1,2,3]").unwrap();
        assert_eq!(
            AppearancePrefs::load_from(&path),
            AppearancePrefs::default()
        );
    }

    #[test]
    fn messages_default_to_bubbles_and_old_files_keep_that() {
        assert_eq!(
            AppearancePrefs::default().message_style,
            MessageStyle::Bubbles
        );
        let old: AppearancePrefs = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(old.message_style, MessageStyle::Bubbles);
    }

    #[test]
    fn message_style_loads_saves_and_survives_a_bad_value() {
        let p: AppearancePrefs = serde_json::from_str(r#"{"message_style":"compact"}"#).unwrap();
        assert_eq!(p.message_style, MessageStyle::Compact);
        let bad: AppearancePrefs =
            serde_json::from_str(r#"{"message_style":"irc","density":"compact"}"#).unwrap();
        assert_eq!(bad.message_style, MessageStyle::Bubbles);
        assert_eq!(bad.density, Density::Compact);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let saved = AppearancePrefs {
            message_style: MessageStyle::Flat,
            ..Default::default()
        };
        saved.save_to(&path).unwrap();
        assert_eq!(AppearancePrefs::load_from(&path), saved);
    }

    #[test]
    fn message_style_slugs_round_trip() {
        for s in [
            MessageStyle::Bubbles,
            MessageStyle::Flat,
            MessageStyle::Compact,
        ] {
            assert_eq!(MessageStyle::from_slug(s.slug()), Some(s));
        }
        assert_eq!(MessageStyle::from_slug("irc"), None);
    }

    #[test]
    fn file_format_is_stable() {
        let p = AppearancePrefs {
            theme: Theme::Light,
            accent: Some(Rgb::new(0, 120, 212)),
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::to_value(p).unwrap();
        assert_eq!(v["theme"], "light");
        assert_eq!(v["accent"], "#0078d4");
        assert_eq!(v["density"], "comfy");
        assert_eq!(v["font_scale"], 1.0);
        assert_eq!(v["message_style"], "bubbles");
    }
}
