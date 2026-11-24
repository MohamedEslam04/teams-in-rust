# ui/cal

## Overview
Calendar UI helpers: copying the event form between `FormFields` and the Slint calendar store (`form.rs`), converting engine data into Slint types (`push.rs`) and `UiHandle` calendar push methods (`wire.rs`).

Files:
- `form.rs`: Copies the calendar event form between FormFields and the CalForm Slint global (write, read, open), and pushes Scheduler and rooms data into it.
- `push.rs`: Converts engine data (CalData, Panels, PeekData, DetailData) into Slint structs and sets them on the CalStore global.
- `wire.rs`: UiHandle calendar push methods that run on the UI thread, plus wire() connecting CalStore/CalForm/window callbacks to the engine Cmd channel.

## Interface
- `[form.rs:read_form]` pub fn(&AppWindow) -> FormFields; reads every CalForm property back
- `[form.rs:write_form]` pub(super) fn(&AppWindow, &FormFields); sets all CalForm fields, show_optional = optional non-empty, then refresh_sel
- `[form.rs:refresh_sel]` pub(super) fn(&AppWindow); sets CalForm sel_start/sel_end from parsed HH:MM (-1 if unparsable)
- `[form.rs:open_form]` pub(super) fn(&AppWindow, FormOpen); writes fields, heading/editing/channel_mode/categories/channels, clears error, sched, suggestions, rooms, closes sched/rooms panels, sets open=true
- `[form.rs:set_sched]` pub(super) fn(&AppWindow, SchedPush); sets CalForm sched rows, first_min, slot_min, loading, note, suggestions
- `[form.rs:set_rooms]` pub(super) fn(&AppWindow, RoomsPush); sets room_lists, rooms, rooms_loading, rooms_note
- `[form.rs:UiHandle::read_form_for_test]` #[cfg(test)] pub; reads the form via the weak window
- `[push.rs:set]` pub(super) fn(&AppWindow, CalData); sets CalStore title/view_kind/day_count/scale/status/days/blocks/all_day/month/mini/week numbers/work hours/second tz/now marker
- `[push.rs:set_panels]` pub(super) fn(&AppWindow, Panels); sets filter_opts, filtered, calendars, saved_views, settings, second_tz_label, categories
- `[push.rs:peek_row]` pub(super) fn(PeekData) -> CalPeek
- `[push.rs:detail_row]` pub(super) fn(DetailData) -> CalDetail
- `[wire.rs:UiHandle::calendar]` pub fn(&self, CalData); posts set() to the UI thread
- `[wire.rs:UiHandle::calendar_loading]` pub fn(&self, bool); CalStore.loading
- `[wire.rs:UiHandle::join_banner]` pub fn(&self, (String,String)); sets join_text and join_url (empty text hides it)
- `[wire.rs:UiHandle::cal_panels]` pub fn(&self, Panels)
- `[wire.rs:UiHandle::cal_peek]` pub fn(&self, PeekData)
- `[wire.rs:UiHandle::cal_detail]` pub fn(&self, DetailData)
- `[wire.rs:UiHandle::cal_modal]` pub fn(&self, &'static str); opens a dialog (full/share/meet-link) or closes with ""
- `[wire.rs:UiHandle::cal_modal_if]` pub fn(&self, from, to); switches modal only if the current one equals from
- `[wire.rs:UiHandle::cal_close_all]` pub fn(&self); clears popup and modal, peek_open=false
- `[wire.rs:UiHandle::cal_show_chat]` pub fn(&self); window view = "chat"
- `[wire.rs:UiHandle::cal_show_calendar]` pub fn(&self); window view = "calendar"
- `[wire.rs:UiHandle::cal_prompt_error]` pub fn(&self, String); CalStore.prompt_error
- `[wire.rs:UiHandle::cal_ask]` pub fn(&self, kind, title, message, confirm); invokes CalStore.ask with three empty strings
- `[wire.rs:UiHandle::cal_form_open]` pub fn(&self, FormOpen)
- `[wire.rs:UiHandle::cal_form_error]` pub fn(&self, String)
- `[wire.rs:UiHandle::cal_form_close]` pub fn(&self); CalForm.open=false
- `[wire.rs:UiHandle::cal_sched]` pub fn(&self, SchedPush)
- `[wire.rs:UiHandle::cal_rooms]` pub fn(&self, RoomsPush)
- `[wire.rs:UiHandle::cal_zones]` pub fn(&self, Vec<(String,String)>); sets CalStore.zone_matches (name, display name) for the settings time-zone picker
- `[wire.rs:UiHandle::cal_perms]` pub fn(&self, Vec<PermRow>, String); sets CalStore share_roles, perms and share_note
- `[wire.rs:UiHandle::cal_meet_link]` pub fn(&self, String); sets meet_link and opens the "meet-link" modal
- `[wire.rs:wire]` pub fn(&AppWindow, UnboundedSender<Cmd>); registers CalStore.on_command, CalForm on_submit/on_cancel/on_times_edited/on_pick_slot/on_apply_suggestion/on_pick_room, window.on_schedule_meeting

## Invariants
- `[wire.rs:wire]` callbacks hold only a weak window handle (as_weak, upgrade) and return early if the window is gone, so they do not keep it alive
- `[wire.rs:wire/on_submit]` the form is validated with to_new_event before Cmd::Calendar(Submit) is sent; on Err the error is shown in the form and nothing is sent
- `[wire.rs:command]` "sched", "rooms" and "room-list" commands are built here (sched from the form; room-list as CalCmd::RoomList{email: arg, slot} from the form's start/end date and time); every other command goes to CalCmd::parse(name, arg)
- `[wire.rs:command/sched]` the room person is set only when room_email is non-empty
- `[form.rs:refresh_sel]` sel_start and sel_end are -1 when the time fails parse_hm
- `[form.rs:write_form]` show_optional is derived from optional being non-empty; refresh_sel always runs after writing the fields
- `[form.rs:open_form]` opening resets error, sched, suggestions and rooms, closes the sched and rooms panels, and sets open=true last
- `[wire.rs:cal_modal_if]` changes the modal only when the current modal equals `from`, leaving any other dialog alone
- `[wire.rs:cal_modal]` modal names are &'static str: full, share, meet-link; empty string closes
- `[push.rs:set]` now_min and now_col use -1 for None; Slint integers come from int()/as i32 casts
- `[wire.rs:wire/on_pick_slot,on_apply_suggestion,on_pick_room]` indices are clamped with i.max(0) as usize; out-of-range suggestion or room rows are ignored (row_data returns None)
- `[wire.rs:UiHandle methods]` all Slint property writes go through self.run(closure taking the window) rather than being made directly, so they run on the UI side

## Side Effects
- `[wire.rs:wire]` sends Cmd::Calendar(...) on the UnboundedSender (send errors ignored) for command, submit, schedule_meeting
- `[wire.rs:UiHandle::*]` mutates Slint globals CalStore and CalForm and the window `view` property via UiHandle::run
- `[wire.rs:UiHandle::cal_ask]` invokes the CalStore.ask callback, which shows a yes/no dialog
- `[wire.rs:wire/on_pick_slot,on_apply_suggestion,on_pick_room,on_times_edited]` write CalForm start/end times, room and sel properties directly on the UI thread
- `[form.rs:open_form]` opens the event form (CalForm.open=true) and resets its sub-panels

## Verification
- Run: `cargo test -p ironteams`
