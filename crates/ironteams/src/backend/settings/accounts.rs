use super::*;

// ---------------------------------------------------------------------------------------------
// Accounts and orgs.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub label: String,
    pub name: String,
    pub upn: String,
    pub active: bool,
    /// The unread total the account had when it last quit (`Account.last_unread`); 0 when
    /// unknown.
    pub unread: u32,
    /// The id of the theme set for this account (`ThemesFile.accounts`); "" for the app's.
    pub theme: String,
}

/// The usable accounts (an account of a kind this version does not know is left out), each with
/// the unread count it had when it last ran and the theme set for it.
pub fn account_rows(cfg: &session::Config, themes: &appearance::ThemesFile) -> Vec<AccountRow> {
    let mut cfg = cfg.clone();
    cfg.sync_active();
    cfg.usable_accounts()
        .map(|a| AccountRow {
            label: a.label.clone(),
            name: a.display_name.clone().unwrap_or_default(),
            upn: a.upn.clone().unwrap_or_default(),
            active: a.label == cfg.account,
            unread: a.last_unread.unwrap_or(0),
            theme: themes.accounts.get(&a.label).cloned().unwrap_or_default(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgRow {
    pub tenant: String,
    pub name: String,
    pub home: bool,
    pub active: bool,
}

/// The organizations of the active account: the list read from Teams when there is one, else the
/// one cached in the config. The home org is active while the account signs in to
/// `organizations`.
pub fn org_rows(cfg: &session::Config, live: Option<&[Tenant]>) -> Vec<OrgRow> {
    let cached: Vec<session::OrgRef>;
    let orgs: Vec<(String, String, bool)> = match live {
        Some(t) => t
            .iter()
            .map(|t| (t.id.clone(), t.name.clone(), t.is_home))
            .collect(),
        None => {
            cached = cfg
                .active_account()
                .map(|a| a.orgs.clone())
                .unwrap_or_default();
            cached
                .iter()
                .map(|o| (o.tenant_id.clone(), o.name.clone(), o.is_home))
                .collect()
        }
    };
    orgs.into_iter()
        .map(|(tenant, name, home)| OrgRow {
            active: cfg.tenant == tenant
                || (home && cfg.tenant == teams_proto::client_info::DEFAULT_TENANT),
            tenant,
            name,
            home,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// What the engine reads from Teams.

/// An app that asks for device permissions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppAsk {
    pub id: String,
    pub name: String,
    pub requested: Vec<DevicePermission>,
}

/// The Teams side of Settings, as read by [`SettingsCmd::Load`].
#[derive(Debug, Clone, Default)]
pub struct ServerData {
    pub offline: bool,
    pub chat_service: String,
    pub middle_tier: String,
    pub notifications: Option<NotificationSettings>,
    pub privacy: Option<PrivacySettings>,
    /// `(mri, name)`.
    pub blocked: Vec<(String, String)>,
    pub priority: Vec<(String, String)>,
    pub tenants: Option<Vec<Tenant>>,
    pub me_name: String,
    pub me_email: Option<String>,
    pub me_status: Option<String>,
    /// When my status message clears (ms), `None` for never.
    pub me_status_ends: Option<i64>,
    pub me_presence: Option<String>,
    /// What I am doing (`InACall`, `InAMeeting`, `OffWork`, …), for the words of my status.
    pub me_activity: Option<String>,
    /// My work location pin as the presence service reports it (`office`, `remote`, a code).
    pub me_work_location: Option<String>,
    pub apps: Vec<AppAsk>,
    /// What could not be read, one line each.
    pub errors: Vec<String>,
}

/// Commands the engine runs for Settings and the profile menu. Reads, plus the user's own status
/// and work location (allowed on the work account since 2026-11-19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsCmd {
    /// Forces my status (`Available`, `Busy`, `DoNotDisturb`, `BeRightBack`, `Away`, `Offline`);
    /// an empty value resets it to automatic. The second value is when the status ends
    /// (`desiredExpirationTime`, UTC text), `None` for never. Then reads the profile menu again.
    SetStatus(String, Option<String>),
    /// Pins my work location for today: `office`, `remote`, or `clear`.
    SetWorkLocation(String),
    /// Sets my status message; an empty text clears it. Then when it is cleared (UTC text, `None`
    /// for never) and whether it also shows when people message me.
    SetStatusNote(String, Option<String>, bool),
    /// Opens a file picker and makes the picture my profile photo (Graph; UNVERIFIED).
    ChangePhoto,
    /// Removes my profile photo (Graph beta; UNVERIFIED).
    RemovePhoto,
    /// "When I open a chat take me to".
    SetOpenChatAt(session::OpenChatAt),
    /// Sends a Teams settings change (notifications, privacy, a contact list), then reads the
    /// settings again.
    Apply(teams_proto::user_settings::PlannedRequest),
    /// Read the Teams side of Settings and push it to the window.
    Load,
    /// "Refresh license and restart": redeem the refresh token again (new access tokens and a new
    /// skypetoken from the authz exchange, which is where Teams hands out the license-dependent
    /// service addresses), save the rotated token, then tell the window to restart.
    RefreshLicense,
    /// The date or time format changed: draw the rows that show times again.
    Redraw,
    /// The Storage page opened: read how much disk each kind of data takes and show it.
    LoadStorage,
    /// The media cache limit was written to the config: sweep to the new budget now, then show
    /// the sizes.
    ApplyMediaLimit,
    /// Storage "Delete old update backups": the updater's `backup-*` folders, not the live
    /// message cache.
    DeleteUpdateBackups,
}

/// A permission's label on the App permissions page.
pub fn permission_label(p: DevicePermission) -> &'static str {
    match p {
        DevicePermission::Media => "Camera and microphone",
        DevicePermission::Geolocation => "Location",
        DevicePermission::Notifications => "Notifications",
        DevicePermission::Midi => "MIDI devices",
        DevicePermission::OpenExternal => "Open external links",
    }
}

/// One app on the App permissions page: `(app id, name, [(permission, allowed)])`.
pub type PermissionRow = (String, String, Vec<(DevicePermission, bool)>);

/// App permission rows: every app that asks (from Teams) plus every app with a saved grant,
/// sorted by name.
pub fn permission_rows(
    asks: &[AppAsk],
    grants: &Grants,
    saved: &serde_json::Value,
) -> Vec<PermissionRow> {
    let mut rows: Vec<PermissionRow> = asks
        .iter()
        .map(|a| {
            (
                a.id.clone(),
                a.name.clone(),
                grants.for_app(&a.id, &a.requested),
            )
        })
        .collect();
    // Apps with a saved grant that Teams did not list (offline, or since uninstalled).
    for (app, perms) in saved.as_object().into_iter().flatten() {
        if rows.iter().any(|(id, _, _)| id == app) {
            continue;
        }
        let requested: Vec<DevicePermission> = perms
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(p, _)| DevicePermission::parse(p))
            .collect();
        rows.push((app.clone(), app.clone(), grants.for_app(app, &requested)));
    }
    rows.sort_by_key(|r| r.1.to_lowercase());
    rows
}
