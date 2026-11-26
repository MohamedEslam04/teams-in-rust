# backend/settings

## Overview
Settings data and Engine handling: preference structs with atomic JSON persistence, theme palettes, notification settings, account and org rows, settings page rows, and the "Find in Settings" index.

Files:
- `accounts.rs`: Account/org row builders, ServerData (Teams-side settings snapshot), SettingsCmd enum, app-permission rows.
- `engine.rs`: impl Engine: runs SettingsCmd, reloads/reads Teams settings, sets presence/work location, license refresh, 10-min notification-prefs poller.
- `index.rs`: Settings page list, 'Find in Settings' keyword INDEX and find(), device list types and audio/camera row builders.
- `look.rs`: Theme palettes (dark/light/high-contrast), accent mixing, Look resolution from local prefs + portal, font-scale stepping, slug converters.
- `notifications.rs`: Global remembered notification settings/priority people, incoming message kind classification, banner decision (mute/pause/delivery/preview), work-location pin helpers.
- `prefs.rs`: Files/Calls prefs, link-previews flag, atomic JSON read/write, Local struct loading and saving every local preference file.
- `rows.rs`: Pure builders of settings-page rows (toggles, notify, privacy, sound, meeting chat) from prefs/Teams settings, plus dry-run builders that clone settings, apply one change and return a PlannedRequest without sending.

