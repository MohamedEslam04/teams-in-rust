//! Session persistence for ironteams.
//!
//! - The refresh token (the only long-lived secret) lives in the system keyring via the
//!   Secret Service API (KWallet or GNOME Keyring). It is never written to disk by ironteams.
//! - Non-secret settings (tenant, endpoint id) live in `config.json` in the config dir.
//! - Several accounts can be signed in. [`Config::accounts`] lists them and the legacy
//!   `account`/`tenant`/`epid`/`self_mri`/`display_name` fields always mirror the active one, so
//!   code (and older builds) that only knows one account keeps working. Each account has a
//!   keyring entry named by its label; a guest organization of the same account gets its own
//!   entry `label@tenant` once it issued a token.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

pub mod window_state;

const SERVICE: &str = "ironteams";
/// The keyring service before the rename from kelma. Entries found there are moved to
/// [`SERVICE`] when they are read.
const LEGACY_SERVICE: &str = "kelma";

/// One writer at a time for `config.json` inside this process: every read-modify-write of the
/// file ([`Paths::update_config`], [`Paths::save_app_prefs`]) holds it.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// Account labels whose tokens were removed (sign-out, remove account). While a label is in
/// here, [`persist_in`] does not write its token back: the engine saves the rotated token every
/// 240 s, and a save that was already on its way when the tokens were forgotten must not undo
/// the removal. The set is also the lock that orders a save against a removal.
static REVOKED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn revoked() -> std::sync::MutexGuard<'static, BTreeSet<String>> {
    REVOKED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the tokens of this account label were removed in this process.
pub fn tokens_revoked(label: &str) -> bool {
    revoked().contains(label)
}

/// Lets an account label store tokens again (a new account that reuses a removed label).
pub fn allow_account_tokens(label: &str) {
    revoked().remove(label);
}

/// Errors from session persistence.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no home directory")]
    NoHome,
    #[error("account: {0}")]
    Account(String),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Where a chat opens ("When I open a chat, take me to" in Teams settings).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenChatAt {
    /// The first unread message (at the "New messages" divider); the newest when all is read.
    #[default]
    FirstUnread,
    /// Always the newest message.
    Newest,
}

/// Local calendar display settings (Calendar settings in the toolbar menu). Every field has a
/// default so config files written before it still load.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CalendarSettings {
    /// First day of the week: 0 for Sunday up to 6 for Saturday (Monday by default).
    pub first_day: u8,
    /// Work hours as minutes after midnight; `None` takes the mailbox's, then 09:00 to 17:00.
    pub work_start_min: Option<u32>,
    pub work_end_min: Option<u32>,
    /// Week and month views include Saturday and Sunday.
    pub show_weekends: bool,
    /// Show week numbers in the toolbar, the mini calendar and the month view.
    pub week_numbers: bool,
    /// A second time column in the day and week views, as a fixed UTC offset in minutes. Older
    /// configs only have this; it is shown until a zone is picked in `second_tz`.
    pub second_tz_min: Option<i32>,
    /// The second time column's zone, a Windows or IANA name, so daylight saving moves it.
    /// Wins over `second_tz_min`.
    pub second_tz: Option<String>,
}

impl Default for CalendarSettings {
    fn default() -> Self {
        CalendarSettings {
            first_day: 1,
            work_start_min: None,
            work_end_min: None,
            show_weekends: true,
            week_numbers: false,
            second_tz_min: None,
            second_tz: None,
        }
    }
}

/// A named calendar view: the view key (`workweek`, `week`, `month`, `day:N`) and time scale.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedView {
    pub name: String,
    pub view: String,
    pub scale: u32,
}

/// One organization (tenant) an account can work in: its home tenant or a guest tenant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrgRef {
    pub tenant_id: String,
    pub name: String,
    #[serde(default)]
    pub is_home: bool,
}

impl From<&teams_proto::tenants::Tenant> for OrgRef {
    fn from(t: &teams_proto::tenants::Tenant) -> OrgRef {
        OrgRef {
            tenant_id: t.id.clone(),
            name: t.name.clone(),
            is_home: t.is_home,
        }
    }
}

/// A signed-in (or signed-out but remembered) account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    /// Keyring label and the name used on the command line.
    pub label: String,
    /// Sign-in name once known.
    #[serde(default)]
    pub upn: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub self_mri: Option<String>,
    /// The account's home tenant id once the org list was read.
    #[serde(default)]
    pub home_tenant: Option<String>,
    /// The tenant segment in use: `organizations`, the home tenant id or a guest tenant id.
    pub tenant: String,
    /// Endpoint id of this account (a UUID reused across runs).
    pub epid: String,
    /// Unix milliseconds of the last time this account was made active.
    #[serde(default)]
    pub last_used_ms: Option<i64>,
    /// The organizations last read from the Teams middle tier.
    #[serde(default)]
    pub orgs: Vec<OrgRef>,
    /// What kind of account this is. A file without the key is a work account. A kind this
    /// build does not know is kept as it is and the account is left alone (see
    /// [`AccountKind::Unknown`]).
    #[serde(default, skip_serializing_if = "AccountKind::is_work")]
    pub kind: AccountKind,
    /// The unread total (activity plus unread chats) when this account last quit; shown for an
    /// account that is not running. `None` until it ran once with a build that saves it.
    #[serde(default)]
    pub last_unread: Option<u32>,
    /// Fields a newer build wrote that this one does not know. They are written back unchanged.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// The kind of an [`Account`]. Only work accounts exist today; a later build adds more (for
/// example `"personal"`). A build that does not know a kind reads it as [`AccountKind::Unknown`]:
/// the account is skipped (not listed, never started, never removed or rewritten) and its kind is
/// written back as it was.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AccountKind {
    /// An Entra work or school account (also a guest in another organization).
    #[default]
    Work,
    /// A kind this build does not understand, with the value found in the file.
    Unknown(String),
}

impl AccountKind {
    /// True for [`AccountKind::Work`], the only kind this build can use.
    pub fn is_work(&self) -> bool {
        matches!(self, AccountKind::Work)
    }
}

impl Serialize for AccountKind {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            AccountKind::Work => ser.serialize_str("work"),
            AccountKind::Unknown(raw) => ser.serialize_str(raw),
        }
    }
}

impl<'de> Deserialize<'de> for AccountKind {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        // Any value reads: a wrong type from a newer build must not make the whole config fail.
        Ok(match serde_json::Value::deserialize(de)? {
            serde_json::Value::Null => AccountKind::Work,
            serde_json::Value::String(s) if s == "work" => AccountKind::Work,
            serde_json::Value::String(s) => AccountKind::Unknown(s),
            other => AccountKind::Unknown(other.to_string()),
        })
    }
}

impl Account {
    /// True when this build can sign in to the account (its kind is [`AccountKind::Work`]).
    pub fn is_usable(&self) -> bool {
        self.kind.is_work()
    }

    fn fresh(label: &str) -> Account {
        Account {
            label: label.into(),
            upn: None,
            display_name: None,
            self_mri: None,
            home_tenant: None,
            tenant: teams_proto::client_info::DEFAULT_TENANT.into(),
            epid: teams_proto::client::new_epid(),
            last_used_ms: None,
            orgs: Vec::new(),
            kind: AccountKind::Work,
            last_unread: None,
            extra: Default::default(),
        }
    }
}

/// A person on the Calls page's speed dial. The list is kept per account: `owner` is the MRI of
/// the account that added them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeedDial {
    pub owner: String,
    pub mri: String,
    pub name: String,
}

