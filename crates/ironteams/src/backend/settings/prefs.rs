use super::*;

/// Where Office files open (X1.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenFilesIn {
    Browser,
    Desktop,
    /// IronTeams's own preview: how Office files opened before there was a choice. Saved as
    /// `kelma` before the rename, still read.
    #[default]
    #[serde(alias = "kelma")]
    IronTeams,
}

impl OpenFilesIn {
    pub fn slug(self) -> &'static str {
        match self {
            OpenFilesIn::Browser => "browser",
            OpenFilesIn::Desktop => "desktop",
            OpenFilesIn::IronTeams => "ironteams",
        }
    }

    pub fn from_slug(s: &str) -> Option<OpenFilesIn> {
        [
            OpenFilesIn::Browser,
            OpenFilesIn::Desktop,
            OpenFilesIn::IronTeams,
        ]
        .into_iter()
        .find(|o| o.slug() == s)
    }
}

/// Files and links preferences, in `files.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesPrefs {
    pub open_office_in: OpenFilesIn,
}

/// The saved "open Office files in" choice, read from `files.json` in the config dir. The file is
/// tiny, so the open path reads it each time instead of keeping a copy that could go stale.
pub fn files_prefs() -> FilesPrefs {
    session::Paths::new()
        .map(|p| read_json(&p.config_dir.join(FILES_FILE)))
        .unwrap_or_default()
}

/// Calls preferences, in `calls.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CallsPrefs {
    /// "Ask about call quality after calls".
    pub ask_feedback: call_feedback::Ask,
    /// The sound of an incoming call. A file saved without one gets the Default.
    pub ringtone: super::super::ringtone::RingtoneChoice,
    /// The sound file of the "Custom" ringtone.
    pub ringtone_file: Option<String>,
}

/// The saved calls preferences, read from `calls.json` (the ringing path reads it each time, so
/// a change in Settings applies to the next call).
pub fn calls_prefs() -> CallsPrefs {
    session::Paths::new()
        .map(|p| read_json(&p.config_dir.join(CALLS_FILE)))
        .unwrap_or_default()
}

pub(super) static LINK_PREVIEWS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Whether messages show a card for their links (Settings › Chats › Link previews).
pub fn link_previews() -> bool {
    LINK_PREVIEWS.load(Ordering::Relaxed)
}

/// Applies the saved choice to the message renderer.
pub fn set_link_previews(on: bool) {
    LINK_PREVIEWS.store(on, Ordering::Relaxed);
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Writes JSON atomically (temp file in the same directory, then rename).
pub(crate) fn write_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("prefs");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut body = serde_json::to_vec_pretty(value)?;
    body.push(b'\n');
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Every local preference, loaded from and saved to one config directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub dir: PathBuf,
    pub appearance: AppearancePrefs,
    pub general: GeneralPrefs,
    pub a11y: AccessibilityPrefs,
    pub devices: DevicePrefs,
    pub files: FilesPrefs,
    pub calls: CallsPrefs,
    pub sounds: super::super::sounds::SoundPrefs,
    pub grants: Grants,
    /// `themes.json`: the theme choice, the account and call themes, the fonts and the custom
    /// themes.
    pub themes: appearance::ThemesFile,
}

/// Reads `themes.json` from `dir`, logging once what the loader had to drop.
fn load_themes(dir: &Path) -> appearance::ThemesFile {
    let (themes, note) =
        appearance::ThemesFile::load_from(&dir.join(appearance::themes::FILE_NAME));
    if note.skipped > 0 || note.renamed_bad {
        tracing::warn!(
            skipped = note.skipped,
            renamed_bad = note.renamed_bad,
            "settings: themes.json had entries that could not be read"
        );
    }
    themes
}

/// "Reset theme" on the files in `dir`, before any window opens (`--reset-theme`): the default
/// choice, no theme for `account`, the call theme as the app's and no fonts, in `themes.json`;
/// `appearance.json`'s old theme field follows the default choice. Custom themes stay.
pub fn reset_theme_files(dir: &Path, account: Option<&str>) -> anyhow::Result<()> {
    let mut local = Local::load(dir);
    local.themes.reset(account);
    local.save_themes()
}

