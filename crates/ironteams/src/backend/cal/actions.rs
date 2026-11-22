//! What the peek, the full view, the new-event form, the panels and the dialogs do: every
//! [`CalCmd`] that is more than navigation. Calls go through the calendar crate; in demo mode
//! nothing is sent and the cache is changed locally so the screens still work.
//!
//! Live use is limited by what is safe: IronTeams's own tests never answer, cancel, forward or
//! create meetings on a real account. Only the pure parts (bodies, parsing, view models) are
//! tested here; the calls are the calendar crate's, with its own tests.

use super::*;
use ::calendar::{
    EventChange, Person, PersonView, ShowAs, add_permission, calendar_permissions, calendar_view,
    calendars, cancel_event, categories, create_calendar, create_event, delete_event,
    download_attachment, event_detail, event_proposals, find_meeting_times, forward_event,
    get_schedule, join_by_id_url, mailbox_settings, meet_now, meeting_chat_thread_id, move_event,
    person_view, remove_permission, respond, room_lists, room_schedules, rooms, set_work_location,
    supported_time_zones, target_id, to_ics, update_event, update_permission,
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

mod helpers;
pub(super) use helpers::*;
mod dispatch;
mod event_ops;
mod moves_and_forms;
mod rooms_and_sharing;

#[cfg(test)]
mod tests;
