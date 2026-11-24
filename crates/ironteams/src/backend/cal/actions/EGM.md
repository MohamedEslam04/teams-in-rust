# backend/cal/actions

## Overview
`impl Engine` handlers for `CalCmd`: dispatch and helpers, peek/full-view/RSVP/join operations, calendar creation and moves, room lists and sharing.

Files:
- `dispatch.rs`: Engine helpers over the calendar state (notes, open event, form tickets, cache upsert/remove, peek refresh, meeting links), load_calendar_meta, and the calendar_action CalCmd match.
- `event_ops.rs`: Engine async handlers for peek/full view, chat, RSVP, propose, PATCH, edit/duplicate, forward, cancel, delete, ICS export, file save, attachments, and refresh_extras for ticked calendars.
- `helpers.rs`: Free helpers: demo zones/calendars, visible range, UTC wall strings, default new-event slot, safe file names, temp-file opening, synthetic demo events, pasted meeting links, read-only peek, free/busy events, settings parsing.
- `moves_and_forms.rs`: Engine handlers for creating calendars, adding people, settings, move (with organizer confirmation), blank/new forms, submit (create/update/demo), and the scheduling assistant.
- `rooms_and_sharing.rs`: impl Engine calendar actions: room lists/rooms with free-busy, Meet now, calendar sharing permissions, print/ICS export, work location pins, schedule-from-chat form, sync_soon.