impl Local {
    /// Loads everything from `dir` (the app config dir). Missing or broken files give defaults.
    pub fn load(dir: &Path) -> Local {
        let grants_path = dir.join(PERMISSIONS_FILE);
        let perms: serde_json::Value = if grants_path.exists() {
            read_json(&grants_path)
        } else {
            read_json(&dir.join(LEGACY_PERMISSIONS_FILE))
        };
        Local {
            dir: dir.to_path_buf(),
            appearance: AppearancePrefs::load_from(&dir.join(appearance::prefs::FILE_NAME)),
            general: GeneralPrefs::load_from(&dir.join(appearance::general::FILE_NAME)),
            a11y: AccessibilityPrefs::load_from(&dir.join(appearance::accessibility::FILE_NAME)),
            devices: read_json(&dir.join(DEVICES_FILE)),
            files: read_json(&dir.join(FILES_FILE)),
            calls: read_json(&dir.join(CALLS_FILE)),
            sounds: read_json(&dir.join(super::super::sounds::SOUNDS_FILE)),
            grants: Grants::from_json(&perms),
            themes: load_themes(dir),
        }
    }

    /// The app's theme choice: `themes.json`'s, or the old `appearance.json` theme before one was
    /// picked.
    pub fn theme_choice(&self) -> appearance::ThemeChoice {
        self.themes
            .choice
            .clone()
            .unwrap_or_else(|| appearance::themes::choice_from_legacy(self.appearance.theme))
    }

    /// Saves `themes.json`, then `appearance.json` with its old theme field written from the kind
    /// of the app choice, so a build without themes still opens light or dark as it should.
    pub fn save_themes(&mut self) -> anyhow::Result<()> {
        self.themes
            .save_to(&self.dir.join(appearance::themes::FILE_NAME))?;
        self.appearance.theme =
            appearance::themes::legacy_theme(&self.theme_choice(), &self.themes);
        self.save_appearance()
    }

    pub fn save_appearance(&self) -> anyhow::Result<()> {
        Ok(self
            .appearance
            .save_to(&self.dir.join(appearance::prefs::FILE_NAME))?)
    }

    pub fn save_general(&self) -> anyhow::Result<()> {
        // Focus mode can be switched from the tray; this copy of the preferences may be older.
        let mut general = self.general.clone();
        if let Some(until) = crate::support::quiet::focus_override() {
            general.focus_until_ms = until;
        }
        Ok(general.save_to(&self.dir.join(appearance::general::FILE_NAME))?)
    }

    pub fn save_a11y(&self) -> anyhow::Result<()> {
        Ok(self
            .a11y
            .save_to(&self.dir.join(appearance::accessibility::FILE_NAME))?)
    }

    pub fn save_devices(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(DEVICES_FILE),
            &serde_json::to_value(&self.devices)?,
        )?)
    }

    pub fn save_files(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(FILES_FILE),
            &serde_json::to_value(self.files)?,
        )?)
    }

    pub fn save_calls(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(CALLS_FILE),
            &serde_json::to_value(&self.calls)?,
        )?)
    }

    pub fn save_sounds(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(super::super::sounds::SOUNDS_FILE),
            &serde_json::to_value(&self.sounds)?,
        )?)
    }

    pub fn save_grants(&self) -> anyhow::Result<()> {
        Ok(write_json(
            &self.dir.join(PERMISSIONS_FILE),
            &self.grants.to_json(),
        )?)
    }

    /// Changes one grant and saves. Reads `app-grants.json` again first, so a switch the Store
    /// saved since Settings loaded the file is kept (both write the whole file).
    pub fn set_grant(
        &mut self,
        app_id: &str,
        p: DevicePermission,
        allowed: bool,
    ) -> anyhow::Result<()> {
        let path = self.dir.join(PERMISSIONS_FILE);
        if path.exists() {
            let v: serde_json::Value = read_json(&path);
            if !v.is_null() {
                self.grants = Grants::from_json(&v);
            }
        }
        self.grants.set(app_id, p, allowed);
        self.save_grants()
    }
}