/// Non-secret settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// Keyring account label for the refresh token. This is the active account's label.
    pub account: String,
    /// Tenant segment for sign-in (`organizations` or a tenant id).
    pub tenant: String,
    /// Persistent endpoint id.
    pub epid: String,
    /// Our MRI once known.
    #[serde(default)]
    pub self_mri: Option<String>,
    /// Our display name once known.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Where a chat opens. Missing in older config files.
    #[serde(default)]
    pub open_chat_at: OpenChatAt,
    /// The calendar view last used (`workweek`, `week`, `month` or `day:N`).
    #[serde(default)]
    pub calendar_view: Option<String>,
    /// The calendar time scale in minutes (15, 30 or 60).
    #[serde(default)]
    pub calendar_scale: Option<u32>,
    /// First day, work hours, weekends, week numbers and second time zone.
    #[serde(default)]
    pub calendar_settings: CalendarSettings,
    /// Views saved by name.
    #[serde(default)]
    pub calendar_saved_views: Vec<SavedView>,
    /// Composer: Enter sends the message (default). When false, Enter starts a new line and
    /// Ctrl+Enter sends.
    #[serde(default = "default_true")]
    pub enter_sends: bool,
    /// Emoji skin tone: 0 for the default yellow, 1..=5 for light to dark.
    #[serde(default)]
    pub skin_tone: u8,
    /// Recently used emoji, newest first.
    #[serde(default)]
    pub recent_emoji: Vec<String>,
    /// Every account. Empty in a file written before accounts existed; [`Config::normalize`]
    /// then adds the one the legacy fields describe.
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// The Calls page's speed dial, in the order shown, for every account.
    #[serde(default)]
    pub speed_dial: Vec<SpeedDial>,
    /// Calls the user removed from the Calls page's history on this computer (Teams has no
    /// delete), as `owner` U+001F `call id`. The call stays in Teams.
    #[serde(default)]
    pub hidden_calls: Vec<String>,
    /// How many megabytes the media cache on disk (pictures, voice messages, clips) may take;
    /// the oldest files go first. `None` (`null` in the file) is no limit; a file without the
    /// key gets [`DEFAULT_MEDIA_CACHE_MB`].
    #[serde(default = "default_media_cache_mb")]
    pub media_cache_mb: Option<u32>,
}

/// The media cache budget of a config file that does not have one.
pub const DEFAULT_MEDIA_CACHE_MB: u32 = 500;

fn default_media_cache_mb() -> Option<u32> {
    Some(DEFAULT_MEDIA_CACHE_MB)
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            account: "default".into(),
            tenant: teams_proto::client_info::DEFAULT_TENANT.into(),
            epid: teams_proto::client::new_epid(),
            self_mri: None,
            display_name: None,
            open_chat_at: OpenChatAt::default(),
            calendar_view: None,
            calendar_scale: None,
            calendar_settings: CalendarSettings::default(),
            calendar_saved_views: Vec::new(),
            enter_sends: true,
            skin_tone: 0,
            recent_emoji: Vec::new(),
            accounts: Vec::new(),
            speed_dial: Vec::new(),
            hidden_calls: Vec::new(),
            media_cache_mb: default_media_cache_mb(),
        }
    }
}

fn now_ms() -> i64 {
    teams_proto::time::now_ms()
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 64
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl Config {
    /// Makes `accounts` hold the active account, which a config of the one-account era lacks.
    /// Pure and in memory: nothing is written and the keyring is not touched.
    pub fn normalize(&mut self) {
        if self.account_by_label(&self.account).is_none() {
            self.accounts.push(Account {
                label: self.account.clone(),
                upn: None,
                display_name: self.display_name.clone(),
                self_mri: self.self_mri.clone(),
                home_tenant: None,
                tenant: self.tenant.clone(),
                epid: self.epid.clone(),
                last_used_ms: None,
                orgs: Vec::new(),
                kind: AccountKind::Work,
                last_unread: None,
                extra: Default::default(),
            });
        }
        // This build cannot run an account of another kind: start on the most recently used
        // one it can (or a fresh signed-out one). The other entry stays in the list untouched.
        if self.active_account().is_some_and(|a| !a.is_usable()) {
            let recent = self.most_recent_usable().map(|a| a.label.clone());
            let next = recent.unwrap_or_else(|| {
                let label = self.free_label("default");
                self.accounts.push(Account::fresh(&label));
                label
            });
            self.load_active(&next);
        }
    }

    /// The accounts this build can sign in to: every one except those of an unknown kind.
    pub fn usable_accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.iter().filter(|a| a.is_usable())
    }

    fn most_recent_usable(&self) -> Option<&Account> {
        self.usable_accounts()
            .max_by_key(|a| a.last_used_ms.unwrap_or(0))
    }

    /// `base`, or `base-2`, `base-3`, ... when that label is taken.
    fn free_label(&self, base: &str) -> String {
        let mut label = base.to_string();
        let mut n = 1;
        while self.account_by_label(&label).is_some() {
            n += 1;
            label = format!("{base}-{n}");
        }
        label
    }

    /// Copies the legacy fields (which callers edit directly) into the active account.
    pub fn sync_active(&mut self) {
        self.normalize();
        let (label, tenant, epid) = (self.account.clone(), self.tenant.clone(), self.epid.clone());
        let (mri, name) = (self.self_mri.clone(), self.display_name.clone());
        if let Some(a) = self.accounts.iter_mut().find(|a| a.label == label) {
            a.tenant = tenant;
            a.epid = epid;
            a.self_mri = mri;
            a.display_name = name;
        }
    }

    /// Sets the legacy fields from the account with this label, which must exist.
    fn load_active(&mut self, label: &str) {
        if let Some(a) = self.account_by_label(label).cloned() {
            self.account = a.label;
            self.tenant = a.tenant;
            self.epid = a.epid;
            self.self_mri = a.self_mri;
            self.display_name = a.display_name;
        }
    }

    pub fn account_by_label(&self, label: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.label == label)
    }

    /// The active account (the one `account` names).
    pub fn active_account(&self) -> Option<&Account> {
        self.account_by_label(&self.account)
    }

    /// Adds a signed-out account with its own endpoint id. It does not become active.
    pub fn add_account(&mut self, label: &str) -> Result<&Account> {
        self.sync_active();
        if !valid_label(label) {
            return Err(Error::Account(format!(
                "label {label:?} must be 1 to 64 letters, digits, '.', '_' or '-'"
            )));
        }
        if self.account_by_label(label).is_some() {
            return Err(Error::Account(format!("account {label:?} already exists")));
        }
        self.accounts.push(Account::fresh(label));
        Ok(self.accounts.last().expect("just pushed"))
    }

    /// Makes another account active; the legacy fields then hold its values.
    pub fn switch_account(&mut self, label: &str) -> Result<()> {
        self.sync_active();
        if self.account_by_label(label).is_none() {
            return Err(Error::Account(format!("no account {label:?}")));
        }
        self.ensure_usable(label)?;
        if let Some(a) = self.accounts.iter_mut().find(|a| a.label == label) {
            a.last_used_ms = Some(now_ms());
        }
        self.load_active(label);
        Ok(())
    }

    /// Removes an account from the list (its keyring entries go with
    /// [`forget_account_tokens`]). Removing the active one activates the most recently used
    /// other account; removing the only one leaves a fresh signed-out `default`.
    pub fn remove_account(&mut self, label: &str) -> Result<()> {
        self.sync_active();
        if self.account_by_label(label).is_none() {
            return Err(Error::Account(format!("no account {label:?}")));
        }
        self.ensure_usable(label)?;
        self.accounts.retain(|a| a.label != label);
        if self.usable_accounts().next().is_none() {
            let fresh = self.free_label("default");
            self.accounts.push(Account::fresh(&fresh));
        }
        if self.account == label {
            let next = self
                .most_recent_usable()
                .map(|a| a.label.clone())
                .expect("at least one usable account");
            self.load_active(&next);
        }
        Ok(())
    }

    /// An error unless the account `label` (which must exist) is of a kind this build can use.
    fn ensure_usable(&self, label: &str) -> Result<()> {
        match self.account_by_label(label) {
            Some(a) if !a.is_usable() => Err(Error::Account(format!(
                "account {label:?} is of a kind this version of IronTeams does not support"
            ))),
            _ => Ok(()),
        }
    }

    /// Remembers the unread total an account had when it quit. Fails for an unknown label and
    /// for an account of an unknown kind, which is never rewritten.
    pub fn set_last_unread(&mut self, label: &str, unread: u32) -> Result<()> {
        self.sync_active();
        if self.account_by_label(label).is_none() {
            return Err(Error::Account(format!("no account {label:?}")));
        }
        self.ensure_usable(label)?;
        if let Some(a) = self.accounts.iter_mut().find(|a| a.label == label) {
            a.last_unread = Some(unread);
        }
        Ok(())
    }

    /// Picks the tenant the active account works in (`organizations`, a home or a guest tenant
    /// id). Tokens for it are minted by the next sign-in or refresh.
    pub fn switch_tenant(&mut self, tenant: &str) -> Result<()> {
        if tenant.is_empty() {
            return Err(Error::Account("empty tenant".into()));
        }
        self.sync_active();
        self.tenant = tenant.into();
        self.sync_active();
        Ok(())
    }

    /// Remembers the organizations of the active account and its home tenant.
    pub fn set_orgs(&mut self, orgs: Vec<OrgRef>) {
        self.sync_active();
        let label = self.account.clone();
        if let Some(a) = self.accounts.iter_mut().find(|a| a.label == label) {
            if let Some(h) = orgs.iter().find(|o| o.is_home) {
                a.home_tenant = Some(h.tenant_id.clone());
            }
            a.orgs = orgs;
        }
    }

    fn home_tenant(&self) -> Option<&str> {
        self.active_account().and_then(|a| a.home_tenant.as_deref())
    }

    /// The [`token_key`] of the active account in the tenant it works in: the label, or
    /// `label@tenant` in a guest organization. Per-account state is keyed by it.
    pub fn active_token_key(&self) -> String {
        token_key(&self.account, &self.tenant, self.home_tenant())
    }

    /// Copies the settings a running app edits (composer, calendar) from `app` onto `self`.
    /// Who is signed in (`account`, `tenant`, `epid`, the identity and `accounts`) stays as it is
    /// in `self`: those belong to whoever changed them last on disk (Settings), not to the
    /// engine's copy from start-up.
    pub fn take_app_prefs(&mut self, app: &Config) {
        self.open_chat_at = app.open_chat_at;
        self.calendar_view = app.calendar_view.clone();
        self.calendar_scale = app.calendar_scale;
        self.calendar_settings = app.calendar_settings.clone();
        self.calendar_saved_views = app.calendar_saved_views.clone();
        self.enter_sends = app.enter_sends;
        self.skin_tone = app.skin_tone;
        self.recent_emoji = app.recent_emoji.clone();
    }

    /// The file name of this account's message cache in the data dir. The first account
    /// (`default`) keeps the name from before accounts existed; every other account, and every
    /// guest organization, has a cache of its own so chats never mix.
    pub fn db_file_name(&self) -> String {
        let home = self.home_tenant();
        let guest = self.tenant != teams_proto::client_info::DEFAULT_TENANT
            && Some(self.tenant.as_str()) != home;
        match (self.account.as_str(), guest) {
            ("default", false) => "cache.sqlite".into(),
            (label, false) => format!("cache-{label}.sqlite"),
            (label, true) => format!("cache-{label}-{}.sqlite", self.tenant),
        }
    }
}

