# calendar

## Overview
Library crate (package `calendar`, `publish = false`) that reads and writes the user's Outlook calendar over Microsoft Graph, with a few Outlook REST and presence-service calls. It holds the event model and JSON parsers, a delta-sync event cache, free/busy and room lookup, event create/update/RSVP/cancel/forward, calendar sharing permissions, mailbox settings, time-zone math, and pure view helpers (week ranges, column layout, filtering, ICS export). All network I/O goes through `teams_proto::Client` (`Audience::Graph` / `Audience::Outlook`); parsers and builders are pure. `src/lib.rs` declares every module privately (`mod ...`) and exposes the API only through `pub use` re-exports. Dependencies: `serde` (derive), `serde_json`, `reqwest` (`default-features = false`), `chrono`, `chrono-tz`, `base64`, `teams-proto`. The event model and `EventCache` serialize, so a caller can save the cache between runs.

## Interface
All items below are re-exported from `[src/lib.rs]` unless noted.
- Read model and sync (`[src/api.rs]`, `[src/event.rs]`): `Calendar::{view, event, delta}`, `view_url`, `delta_url`, `delta_link_expired(&Error) -> bool`, `EventCache` (`Clone`, `Serialize`, `Deserialize`; `apply`, `apply_full(start_ms, end_ms, changes)`, `missing_masters`, `forget_master_request`, `in_range`, `within(start_ms, end_ms) -> EventCache`, `len`, `is_empty`, field `delta_link`), `Event`, `Attendee`, `AttendeeKind`, `Response`, `ShowAs`, `SeriesType` (all `Serialize` + `Deserialize`), `EventChange`, `DeltaPage`, `parse_events`, `parse_delta_page`, `next_link`, `delta_link`.
- Detail (`[src/detail.rs]`): `event_detail`, `instances`, `download_attachment`, `EventDetail`, `Attachment`, `AttachmentKind`, `DialIn`, `ResponseCounts`, `parse_event_detail`, `parse_attachments`, `response_counts`.
- Calendars (`[src/calendars.rs]`): `calendars`, `calendar_groups`, `calendars_in_group`, `create_calendar`, `delete_calendar`, `calendar_view`, `person_view` (returns `PersonView::{Events, FreeBusyOnly}`), `CalendarInfo`, `CalendarGroup`, `CALENDAR_COLORS`, `is_access_denied`, `parse_calendars`, `parse_calendar_groups`, `create_calendar_body`.
- Graph request value (`[src/graph.rs]`): `GraphRequest { method, path, body }` with `send`; builders such as `add_permission_request` and `accept_proposal_request` return it without sending (dry-run description).
- Writes (`[src/new_event.rs]`, `[src/write.rs]`): `NewEvent` (`validate`, `create_body`, `update_body`, `duplicate`, `from_chat_members`), `Person`, `Recurrence`, `Pattern`, `RecurrenceEnd`, `WeekIndex`, `Reminder`, `Sensitivity`; `create_event`, `update_event`, `move_event`, `respond`, `cancel_event`, `delete_event`, `forward_event`, `Rsvp`, and the pure `respond_body`, `cancel_body`, `forward_body`, `move_body`, `target_id`.
- Scheduling (`[src/schedule.rs]`): `get_schedule`, `room_schedules`, `find_meeting_times`, `room_lists`, `rooms`, `free_between`, `Schedule`, `ScheduleItem`, `Availability`, `FindTimes`, `FoundTimes`, `Suggestion`, `RoomList`, `Room`, and the `*_body` / `parse_*` / `find_rooms_url` helpers.
- Permissions (`[src/permissions.rs]`): `calendar_permissions`, `add_permission`, `update_permission`, `remove_permission`, `Permission`, `PermissionRole`, plus `*_body`, `*_request`, `parse_permissions`.
- Proposals (`[src/proposals.rs]`): `event_proposals`, `proposal_messages`, `accept_proposal_request`, `Proposal`, `parse_event_proposals`, `parse_proposal_messages`, `event_proposals_path`, `proposal_messages_path`.
- Settings (`[src/settings.rs]`): `mailbox_settings`, `categories`, `supported_time_zones`, `work_location`, `set_work_location`, `WorkingHours`, `MailboxSettings`, `Category`, `TimeZoneInfo`, `WorkLocation`, `work_intervals`, `work_slots`, `parse_*`, `work_location_body`, `MAILBOX_SETTINGS_URL`, `GRAPH_MAILBOX_SETTINGS_PATH`.
- Teams meetings (`[src/teams_meet.rs]`): `meet_now`, `MeetNow`, `meeting_chat_thread_id`, `meeting_options_url`, `join_by_id_url`, `EventType`, `meet_now_body`, `parse_meet_now`.
- Time zones (`[src/tz.rs]`): `iana_name`, `windows_name`, `utc_offset_min`, `local_to_utc_ms`, `date_time_zone_ms`, `parse_custom_zone`, `CustomZone`, `Transition`. The `Zone` enum is `pub(crate)` and not re-exported.
- View helpers: `EventFilter`, `EventKind`, `RecurrenceFilter`, `kind_of` (`[src/filter.rs]`); `to_ics` (`[src/ics.rs]`); `layout_columns` (`[src/layout.rs]`); `Date`, `View`, `Weekday`, `format_iso_ms`, `week_range` (`[src/range.rs]`).

