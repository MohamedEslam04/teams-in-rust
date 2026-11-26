//! UI-thread side of Settings and the profile menu: fills the `SettingsStore` global, applies the
//! appearance and accessibility preferences to the `Theme` global (live, including desktop portal
//! changes), saves local preferences, and turns Teams changes into dry runs.
//!
//! Nothing here talks to Teams. Reads go through the engine (`SettingsCmd::Load`); every Teams
//! write is built as a `PlannedRequest` and sent by the engine (`SettingsCmd::Apply`), which then
//! reads the settings again. Without an engine (tests) a write stays a dry run, logged and shown.
//!
//! Commands from `SettingsStore.command(name, argument)`:
//! open, close, page, query, profile, theme, theme-pick, theme-mode, theme-slot, theme-time,
//! reset-theme, font-code, font-ui, theme-new, theme-edit, theme-import, theme-copy, theme-rename, theme-delete,
//! editor-hex, editor-lch, editor-name, editor-keep, editor-save, editor-cancel (`themes.rs`),
//! accent, density, message-style, show-avatars, show-timestamps, show-reactions,
//! font-step, font-scale, skin-tone,
//! language, date-format, time-format, time-zone, startup, renderer, high-contrast, reduced-motion,
//! larger-text, a11y, rail-move, rail-show, rail-auto, notify, pause, test-notification, privacy, unblock, priority-remove,
//! speaker, mic, camera, noise-suppression, echo-cancellation, all-devices, test-sound, mic-test,
//! call-feedback, ringtone, ringtone-preview, ringtone-file, message-sound, message-sound-preview,
//! message-sound-file, urgent-sound, urgent-sound-preview, teams-sounds, open-files, perm, switch-account, switch-org, switch-confirm, switch-cancel, add-account, remove-account,
//! refresh-license, restart, sign-out, view-account, report-problem, check-now,
//! release-notes, status, work-location, status-note, notification-style, banner-photo, banner-actions, notify-sound,
//! meeting-chat, check-spelling, quiet-on, quiet-day, quiet-start, quiet-end, focus,
//! focus-duration, keyword-add, keyword-remove, open-chat-at, link-previews, photo-change,
//! photo-remove, check-updates-menu, beta-updates, restart-update,
//! media-cache-limit, clear-media-cache, clear-all-cache, delete-update-backups.

use super::{UiHandle, emoji_model, model, rgba};
use crate::backend::Cmd;
use crate::backend::call_feedback::Ask;
use crate::backend::expiry;
use crate::backend::people_presence;
use crate::backend::ringtone::RingtoneChoice;
use crate::backend::settings::{
    self as st, DeviceLists, Local, Look, OpenFilesIn, ServerData, SettingsCmd,
};
use crate::backend::sounds::teams_assets::Slot;
use crate::backend::sounds::{self, Kind as SoundKind};
use crate::update_check::{self, Release};
use crate::updater::{self, Event, Phase};
use crate::{
    AppWindow, AppsStore, RailFixed, SAccount, SAdjusted, SAppPerm, SBaseRow, SDevice, SFind,
    SNotify, SOption, SOrg, SPairRow, SPerson, STheme, SToggle, SettingsStore, Theme,
};
use app_store::DevicePermission;
use appearance::{Density, MessageStyle, PortalChange, PortalSettings, Rgb};
use session::OpenChatAt;
use slint::{ComponentHandle, ModelRc, SharedString};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use teams_proto::user_settings::{
    ContactList, Delivery, Kind, MeetingChatMode, NotificationSettings, PrivacyFlag, SoundField,
};

mod hooks;
pub use hooks::*;
mod install;
pub use install::*;
mod push;
pub(super) use push::*;
// The UI tests play desktop portal changes.
#[cfg(test)]
pub(crate) use push::portal_changed;
mod misc;
use misc::*;
mod updates;
pub use updates::*;
mod command;
use command::*;
mod fonts;
mod switch;
mod themes;
pub use themes::route_theme_preview;

/// Polls a running microphone test; `None` when there is no new reading.
pub type MicPoll = Box<dyn FnMut() -> Option<f32>>;

/// Opens a microphone (`None`: the default one) for the level meter. Blocking (it asks the sound
/// server): it runs on the meter's worker thread, never on the UI thread.
pub type OpenMic = Arc<dyn Fn(Option<&str>) -> Result<MicPoll, String> + Send + Sync>;

struct Ctl {
    env: SettingsEnv,
    local: Local,
    portal: Option<PortalSettings>,
    server: Option<ServerData>,
    tx: Option<tokio::sync::mpsc::UnboundedSender<Cmd>>,
    last_load: Option<Instant>,
    dry_runs: Vec<String>,
    lists: Option<DeviceLists>,
    listing: bool,
    mic: Option<slint::Timer>,
    /// The worker that listens to the microphone while `mic` runs.
    mic_probe: crate::support::meter::Probe,
    /// An update check is running.
    checking: bool,
    /// The one automatic check of this start has been made.
    update_auto_done: bool,
    /// The release page of the newer version found, for "Release notes".
    update_url: Option<String>,
    /// Self-updates (Velopack installs): the state and the schedule.
    updater: updater::Core,
    /// Runs the automatic update checks (kept alive here).
    _update_timer: Option<slint::Timer>,
    /// What the button of the banner does.
    banner_action: BannerAction,
    /// "When I open a chat take me to", as the engine has it.
    open_chat_at: OpenChatAt,
    /// The status picked here with an end ("Reset status after"): `(availability, end ms)`, for
    /// the "Until 3:45 PM" line while the service still reports that status.
    status_until: Option<(String, i64)>,
    /// What the menu showed before a status pick (presence, activity, its "Until" line), put back
    /// when the service refuses the change.
    status_before: Option<(String, String, String)>,
    /// The account switch being decided (`switch`).
    switching: Option<switch::Pending>,
    /// The last look applied to the windows (`look_changed`), `None` before the first.
    last_look: Option<Look>,
    /// The active account's label (`session::Config.account`): its own theme wins over the app
    /// choice. Known at start; switching accounts restarts IronTeams.
    account: Option<String>,
    /// The theme fallbacks already logged, so each is logged once.
    logged_fallbacks: HashSet<appearance::Fallback>,
    /// Runs `look_changed` once a minute while a schedule picks the theme.
    schedule_timer: Option<slint::Timer>,
    /// The custom theme being edited or previewed (`themes.rs`).
    editor: Option<themes::Editor>,
}

/// What the button of the banner does, for the banners Settings shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BannerAction {
    None,
    ReleaseNotes,
    RestartToUpdate,
    GoBack(String),
}

type Shared = Rc<RefCell<Ctl>>;

thread_local! {
    static CTL: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

fn ctl() -> Option<Shared> {
    CTL.with(|c| c.borrow().clone())
}

/// How many theme fallbacks were logged (each one once).
#[cfg(test)]
pub(crate) fn logged_fallback_count() -> usize {
    ctl().map_or(0, |c| c.borrow().logged_fallbacks.len())
}

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}