/// Standard directories for ironteams.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    /// XDG directories (`~/.config/ironteams`, `~/.local/share/ironteams`, `~/.cache/ironteams`).
    /// `IRONTEAMS_HOME` overrides all three (used by tests).
    pub fn new() -> Result<Paths> {
        if let Ok(home) = compat::var("IRONTEAMS_HOME") {
            let h = PathBuf::from(home);
            return Ok(Paths {
                config_dir: h.join("config"),
                data_dir: h.join("data"),
                cache_dir: h.join("cache"),
            });
        }
        compat::migrate_legacy_dirs();
        let d = directories::ProjectDirs::from("", "", SERVICE).ok_or(Error::NoHome)?;
        Ok(Paths {
            config_dir: d.config_dir().to_path_buf(),
            data_dir: d.data_dir().to_path_buf(),
            cache_dir: d.cache_dir().to_path_buf(),
        })
    }

    /// SQLite cache path.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("cache.sqlite")
    }

    fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    /// Loads the config, creating a default one (with a fresh epid) if none exists.
    pub fn load_config(&self) -> Result<Config> {
        match std::fs::read_to_string(self.config_path()) {
            Ok(s) => {
                let mut c: Config = serde_json::from_str(&s)?;
                c.normalize();
                Ok(c)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut c = Config::default();
                c.normalize();
                self.save_config(&c)?;
                Ok(c)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The message cache of the account (and organization) `c` is on.
    pub fn db_path_for(&self, c: &Config) -> PathBuf {
        self.data_dir.join(c.db_file_name())
    }

    /// Reads the config, lets `f` change it and saves it, all under the process-wide config
    /// lock, so two writers (Settings and the engine) cannot overwrite each other. Nothing is
    /// saved when `f` fails.
    pub fn update_config<T>(&self, f: impl FnOnce(&mut Config) -> Result<T>) -> Result<T> {
        let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut c = self.load_config()?;
        let out = f(&mut c)?;
        self.save_config(&c)?;
        Ok(out)
    }

    /// Saves the settings the running app edits (see [`Config::take_app_prefs`]) onto the file
    /// as it is now, so an account or organization switch made in the meantime survives.
    pub fn save_app_prefs(&self, app: &Config) -> Result<()> {
        self.update_config(|c| {
            c.take_app_prefs(app);
            Ok(())
        })
    }

    /// Saves the config atomically, after copying the legacy fields into the active account.
    pub fn save_config(&self, c: &Config) -> Result<()> {
        let mut c = c.clone();
        c.sync_active();
        std::fs::create_dir_all(&self.config_dir)?;
        let tmp = self.config_path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&c)?)?;
        std::fs::rename(tmp, self.config_path())?;
        Ok(())
    }
}

/// Where refresh tokens are kept. The system keyring in the app; an in-memory map in tests.
pub trait TokenStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, token: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// The system keyring: the Secret Service (KWallet or GNOME Keyring) on Linux, the login Keychain
/// on macOS and the Credential Manager on Windows.
pub struct SystemKeyring;

/// The entries of one keyring service, with no size handling.
struct ServiceEntries(&'static str);

impl TokenStore for ServiceEntries {
    fn get(&self, key: &str) -> Result<Option<String>> {
        match keyring::Entry::new(self.0, key)?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn set(&self, key: &str, token: &str) -> Result<()> {
        keyring::Entry::new(self.0, key)?.set_password(token)?;
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        match keyring::Entry::new(self.0, key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Entries kept under a service `old` that is no longer written: a read that finds nothing in
/// `new` looks in `old`, copies what it finds to `new` and deletes it from `old`, so a sign-in
/// made before the rename from kelma survives it. Writes go to `new` only; a delete removes
/// both. Failures on `old` never fail a read: it is as if there were no old entry.
struct Migrating<'a> {
    new: &'a dyn TokenStore,
    old: &'a dyn TokenStore,
}

impl TokenStore for Migrating<'_> {
    fn get(&self, key: &str) -> Result<Option<String>> {
        if let Some(t) = self.new.get(key)? {
            return Ok(Some(t));
        }
        let Ok(Some(t)) = self.old.get(key) else {
            return Ok(None);
        };
        // Only drop the old entry once the new one is safely written.
        if self.new.set(key, &t).is_ok() {
            let _ = self.old.delete(key);
        }
        Ok(Some(t))
    }
    fn set(&self, key: &str, token: &str) -> Result<()> {
        self.new.set(key, token)?;
        // A stale old entry would only come back after a sign-out.
        let _ = self.old.delete(key);
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.new.delete(key)?;
        let _ = self.old.delete(key);
        Ok(())
    }
}

/// The keyring entries of the app, with the ones of the old service migrated on read.
fn keyring_entries() -> Migrating<'static> {
    static NEW: ServiceEntries = ServiceEntries(SERVICE);
    static OLD: ServiceEntries = ServiceEntries(LEGACY_SERVICE);
    Migrating {
        new: &NEW,
        old: &OLD,
    }
}

/// The longest password one entry holds. The Credential Manager keeps at most 2560 bytes of
/// UTF-16 (1280 characters), shorter than some refresh tokens; the other keyrings have no
/// practical limit.
#[cfg(windows)]
const ENTRY_MAX: usize = 1200;
#[cfg(not(windows))]
const ENTRY_MAX: usize = usize::MAX;

impl TokenStore for SystemKeyring {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Chunked::new(&keyring_entries(), ENTRY_MAX).get(key)
    }
    fn set(&self, key: &str, token: &str) -> Result<()> {
        Chunked::new(&keyring_entries(), ENTRY_MAX).set(key, token)
    }
    fn delete(&self, key: &str) -> Result<()> {
        Chunked::new(&keyring_entries(), ENTRY_MAX).delete(key)
    }
}

/// Marks an entry whose token is split over `<key>#0`..`<key>#<n-1>`: the entry holds
/// `ironteams-chunks:<n>` (`kelma-chunks:<n>` before the rename, still read). Tokens are
/// base64url-like and never start with either.
const CHUNKS_PREFIX: &str = "ironteams-chunks:";
const LEGACY_CHUNKS_PREFIX: &str = "kelma-chunks:";

/// The part count of a chunk head, whichever prefix it was written with.
fn chunk_count(head: &str) -> Option<usize> {
    head.strip_prefix(CHUNKS_PREFIX)
        .or_else(|| head.strip_prefix(LEGACY_CHUNKS_PREFIX))?
        .parse()
        .ok()
}

/// A token store that splits values longer than `max` characters over several entries of
/// `inner`. A short value is stored as is, so entries written before chunking existed still read.
struct Chunked<'a, S: TokenStore + ?Sized> {
    inner: &'a S,
    max: usize,
}