## Invariants
- `[src/api.rs:PREFER]` and `[src/graph.rs:send_raw]` send `Prefer: outlook.timezone="UTC", IdType="ImmutableId"` on reads and writes, so times are UTC and ids stay stable across folder moves.
- `[src/api.rs:MAX_PAGES]` view/delta paging stops with a protocol error after 100 pages; `[src/schedule.rs:MAX_PAGES]` room paging after 50 pages.
- `[src/api.rs:Calendar::delta]` every page must carry `nextLink` or `deltaLink`; a stale delta link makes Graph answer 410 (or another 4xx whose body names `syncstatenotfound` or `syncstateinvalid`; `[src/api.rs:delta_link_expired]` is true for both) and the caller must restart with `None`.
- `[src/api.rs:EventCache]` series masters are stored separately and never returned by `in_range`; bare occurrences are filled from the cached master; `missing_masters` returns each id at most once until `forget_master_request`. The `asked` set is `#[serde(skip)]`, so a reloaded cache asks again. `apply_full` (a delta started without a link lists the whole window) drops window events it does not list; `within` copies the window's events, all masters and the delta link, for saving.
- `[src/event.rs:parse_event]` returns `None` without a non-empty `id`; unparseable start/end fall back to 0 / start.
- `[src/new_event.rs:NewEvent::validate]` rejects start/end shorter than 19 chars, end before start (string compare, same zone/format assumed), all-day end <= start, attendee emails without `@`, and invalid recurrence (interval 0, empty weekly days, bad day/month, `After(0)`).
- `[src/new_event.rs:NewEvent::body]` a recurring series is sent with the Windows time-zone name; the room is appended as a `resource` attendee; `update_body` never sends `transactionId`.
- `[src/write.rs:create_event]` validates before any request and fills a fresh `transactionId` when none is set so a retry does not duplicate; `forward_event` with no recipients fails before any request; `respond_body` rejects a proposed new time for `Rsvp::Accept` or with end <= start.
- `[src/schedule.rs:room_schedules]` chunks addresses 20 per `getSchedule` call, sequentially; the first error aborts. `room_lists` / `rooms` try Graph v1.0 `places` first and fall back to the beta `findRoomLists` / `findRooms`.
- `[src/settings.rs:mailbox_settings]` fallback order: Graph `/me/mailboxSettings`, then Outlook REST, then `getSchedule` for the user's own address only when Outlook answers 403.
- `[src/settings.rs:work_intervals]` returns empty for no days, zero-length hours or an empty range; unknown zone falls back to UTC; `work_slots` output length always equals `slots`.
- `[src/tz.rs:local_to_utc_ms]` ambiguous local times resolve to the earlier instant and supports named zones only; `[src/tz.rs:date_time_zone_ms]` treats a missing zone as UTC.
- `[src/range.rs]` all date math is UTC; `week_range` returns a half-open `[start, end)` at UTC midnights.
- `[src/filter.rs:EventFilter::matches]` choices inside a group are OR, groups combine with AND, empty group matches everything.
- `[src/ics.rs:to_ics]` skips cancelled events, takes `now_ms` as an argument, folds lines at 75 octets and ends every line with CRLF.
- `[src/layout.rs:layout_columns]` events that only touch do not overlap; each cluster shares one `column_count`.
- `[src/permissions.rs:PermissionRole::from_graph]` unknown role strings map to `Custom`.
- `[src/teams_meet.rs:meeting_options_url]` returns `None` unless the thread id and `context` `Tid`/`Oid` values validate.

## Side Effects
- Reads (Graph v1.0 unless noted): `/me/calendarView`, `/me/calendarView/delta`, `/me/events/{id}` (with attachments expand), `/instances`, attachments, `/me/calendars`, `/me/calendarGroups`, `/users/{upn}/calendarView`, `/places/...`, `/me/outlook/supportedTimeZones`, `/me/mailboxSettings`, Outlook REST `MailboxSettings` and `MasterCategories` (`Audience::Outlook`), `/me/messages` (proposal messages), `/me/calendars/{id}/calendarPermissions`, beta `findRoomLists` / `findRooms`.
- Writes to the remote mailbox: `POST /me/events`, `PATCH /me/events/{id}`, `POST .../accept|tentativelyAccept|decline|cancel|forward`, `DELETE /me/events/{id}`, `POST`/`DELETE /me/calendars`, `POST`/`PATCH`/`DELETE` calendar permissions (adding a permission notifies the invitee by email), `POST /me/calendar/getSchedule`, `POST /me/findMeetingTimes` (read-style POSTs).
- Presence service via `client.presence_json`: `PUT me/workLocation/` (sets the work-location pin), `POST presence/getpresence/`.
- Middle tier: `POST {middle_tier}/beta/me/calendarEvents/privateMeeting/schedulingService/create` (`meet_now` creates a real Teams meeting).
- Randomness: `create_event` generates a UUID v4 `transactionId` when none is set. Clock: `mailbox_settings` reads `teams_proto::time::now_ms()` for its `getSchedule` fallback window.
- No disk or environment access; all other functions (parsers, body builders, `tz`, `range`, `layout`, `filter`, `ics`) are pure.

## Verification
- Run: `cargo test -p calendar`

Docs synced to commit c7516012 ("Merge fix/release-0.1.9: version 0.1.9", 2026-11-24)
