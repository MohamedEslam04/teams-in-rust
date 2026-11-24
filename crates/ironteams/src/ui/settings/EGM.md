# ui/settings

## Overview
Settings UI controller parts: a single command dispatcher for every SettingsStore command (`command.rs`), injectable side-effect seams (`hooks.rs`), wiring into the AppWindow (`install.rs`), command-handler helpers (`misc.rs`), pushing prefs/theme/server data/accounts into Slint (`push.rs`) and the update check and self-update flow (`updates.rs`).

Files:
- `command.rs`: Single `command(shared, window, name, arg)` dispatcher for every SettingsStore command (appearance, general, a11y, notifications/privacy dry runs, devices, sounds, accounts, updates, focus/quiet hours, profile status, sign-out, report), U+001F-separated args.
- `hooks.rs`: Injectable side-effect seams: Autostart (desktop entry), Hooks (notification, open_url, restart, sign-out, report, device enumeration, sounds, mic, update check, quit), SettingsEnv with real `system()` wiring.
- `install.rs`: Wires Settings into the AppWindow: loads Local prefs, builds the shared Ctl, registers SettingsStore.on_command, banner/activity callbacks, update checks and portal watcher; also show_focus, focus_slug, bit helpers.
- `misc.rs`: Helpers for command handlers: notice/saved, Teams write (write_or_plan/dry_run), request_load throttle, background device listing, mic test timer, sound file picker, account cache removal, license/report/sign-out completions.
- `push.rs`: Pushes local prefs, portal theme, server (Teams) data, profile, accounts and devices into the Slint Theme/SettingsStore globals; portal read/watch; UiHandle settings_* notices.
- `updates.rs`: Update-check (About page notice) and self-update flow: check, download thread, banners, restart-to-update, apply on quit.

