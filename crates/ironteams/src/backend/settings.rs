//! Settings data (plans/settings-and-platform.md X1 and X2.1, chat plan 9.1, 9.3 and 9.9).
//!
//! Two halves:
//! * [`Local`]: preferences kept on this machine (`appearance.json`, `general.json`,
//!   `accessibility.json` from the `appearance` crate, plus `devices.json`, `files.json` and
//!   `app-grants.json` written here, the file the web host reads), the colour palette they give, and "Find in Settings".
//!   The UI thread owns it (`ui/settings.rs`).
//! * [`SettingsCmd::Load`]: the engine reads the Teams side (notification and privacy settings,
//!   blocked and priority people, organizations, profile, apps that ask for permissions). Reads
//!   only: every Teams *change* is turned into a `PlannedRequest` by the UI and shown as a dry
//!   run (`dry_run_*` below); nothing here sends one.

use super::Engine;
use super::call_feedback;
use app_store::{DevicePermission, Grants};
use appearance::{
    AccessibilityPrefs, AppearancePrefs, AutoSwitch, DateFormat, GeneralPrefs, PortalSettings,
    ResolvedTheme, Rgb, TimeFormat,
};
use devices::DevicePrefs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use teams_proto::mentions::{Mention, MentionKind};
use teams_proto::tenants::Tenant;
use teams_proto::user_settings::{
    self as us, Change, ContactList, Delivery, Kind, MeetingChatMode, NotificationSettings,
    PlannedRequest, PrivacyFlag, PrivacySettings, SoundField,
};

mod prefs;
pub use prefs::*;
mod look;
pub use look::*;
mod rows;
pub use rows::*;
mod index;
pub use index::*;
mod notifications;
pub use notifications::*;
mod accounts;
pub use accounts::*;
mod engine;

pub const DEVICES_FILE: &str = "devices.json";
pub const FILES_FILE: &str = "files.json";
pub const CALLS_FILE: &str = "calls.json";
/// The per-app grants. The same file the Store settings write and the web host reads
/// (`appstore/webhost_open.rs`), in the `app_store::Grants::to_json` shape.
pub const PERMISSIONS_FILE: &str = "app-grants.json";
/// Where the first Settings build wrote the grants; read once when the new file does not exist.
const LEGACY_PERMISSIONS_FILE: &str = "app-permissions.json";

#[cfg(test)]
mod tests;
