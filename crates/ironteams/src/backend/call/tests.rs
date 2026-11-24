use super::*;
use serde_json::json;

mod lifecycle;
mod other_device;
mod reactions_and_commands;
mod roster;

fn devices() -> Devices {
    FakeDriver::manual().devices()
}

fn meeting() -> CallKind {
    CallKind::Meeting {
        title: "Standup".into(),
    }
}

fn ready() -> CallModel {
    let mut m = CallModel {
        my_name: "Me".into(),
        ..Default::default()
    };
    m.prejoin(
        meeting(),
        JoinTarget::Link("l".into()),
        "19:c".into(),
        devices(),
    )
    .unwrap();
    m
}

/// A connected meeting with a roster of us (`role`), a guest in the lobby, Ada, and the
/// recorder bot.
fn lobby_meeting(role: &str, t0: Instant) -> CallModel {
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    let p = |mri: &str, name: &str, lobby, me, role: &str| Person {
        mri: mri.into(),
        name: name.into(),
        lobby,
        me,
        role: role.into(),
        ..Default::default()
    };
    m.apply(
        Event::Roster(vec![
            p("8:orgid:me", "Me", false, true, role),
            p("8:orgid:guest", "Guest", true, false, ""),
            p("28:bot-1", "Unknown", false, false, ""),
            p("8:orgid:ada", "Ada", false, false, "attendee"),
        ]),
        t0,
    );
    m
}

fn connected(t0: Instant) -> CallModel {
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    m
}

fn floating(m: &CallModel, now: Instant) -> Vec<(&'static str, String, bool)> {
    m.data(now)
        .reactions
        .into_iter()
        .map(|r| (r.name, r.who, r.mine))
        .collect()
}