## Interface
- `[accounts.rs:AccountRow]` pub struct {label,name,upn,active}
- `[rows.rs:ToggleRow]` pub struct {key,label,hint: String; on,enabled: bool}; Debug/Clone/PartialEq/Eq
- `[accounts.rs:account_rows]` pub fn(&session::Config) -> Vec<AccountRow>; syncs active on a clone
- `[rows.rs:toggle]` pub(super) fn toggle(key,label,hint:&str,on,enabled:bool)->ToggleRow
- `[accounts.rs:OrgRow]` pub struct {tenant,name,home,active}
- `[rows.rs:startup_rows]` pub fn startup_rows(&GeneralPrefs)->Vec<ToggleRow> (keys auto-start, open-in-background, keep-running)
- `[accounts.rs:org_rows]` pub fn(&Config, Option<&[Tenant]>) -> Vec<OrgRow>; live list else cached active-account orgs
- `[rows.rs:set_startup]` pub fn set_startup(&mut GeneralPrefs,key,on)->bool; false for unknown key
- `[accounts.rs:AppAsk]` pub struct {id,name,requested: Vec<DevicePermission>}
- `[rows.rs:a11y_rows]` pub fn a11y_rows(&AccessibilityPrefs)->Vec<ToggleRow> (7 toggles)
- `[accounts.rs:ServerData]` pub struct (Default) of Teams-side settings: notifications, privacy, blocked, priority, tenants, me_* presence fields, apps, errors
- `[rows.rs:set_a11y_flag]` pub fn set_a11y_flag(&mut AccessibilityPrefs,key,on)->bool; false for unknown key
- `[accounts.rs:SettingsCmd]` pub enum: SetStatus, SetWorkLocation, SetStatusNote, ChangePhoto, RemovePhoto, SetOpenChatAt, Apply(PlannedRequest), Load, RefreshLicense, Redraw, LoadStorage, ApplyMediaLimit, DeleteUpdateBackups
- `[rows.rs:NOTIFY]` pub(super) const [(Kind,group,label);19] notification types in Teams order
- `[accounts.rs:permission_label]` pub fn(DevicePermission) -> &'static str
- `[rows.rs:delivery_slug]` pub fn delivery_slug(Delivery)->&'static str (banner|feed|off)
- `[accounts.rs:PermissionRow]` pub type (app id, name, Vec<(DevicePermission,bool)>)
- `[rows.rs:NotifyRow]` pub struct {key,label,group,value: String}; group set on first row of each group only
- `[accounts.rs:permission_rows]` pub fn(&[AppAsk], &Grants, &serde_json::Value) -> Vec<PermissionRow>; sorted by lowercase name, includes saved-only apps
- `[rows.rs:notify_rows]` pub fn notify_rows(Option<&NotificationSettings>)->Vec<NotifyRow>; value empty when unknown
- `[engine.rs:Engine::settings_cmd]` pub(in super::super) async fn(self: &Arc<Self>, SettingsCmd) -> Result<()>; entry point for settings commands
- `[rows.rs:privacy_label]` pub fn privacy_label(PrivacyFlag)->(label,hint)
- `[engine.rs:Engine::settings_reload]` pub(super) async fn(&Arc<Self>, remember: bool)
- `[rows.rs:privacy_rows]` pub fn privacy_rows(Option<&PrivacySettings>)->Vec<ToggleRow>; off+disabled when None
- `[engine.rs:Engine::start_notify_prefs]` pub(in super::super) fn; spawns 10-minute poller
- `[rows.rs:SOUND_SWITCHES]` pub(super) const &[(SoundField,label,hint)] 5 sound/preview switches
- `[engine.rs:Engine::display_name]` pub(super) fn(&self, mri) -> String; store name or MRI tail
- `[rows.rs:sound_rows]` pub fn sound_rows(Option<&NotificationSettings>)->Vec<ToggleRow>; on iff value eq_ignore_ascii_case "on"; disabled when None
- `[engine.rs:Engine::settings_read]` pub(super) async fn -> ServerData
- `[rows.rs:dry_run_sound]` pub fn dry_run_sound(chat_service:&str,&NotificationSettings,SoundField,on)->PlannedRequest
- `[engine.rs:Engine::set_my_presence]` pub(super) async fn(&SettingsCmd) -> Result<()>
- `[rows.rs:meeting_chat_slug]` pub fn meeting_chat_slug(Option<&NotificationSettings>)->&'static str ("" if unknown)
- `[engine.rs:Engine::refresh_license]` pub(super) async fn -> Result<()>
- `[rows.rs:dry_run_meeting_chat]` pub fn dry_run_meeting_chat(chat_service,&NotificationSettings,MeetingChatMode)->PlannedRequest
- `[index.rs:PAGES]` pub const [(&str,&str);14] settings pages (a `storage` page sits between `files` and `permissions`; the `shortcuts` page, Keyboard shortcuts, follows `accessibility`)
- `[rows.rs:dry_run_text]` pub fn dry_run_text(&PlannedRequest)->String (PlannedRequest::dry_run)
- `[index.rs:page_title]` pub fn(&str) -> &'static str; falls back to "Settings"
- `[rows.rs:dry_run_notify]` pub fn dry_run_notify(chat_service,&NotificationSettings,Kind,Delivery)->PlannedRequest
- `[index.rs:INDEX]` pub(super) const &[(page,label,keywords)]; covers the Storage page (media cache limit, clear media cache, delete old update backups, clear all cached data) as well as the other pages
- `[rows.rs:dry_run_pause]` pub fn dry_run_pause(chat_service,&NotificationSettings,until:Option<String>)->PlannedRequest
- `[index.rs:Found]` pub struct {page,page_title,label}
- `[rows.rs:dry_run_privacy]` pub fn dry_run_privacy(chat_service,PrivacyFlag,on)->PlannedRequest via us::privacy_flag_request
- `[index.rs:find]` pub fn(&str) -> Vec<Found>; all words must match label+page title+keywords
- `[rows.rs:dry_run_remove]` pub fn dry_run_remove(middle_tier,ContactList,mri)->PlannedRequest via us::manage_request(Change::Remove)
- `[index.rs:DeviceLists]` pub struct (Default) {inputs,outputs,cameras,error}
- `[index.rs:DeviceRow]` pub type (id,name)
- `[index.rs:audio_rows]` pub fn(&[AudioDevice], all, chosen) -> Vec<DeviceRow>; hides raw ALSA hw ids unless all/chosen
- `[index.rs:camera_rows]` pub fn(&[Camera]) -> Vec<DeviceRow> (bus,name)
- `[look.rs:Palette/DARK/LIGHT/CONTRAST_DARK/CONTRAST_LIGHT]` pub 0xrrggbb palette struct and constants; fields include `code_bg` (code block background)
- `[look.rs:palette]` pub fn(ResolvedTheme, high_contrast, Option<Rgb>) -> Palette
- `[look.rs:Look/look]` pub struct and fn(&Local, Option<&PortalSettings>) -> Look
- `[look.rs:step_font_scale/FONT_STEP]` pub fn(f32,i32)->f32 and 0.05 step; plus pub slug fns switch_slug/switch_from_slug/date_slug/date_from_slug/time_slug/time_from_slug
- `[notifications.rs:remember_notification_settings/remember_priority_people/is_priority_person]` pub store/query of global settings; plus pub(in super::super) remembered_notification_settings

