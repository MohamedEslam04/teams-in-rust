//! The Outlook calendar over Microsoft Graph: events, side-by-side layout, the time ranges of
//! the calendar views, and everything that changes or surrounds them (RSVP, create and edit,
//! scheduling, rooms, proposals, calendars, sharing, settings, time zones, export).

mod api;
mod calendars;
mod detail;
mod event;
mod filter;
mod graph;
mod ics;
mod layout;
mod new_event;
mod permissions;
mod proposals;
mod range;
mod schedule;
mod settings;
mod teams_meet;
mod tz;
mod write;

pub use api::{Calendar, EventCache, delta_url, view_url};
pub use calendars::{
    CALENDAR_COLORS, CalendarGroup, CalendarInfo, PersonView, calendar_groups, calendar_view,
    calendars, calendars_in_group, create_calendar, create_calendar_body, delete_calendar,
    is_access_denied, parse_calendar_groups, parse_calendars, person_view,
};
pub use detail::{
    Attachment, AttachmentKind, DialIn, EventDetail, ResponseCounts, download_attachment,
    event_detail, instances, parse_attachments, parse_event_detail, response_counts,
};
pub use event::{
    Attendee, AttendeeKind, DeltaPage, Event, EventChange, EventType, Response, ShowAs, delta_link,
    next_link, parse_delta_page, parse_events,
};
pub use filter::{EventFilter, EventKind, Recurrence as RecurrenceFilter, kind_of};
pub use graph::GraphRequest;
pub use ics::to_ics;
pub use layout::layout_columns;
pub use new_event::{
    NewEvent, Pattern, Person, Recurrence, RecurrenceEnd, Reminder, Sensitivity, WeekIndex,
};
pub use permissions::{
    Permission, PermissionRole, add_permission, add_permission_body, add_permission_request,
    calendar_permissions, parse_permissions, remove_permission, remove_permission_request,
    update_permission, update_permission_body, update_permission_request,
};
pub use proposals::{
    Proposal, accept_proposal_request, event_proposals, event_proposals_path,
    parse_event_proposals, parse_proposal_messages, proposal_messages, proposal_messages_path,
};
pub use range::{Date, View, Weekday, format_iso_ms, week_range};
pub use schedule::{
    Availability, FindTimes, FoundTimes, Room, RoomList, Schedule, ScheduleItem, Suggestion,
    find_meeting_times, find_rooms_url, find_times_body, free_between, get_schedule,
    get_schedule_body, parse_found_room_lists, parse_found_rooms, parse_found_times,
    parse_room_lists, parse_rooms, parse_schedules, room_lists, room_schedules, rooms,
};
pub use settings::{
    Category, GRAPH_MAILBOX_SETTINGS_PATH, MAILBOX_SETTINGS_URL, MailboxSettings, TimeZoneInfo,
    WorkLocation, WorkingHours, categories, mailbox_settings, parse_categories,
    parse_mailbox_settings, parse_time_zones, parse_work_location, parse_working_hours,
    set_work_location, supported_time_zones, time_zones_path, work_intervals, work_location,
    work_location_body, work_slots,
};
pub use teams_meet::{
    EventType, MeetNow, join_by_id_url, meet_now, meet_now_body, meeting_chat_thread_id,
    meeting_options_url, parse_meet_now,
};
pub use tz::{
    CustomZone, Transition, date_time_zone_ms, iana_name, local_to_utc_ms, parse_custom_zone,
    utc_offset_min, windows_name,
};
pub use write::{
    Rsvp, cancel_body, cancel_event, create_event, delete_event, forward_body, forward_event,
    move_body, move_event, respond, respond_body, target_id, update_event,
};
