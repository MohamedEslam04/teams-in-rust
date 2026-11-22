use super::*;

#[test]
fn join_with_an_id_takes_a_pasted_meeting_link() {
    let r = Rig::new();
    let s = r.app.global::<CalStore>();
    let link = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_pastedlink%40thread.v2/0";
    s.set_modal("prompt".into());
    r.run(CalCmd::JoinId {
        id: format!("  {link}  "),
        passcode: String::new(),
    });
    assert_eq!(s.get_modal(), "", "the dialog closes once it joins");
    assert!(was_joined(&r, link));

    // A copy without the scheme works too.
    let bare = "teams.live.com/meet/1234567890123?p=PastedPass01";
    s.set_modal("prompt".into());
    r.run(CalCmd::JoinId {
        id: bare.into(),
        passcode: String::new(),
    });
    assert_eq!(s.get_modal(), "");
    assert!(was_joined(&r, &format!("https://{bare}")));
}

#[test]
fn join_with_an_id_refuses_a_link_that_is_not_a_meeting() {
    let r = Rig::new();
    let s = r.app.global::<CalStore>();
    let other = "https://example.invalid/not-a-meeting-link";
    s.set_modal("prompt".into());
    r.run(CalCmd::JoinId {
        id: other.into(),
        passcode: String::new(),
    });
    assert!(s.get_prompt_error().contains("meeting link"));
    assert_eq!(s.get_modal(), "prompt");
    assert!(!was_joined(&r, other));
}

#[test]
fn pasted_links_are_told_from_meeting_ids() {
    assert_eq!(pasted_link("123 456 789"), None);
    assert_eq!(pasted_link("ab12cd34"), None);
    assert_eq!(
        pasted_link(" https://teams.microsoft.com/meet/1 ").as_deref(),
        Some("https://teams.microsoft.com/meet/1")
    );
    assert_eq!(
        pasted_link("teams.cloud.microsoft/meet/1").as_deref(),
        Some("https://teams.cloud.microsoft/meet/1")
    );
}

#[test]
fn join_by_id_rejects_junk_and_the_calendars_list_toggles() {
    let r = Rig::new();
    let s = r.app.global::<CalStore>();
    s.set_modal("prompt".into());
    r.run(CalCmd::JoinId {
        id: "not an id!".into(),
        passcode: String::new(),
    });
    assert!(s.get_prompt_error().contains("meeting ID"));
    assert_eq!(s.get_modal(), "prompt");
    r.run(CalCmd::CalendarToggle("demo-3".into()));
    assert!(r.engine.cal.lock().visible.contains("demo-3"));
    r.run(CalCmd::CalendarToggle("demo-3".into()));
    assert!(r.engine.cal.lock().visible.is_empty());
}
