# backend/manage

## Overview
Teams and channels management: `TeamsCmd` parsing and Engine handlers (`mod.rs`), plain view data (`data.rs`) and a pure network-free view model of "Your teams and channels" (`view.rs`).

Files:
- `mod.rs`: Teams and channels management backend: TeamsCmd parsing, per-Engine manage State, command dispatch (manage_run) against the service client, sandbox write guard, dialogs, members, tags, email, communities, clipboard links.
- `data.rs`: Plain view-data structs (TeamsData, DialogData, row structs) built by the backend and pushed to the TeamsStore UI global.
- `live.rs`: Test-only (mod live behind #[cfg(test)] in mod.rs): #[ignore]d live end-to-end check of the Teams management commands against the real account, restricted to the sandbox team via IRONTEAMS_SANDBOX_FILE; creates and removes channels/tags.
- `view.rs`: Pure, network-free view model of the 'Your teams and channels' page: Page state with filter/sort/search over TeamEntry lists, producing TeamRowData/ChannelRowData, stats, hidden ids and lookups.

## Interface
- `[mod.rs:TeamsCmd]` pub struct { name: String, args: Vec<String> }; Debug, Clone, PartialEq
- `[view.rs:SEP]` pub const char '\u{1f}' separating multi-value command arguments (re-exported from manage/mod.rs)
- `[mod.rs:TeamsCmd::parse]` pub fn parse(name: &str, arg: &str) -> Option<TeamsCmd>; None unless name is in NAMES; args split on SEP, empty arg gives no args
- `[view.rs:role_name]` pub fn(Role) -> &'static str: Owner/Member/Guest (re-exported)
- `[mod.rs:State]` pub(crate) struct (Default) holding page, loaded/loading, error/notice, discover/requested, communities/posts, dialog, picks, tag state, names cache, sandbox guard, seq counters; field `page` is pub(crate)
- `[view.rs:ChannelRowData]` pub struct {id,name,kind:&'static str,shown,pinned,muted,archived,general,member,owner}; Clone, Debug, PartialEq
- `[mod.rs:Engine::manage_push]` pub(crate) fn(&self); builds TeamsData from State and calls ui.teams_store
- `[view.rs:TeamRowData]` pub struct {id,group,name,description,access,role,people:u32,channel_count,shown_count,shown,archived,expanded,owner,initials,color:u32,channels:Vec<ChannelRowData>}; Clone, Debug, PartialEq
- `[mod.rs:Engine::hidden_ids]` pub(super) fn(&self) -> HashSet<String>; hidden teams/channels for the sidebar
- `[view.rs:Page]` pub struct {entries:Vec<TeamEntry>, pinned:Vec<String>, filter:String, sort:String, query:String, expanded:HashSet<String>}; Debug, Clone (re-exported)
- `[mod.rs:Engine::pinned_ids]` pub(super) fn(&self) -> Vec<String>; pinned channel ids for the sidebar
- `[view.rs:Page::default]` Default impl: filter 'all', sort 'az', empty query/entries/pinned/expanded
- `[mod.rs:Engine::start_manage]` pub(super) fn(self: &Arc<Self>); spawns a tokio task that loads the list and pushes
- `[view.rs:Page::rows]` pub fn(&self) -> Vec<TeamRowData>: teams after filter, search and sort
- `[mod.rs:Engine::manage_cmd]` pub(super) async fn(self: &Arc<Self>, TeamsCmd) -> anyhow::Result<()>; runs a command, routes errors into page or dialog error, always pushes, always returns Ok
- `[view.rs:Page::all_rows]` pub fn(&self) -> Vec<TeamRowData>: every team, ignoring filter and query, keeping sort and pinned
- `[mod.rs:Engine::manage_set_entries]` pub(crate), cfg(test) only; sets page entries for tests
- `[view.rs:Page::stats]` pub fn(&self) -> Vec<(String,String)>: 10 Analytics totals (Teams, Teams you own, Public teams, Archived teams, Hidden teams, Channels, Private and shared channels, Hidden channels, Pinned channels, Muted channels)
- `[mod.rs:Page]` pub re-export of view::Page
- `[view.rs:Page::hidden_ids]` pub fn(&self) -> HashSet<String>: ids of hidden teams and hidden live channels, for the sidebar
- `[mod.rs:SEP]` pub re-export of view::SEP argument separator
- `[view.rs:Page::team]` pub fn(&self, thread_id:&str) -> Option<&TeamEntry>
- `[mod.rs:role_name]` pub re-export of view::role_name
- `[view.rs:Page::channel]` pub fn(&self, channel_id:&str) -> Option<(&TeamEntry,&ChannelEntry)>: includes deleted channels in lookup
- `[mod.rs:data::*]` pub re-export of all data.rs items
- `[live.rs:live_teams_ui_commands_in_the_sandbox_team]` #[tokio::test] #[ignore] live check; test-only
- `[data.rs:TeamRowData]` pub re-export of super::view::TeamRowData
- `[data.rs:TeamsData]` pub struct (Clone, Debug, Default): teams, all_teams, stats, loading, error, notice, discover, requests, join_info, communities, community_name, posts, posts_more, dialog
- `[data.rs:DialogData]` pub struct (Clone, Debug, Default, PartialEq): kind (empty = closed), busy, error, team/channel ids and names, channel_kind, name, description, access, owner, people, candidates, tags, tag_people, deleted, settings, email, email_senders, post/reply/pin_perm, allow_bots/connectors, notify_level/replies, team_code
- `[data.rs:DiscoverData]` pub struct: group, name, description, people, access, requested
- `[data.rs:PersonData]` pub struct: mri, name, detail, role: &'static str, me, picked
- `[data.rs:TagData]` pub struct: id, name, description, count, mine, selected
- `[data.rs:DeletedData]` pub struct: id, name, when
- `[data.rs:SettingData]` pub struct: key, group: &'static str, label: &'static str, on
- `[data.rs:CommunityData]` pub struct: id, name, description, people, company
- `[data.rs:PostData]` pub struct: id, sender, time, text, likes, reply, url
- `[mod.rs:NAMES]` private const list of 52 accepted command names (open, refresh, filter, sort, query, expand, team-/chan-/channel-/member-/tag-/email- commands, discover, join*, communities, open-post, ...)

## Invariants
- `[mod.rs:TeamsCmd::parse]` only names listed in NAMES parse; unknown names yield None; multiple args are joined by SEP
- `[view.rs:live_channels]` deleted channels (is_deleted) are excluded from rows, counts, stats, search and hidden_ids; teams with is_deleted are skipped in rows/stats/hidden_ids
- `[mod.rs:Guard::from_env / State::allow_team / allow_channel]` when IRONTEAMS_SANDBOX_FILE names a readable JSON with /team/thread_id and /team/group_id, every write is refused unless it targets the sandbox team or its listed channels
- `[view.rs:Page::rows]` sort keys are lowercase names; 'za' reverses name, 'people' by member_count desc then name, 'channels' by live channel count desc then name, any other value falls back to a-z
- `[mod.rs:manage_run "join"/"join-code", manage_team_create]` with a sandbox guard set, joining teams and creating teams are refused; join-code only accepts the sandbox team's own code
- `[view.rs:passes_filter]` filters owned/member/guest by role, hidden = team hidden OR any live channel hidden, archived by is_archived; unknown filter passes everything
- `[mod.rs:Engine::manage_state]` State is behind a std Mutex (self.manage.lock().unwrap()); closures run under the lock and no .await is held across them
- `[view.rs:Page::rows]` channel order within a team: General first, then pinned, then lowercase name; hidden channels are kept in the list
- `[mod.rs:State::discover_seq/member_seq]` bumped per discover and per member search; a response with a stale seq is dropped
- `[view.rs:Page::rows]` search matches team name or any live channel name; when only channels match the team row is forced expanded and only matching channels listed; channel_count/shown_count stay whole-team totals
- `[mod.rs:State::pin_seq / manage_run "chan-pin"]` pin is applied optimistically; only the answer to the latest pin updates or rolls back the pinned list
- `[view.rs:Page::channel_row]` pinned = c.is_pinned OR id in Page.pinned; member = is_member OR kind Standard
- `[mod.rs:Engine::manage_cmd]` failures never propagate as Err: loading/dialog.busy reset, error goes to dialog.error if a dialog is open (kind non-empty) else to page error; manage_push always runs
- `[view.rs:Page::all_rows]` built from a fresh default Page so filter/query/expanded are dropped; keeps entries, pinned, sort
- `[mod.rs:Engine::manage_feed]` a feed page is applied only if the community is unchanged and the last post id still equals older_than; PAGE = 20, posts_more = page len >= 20
- `[view.rs:access_name]` org-wide overrides access: 'Org-wide' > 'Public' > 'Private'
- `[mod.rs:manage_names]` only mris starting with "8:" are looked up for display names; unknown shown as "Unknown person"; roster drops bots
- `[view.rs:SEP]` unit separator 0x1F assumed to appear in no name or description
- `[mod.rs:manage_run "open-post"]` only urls starting with https:// are opened externally
- `[view.rs]` module is pure functions over data: no network, no I/O
- `[mod.rs:manage_member_search]` queries shorter than 2 chars clear candidates without a request; candidates are filtered to "8:" mris not already in the roster
- `[live.rs:header/Guard::from_env]` with IRONTEAMS_SANDBOX_FILE set the engine refuses writes outside the sandbox team (State::allow_team); test skips if unset
- `[mod.rs:manage_tag "tag-rename"]` tags are reloaded before a rename since the quoted version changes with every edit; tag-create passes self.me as the required first member
- `[live.rs:engine]` test needs a stored session and self_mri in config; keyring D-Bus is retried up to 4 times with 5s sleeps; Engine built with in-memory Store
- `[mod.rs:SETTINGS / settings_rows]` only keys already present in the stored userSettings/guestSettings are listed, so unknown keys are never written; bool or "true"/"false" string values accepted

## Side Effects
- `[mod.rs:Engine::manage_reload]` network: client.teams_and_channels and pinned_channels; replaces page entries/pinned; marks teams dirty
- `[live.rs:engine]` reads real config and keyring session, resumes teams_proto Client, persists session (session::persist), leaks an AppWindow via Box::leak with the Slint testing backend (test-only)
- `[mod.rs:Engine::start_manage]` spawns a detached tokio task to load the list at start
- `[live.rs:live_teams_ui_commands_in_the_sandbox_team]` network writes against the real sandbox team: creates, renames, archives, deletes, restores channels; moderation, email address, notification, tag, team settings/code/description/archive changes (restored afterwards)
- `[mod.rs:manage_run team-show/chan-show/chan-pin/chan-default/leave-*/channel-*/team-*/mod-set/notify-set]` network writes to the Teams service via self.client
- `[live.rs:remove_test_channels]` deletes channels named ironteams-ui-live* via client.delete_channel; cleanup asserts none left
- `[mod.rs:manage_member_change/manage_channel_create/manage_tag/manage_email/manage_team_create]` network writes to the service (members, channels, tags, channel email, teams)
- `[live.rs:eventually]` tokio sleeps and manage_reload polling against the server
- `[mod.rs:manage_copy_team_link/manage_copy_channel_link/manage_email email-copy]` system clipboard write via arboard::Clipboard; link builders also fetch client.tenant_id
- `[live.rs:run]` prints results to stdout; takes and clears state error fields
- `[mod.rs:manage_run "open-post"]` opens an https URL via crate::external::open (external process/browser)
- `[view.rs]` None
- `[mod.rs:Engine::manage_push / manage_close_dialog / manage_open_dialog]` UI calls ui.teams_store and ui.teams_dialog (UI-thread state push)
- `[mod.rs:Guard::from_env]` reads the file named by env IRONTEAMS_SANDBOX_FILE via compat::var and std::fs when State::default runs
- `[mod.rs:manage_cmd / manage_channel_create]` tracing::warn on command failure, tracing::info on channel created
- `[mod.rs:manage_names / manage_member_search]` network profile and people-search lookups; fills the State names cache

## Verification
- Run: `cargo test -p ironteams`