## Interface
- `[command.rs:command]` pub(super) fn command(&Shared, &AppWindow, name:&str, arg:&str); the SettingsStore command dispatcher, UI thread only
- `[push.rs:read_portal]` pub(super) fn() -> Option<PortalSettings>; blocking read of xdg portal with 400ms timeout on a temp current-thread runtime
- `[hooks.rs:PlaySound]` pub type Arc<dyn Fn(Option<String>)->Result<(),String>+Send+Sync>
- `[push.rs:watch_portal]` pub(super) fn(Weak<AppWindow>); spawns thread 'ironteams-portal' streaming portal changes to portal_changed
- `[hooks.rs:Autostart]` pub struct { set: Box<dyn Fn(bool)->Result<(),String>>, get: Box<dyn Fn()->bool> }
- `[push.rs:portal_changed]` pub(super) fn(&AppWindow, PortalChange); applies change to ctl().portal then apply_look
- `[hooks.rs:Autostart::desktop]` pub fn(config_home: Option<PathBuf>) -> Autostart; XDG autostart entry launching with --hidden, or under config_home (tests)
- `[push.rs:apply_look]` pub(super) fn(&AppWindow, &Ctl); sets Theme global colors/flags and SettingsStore theme/accent/a11y 'shown' strings
- `[hooks.rs:Hooks]` pub struct of seams: autostart, test_notification, open_url, restart, signed_out, report_problem, enumerate, test_sound, play_sound, pick_sound_file, mic_level, check_update, updates, quit
- `[push.rs:apply_general]` pub(super) fn(&GeneralPrefs); sets date/time formats, language, tray keep_running, link previews
- `[hooks.rs:SettingsEnv]` pub struct { paths, tokens, portal, background, apply_globals, crash_loop, hooks }
- `[push.rs:toggles]` pub(super) fn(Vec<st::ToggleRow>) -> ModelRc<SToggle>
- `[hooks.rs:SettingsEnv::system]` pub fn() -> Option<SettingsEnv>; real environment (None if no config dir)
- `[push.rs:push_local]` pub(super) fn(&AppWindow, &Ctl); pushes every local preference into SettingsStore, then push_devices/push_perms
- `[install.rs:install_system]` pub fn(&AppWindow, UnboundedSender<Cmd>); called from main, warns and leaves Settings empty if no env
- `[push.rs:push_devices]` pub(super) fn(&AppWindow, &Ctl); mics/speakers/cameras/status from c.lists
- `[install.rs:install]` pub fn(&AppWindow, Option<UnboundedSender<Cmd>>, SettingsEnv); loads prefs, builds Ctl, registers callbacks (tests use custom env)
- `[push.rs:push_perms]` pub(super) fn(&AppWindow, &Ctl); app permission rows from server asks + local grants
- `[install.rs:show_focus]` pub fn(&AppWindow, until_ms: Option<i64>); sets focus_on/focus_end_kind/focus_end_time on SettingsStore
- `[push.rs:push_server]` pub(super) fn(&AppWindow, &Ctl); notifications, privacy, blocked/priority, status text, dry runs, profile, perms
- `[install.rs:focus_slug]` pub(super) fn(FocusDuration)->&'static str (1h/2h/tomorrow/manual)
- `[push.rs:config]` pub(super) fn(&Ctl) -> Option<session::Config>; loads config via env.paths, logs warn on error
- `[install.rs:bit]` pub(super) fn(&str)->bool; true for "1"/"true"
- `[push.rs:status_until_line]` pub(super) fn(&Ctl, &str) -> String; 'Until ...' label while status_until still in the future and matches presence
- `[misc.rs:file_label]` pub(super) fn(Option<&str>)->String; file name of a sound path
- `[push.rs:show_my_status]` pub(in super::super) fn(&SettingsStore, presence, activity); sets status label/activity/presence
- `[misc.rs:SoundFor]` pub(super) enum { Message, Ringtone }
- `[push.rs:push_profile]` pub(super) fn(&AppWindow, &Ctl); name/email/status/presence/work location/initials
- `[misc.rs:choose_sound_file]` pub(super) fn(&Shared,&AppWindow,SoundFor); picker, validates with sounds::check_file, saves Custom choice
- `[push.rs:push_accounts]` pub(super) fn(&AppWindow, &Ctl); accounts and orgs lists, or error status if config unreadable
- `[misc.rs:notice]` pub(super) fn(&AppWindow, impl Into<SharedString>); sets SettingsStore.notice
- `[push.rs:UiHandle::settings_notice]` pub fn(&self, String); shows notice on UI thread
- `[misc.rs:write_or_plan]` pub(super) fn(&AppWindow,&mut Ctl,PlannedRequest); sends SettingsCmd::Apply via tx, else dry_run
- `[push.rs:UiHandle::settings_photo]` pub fn(&self, Option<data::Pixels>); sets profile photo
- `[misc.rs:dry_run]` pub(super) fn(&AppWindow,&mut Ctl,PlannedRequest); logs and keeps last 5 plans, re-pushes server state
- `[push.rs:UiHandle::settings_license]` pub fn(&self, Result<(),String>); forwards to license_done
- `[misc.rs:saved]` pub(super) fn(&AppWindow, anyhow::Result<()>); notice 'Saved.' or error
- `[push.rs:UiHandle::settings_server]` pub fn(&self, ServerData); stores into ctl().server, push_server + push_accounts
- `[misc.rs:request_load]` pub(super) fn(&mut Ctl, force:bool); sends SettingsCmd::Load, throttled to 60 s
- `[push.rs:UiHandle::settings_status_failed]` pub fn(&self, String); restores prior status display and shows banner
- `[misc.rs:list_devices]` pub(super) fn(&AppWindow,&Shared); enumerates devices once (thread when background)
- `[updates.rs:check_updates]` pub(super) fn(&AppWindow, &mut Ctl, force: bool); starts an update-notice check (thread when env.background, else inline)
- `[misc.rs:start_mic]` pub(super) fn(&AppWindow,&mut Ctl); starts 50 ms polling timer, auto-stops after 15 s
- `[updates.rs:update_checked]` pub(super) fn(&AppWindow, &mut Ctl, Result<Release,String>); shows status and info banner if newer
- `[misc.rs:stop_mic]` pub(super) fn(&AppWindow,&mut Ctl); drops timer, resets mic_testing/mic_level
- `[updates.rs:banner]` pub(super) fn(&AppWindow, &mut Ctl, String, &str, BannerAction); sets info banner and banner_action
- `[misc.rs:with_server_note]` pub(super) fn(&Ctl)->Option<(String,NotificationSettings)>; chat_service + notification settings if read
- `[updates.rs:update_line]` pub fn(&Phase) -> String; fixed status sentences per updater phase
- `[misc.rs:remove_account_cache]` pub(super) fn(&Paths,&Account); deletes db file + -wal/-shm unless the active account shares it
- `[updates.rs:push_update]` pub(super) fn(&AppWindow, &Ctl); shows self-update state (no-op without hooks.updates)
- `[misc.rs:license_done]` pub(super) fn(&AppWindow,Result<(),String>); on Ok calls hooks.restart
- `[updates.rs:install_self_update]` pub(super) fn(&AppWindow, &mut Ctl); startup: restore pending update, crash-loop banner, 30s due-timer
- `[misc.rs:report_done]` pub(super) fn(&AppWindow,Result<String,String>); notice with report path
- `[updates.rs:ready_banner]` pub(super) fn(&AppWindow, &mut Ctl); 'Restart to update' banner if an update is ready
- `[misc.rs:signed_out]` pub(super) fn(&AppWindow,session::Result<()>); on Ok calls hooks.signed_out, else reopens profile with error
- `[updates.rs:start_update]` pub(super) fn(&AppWindow, &mut Ctl, Option<String>); check+download (or rollback to given version) on a thread
- `[updates.rs:update_event]` pub(super) fn(&AppWindow, &mut Ctl, Event); applies one updater event; downgrade ready triggers restart_to_update
- `[updates.rs:restart_to_update]` pub(super) fn(&AppWindow, &mut Ctl); apply_on_exit(true) then hooks.quit, else apply_failed and clear banner
- `[updates.rs:apply_update_on_quit]` pub fn(); called from main after event loop; hands a ready update to apply_on_exit(false)