impl<'a, S: TokenStore + ?Sized> Chunked<'a, S> {
    fn new(inner: &'a S, max: usize) -> Self {
        Chunked { inner, max }
    }

    fn part_key(key: &str, i: usize) -> String {
        format!("{key}#{i}")
    }

    /// How many parts `key` is split over (0 when it is stored whole or not at all).
    fn parts(&self, key: &str) -> Result<usize> {
        Ok(self
            .inner
            .get(key)?
            .and_then(|v| chunk_count(&v))
            .unwrap_or(0))
    }

    fn delete_parts(&self, key: &str, from: usize, to: usize) -> Result<()> {
        (from..to).try_for_each(|i| self.inner.delete(&Self::part_key(key, i)))
    }
}

impl<S: TokenStore + ?Sized> TokenStore for Chunked<'_, S> {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let Some(head) = self.inner.get(key)? else {
            return Ok(None);
        };
        let Some(n) = chunk_count(&head) else {
            return Ok(Some(head));
        };
        let mut token = String::new();
        for i in 0..n {
            match self.inner.get(&Self::part_key(key, i))? {
                Some(part) => token.push_str(&part),
                // A part went missing: the token is unusable, as if there were none.
                None => return Ok(None),
            }
        }
        Ok(Some(token))
    }

    fn set(&self, key: &str, token: &str) -> Result<()> {
        let old = self.parts(key)?;
        if token.chars().count() <= self.max {
            self.inner.set(key, token)?;
            return self.delete_parts(key, 0, old);
        }
        let chars: Vec<char> = token.chars().collect();
        let parts: Vec<String> = chars.chunks(self.max).map(|c| c.iter().collect()).collect();
        for (i, part) in parts.iter().enumerate() {
            self.inner.set(&Self::part_key(key, i), part)?;
        }
        self.inner
            .set(key, &format!("{CHUNKS_PREFIX}{}", parts.len()))?;
        self.delete_parts(key, parts.len(), old)
    }

    fn delete(&self, key: &str) -> Result<()> {
        let n = self.parts(key)?;
        self.inner.delete(key)?;
        self.delete_parts(key, 0, n)
    }
}

/// A token store in memory, for tests.
#[derive(Default)]
pub struct MemoryStore(std::sync::Mutex<std::collections::BTreeMap<String, String>>);

impl MemoryStore {
    /// The stored keys, sorted.
    pub fn keys(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
}

impl TokenStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, token: &str) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), token.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

/// The keyring key of an account's token for a tenant. The home tenant (the sign-in segment
/// `organizations`, or the home tenant id when known) uses the plain label, as before accounts
/// existed; a guest tenant uses `label@tenant`. Labels cannot contain `@`.
pub fn token_key(label: &str, tenant: &str, home_tenant: Option<&str>) -> String {
    if tenant == teams_proto::client_info::DEFAULT_TENANT || Some(tenant) == home_tenant {
        label.to_string()
    } else {
        format!("{label}@{tenant}")
    }
}

/// Stores a refresh token for an account in a tenant.
pub fn save_token_in(
    store: &dyn TokenStore,
    label: &str,
    tenant: &str,
    home_tenant: Option<&str>,
    token: &str,
) -> Result<()> {
    store.set(&token_key(label, tenant, home_tenant), token)
}

/// Removes every refresh token of an account: the home one and each guest tenant it knows.
///
/// The label is also marked as removed for [`persist_in`], before anything is deleted, so a
/// token save that is already running cannot write the token back.
pub fn forget_account_tokens(store: &dyn TokenStore, account: &Account) -> Result<()> {
    let mut gone = revoked();
    gone.insert(account.label.clone());
    store.delete(&account.label)?;
    let tenants = account
        .orgs
        .iter()
        .map(|o| o.tenant_id.as_str())
        .chain(std::iter::once(account.tenant.as_str()));
    for t in tenants {
        store.delete(&token_key(
            &account.label,
            t,
            account.home_tenant.as_deref(),
        ))?;
    }
    drop(gone);
    Ok(())
}

/// Stores the refresh token in the system keyring.
pub fn save_refresh_token(account: &str, token: &str) -> Result<()> {
    SystemKeyring.set(account, token)
}

/// Loads the refresh token, or `None` if the user has not signed in.
pub fn load_refresh_token(account: &str) -> Result<Option<String>> {
    SystemKeyring.get(account)
}

/// Removes the refresh token (sign out).
pub fn delete_refresh_token(account: &str) -> Result<()> {
    SystemKeyring.delete(account)
}

/// Builds a protocol session from config plus a token store, if signed in. A guest tenant
/// without a token of its own starts from the account's home token, which the first refresh
/// redeems at the guest tenant.
pub fn stored_session_in(
    store: &dyn TokenStore,
    c: &Config,
) -> Result<Option<teams_proto::Session>> {
    let home = c.home_tenant();
    let own = token_key(&c.account, &c.tenant, home);
    let token = match store.get(&own)? {
        Some(t) => Some(t),
        None if own != c.account => store.get(&c.account)?,
        None => None,
    };
    Ok(token.map(|rt| teams_proto::Session {
        tenant: c.tenant.clone(),
        refresh_token: rt,
        epid: c.epid.clone(),
    }))
}

/// Builds a protocol session from config plus keyring, if signed in.
pub fn stored_session(c: &Config) -> Result<Option<teams_proto::Session>> {
    stored_session_in(&SystemKeyring, c)
}

/// Persists the (possibly rotated) refresh token of a live client, under the tenant the
/// client works in.
pub async fn persist_in(
    store: &dyn TokenStore,
    c: &Config,
    client: &teams_proto::Client,
) -> Result<()> {
    persist_session_in(store, c, &client.session().await)
}

/// [`persist_in`] for a session that was already read from the client.
pub fn persist_session_in(
    store: &dyn TokenStore,
    c: &Config,
    s: &teams_proto::Session,
) -> Result<()> {
    // Held while writing, so a removal either happens before (and the save is skipped) or after
    // (and deletes what was written).
    let gone = revoked();
    if gone.contains(&c.account) {
        return Ok(());
    }
    let r = save_token_in(
        store,
        &c.account,
        &s.tenant,
        c.home_tenant(),
        &s.refresh_token,
    );
    drop(gone);
    r
}

