use super::*;

mod ai_meeting;
mod drag;
mod form;
mod grid;
mod peek;
mod scene;
mod toolbar;

/// What the calendar's own labels (`CalFmt.hhmm`) say for `min` while Theme.h24 is off.
fn hh(min: i32) -> String {
    let h = min / 60;
    let pm = if h % 24 < 12 { "AM" } else { "PM" };
    format!("{}:{:02} {pm}", (h + 11) % 12 + 1, min % 60)
}
use crate::AppWindow;
use crate::backend::{Ctx, DEFAULT_PREFS, build, filter_opts, peek_data, sched_rows};
use calendar::{Date, Event, View, Weekday, parse_events};
use i_slint_backend_testing::{self as testing, ElementHandle};
use serde_json::json;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::RefCell;
use std::rc::Rc;

const H: i64 = 3_600_000;

/// An event on Wednesday 2026-11-18 plus `day` days, from `start_h` to `end_h` (UTC).
fn ev(id: &str, day: i64, start_h: f64, end_h: f64, extra: serde_json::Value) -> Event {
    let base = Date::new(2026, 11, 18).add_days(day).midnight_ms();
    let t = |h: f64| {
        json!({"dateTime": calendar::format_iso_ms(base + (h * H as f64) as i64),
        "timeZone": "UTC"})
    };
    let mut v = json!({"id": id, "subject": id, "start": t(start_h), "end": t(end_h)});
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    parse_events(&json!({ "value": [v] })).remove(0)
}

fn app(view: View, evs: &[Event]) -> AppWindow {
    app_with(view, evs, &DEFAULT_PREFS)
}

fn app_with(view: View, evs: &[Event], prefs: &crate::backend::Prefs) -> AppWindow {
    thread_local! { static INIT: std::sync::Once = const { std::sync::Once::new() }; }
    INIT.with(|once| once.call_once(testing::init_no_event_loop));
    let app = AppWindow::new().unwrap();
    app.window().set_size(LogicalSize::new(1400., 900.));
    app.set_login_state("ready".into());
    app.set_view("calendar".into());
    let today = Date::new(2026, 11, 18);
    let refs: Vec<&Event> = evs.iter().collect();
    let data = build(
        &refs,
        &Ctx {
            date: today,
            view,
            scale: 30,
            now_ms: today.midnight_ms() + 10 * H,
            first_day: Weekday::Monday,
            mini_anchor: today,
            off: &|_| 0,
            prefs,
        },
    );
    set(&app, data);
    app.show().unwrap();
    app
}

fn label(app: &AppWindow, title: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(app, title)
        .next()
        .unwrap_or_else(|| panic!("no event block labelled {title}"))
}

fn commands(app: &AppWindow) -> Rc<RefCell<Vec<(String, String)>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    app.global::<CalStore>()
        .on_command(move |n, a| l.borrow_mut().push((n.to_string(), a.to_string())));
    log
}

// ---- peek, form, filters, scheduler, drag ----

/// The button, menu row or checkbox with this label.
fn button(app: &AppWindow, label: &str) -> ElementHandle {
    ["FlexButton", "MenuItem", "Check"]
        .iter()
        .flat_map(|t| ElementHandle::find_by_element_type_name(app, t))
        .find(|e| e.accessible_label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no button labelled {label}"))
}

fn has_button(app: &AppWindow, label: &str) -> bool {
    ["FlexButton", "MenuItem", "Check"]
        .iter()
        .flat_map(|t| ElementHandle::find_by_element_type_name(app, t))
        .any(|e| e.accessible_label().as_deref() == Some(label))
}

fn click(e: &ElementHandle) {
    e.mock_single_click(PointerEventButton::Left);
}

fn log_of(log: &Rc<RefCell<Vec<(String, String)>>>) -> Vec<String> {
    log.borrow()
        .iter()
        .map(|(n, a)| format!("{n} {a}"))
        .collect()
}

fn invitation(response: &str) -> Event {
    ev(
        "Invite",
        0,
        9.0,
        10.0,
        json!({
            "responseStatus": {"response": response},
            "organizer": {"emailAddress": {"name": "Alice Chen", "address": "alice@contoso.com"}},
            "attendees": [
                {"emailAddress": {"name": "Me", "address": "me@contoso.com"}, "type": "required", "status": {"response": response}},
                {"emailAddress": {"name": "Bob", "address": "bob@contoso.com"}, "type": "required", "status": {"response": "accepted"}}
            ],
            "isOnlineMeeting": true,
            "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0"},
            "type": "occurrence"
        }),
    )
}

fn show_peek(app: &AppWindow, e: &Event) {
    let s = app.global::<CalStore>();
    s.set_peek(peek_row(peek_data(e, &|_| 0)));
    s.set_peek_open(true);
}

// The form

fn open_blank_form() -> (
    AppWindow,
    tokio::sync::mpsc::UnboundedReceiver<crate::backend::Cmd>,
) {
    let app = app(View::WorkWeek, &[]);
    app.window().set_size(LogicalSize::new(1400., 1400.));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    wire(&app, tx);
    open_form(
        &app,
        FormOpen {
            fields: FormFields::blank(Date::new(2026, 12, 18), 600),
            heading: "New event".into(),
            editing: false,
            channel_mode: false,
            categories: vec!["Blue category".into()],
            channels: vec![],
        },
    );
    (app, rx)
}

fn submitted(rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::backend::Cmd>) -> FormFields {
    match rx.try_recv() {
        Ok(crate::backend::Cmd::Calendar(CalCmd::Submit(f))) => *f,
        other => panic!("expected a submit, got {other:?}"),
    }
}