## Invariants
- `[command.rs:command]` Runs on the UI thread; shared Ctl is an Rc<RefCell> borrowed per arm (borrow_mut); args split on U+001F
- `[push.rs:portal_changed]` Runs on UI thread via upgrade_in_event_loop; ctl() may be None (early return); borrows Ctl RefCell mutably
- `[command.rs:"open"|"page"]` Mic test is stopped on any page other than "devices"; "close" also stops it; changing the mic device stops a running test
- `[push.rs:read_portal]` Portal read bounded by 400ms timeout so first frame is never blocked long; failure yields None (default theme)
- `[command.rs:"switch-account"|"remove-account"|"add-account"]` Config changes go through paths.update_config (config lock) because the running engine saves its own prefs to the same file; they only set restart_needed, never switch live
- `[push.rs:push_server]` Status text: None server -> 'Reading your Teams settings…', offline -> offline message; shown only when notifications/privacy missing or errors present
- `[command.rs:"remove-account"]` Tokens are forgotten first (session::forget_account_tokens), then config updated; message cache removed only on success
- `[push.rs:push_local]` Enabling check_spelling triggers spell::ensure_loaded; time format also mirrored to Theme.h24
- `[command.rs:"notify"|"pause"|"privacy"|"unblock"]` Teams settings writes require server data to have been read (with_server_note / privacy.is_some()); otherwise refused with a notice
- `[push.rs:settings_status_failed]` c.status_until cleared and status_before taken (restored) on refusal
- `[command.rs:"general" arm]` autostart entry is set only if saving general prefs succeeded (r.is_ok())
- `[updates.rs:check_updates]` No check if hooks.updates is Some (self-updating), check_updates off, one already running (c.checking), or auto check already done and !force
- `[command.rs:"date-format"|"time-format"|"link-previews"]` Engine Redraw is sent so already drawn rows pick up the change; globals applied only when env.apply_globals
- `[updates.rs:check_updates]` update_auto_done set only for non-forced (automatic) checks, so at most one automatic check per start
- `[command.rs:"pause"]` Legacy args "0"/"" mean off and "1" means tomorrow; unknown expiry returns an error notice and re-pushes server state
- `[updates.rs:update_checked]` If check_updates switched off mid-flight the answer is discarded (status cleared)
- `[install.rs:install]` Autostart state from hooks.autostart.get overrides stored general.auto_start; CTL thread-local holds the shared Ctl for async callbacks (ctl())
- `[updates.rs:update_checked]` Newer-ness decided by update_check::is_newer(CARGO_PKG_VERSION, rel.version)
- `[install.rs:install]` Update check runs once per start and only if opted in (check_updates); portal watcher only when env.portal && env.background
- `[updates.rs:install_self_update]` pending update takes precedence over crash-loop-paused banner; timer only started when env.background; timer stored in c._update_timer to keep it alive
- `[misc.rs:request_load]` Load requests throttled to once per 60 s unless forced; no-op without engine tx
- `[updates.rs:start_update]` updater.begin(Instant) gates concurrent runs; if refused only push_update; background thread else inline (tests)
- `[misc.rs:start_mic]` Mic test timeout is 15 s, poll interval 50 ms; [list_devices] only one enumeration at a time (lists/listing guards)
- `[updates.rs:apply_update_on_quit]` take_for_apply ensures an update is handed over once; failure goes to updater.apply_failed