## Interface
- `[dispatch.rs:Engine::calendar_action]` pub(in super::super) async fn(self: &Arc<Self>, cmd: CalCmd) -> anyhow::Result<()>; main CalCmd dispatcher (navigation/filter/saved-view variants fall through to a no-op, handled by calendar_cmd)
- `[rooms_and_sharing.rs:Engine::my_working_hours]` pub(super) fn(&self) -> ::calendar::WorkingHours; Mon-Fri, settings/mailbox hours in local tz
- `[dispatch.rs:Engine::load_calendar_meta]` pub(in super::super) async fn(self: Arc<Self>); loads calendars, categories, mailbox work hours and time zones, then push_calendar
- `[rooms_and_sharing.rs:Engine::cal_room_lists]` pub(super) async fn(self:&Arc<Self>); loads room lists, pushes RoomsPush via ui.cal_rooms, caches in cal.room_lists
- `[dispatch.rs:Engine::cal_form_ticket]` pub(in crate::backend) fn(&self) -> u64; bumps form_seq and returns the ticket
- `[rooms_and_sharing.rs:Engine::cal_rooms]` pub(super) async fn(&Arc<Self>, list_email:&str, slot:&SlotReq); rooms of a list with free/busy for the form slot
- `[dispatch.rs:Engine::cal_form_current]` pub(in crate::backend) fn(&self, ticket: u64) -> bool; true if no later form command came
- `[rooms_and_sharing.rs:Engine::cal_meet_now]` pub(super) async fn(&Arc<Self>, subject:&str); creates instant meeting, stores cal.meet_link, ui.cal_meet_link
- `[dispatch.rs:Engine::cal_still_open]` pub(in crate::backend) fn(&self, id: &str) -> bool
- `[rooms_and_sharing.rs:Engine::default_calendar_id]` pub(super) fn(&self) -> Option<String>; id of the is_default calendar
- `[dispatch.rs:Engine::cal_change_cached]` pub(in crate::backend) fn(&self, id: &str, f: impl FnOnce(&mut Event)); re-reads the cached event, applies f, upserts; no-op if evicted
- `[rooms_and_sharing.rs:Engine::cal_share_open]` pub(super) async fn(&Arc<Self>); opens 'share' modal then reloads permissions
- `[dispatch.rs:Engine::cal_upsert]` pub(super) fn(&self, e: Event); cache apply + push_calendar + cal_refresh_peek
- `[rooms_and_sharing.rs:Engine::cal_share_reload]` pub(super) async fn(&Arc<Self>, note:&str); fetches calendar permissions and pushes PermRow list via ui.cal_perms
- `[dispatch.rs:Engine::cal_remove]` pub(super) fn(&self, ids: &[String]); removes from cache, clears open if removed, push_calendar
- `[rooms_and_sharing.rs:Engine::cal_share_add]` pub(super) async fn(&Arc<Self>, email:&str, role:&str); add_permission then reload
- `[dispatch.rs:Engine::cal_note]` pub(super) fn(&self, msg: &str); sets cal status text and pushes
- `[rooms_and_sharing.rs:Engine::cal_share_role]` pub(super) async fn(&Arc<Self>, id:&str, role:&str); update_permission then reload
- `[dispatch.rs:Engine::cal_open_event]` pub(super) fn(&self) -> Option<Event>
- `[rooms_and_sharing.rs:Engine::cal_share_remove]` pub(super) async fn(&Arc<Self>, id:&str); remove_permission then reload
- `[dispatch.rs:Engine::cal_open_peek]` pub(super) fn(&self) -> Option<PeekData>; adds links from loaded detail body, read_only() for extra-calendar events
- `[rooms_and_sharing.rs:Engine::cal_print]` pub(super) async fn(&Arc<Self>, ics:bool); exports visible range as .ics or PDF agenda via save_file
- `[dispatch.rs:Engine::cal_refresh_peek]` pub(super) fn(&self); sends peek to UI if an event is open
- `[rooms_and_sharing.rs:Engine::cal_work_location]` pub(super) async fn(&Arc<Self>, day:&str, loc:&str); sets local pin (office/remote/unset) and posts presence work location
- `[dispatch.rs:Engine::cal_meeting_link]` pub(super) fn(&self, kind: &str); opens notes/recap/options URL from the open event, https only
- `[rooms_and_sharing.rs:Engine::cal_from_chat]` pub(super) async fn(&Arc<Self>, chat_id:&str, title:&str); opens New meeting form prefilled with chat members
- `[dispatch.rs:Engine::is_offline]` pub(super) fn(&self) -> bool
- `[rooms_and_sharing.rs:Engine::sync_soon]` pub(super) fn(self:&Arc<Self>); spawns a delta calendar sync
- `[event_ops.rs:Engine::refresh_extras]` pub(in super::super) async fn(self: &Arc<Self>); fetches ticked person/other calendars for the wanted range into s.extras
- `[event_ops.rs:Engine::save_file]` pub(super) async fn(self: &Arc<Self>, name: &str, bytes: Vec<u8>); native save dialog + write
- `[event_ops.rs:Engine::cal_peek]` pub(super) fn(&self, id: &str); sets open, clears detail/proposals
- `[event_ops.rs:Engine::cal_open_full]` pub(super) async fn; opens the full modal and loads event_detail + proposals (organizer)
- `[event_ops.rs:Engine::cal_rsvp]` pub(super) async fn(&Arc<Self>, Rsvp, send: bool, comment: &str, proposed: Option<(i64,i64)>)
- `[event_ops.rs:Engine::cal_patch]` pub(super) async fn(&Arc<Self>, body: Value, local: impl FnOnce(&mut Event)); Graph PATCH on the open event then cached change
- `[event_ops.rs:Engine::cal_edit]` pub(super) async fn(&Arc<Self>, series: bool, duplicate: bool); opens the event form
- `[moves_and_forms.rs:Engine::cal_open_blank]` pub(super) fn(&self, date: Date, start_min: u32, channel_mode: bool, channels: Vec<String>)
- `[moves_and_forms.rs:Engine::cal_new]` pub(super) async fn(&Arc<Self>, kind: &str); opens a web page for webinar/townhall/appointment/live types, else a blank form (channel list for "channel")
- `[moves_and_forms.rs:Engine::cal_submit]` pub(super) async fn(&Arc<Self>, FormFields); create/update event, or synth event offline
- `[moves_and_forms.rs:Engine::cal_move_to]` pub(super) async fn(&Arc<Self>, e: Event, extra: bool, start: i64, end: i64)
- `[moves_and_forms.rs:Engine::cal_do_move]` pub(super) async fn(&Arc<Self>, id: &str, start: i64, end: i64)
- `[moves_and_forms.rs:Engine::cal_sched]` pub(super) async fn(&Arc<Self>, SchedReq); free/busy rows plus find_meeting_times suggestions
- `[helpers.rs:parse_settings]` pub(in super::super) fn(arg: &str) -> Result<session::CalendarSettings, String>; parses `first|start|end|weekends|weeks|tz`
- `[helpers.rs:demo_calendars]` pub(in super::super) fn() -> Vec<CalendarInfo>
- `[helpers.rs:pasted_link]` pub(super) fn(field: &str) -> Option<String>; adds https:// to Teams hosts, None for a meeting ID
- `[helpers.rs:open_temp_file]` pub(super) fn(name: &str, bytes: &[u8]) -> io::Result<()>; writes under temp_dir/ironteams-attachments with safe_name, then opens
- `[event_ops.rs:Engine::cal_open_chat]`, `cal_propose`, `cal_forward`, `cal_cancel`, `cal_delete`, `cal_export_event`, `cal_open_attachment` pub(super) async handlers for the open event (chat, propose new time or decline, forward, cancel, delete, ICS export, attachment open)
- `[moves_and_forms.rs:Engine::cal_create_calendar]`, `cal_add_person`, `cal_settings`, `cal_move`, `cal_accept_proposal` pub(super) handlers (new calendar, add a person's calendar, settings string via parse_settings, drag move that goes to cal_move_to, accept a proposed time)
- `[helpers.rs:demo_zones]` and other pub(super) free helpers: `demo_zones`, `visible_days`, `wall_utc`, `default_slot`, `synth_event`, `read_only`, `friendly`, `event_url`, `free_busy_events`

## Invariants
- `[dispatch.rs:cal_form_ticket/cal_form_current]` form_seq is bumped by every form command; an async form opener must only open its form if its ticket is still current (checked after awaits in cal_edit and cal_new)
- `[rooms_and_sharing.rs:cal_meet_now]` meet_link is cleared before creating so Copy link never yields the previous meeting's link
- `[moves_and_forms.rs:cal_open_blank]` also bumps form_seq and sets s.editing = None, so a blank form invalidates in-flight edit/new tickets
- `[rooms_and_sharing.rs:cal_rooms]` free/busy requested only when slot end > start; minutes clamped 5..=1440; room_schedules failure degrades to empty (rooms shown without status)
- `[dispatch.rs:cal_change_cached]` applies the change to the cache copy as it is now, not as captured when the request started; skips if the event is no longer cached
- `[rooms_and_sharing.rs:cal_share_add]` email must contain '@' else a note and no request; 'inside' = same domain (case-insensitive) as cal.me_email
- `[event_ops.rs:cal_open_full]` after the detail await, results are dropped if s.open is no longer this event id; proposals and detail are stored under the same lock
- `[rooms_and_sharing.rs:cal_share_add/role/remove]` silently return if no default calendar or unknown role key
- `[dispatch.rs:cal_meeting_link]` URL comes from the open event's peek, never from the UI, and only opened when it starts with https://
- `[rooms_and_sharing.rs:cal_work_location]` local pin is updated and calendar pushed before the remote call; remote failure only yields a note (endpoint unverified); expiry = local midnight of next day
- `[dispatch.rs:calendar_action:JoinEvent]` join_url opened externally only if it starts with https:// and call_prejoin_link returns false
- `[rooms_and_sharing.rs:cal_from_chat]` form ticket (cal_form_ticket/cal_form_current) checked after the async snapshot; stale result discarded; falls back to one_on_one_members when snapshot missing/fails
- `[dispatch.rs:calendar_action:JoinId]` pasted input is opened only when ::render::is_meeting_link accepts it; otherwise a prompt error
- `[rooms_and_sharing.rs:cal_from_chat]` subject prefilled only for groups (>2 members); me_email lazily filled from the store
- `[moves_and_forms.rs:cal_move_to]` extra-calendar events are refused; meetings with non-resource attendees can only be moved by the organizer and require MoveOk confirmation via pending_move
- `[rooms_and_sharing.rs:cal.lock/store.lock]` locks taken in short scopes and not held across .await (cal_print, cal_rooms, cal_from_chat)
- `[dispatch.rs:calendar_action:MoveOk]` pending_move is take()n, so a confirmation applies once
- `[dispatch.rs:Engine::is_offline]` cal_room_lists, cal_rooms, cal_meet_now, cal_share_reload, cal_work_location, cal_from_chat and sync_soon (all in rooms_and_sharing.rs) have an offline demo branch with canned data and no requests (sync_soon returns early); cal_share_add, cal_share_role and cal_share_remove have no such guard and call add/update/remove_permission directly
- `[helpers.rs:safe_name]` allows only alphanumerics and `. - _ space` (any other char becomes `_`), so attachment names cannot leave the temp folder; `[helpers.rs:open_temp_file]` turns an all-dot or empty result into "attachment"
- `[dispatch.rs:cal_open_peek]` events from extra calendars (s.is_extra) always get read_only (respond/edit/cancel/delete disabled)
- `[moves_and_forms.rs:cal_sched]` FIRST_MIN=360, SLOT=30, SLOTS=32 (06:00-22:00); at most 20 people; duration clamped 15..1440; suggestions window is 3 days with max 5 candidates and 100% attendee percentage

## Side Effects
- `[event_ops.rs:cal_patch]` Graph PATCH of the event (Prefer outlook.timezone=UTC, ImmutableId) via client.request_as
- `[rooms_and_sharing.rs:cal_room_lists/cal_rooms]` network room list/rooms/schedule requests via self.client; mutates cal.room_lists; UI pushes
- `[event_ops.rs:cal_rsvp/cal_forward/cal_cancel/cal_delete]` network calls respond/forward_event/cancel_event/delete_event, skipped when offline
- `[rooms_and_sharing.rs:cal_meet_now]` network meet_now; mutates cal.meet_link
- `[moves_and_forms.rs:cal_submit/cal_do_move/cal_create_calendar/cal_sched]` network create_event/update_event/move_event/create_calendar/get_schedule/find_meeting_times, skipped or faked offline
- `[rooms_and_sharing.rs:cal_share_reload/add/role/remove]` network calendar permission get/add/update/remove
- `[dispatch.rs:load_calendar_meta]` network reads of calendars, categories, mailbox settings, time zones; writes cal state
- `[rooms_and_sharing.rs:cal_print]` builds ICS/PDF bytes and writes via self.save_file (disk, file dialog)
- `[event_ops.rs:refresh_extras]` network calendar_view/person_view per ticked calendar; removes a calendar from visible on failure
- `[rooms_and_sharing.rs:cal_work_location]` mutates cal.pins, push_calendar, network set_work_location, cal_note
- `[dispatch.rs:calendar_action:CopyLink/CopyMeetLink]` writes the system clipboard via arboard
- `[rooms_and_sharing.rs:cal_from_chat]` network client.snapshot; reads local store (mutex); mutates cal.me_email and cal.editing; opens form/calendar UI
- `[event_ops.rs:save_file]` native file dialog (rfd) and disk write to the chosen path
- `[rooms_and_sharing.rs:sync_soon]` tokio::spawn running sync_calendar (network, local state); logs failure via tracing
- `[helpers.rs:open_temp_file]` creates temp_dir/ironteams-attachments, writes the file, and launches it via crate::external::open
- `[rooms_and_sharing.rs:cal_*]` tracing::warn! logging on failures
- `[dispatch.rs:cal_meeting_link/calendar_action/cal_new]` opens URLs in the browser/OS via crate::external::open (join links, Outlook pages, event-type web URLs)
- `[moves_and_forms.rs:cal_settings]` persist_calendar() writes calendar settings; also dispatches Cmd::SelectChat, sync_soon() and UI pushes (ui.cal_*, push_calendar)

## Verification
- Run: `cargo test -p ironteams`