/// Persists the (possibly rotated) refresh token of a live client.
pub async fn persist(c: &Config, client: &teams_proto::Client) -> Result<()> {
    persist_in(&SystemKeyring, c, client).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- tokens longer than one keyring entry holds ----

    #[test]
    fn a_short_token_is_stored_whole() {
        let mem = MemoryStore::default();
        let store = Chunked::new(&mem, 8);
        store.set("me", "abc").unwrap();
        assert_eq!(mem.keys(), ["me"]);
        assert_eq!(store.get("me").unwrap().as_deref(), Some("abc"));
    }

    #[test]
    fn a_long_token_is_split_and_read_back() {
        let mem = MemoryStore::default();
        let store = Chunked::new(&mem, 4);
        store.set("me", "0123456789").unwrap();
        assert_eq!(mem.keys(), ["me", "me#0", "me#1", "me#2"]);
        assert_eq!(
            mem.get("me").unwrap().as_deref(),
            Some("ironteams-chunks:3")
        );
        assert_eq!(store.get("me").unwrap().as_deref(), Some("0123456789"));
    }

    #[test]
    fn rewriting_a_token_drops_the_parts_it_no_longer_needs() {
        let mem = MemoryStore::default();
        let store = Chunked::new(&mem, 4);
        store.set("me", "0123456789").unwrap();
        store.set("me", "01234").unwrap();
        assert_eq!(mem.keys(), ["me", "me#0", "me#1"]);
        assert_eq!(store.get("me").unwrap().as_deref(), Some("01234"));
        store.set("me", "ab").unwrap();
        assert_eq!(mem.keys(), ["me"]);
        store.delete("me").unwrap();
        assert!(mem.keys().is_empty());
        assert_eq!(store.get("me").unwrap(), None);
    }

    #[test]
    fn deleting_a_split_token_removes_every_part() {
        let mem = MemoryStore::default();
        let store = Chunked::new(&mem, 3);
        store.set("a@t", "abcdefg").unwrap();
        store.set("b", "xyz").unwrap();
        store.delete("a@t").unwrap();
        assert_eq!(mem.keys(), ["b"]);
    }

    #[test]
    fn a_missing_part_reads_as_no_token() {
        let mem = MemoryStore::default();
        let store = Chunked::new(&mem, 4);
        store.set("me", "0123456789").unwrap();
        mem.delete("me#1").unwrap();
        assert_eq!(store.get("me").unwrap(), None);
    }

    // ---- the rename from kelma ----

    #[test]
    fn a_chunk_head_with_either_prefix_reads_back() {
        for head in ["kelma-chunks:2", "ironteams-chunks:2"] {
            let mem = MemoryStore::default();
            mem.set("me", head).unwrap();
            mem.set("me#0", "ab").unwrap();
            mem.set("me#1", "cd").unwrap();
            assert_eq!(
                Chunked::new(&mem, 2).get("me").unwrap().as_deref(),
                Some("abcd")
            );
        }
    }

    #[test]
    fn rewriting_an_old_chunked_token_drops_its_parts() {
        let mem = MemoryStore::default();
        mem.set("me", "kelma-chunks:3").unwrap();
        for i in 0..3 {
            mem.set(&format!("me#{i}"), "xx").unwrap();
        }
        Chunked::new(&mem, 8).set("me", "short").unwrap();
        assert_eq!(mem.keys(), ["me"]);
    }

    #[test]
    fn a_sign_in_under_the_old_service_moves_to_the_new_one_on_read() {
        let (new, old) = (MemoryStore::default(), MemoryStore::default());
        old.set("default", "rt-old").unwrap();
        let m = Migrating {
            new: &new,
            old: &old,
        };
        assert_eq!(m.get("default").unwrap().as_deref(), Some("rt-old"));
        assert_eq!(new.get("default").unwrap().as_deref(), Some("rt-old"));
        assert!(old.keys().is_empty());
        // The second read comes from the new service.
        assert_eq!(m.get("default").unwrap().as_deref(), Some("rt-old"));
    }

    #[test]
    fn the_new_service_wins_over_a_stale_old_entry() {
        let (new, old) = (MemoryStore::default(), MemoryStore::default());
        new.set("default", "rt-new").unwrap();
        old.set("default", "rt-old").unwrap();
        let m = Migrating {
            new: &new,
            old: &old,
        };
        assert_eq!(m.get("default").unwrap().as_deref(), Some("rt-new"));
        assert_eq!(old.get("default").unwrap().as_deref(), Some("rt-old"));
    }

    #[test]
    fn nothing_anywhere_reads_as_no_token_and_writes_go_to_the_new_service() {
        let (new, old) = (MemoryStore::default(), MemoryStore::default());
        let m = Migrating {
            new: &new,
            old: &old,
        };
        assert_eq!(m.get("default").unwrap(), None);
        old.set("default", "stale").unwrap();
        m.set("default", "rt").unwrap();
        assert_eq!(new.get("default").unwrap().as_deref(), Some("rt"));
        assert!(old.keys().is_empty());
        m.delete("default").unwrap();
        assert!(new.keys().is_empty());
    }

    #[test]
    fn an_old_chunked_token_migrates_part_by_part_and_reads_whole() {
        let (new, old) = (MemoryStore::default(), MemoryStore::default());
        old.set("me", "kelma-chunks:2").unwrap();
        old.set("me#0", "ab").unwrap();
        old.set("me#1", "cd").unwrap();
        let m = Migrating {
            new: &new,
            old: &old,
        };
        assert_eq!(
            Chunked::new(&m, 2).get("me").unwrap().as_deref(),
            Some("abcd")
        );
        assert!(old.keys().is_empty());
        assert_eq!(new.keys(), ["me", "me#0", "me#1"]);
        // Deleting through the wrapper clears every part.
        Chunked::new(&m, 2).delete("me").unwrap();
        assert!(new.keys().is_empty());
    }

    // ---- accounts and orgs (X2.1) ----

    const OLD: &str = r#"{"account":"default","tenant":"organizations","epid":"e1","self_mri":"8:orgid:me","display_name":"Me"}"#;

    fn old_config() -> Config {
        let mut c: Config = serde_json::from_str(OLD).unwrap();
        c.normalize();
        c
    }

    #[test]
    fn old_config_migrates_to_one_account_with_the_old_label() {
        let c = old_config();
        assert_eq!(c.accounts.len(), 1);
        let a = &c.accounts[0];
        assert_eq!(a.label, "default");
        assert_eq!(a.epid, "e1");
        assert_eq!(a.tenant, "organizations");
        assert_eq!(a.self_mri.as_deref(), Some("8:orgid:me"));
        assert_eq!(a.display_name.as_deref(), Some("Me"));
        // The legacy fields still hold the active account's values.
        assert_eq!((c.account.as_str(), c.epid.as_str()), ("default", "e1"));
    }

    #[test]
    fn migrated_account_reads_the_old_keyring_entry_without_writes() {
        let store = MemoryStore::default();
        store.set("default", "rt-old").unwrap();
        let c = old_config();
        let s = stored_session_in(&store, &c).unwrap().unwrap();
        assert_eq!(s.refresh_token, "rt-old");
        assert_eq!(s.tenant, "organizations");
        assert_eq!(s.epid, "e1");
        assert_eq!(store.keys(), vec!["default".to_string()], "nothing written");
    }

    #[test]
    fn load_config_of_an_old_file_migrates_in_memory_and_leaves_the_file_alone() {
        let dir =
            std::env::temp_dir().join(format!("ironteams-session-mig-{}", std::process::id()));
        let p = Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        std::fs::create_dir_all(&p.config_dir).unwrap();
        std::fs::write(p.config_dir.join("config.json"), OLD).unwrap();
        let c = p.load_config().unwrap();
        assert_eq!(c.accounts.len(), 1);
        assert_eq!(
            std::fs::read_to_string(p.config_dir.join("config.json")).unwrap(),
            OLD
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_config_keeps_the_legacy_fields_so_an_older_build_still_reads_it() {
        let mut c = old_config();
        c.add_account("work").unwrap();
        c.switch_account("work").unwrap();
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["account"], "work");
        assert_eq!(json["tenant"], "organizations");
        assert!(json["epid"].as_str().unwrap().len() == 36);
        assert_eq!(json["accounts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn add_switch_remove_accounts() {
        let mut c = old_config();
        c.add_account("work").unwrap();
        assert!(c.add_account("work").is_err(), "duplicate label");
        assert!(c.add_account("").is_err(), "empty label");
        assert!(c.add_account("a@b").is_err(), "label must not contain @");
        assert_eq!(c.account, "default", "adding does not switch");
        let work_epid = c.account_by_label("work").unwrap().epid.clone();
        assert_ne!(work_epid, "e1", "per-account endpoint id");

        c.switch_account("work").unwrap();
        assert_eq!(
            (c.account.as_str(), c.epid.as_str()),
            ("work", work_epid.as_str())
        );
        assert_eq!(c.self_mri, None, "the new account has no identity yet");
        assert!(c.account_by_label("work").unwrap().last_used_ms.is_some());
        assert!(c.switch_account("nope").is_err());

        // Edits through the legacy fields land on the active account.
        c.self_mri = Some("8:orgid:w".into());
        c.sync_active();
        c.switch_account("default").unwrap();
        assert_eq!(c.self_mri.as_deref(), Some("8:orgid:me"));
        assert_eq!(
            c.account_by_label("work").unwrap().self_mri.as_deref(),
            Some("8:orgid:w")
        );

        c.remove_account("default").unwrap();
        assert_eq!(
            c.account, "work",
            "removing the active account switches to another"
        );
        assert_eq!(c.accounts.len(), 1);
        c.remove_account("work").unwrap();
        assert_eq!(
            c.accounts.len(),
            1,
            "the last account is replaced by a fresh signed-out one"
        );
        assert_eq!(c.account, "default");
        assert_ne!(c.epid, work_epid);
        assert!(c.remove_account("ghost").is_err());
    }

    #[test]
    fn save_config_syncs_legacy_field_edits_into_the_active_account() {
        let dir =
            std::env::temp_dir().join(format!("ironteams-session-sync-{}", std::process::id()));
        let p = Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        let mut c = p.load_config().unwrap();
        c.display_name = Some("Someone".into());
        p.save_config(&c).unwrap();
        let back = p.load_config().unwrap();
        assert_eq!(back.accounts[0].display_name.as_deref(), Some("Someone"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tokens_are_per_account_and_per_tenant() {
        let store = MemoryStore::default();
        let mut c = old_config();
        c.add_account("work").unwrap();
        store.set("default", "rt-default").unwrap();
        store.set("work", "rt-work").unwrap();
        c.switch_account("work").unwrap();
        assert_eq!(
            stored_session_in(&store, &c)
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-work"
        );

        // A guest tenant without its own token re-mints from the home token.
        c.switch_tenant("guest-1").unwrap();
        assert_eq!(c.tenant, "guest-1");
        let s = stored_session_in(&store, &c).unwrap().unwrap();
        assert_eq!(
            (s.tenant.as_str(), s.refresh_token.as_str()),
            ("guest-1", "rt-work")
        );

        // Once the guest tenant issued its own token, that one wins and the home one stays.
        store.set("work@guest-1", "rt-work-guest").unwrap();
        let s = stored_session_in(&store, &c).unwrap().unwrap();
        assert_eq!(s.refresh_token, "rt-work-guest");
        assert_eq!(store.get("work").unwrap().as_deref(), Some("rt-work"));
        assert_eq!(token_key("work", "guest-1", None), "work@guest-1");
        assert_eq!(token_key("work", "organizations", None), "work");
        assert_eq!(token_key("work", "home-id", Some("home-id")), "work");

        // Back to the home tenant.
        c.switch_tenant("organizations").unwrap();
        assert_eq!(
            stored_session_in(&store, &c)
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt-work"
        );
    }

    #[test]
    fn tenant_choice_is_remembered_per_account() {
        let mut c = old_config();
        c.add_account("work").unwrap();
        c.switch_account("work").unwrap();
        c.switch_tenant("guest-1").unwrap();
        c.switch_account("default").unwrap();
        assert_eq!(c.tenant, "organizations");
        c.switch_account("work").unwrap();
        assert_eq!(c.tenant, "guest-1");
    }

    #[test]
    fn org_list_is_cached_and_names_the_home_tenant() {
        let mut c = old_config();
        c.set_orgs(vec![
            OrgRef {
                tenant_id: "home-id".into(),
                name: "Home".into(),
                is_home: true,
            },
            OrgRef {
                tenant_id: "guest-1".into(),
                name: "Guest".into(),
                is_home: false,
            },
        ]);
        let a = c.active_account().unwrap();
        assert_eq!(a.home_tenant.as_deref(), Some("home-id"));
        assert_eq!(a.orgs.len(), 2);
        // The home tenant id maps to the plain account key.
        let store = MemoryStore::default();
        store.set("default", "rt").unwrap();
        c.switch_tenant("home-id").unwrap();
        assert_eq!(
            stored_session_in(&store, &c)
                .unwrap()
                .unwrap()
                .refresh_token,
            "rt"
        );
    }

    #[test]
    fn org_ref_comes_from_a_parsed_tenant() {
        let t = teams_proto::tenants::parse_tenants(&serde_json::json!([
            {"tenantId":"t1","tenantName":"Home","tenantRelationship":"Default"}
        ]));
        let o = OrgRef::from(&t[0]);
        assert_eq!(
            (o.tenant_id.as_str(), o.name.as_str(), o.is_home),
            ("t1", "Home", true)
        );
    }

    #[test]
    fn persist_and_forget_use_the_matching_keys() {
        let store = MemoryStore::default();
        let mut c = old_config();
        c.set_orgs(vec![OrgRef {
            tenant_id: "guest-1".into(),
            name: "G".into(),
            is_home: false,
        }]);
        save_token_in(
            &store,
            &c.account,
            "guest-1",
            c.active_account().and_then(|a| a.home_tenant.as_deref()),
            "x",
        )
        .unwrap();
        store.set("default", "h").unwrap();
        assert_eq!(
            store.keys(),
            vec!["default".to_string(), "default@guest-1".to_string()]
        );
        forget_account_tokens(&store, c.active_account().unwrap()).unwrap();
        assert!(
            store.keys().is_empty(),
            "sign-out removes the home and every guest token"
        );
    }

    #[test]
    fn config_roundtrip_and_default_epid() {
        let dir =
            std::env::temp_dir().join(format!("ironteams-session-test-{}", std::process::id()));
        let p = Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        };
        let c = p.load_config().unwrap();
        assert_eq!(c.tenant, "organizations");
        assert_eq!(c.epid.len(), 36);
        let again = p.load_config().unwrap();
        assert_eq!(c, again, "epid must persist across loads");
        let raw = std::fs::read_to_string(dir.join("config/config.json")).unwrap();
        assert!(!raw.contains("refresh"), "config must not contain tokens");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_chat_at_defaults_for_old_config_files_and_roundtrips() {
        let old = r#"{"account":"default","tenant":"organizations","epid":"e"}"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert_eq!(c.open_chat_at, OpenChatAt::FirstUnread);

        let c: Config =
            serde_json::from_str(&old.replace('}', r#","open_chat_at":"newest"}"#)).unwrap();
        assert_eq!(c.open_chat_at, OpenChatAt::Newest);
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn media_cache_budget_defaults_to_500_mb_and_null_means_no_limit() {
        let old = r#"{"account":"default","tenant":"organizations","epid":"e"}"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert_eq!(c.media_cache_mb, Some(500), "a file from before the budget");
        assert_eq!(Config::default().media_cache_mb, Some(500));

        let c: Config =
            serde_json::from_str(&old.replace('}', r#","media_cache_mb":128}"#)).unwrap();
        assert_eq!(c.media_cache_mb, Some(128));

        let unlimited: Config =
            serde_json::from_str(&old.replace('}', r#","media_cache_mb":null}"#)).unwrap();
        assert_eq!(unlimited.media_cache_mb, None);
        let back: Config =
            serde_json::from_str(&serde_json::to_string(&unlimited).unwrap()).unwrap();
        assert_eq!(back.media_cache_mb, None, "no limit survives a save");
    }

    #[test]
    fn config_without_calendar_fields_still_loads() {
        let old = r#"{"account":"default","tenant":"organizations","epid":"e"}"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert_eq!((c.calendar_view, c.calendar_scale), (None, None));
        let c = Config {
            calendar_view: Some("day:3".into()),
            calendar_scale: Some(15),
            ..c
        };
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn calendar_settings_and_saved_views_default_for_old_files_and_roundtrip() {
        let old = r#"{"account":"default","tenant":"organizations","epid":"e"}"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert_eq!(c.calendar_settings, CalendarSettings::default());
        assert_eq!(c.calendar_settings.first_day, 1, "Monday");
        assert!(c.calendar_settings.show_weekends);
        assert!(!c.calendar_settings.week_numbers);
        assert!(c.calendar_saved_views.is_empty());

        // A partial settings object keeps the defaults of what it leaves out.
        let c: Config = serde_json::from_str(&old.replace(
            '}',
            r#","calendar_settings":{"first_day":0,"second_tz_min":-300}}"#,
        ))
        .unwrap();
        assert_eq!(c.calendar_settings.first_day, 0);
        assert_eq!(c.calendar_settings.second_tz_min, Some(-300));
        assert_eq!(
            c.calendar_settings.second_tz, None,
            "no zone yet: the fixed offset shows"
        );
        assert!(c.calendar_settings.show_weekends);

        let mut c = c;
        c.calendar_saved_views.push(SavedView {
            name: "Planning".into(),
            view: "day:3".into(),
            scale: 15,
        });
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn config_written_before_the_composer_settings_still_loads() {
        let old = r#"{"account":"default","tenant":"organizations","epid":"e"}"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert!(c.enter_sends, "Enter sends by default");
        assert_eq!(c.skin_tone, 0);
        assert!(c.recent_emoji.is_empty());
        let mut c = c;
        c.enter_sends = false;
        c.skin_tone = 3;
        c.recent_emoji = vec!["👍".into()];
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    // ---- one writer for config.json, and removed tokens stay removed (X2.1) ----

    fn temp_paths(tag: &str) -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths {
            config_dir: dir.path().join(format!("{tag}-config")),
            data_dir: dir.path().join(format!("{tag}-data")),
            cache_dir: dir.path().join(format!("{tag}-cache")),
        };
        (dir, p)
    }

    #[test]
    fn an_account_switch_survives_an_engine_save() {
        let (_dir, p) = temp_paths("race");
        let mut c = p.load_config().unwrap();
        c.add_account("second").unwrap();
        p.save_config(&c).unwrap();

        // The engine started on `default` and keeps its own copy.
        let mut engine = p.load_config().unwrap();
        assert_eq!(engine.account, "default");

        // Settings switches to `second` and to a guest organization.
        p.update_config(|c| {
            c.switch_account("second")?;
            c.switch_tenant("guest-1")
        })
        .unwrap();

        // The engine then saves a composer preference from its (older) copy.
        engine.enter_sends = false;
        engine.skin_tone = 4;
        engine.calendar_scale = Some(15);
        p.save_app_prefs(&engine).unwrap();

        let back = p.load_config().unwrap();
        assert_eq!(back.account, "second", "the switch was not overwritten");
        assert_eq!(back.tenant, "guest-1");
        assert_eq!(back.epid, p.load_config().unwrap().epid);
        assert_eq!(back.accounts.len(), 2);
        assert!(!back.enter_sends, "the engine's own settings were saved");
        assert_eq!(back.skin_tone, 4);
        assert_eq!(back.calendar_scale, Some(15));
    }

    #[test]
    fn update_config_saves_nothing_when_the_change_fails() {
        let (_dir, p) = temp_paths("fail");
        p.load_config().unwrap();
        let before = std::fs::read_to_string(p.config_path()).unwrap();
        let r = p.update_config(|c| {
            c.switch_tenant("guest-1")?;
            c.switch_account("nope")
        });
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(p.config_path()).unwrap(), before);
    }

    #[test]
    fn a_token_save_after_sign_out_does_not_write_the_token_back() {
        let store = MemoryStore::default();
        let sess = teams_proto::Session {
            tenant: "organizations".into(),
            refresh_token: "rotated".into(),
            epid: "e".into(),
        };
        let mut c = old_config();
        c.account = "signout-race".into();
        c.normalize();
        store.set("signout-race", "old").unwrap();

        // The saver runs while the account is signed in: it writes.
        persist_session_in(&store, &c, &sess).unwrap();
        assert_eq!(store.keys(), vec!["signout-race".to_string()]);

        // Sign-out forgets the tokens and marks the account.
        let acct = c.active_account().unwrap().clone();
        forget_account_tokens(&store, &acct).unwrap();
        assert!(store.keys().is_empty());
        assert!(tokens_revoked("signout-race"));

        // The saver's next round (or one already in flight) must not bring them back.
        persist_session_in(&store, &c, &sess).unwrap();
        assert!(store.keys().is_empty(), "the removed token stayed removed");

        // A new account that reuses the label can store tokens again.
        allow_account_tokens("signout-race");
        persist_session_in(&store, &c, &sess).unwrap();
        assert_eq!(store.keys(), vec!["signout-race".to_string()]);
    }

    #[test]
    fn each_account_and_guest_organization_has_its_own_cache_file() {
        let mut c = old_config();
        assert_eq!(
            c.db_file_name(),
            "cache.sqlite",
            "the first account keeps its cache"
        );
        c.add_account("work").unwrap();
        c.switch_account("work").unwrap();
        assert_eq!(c.db_file_name(), "cache-work.sqlite");
        c.switch_tenant("guest-1").unwrap();
        assert_eq!(c.db_file_name(), "cache-work-guest-1.sqlite");
        c.set_orgs(vec![OrgRef {
            tenant_id: "home-id".into(),
            name: "Home".into(),
            is_home: true,
        }]);
        c.switch_tenant("home-id").unwrap();
        assert_eq!(
            c.db_file_name(),
            "cache-work.sqlite",
            "the home tenant id is the home org"
        );
        let p = Paths {
            config_dir: "/c".into(),
            data_dir: "/d".into(),
            cache_dir: "/x".into(),
        };
        assert_eq!(p.db_path_for(&c), PathBuf::from("/d/cache-work.sqlite"));
    }

    // ---- account kinds and forward-compatible entries ----

    fn acct(label: &str, kind: Option<&str>, last_used_ms: Option<i64>) -> serde_json::Value {
        let mut v = serde_json::json!({
            "label": label,
            "tenant": "organizations",
            "epid": format!("ep-{label}"),
            "last_used_ms": last_used_ms,
        });
        if let Some(k) = kind {
            v["kind"] = k.into();
        }
        v
    }

    fn config_with(active: &str, accounts: Vec<serde_json::Value>) -> Config {
        let mut c: Config = serde_json::from_value(serde_json::json!({
            "account": active,
            "tenant": "organizations",
            "epid": format!("ep-{active}"),
            "accounts": accounts,
        }))
        .unwrap();
        c.normalize();
        c
    }

    #[test]
    fn a_missing_kind_is_a_work_account_and_is_not_written() {
        let a: Account = serde_json::from_value(acct("work", None, None)).unwrap();
        assert_eq!(a.kind, AccountKind::Work);
        assert!(a.is_usable());
        assert_eq!(a.last_unread, None);
        assert!(a.extra.is_empty());
        let back = serde_json::to_value(&a).unwrap();
        assert!(back.get("kind").is_none(), "{back}");

        // The same for an explicit "work".
        let a: Account = serde_json::from_value(acct("work", Some("work"), None)).unwrap();
        assert_eq!(a.kind, AccountKind::Work);
    }

    #[test]
    fn an_unknown_kind_and_unknown_fields_are_written_back_unchanged() {
        let mut raw = acct("pers", Some("personal"), Some(7));
        raw["last_unread"] = 3.into();
        raw["future_field"] = serde_json::json!({"a": [1, 2], "b": null});
        let a: Account = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(a.kind, AccountKind::Unknown("personal".into()));
        assert!(!a.is_usable());
        assert_eq!(a.last_unread, Some(3));
        assert_eq!(a.extra.len(), 1);

        let back = serde_json::to_value(&a).unwrap();
        assert_eq!(back["kind"], "personal");
        assert_eq!(back["future_field"], raw["future_field"]);
        assert_eq!(back["last_unread"], 3);
        // And it reads the same again.
        assert_eq!(serde_json::from_value::<Account>(back).unwrap(), a);
    }

    #[test]
    fn a_kind_of_the_wrong_type_does_not_break_the_file() {
        let mut raw = acct("odd", None, None);
        raw["kind"] = serde_json::json!({"x": 1});
        let a: Account = serde_json::from_value(raw).unwrap();
        assert!(matches!(a.kind, AccountKind::Unknown(_)));
        let mut raw = acct("nul", None, None);
        raw["kind"] = serde_json::Value::Null;
        assert_eq!(
            serde_json::from_value::<Account>(raw).unwrap().kind,
            AccountKind::Work
        );
    }

    #[test]
    fn an_old_config_without_the_new_fields_loads() {
        let old = r#"{"account":"default","tenant":"organizations","epid":"e",
            "accounts":[{"label":"default","tenant":"organizations","epid":"e"}]}"#;
        let mut c: Config = serde_json::from_str(old).unwrap();
        c.normalize();
        let a = &c.accounts[0];
        assert_eq!(
            (a.kind.clone(), a.last_unread, a.extra.len()),
            (AccountKind::Work, None, 0)
        );
    }

    #[test]
    fn usable_accounts_skips_unknown_kinds() {
        let c = config_with(
            "work",
            vec![
                acct("work", None, Some(1)),
                acct("pers", Some("personal"), Some(9)),
                acct("other", Some("work"), None),
            ],
        );
        let labels: Vec<_> = c.usable_accounts().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["work", "other"]);
        assert_eq!(c.accounts.len(), 3, "the unknown one stays in the list");
    }

    #[test]
    fn startup_falls_back_when_the_active_account_is_unknown() {
        let (_dir, p) = temp_paths("kind-fallback");
        std::fs::create_dir_all(&p.config_dir).unwrap();
        let mut raw = serde_json::json!({
            "account": "pers",
            "tenant": "t-pers",
            "epid": "ep-pers",
            "self_mri": "8:orgid:pers",
            "accounts": [
                acct("old", None, Some(1)),
                acct("pers", Some("personal"), Some(100)),
                acct("recent", None, Some(50)),
            ],
        });
        raw["accounts"][1]["tenant"] = "t-pers".into();
        raw["accounts"][1]["future_field"] = "keep me".into();
        std::fs::write(p.config_path(), raw.to_string()).unwrap();

        let c = p.load_config().unwrap();
        assert_eq!(c.account, "recent", "the most recently used work account");
        assert_eq!(c.epid, "ep-recent");
        assert_eq!(c.tenant, "organizations");
        assert_eq!(
            c.self_mri, None,
            "the legacy fields follow the new active one"
        );

        // Saving keeps the entry it skipped exactly as it was.
        p.save_config(&c).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(p.config_path()).unwrap()).unwrap();
        assert_eq!(written["account"], "recent");
        let pers = written["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["label"] == "pers")
            .expect("not removed");
        assert_eq!(pers["kind"], "personal");
        assert_eq!(pers["future_field"], "keep me");
        assert_eq!(pers["tenant"], "t-pers");
        assert_eq!(pers["epid"], "ep-pers");
        assert_eq!(pers["last_used_ms"], 100);
    }

    #[test]
    fn startup_adds_a_fresh_account_when_none_is_usable() {
        let c = config_with(
            "default",
            vec![
                acct("default", Some("personal"), Some(1)),
                acct("pers2", Some("personal"), Some(2)),
            ],
        );
        assert_eq!(c.account, "default-2", "a free label, not the unknown one");
        assert_eq!(c.accounts.len(), 3);
        assert!(c.active_account().unwrap().is_usable());
        assert_eq!(
            c.account_by_label("default").unwrap().kind,
            AccountKind::Unknown("personal".into())
        );
    }

    #[test]
    fn switching_to_an_unknown_account_is_refused() {
        let mut c = config_with(
            "work",
            vec![
                acct("work", None, Some(1)),
                acct("pers", Some("personal"), None),
            ],
        );
        let before = c.clone();
        let err = c.switch_account("pers").unwrap_err().to_string();
        assert!(
            err.contains("pers") && err.contains("does not support"),
            "{err}"
        );
        assert_eq!(c.account, "work");
        assert_eq!(
            c.account_by_label("pers"),
            before.account_by_label("pers"),
            "not rewritten, not even its last_used_ms"
        );
        // A usable one still works.
        c.add_account("second").unwrap();
        c.switch_account("second").unwrap();
        assert_eq!(c.account, "second");
    }

    #[test]
    fn an_unknown_account_is_never_removed() {
        let mut c = config_with(
            "work",
            vec![
                acct("work", None, Some(1)),
                acct("pers", Some("personal"), None),
            ],
        );
        assert!(c.remove_account("pers").is_err());
        assert!(c.account_by_label("pers").is_some());

        // Removing the only usable account leaves a fresh signed-out one beside the unknown.
        c.remove_account("work").unwrap();
        assert_eq!(c.account, "default");
        assert!(c.active_account().unwrap().is_usable());
        assert!(c.account_by_label("pers").is_some());
        assert_eq!(c.accounts.len(), 2);
    }

    #[test]
    fn removing_the_active_account_picks_a_usable_one() {
        let mut c = config_with(
            "work",
            vec![
                acct("work", None, Some(1)),
                acct("pers", Some("personal"), Some(999)),
                acct("b", None, Some(5)),
            ],
        );
        c.remove_account("work").unwrap();
        assert_eq!(c.account, "b", "never the more recent unknown account");
    }

    #[test]
    fn last_unread_is_saved_per_account_and_survives_a_save() {
        let (_dir, p) = temp_paths("unread");
        let mut c = p.load_config().unwrap();
        c.add_account("second").unwrap();
        p.save_config(&c).unwrap();

        p.update_config(|c| {
            c.set_last_unread("second", 12)?;
            c.set_last_unread("default", 0)
        })
        .unwrap();
        let back = p.load_config().unwrap();
        assert_eq!(
            back.account_by_label("second").unwrap().last_unread,
            Some(12)
        );
        assert_eq!(
            back.account_by_label("default").unwrap().last_unread,
            Some(0)
        );

        // Switching and saving (which syncs the legacy fields) does not clear it.
        p.update_config(|c| c.switch_account("second")).unwrap();
        p.save_config(&p.load_config().unwrap()).unwrap();
        assert_eq!(
            p.load_config()
                .unwrap()
                .account_by_label("second")
                .unwrap()
                .last_unread,
            Some(12)
        );

        let mut c = p.load_config().unwrap();
        assert!(c.set_last_unread("nope", 1).is_err());
    }

    #[test]
    fn last_unread_is_not_written_for_an_unknown_account() {
        let mut c = config_with(
            "work",
            vec![
                acct("work", None, None),
                acct("pers", Some("personal"), None),
            ],
        );
        assert!(c.set_last_unread("pers", 4).is_err());
        assert_eq!(c.account_by_label("pers").unwrap().last_unread, None);
    }

    #[test]
    fn the_active_token_key_is_the_label_or_label_at_tenant() {
        let mut c = config_with("work", vec![acct("work", None, None)]);
        assert_eq!(c.active_token_key(), "work");
        c.switch_tenant("guest-1").unwrap();
        assert_eq!(c.active_token_key(), "work@guest-1");
        c.set_orgs(vec![OrgRef {
            tenant_id: "home-id".into(),
            name: "Home".into(),
            is_home: true,
        }]);
        c.switch_tenant("home-id").unwrap();
        assert_eq!(c.active_token_key(), "work");
    }
}