## Side Effects
- `[hooks.rs:Autostart::desktop]` Writes/removes the XDG autostart .desktop entry (tray::autostart) pointing to current_exe with --hidden
- `[push.rs:read_portal]` Builds a tokio current-thread runtime and does an xdg desktop portal (D-Bus) read
- `[hooks.rs:SettingsEnv::system test_notification]` Spawns a thread showing a desktop card (desktop_cards client) or notify_rust notification
- `[push.rs:watch_portal]` Spawns thread 'ironteams-portal' with its own runtime subscribing to portal D-Bus change stream
- `[hooks.rs:SettingsEnv::system restart]` Spawns a new process of current_exe and quits the Slint event loop; signed_out and quit also quit the event loop
- `[push.rs:apply_look]` Writes Theme and SettingsStore Slint globals (UI state)
- `[hooks.rs:SettingsEnv::system open_url]` Opens URLs via crate::external::open; [command.rs:"view-account"] opens myaccount.microsoft.com
- `[push.rs:apply_general]` Sets global state: appearance::formats, language, tray keep_running, link previews
- `[hooks.rs:SettingsEnv::system pick_sound_file]` Blocking rfd native file dialog
- `[push.rs:push_local]` Calls iana_time_zone::get_timezone, reads system language, may load spell dictionaries
- `[hooks.rs:SettingsEnv::system]` Audio/camera device enumeration, test sound playback, microphone level capture (devices, sounds crates); report_problem writes a diagnostics file via diag::report_from_args (nothing uploaded)
- `[push.rs:config]` Reads config file from disk via env.paths.load_config; tracing warn on failure
- `[command.rs:command]` Persists local preferences to disk via c.local.save_* (appearance, general, a11y, devices, calls, sounds, files); updates config file via paths.update_config
- `[updates.rs:check_updates]` Spawns std::thread running hooks.check_update (network) when background
- `[command.rs:"focus"]` Sets process-global focus state (support::quiet::set_focus) and updates tray via tray_wiring::apply_focus
- `[updates.rs:start_update]` Spawns std::thread running updater::run_cycle/run_rollback (network download)
- `[command.rs:"test-sound"|"sign-out"|"report-problem"]` Spawn std threads when env.background, reply via weak.upgrade_in_event_loop; sign-out deletes stored tokens from the keyring
- `[updates.rs:install_self_update]` Starts repeating 30s slint::Timer; reads pending update from disk via src.pending()
- `[misc.rs:remove_account_cache]` Deletes the account's SQLite db files; [write_or_plan/request_load] send Cmd::Settings to the engine channel (network writes to Teams); [install.rs:install] apply_general sets process-wide date/time format settings
- `[updates.rs:restart_to_update]` apply_on_exit(true) schedules installer/relaunch, then invokes hooks.quit to quit the process; apply_update_on_quit does likewise on exit

## Verification
- Run: `cargo test -p ironteams`