## Invariants
- `[engine.rs:Engine::settings_cmd Apply/SetStatus/SetWorkLocation/SetStatusNote]` writes take self.settings_writes lock one at a time, in order, because the settings bag is written whole
- `[rows.rs:set_startup]` key strings auto-start/open-in-background/keep-running must match startup_rows keys; unknown key returns false and leaves prefs untouched
- `[engine.rs:Engine::settings_reload]` settings_reads counter ticket (SeqCst); a read superseded by a newer one is dropped
- `[rows.rs:set_a11y_flag]` key strings must match a11y_rows keys (7 flags); unknown key returns false
- `[engine.rs:set_my_presence/refresh_license/Apply]` network commands bail with 'Not available offline.' when self.offline is set
- `[rows.rs:NOTIFY]` fixed length 19 array; order is Teams display order; notify_rows group header relies on consecutive same-group entries
- `[engine.rs:Engine::settings_read]` offline returns early with only local fields; sub-read failures become errors lines, not hard failures
- `[rows.rs:notify_rows]` group string emitted only when it differs from previous row's group
- `[engine.rs:start_notify_prefs]` polls every 600 s; read-only, no write
- `[rows.rs:privacy_rows]` with p=None all rows off and enabled=false
- `[notifications.rs:NOTIFY_SETTINGS/PRIORITY_PEOPLE]` std Mutex statics; lock poison recovered via into_inner; None settings means every message shows with preview
- `[rows.rs:sound_rows]` with n=None all rows off and enabled=false; Teams stores values as "On"/"Off" strings
- `[notifications.rs:banner_decision]` order: tray mute, then Teams pause (paused_until RFC3339 > now), then per-kind delivery; None delivery shows; preview from toastSettings/ShowPreviewInToasts default true
- `[rows.rs:dry_run_sound]` writes "On"/"Off" capitalized, matching Teams format
- `[notifications.rs:notifications_paused]` the only pause check quiet::should_banner relies on; uses Kind::ChatMessage and counts only Muted/Paused
- `[rows.rs:dry_run_notify]` current settings are cloned, not mutated: Teams keeps the old value
- `[notifications.rs:incoming_kind]` channel ids end in @thread.tacv2; mention of me beats other kinds; person mention only counts if mri == me
- `[rows.rs:dry_run_pause]` until=None ends the pause; dry-run builders never send
- `[prefs.rs:write_json]` atomic write: temp file .name.pid.tmp in same dir then rename; temp removed on rename error
- `[prefs.rs:Local::set_grant]` re-reads app-grants.json before change so Store-saved grants are kept (both write the whole file)
- `[prefs.rs:Local::save_general]` overrides focus_until_ms with quiet::focus_override() because tray may have changed Focus mode

## Side Effects
- `[prefs.rs:write_json/Local::save_*]` creates config dir and writes/renames JSON prefs files (devices, files, calls, sounds, permissions, appearance, general, a11y)
- `[prefs.rs:read_json/files_prefs/calls_prefs/Local::load]` reads files.json/calls.json and other pref files from the config dir on each call
- `[engine.rs:Engine::settings_cmd Apply]` sends planned Teams settings write via client.send_planned, then re-reads settings
- `[engine.rs:Engine::set_my_presence]` network calls to set/reset presence, status note, and calendar::set_work_location
- `[engine.rs:Engine::settings_read]` parallel Teams network reads (properties, contacts, tenants, profiles, presence, installed apps)
- `[engine.rs:Engine::refresh_license]` client.refresh_all and session::persist (rotated token saved to disk)
- `[engine.rs:Engine::start_notify_prefs]` tokio::spawn infinite background polling task
- `[engine.rs:Engine::settings_cmd SetOpenChatAt]` mutates cfg under mutex and save_cfg to disk
- `[notifications.rs:NOTIFY_SETTINGS/PRIORITY_PEOPLE and prefs.rs:LINK_PREVIEWS]` process-global statics (Mutex/AtomicBool) written via remember_* / set_link_previews
- `[engine.rs:Engine::settings_cmd LoadStorage/ApplyMediaLimit/DeleteUpdateBackups]` LoadStorage reads cache sizes (cache_ops.rs:Engine::push_storage) and pushes ui.settings_storage; ApplyMediaLimit runs sweep_cache (deletes media files down to the configured budget) then pushes the sizes; DeleteUpdateBackups deletes the updater's backup-* folders under the data dir via cache::clear_backups, then shows a notice and the sizes
- `[engine.rs:Engine::settings_cmd]` pushes UI updates (ui.settings_notice/server/license/status_failed), marks dirty flags, calls push_calendar/remind_push/push_scheduled/push_calls_page

## Verification
- Run: `cargo test -p ironteams`
