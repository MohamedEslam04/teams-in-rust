use super::*;

fn id() -> ScopeId {
    ScopeId::new("work", "tenant-1", "oid-1")
}

const CHAT: &str = "19:abc@thread.v2";
const OTHER_CHAT: &str = "19:def@thread.v2";
const MEETING: &str = "19:meeting_abc@thread.v2";
const CHANNEL: &str = "19:c@thread.tacv2";

/// A policy where the scope `id()` exists and `f` has set it up.
fn policy(f: impl FnOnce(&mut Scope)) -> Policy {
    let mut p = Policy::default();
    f(p.scope_entry(&id()));
    p
}

fn chat(key: &str) -> Resource {
    Resource::Chat(key.into())
}

fn meeting(key: &str) -> Resource {
    Resource::Meeting(key.into())
}

#[test]
fn nothing_is_allowed_without_an_entry() {
    let resources = [
        chat(CHAT),
        Resource::Channel(CHANNEL.into()),
        meeting(MEETING),
        Resource::Calendar,
        Resource::Directory,
        Resource::SelfStatus,
    ];
    for p in [Policy::default(), policy(|_| {})] {
        for r in &resources {
            for need in [Need::Read, Need::Draft] {
                assert_eq!(
                    decide(&p, &id(), r, need),
                    Decision::Blocked,
                    "{r:?} {need:?}"
                );
            }
        }
    }
}

#[test]
fn read_allows_reading_and_read_draft_allows_both() {
    let p = policy(|s| {
        s.conversations.insert(CHAT.into(), Level::Read);
        s.conversations.insert(OTHER_CHAT.into(), Level::ReadDraft);
    });
    assert_eq!(
        decide(&p, &id(), &chat(CHAT), Need::Read),
        Decision::Allowed
    );
    assert_eq!(
        decide(&p, &id(), &chat(CHAT), Need::Draft),
        Decision::Blocked
    );
    assert_eq!(
        decide(&p, &id(), &chat(OTHER_CHAT), Need::Read),
        Decision::Allowed
    );
    assert_eq!(
        decide(&p, &id(), &chat(OTHER_CHAT), Need::Draft),
        Decision::Allowed
    );
}

#[test]
fn a_grant_covers_exactly_one_conversation() {
    let p = policy(|s| {
        s.conversations.insert(CHAT.into(), Level::ReadDraft);
    });
    assert_eq!(
        decide(&p, &id(), &chat(OTHER_CHAT), Need::Read),
        Decision::Blocked
    );
    // Keys are compared as Teams sent them: a different case is a different key, so Blocked.
    assert_eq!(
        decide(&p, &id(), &chat(&CHAT.to_uppercase()), Need::Read),
        Decision::Blocked
    );
}

#[test]
fn every_combination_matches_the_rules() {
    // An independent statement of the rules; `decide` must agree on every row.
    fn expected(master: Master, paused: bool, level: Option<Level>, need: Need) -> Decision {
        if master == Master::Paused {
            Decision::MasterPaused
        } else if paused {
            Decision::AccountPaused
        } else {
            match (level, need) {
                (Some(Level::ReadDraft), _) | (Some(Level::Read), Need::Read) => Decision::Allowed,
                _ => Decision::Blocked,
            }
        }
    }
    let masters = [Master::Active, Master::Paused];
    let levels = [None, Some(Level::Read), Some(Level::ReadDraft)];
    let needs = [Need::Read, Need::Draft];
    for master in masters {
        for paused in [false, true] {
            for level in levels {
                for need in needs {
                    let mut p = policy(|s| {
                        s.paused = paused;
                        if let Some(l) = level {
                            s.conversations.insert(CHAT.into(), l);
                        }
                    });
                    p.master = master;
                    assert_eq!(
                        decide(&p, &id(), &chat(CHAT), need),
                        expected(master, paused, level, need),
                        "master {master:?}, paused {paused}, level {level:?}, need {need:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_master_switch_pauses_every_resource_and_keeps_the_settings() {
    let mut p = policy(|s| {
        s.calendar_people = true;
        s.conversations.insert(CHAT.into(), Level::ReadDraft);
    });
    p.master = Master::Paused;
    for r in [
        chat(CHAT),
        Resource::Channel(CHANNEL.into()),
        meeting(MEETING),
        Resource::Calendar,
        Resource::Directory,
        Resource::SelfStatus,
    ] {
        assert_eq!(
            decide(&p, &id(), &r, Need::Read),
            Decision::MasterPaused,
            "{r:?}"
        );
    }
    // Paused even for an account that has no scope at all.
    let other = ScopeId::new("home", "tenant-2", "oid-2");
    assert_eq!(
        decide(&p, &other, &chat(CHAT), Need::Read),
        Decision::MasterPaused
    );
    // Resuming brings the same grants back.
    p.master = Master::Active;
    assert_eq!(
        decide(&p, &id(), &chat(CHAT), Need::Draft),
        Decision::Allowed
    );
}

#[test]
fn an_account_pause_stops_only_that_account() {
    let mut p = policy(|s| {
        s.paused = true;
        s.calendar_people = true;
        s.conversations.insert(CHAT.into(), Level::Read);
    });
    let home = ScopeId::new("home", "tenant-2", "oid-2");
    let h = p.scope_entry(&home);
    h.conversations.insert(CHAT.into(), Level::Read);
    assert_eq!(
        decide(&p, &id(), &chat(CHAT), Need::Read),
        Decision::AccountPaused
    );
    assert_eq!(
        decide(&p, &id(), &Resource::Calendar, Need::Read),
        Decision::AccountPaused
    );
    assert_eq!(
        decide(&p, &home, &chat(CHAT), Need::Read),
        Decision::Allowed
    );
}

#[test]
fn a_scope_of_another_user_grants_nothing() {
    let p = policy(|s| {
        s.calendar_people = true;
        s.conversations.insert(CHAT.into(), Level::ReadDraft);
    });
    // Same label and tenant, but a different signed-in user (the label was recreated).
    let recreated = ScopeId::new("work", "tenant-1", "oid-2");
    assert_eq!(
        decide(&p, &recreated, &chat(CHAT), Need::Read),
        Decision::Blocked
    );
    assert_eq!(
        decide(&p, &recreated, &Resource::Calendar, Need::Read),
        Decision::Blocked
    );
    assert!(p.scope(&recreated).is_none());
    assert!(p.scope(&id()).is_some());
}

#[test]
fn a_channel_is_read_only_whatever_level_the_file_holds() {
    // The channel truth table: (the channel's entry, the need) -> allowed. Channels can be read
    // and never drafted in, so a `read_draft` entry (only a hand-edited file has one) reads only.
    use Need::{Draft, Read};
    let table = [
        (None, Read, false),
        (None, Draft, false),
        (Some(Level::Read), Read, true),
        (Some(Level::Read), Draft, false),
        (Some(Level::ReadDraft), Read, true),
        (Some(Level::ReadDraft), Draft, false),
    ];
    for (level, need, allowed) in table {
        let p = policy(|s| {
            if let Some(l) = level {
                s.conversations.insert(CHANNEL.into(), l);
            }
        });
        let want = if allowed {
            Decision::Allowed
        } else {
            Decision::Blocked
        };
        assert_eq!(
            decide(&p, &id(), &Resource::Channel(CHANNEL.into()), need),
            want,
            "level {level:?}, need {need:?}"
        );
    }
}

#[test]
fn a_channel_key_is_read_only_even_named_as_a_chat() {
    // Whatever kind of resource a caller builds, a channel's key never allows drafting.
    for key in [CHANNEL, "19:c@thread.skype"] {
        let p = policy(|s| {
            s.conversations.insert(key.into(), Level::ReadDraft);
        });
        assert_eq!(
            decide(&p, &id(), &chat(key), Need::Read),
            Decision::Allowed,
            "{key}"
        );
        assert_eq!(
            decide(&p, &id(), &chat(key), Need::Draft),
            Decision::Blocked,
            "{key}"
        );
        assert_eq!(
            decide(&p, &id(), &meeting(key), Need::Draft),
            Decision::Blocked,
            "{key}"
        );
    }
}

#[test]
fn a_channel_grant_covers_that_channel_only_and_the_switches_come_first() {
    let mut p = policy(|s| {
        s.conversations.insert(CHANNEL.into(), Level::Read);
    });
    let other = Resource::Channel("19:other@thread.tacv2".into());
    assert_eq!(decide(&p, &id(), &other, Need::Read), Decision::Blocked);
    let chan = Resource::Channel(CHANNEL.into());
    p.scope_entry(&id()).paused = true;
    assert_eq!(
        decide(&p, &id(), &chan, Need::Read),
        Decision::AccountPaused
    );
    p.master = Master::Paused;
    assert_eq!(decide(&p, &id(), &chan, Need::Read), Decision::MasterPaused);
    assert_eq!(
        decide(&Policy::deny_all("x"), &id(), &chan, Need::Read),
        Decision::SettingsUnreadable
    );
}

#[test]
fn a_meeting_needs_the_calendar_switch_and_its_chats_level() {
    // The composition table of the design, row by row: (calendar_people, the meeting chat's
    // level, the need) -> allowed.
    use Need::{Draft, Read};
    let table = [
        (false, None, Read, false),
        (false, None, Draft, false),
        (false, Some(Level::Read), Read, false),
        (false, Some(Level::ReadDraft), Read, false),
        (false, Some(Level::ReadDraft), Draft, false),
        (true, None, Read, false),
        (true, None, Draft, false),
        (true, Some(Level::Read), Read, true),
        (true, Some(Level::Read), Draft, false),
        (true, Some(Level::ReadDraft), Read, true),
        (true, Some(Level::ReadDraft), Draft, true),
    ];
    for (calendar, level, need, allowed) in table {
        let p = policy(|s| {
            s.calendar_people = calendar;
            if let Some(l) = level {
                s.conversations.insert(MEETING.into(), l);
            }
        });
        let want = if allowed {
            Decision::Allowed
        } else {
            Decision::Blocked
        };
        assert_eq!(
            decide(&p, &id(), &meeting(MEETING), need),
            want,
            "calendar {calendar}, level {level:?}, need {need:?}"
        );
    }
}

#[test]
fn a_meeting_is_decided_by_its_own_key_and_the_pauses_keep_their_order() {
    let mut p = policy(|s| {
        s.calendar_people = true;
        s.conversations.insert(MEETING.into(), Level::ReadDraft);
    });
    assert_eq!(
        decide(
            &p,
            &id(),
            &meeting("19:meeting_other@thread.v2"),
            Need::Read
        ),
        Decision::Blocked
    );
    // The meeting chat's level also opens the chat itself, and the chat's entry is the meeting's.
    assert_eq!(
        decide(&p, &id(), &chat(MEETING), Need::Draft),
        Decision::Allowed
    );
    p.scope_entry(&id()).paused = true;
    assert_eq!(
        decide(&p, &id(), &meeting(MEETING), Need::Read),
        Decision::AccountPaused
    );
    p.master = Master::Paused;
    assert_eq!(
        decide(&p, &id(), &meeting(MEETING), Need::Read),
        Decision::MasterPaused
    );
}

#[test]
fn a_meeting_resource_is_built_from_a_chat_thread_id_only() {
    assert_eq!(Resource::meeting(MEETING), Ok(meeting(MEETING)));
    assert_eq!(
        Resource::meeting(" 19:meeting_abc@thread.v2;messageid=5 "),
        Ok(meeting(MEETING)),
        "the canonical key"
    );
    assert_eq!(
        Resource::meeting("19:c@thread.tacv2"),
        Err(KeyError::Unknown),
        "a channel is no meeting chat"
    );
    assert_eq!(Resource::meeting("48:calllogs"), Err(KeyError::Unknown));
    assert_eq!(Resource::meeting(""), Err(KeyError::Empty));
    assert_eq!(Resource::meeting("a b"), Err(KeyError::Malformed));
}

#[test]
fn calendar_and_people_follow_one_switch_with_no_levels() {
    let off = policy(|_| {});
    let on = policy(|s| s.calendar_people = true);
    for r in [Resource::Calendar, Resource::Directory] {
        for need in [Need::Read, Need::Draft] {
            assert_eq!(decide(&off, &id(), &r, need), Decision::Blocked);
            assert_eq!(
                decide(&on, &id(), &r, need),
                Decision::Allowed,
                "{r:?} {need:?}"
            );
        }
    }
}

#[test]
fn self_status_is_refused_in_the_first_version_even_with_the_switch_on() {
    let on = policy(|s| s.calendar_people = true);
    for need in [Need::Read, Need::Draft] {
        assert_eq!(
            decide(&on, &id(), &Resource::SelfStatus, need),
            Decision::Blocked
        );
    }
}

#[test]
fn an_unreadable_policy_blocks_everything_and_says_so() {
    let p = Policy::deny_all("corrupt");
    assert_eq!(p.unreadable_reason(), Some("corrupt"));
    for r in [
        chat(CHAT),
        Resource::Channel(CHANNEL.into()),
        meeting(MEETING),
        Resource::Calendar,
        Resource::Directory,
        Resource::SelfStatus,
    ] {
        assert_eq!(
            decide(&p, &id(), &r, Need::Read),
            Decision::SettingsUnreadable
        );
    }
    assert_eq!(Policy::default().unreadable_reason(), None);
}

#[test]
fn the_defaults_are_active_with_nothing_enabled() {
    let p = Policy::default();
    assert_eq!(p.format_version, FORMAT_VERSION);
    assert_eq!(p.generation, 0);
    assert_eq!(p.master, Master::Active);
    assert!(!p.migrated_legacy_allowlist);
    assert!(p.scopes.is_empty());
}

#[test]
fn a_scope_entry_for_a_new_user_starts_empty() {
    let mut p = policy(|s| {
        s.calendar_people = true;
        s.conversations.insert(CHAT.into(), Level::Read);
    });
    let s = p.scope_entry(&ScopeId::new("work", "tenant-1", "oid-2"));
    assert_eq!(s.user_oid, "oid-2");
    assert!(s.conversations.is_empty());
    assert!(!s.calendar_people);
    assert!(!s.paused);
}

#[test]
fn set_conversation_stores_the_canonical_key_and_none_removes_it() {
    let mut s = Scope::new("oid-1");
    s.set_conversation(
        &format!(" {CHAT};messageid=1731234567890 "),
        Some(Level::Read),
    )
    .unwrap();
    assert_eq!(s.conversations.get(CHAT), Some(&Level::Read));
    s.set_conversation(CHAT, None).unwrap();
    assert!(s.conversations.is_empty());
    assert_eq!(
        s.set_conversation("", Some(Level::Read)),
        Err(KeyError::Empty)
    );
    assert_eq!(
        s.set_conversation("19:a b@thread.v2", Some(Level::Read)),
        Err(KeyError::Malformed)
    );
    assert!(s.conversations.is_empty(), "a refused id changes nothing");
}

#[test]
fn purging_a_label_removes_every_tenant_of_it() {
    let mut p = Policy::default();
    p.scope_entry(&ScopeId::new("work", "t1", "o1"));
    p.scope_entry(&ScopeId::new("work", "t2", "o1"));
    p.scope_entry(&ScopeId::new("work2", "t1", "o3"));
    p.scope_entry(&ScopeId::new("home", "t1", "o2"));
    assert!(p.purge_label("work"));
    assert_eq!(
        p.scopes.keys().map(String::as_str).collect::<Vec<_>>(),
        ["home@t1", "work2@t1"]
    );
    assert!(!p.purge_label("work"), "nothing left to remove");
}

// --- canonical_key ---

#[test]
fn canonical_key_keeps_a_plain_id_as_it_is() {
    for id in [
        "48:notes",
        "19:abc@thread.v2",
        "19:aaa_bbb@unq.gbl.spaces",
        "19:meeting_Mjc5ZDI@thread.v2",
        "8:orgid:0123-abcd",
    ] {
        assert_eq!(canonical_key(id).as_deref(), Ok(id));
    }
}

#[test]
fn canonical_key_trims_and_strips_a_thread_suffix_to_the_root() {
    assert_eq!(
        canonical_key("  19:abc@thread.tacv2;messageid=1731234567890\n").as_deref(),
        Ok("19:abc@thread.tacv2")
    );
    assert_eq!(canonical_key(" 48:notes ").as_deref(), Ok("48:notes"));
}

#[test]
fn canonical_key_keeps_the_case_it_was_given() {
    assert_eq!(
        canonical_key("19:AbC@thread.v2").as_deref(),
        Ok("19:AbC@thread.v2")
    );
    assert_ne!(
        canonical_key("19:abc@thread.v2"),
        canonical_key("19:ABC@thread.v2"),
        "two ids that differ by case are two keys"
    );
}

#[test]
fn canonical_key_refuses_empty_ids() {
    for id in ["", "   ", "\t\n", ";messageid=1", "  ;messageid=1"] {
        assert_eq!(canonical_key(id), Err(KeyError::Empty), "{id:?}");
    }
}

#[test]
fn canonical_key_refuses_odd_ids() {
    for id in [
        "19:abc def@thread.v2",         // space inside
        "19:abc\u{0}@thread.v2",        // control character
        "19:abc/../x@thread.v2",        // path separator
        "19:abc\\x@thread.v2",          // backslash
        "19:abc;x=1@thread.v2",         // a stray `;`
        "19:abc@thread.v2;MESSAGEID=1", // not the exact suffix
        "19:ab\u{200b}c@thread.v2",     // zero-width space
        "19:caf\u{e9}@thread.v2",       // not ASCII
    ] {
        assert_eq!(canonical_key(id), Err(KeyError::Malformed), "{id:?}");
    }
    let long = format!("19:{}@thread.v2", "a".repeat(600));
    assert_eq!(
        canonical_key(&long),
        Err(KeyError::Malformed),
        "over the length limit"
    );
}

#[test]
fn canonical_key_cuts_at_the_first_thread_suffix() {
    assert_eq!(
        canonical_key("19:abc@thread.v2;messageid=1;messageid=2").as_deref(),
        Ok("19:abc@thread.v2")
    );
}

#[test]
fn a_conversation_id_becomes_a_chat_or_a_channel_and_unknown_kinds_are_refused() {
    assert_eq!(
        Resource::from_conversation_id("48:notes"),
        Ok(chat("48:notes"))
    );
    assert_eq!(Resource::from_conversation_id(CHAT), Ok(chat(CHAT)));
    assert_eq!(
        Resource::from_conversation_id("19:aaa_bbb@unq.gbl.spaces"),
        Ok(chat("19:aaa_bbb@unq.gbl.spaces"))
    );
    assert_eq!(
        Resource::from_conversation_id("19:meeting_xyz@thread.v2"),
        Ok(chat("19:meeting_xyz@thread.v2"))
    );
    assert_eq!(
        Resource::from_conversation_id("19:c@thread.tacv2;messageid=17"),
        Ok(Resource::Channel("19:c@thread.tacv2".into())),
        "a channel thread resolves to its channel"
    );
    assert_eq!(
        Resource::from_conversation_id("19:c@thread.skype"),
        Ok(Resource::Channel("19:c@thread.skype".into()))
    );
    assert_eq!(
        Resource::from_conversation_id("48:calllogs"),
        Err(KeyError::Unknown)
    );
    assert_eq!(
        Resource::from_conversation_id("hello"),
        Err(KeyError::Unknown)
    );
    assert_eq!(Resource::from_conversation_id(""), Err(KeyError::Empty));
    assert_eq!(
        Resource::from_conversation_id("a b"),
        Err(KeyError::Malformed)
    );
}

#[test]
fn resources_know_their_kind() {
    assert_eq!(chat(CHAT).kind(), ResourceKind::Chat);
    assert_eq!(Resource::Channel("c".into()).kind(), ResourceKind::Channel);
    assert_eq!(meeting(MEETING).kind(), ResourceKind::Meeting);
    assert_eq!(Resource::Calendar.kind(), ResourceKind::Calendar);
    assert_eq!(Resource::Directory.kind(), ResourceKind::Directory);
    assert_eq!(Resource::SelfStatus.kind(), ResourceKind::SelfStatus);
}

// --- decisions, refusals, footer: the wording is part of the security surface ---

#[test]
fn decisions_have_fixed_audit_names() {
    assert_eq!(Decision::Allowed.audit_name(), "allowed");
    assert_eq!(Decision::Blocked.audit_name(), "blocked");
    assert_eq!(Decision::MasterPaused.audit_name(), "master_paused");
    assert_eq!(Decision::AccountPaused.audit_name(), "account_paused");
    assert_eq!(
        Decision::SettingsUnreadable.audit_name(),
        "settings_unreadable"
    );
    assert!(Decision::Allowed.is_allowed());
    for d in [
        Decision::Blocked,
        Decision::MasterPaused,
        Decision::AccountPaused,
        Decision::SettingsUnreadable,
    ] {
        assert!(!d.is_allowed());
    }
}

#[test]
fn the_refusal_texts_are_pinned() {
    assert_eq!(
        REFUSAL,
        "Conversation not available: it does not exist, or the user has not allowed AI to use it in IronTeams."
    );
    assert_eq!(
        REFUSAL_CALENDAR_PEOPLE,
        "The user has not allowed AI to use the calendar and people in IronTeams."
    );
    assert_eq!(
        REFUSAL_MEETING,
        "Meeting not available: it is not shared with AI in IronTeams, or it has no Teams chat the user can share."
    );
    assert_eq!(
        REFUSAL_SELF_STATUS,
        "AI may not change the user's status or status message in this version of IronTeams."
    );
    assert_eq!(
        REFUSAL_MASTER_PAUSED,
        "AI access is paused in IronTeams. Ask the user to resume it."
    );
    assert_eq!(
        REFUSAL_ACCOUNT_PAUSED,
        "AI access is paused for this account in IronTeams. Ask the user to resume it."
    );
    assert_eq!(
        REFUSAL_UNREADABLE,
        "The AI access settings could not be read. Ask the user to open IronTeams and check them."
    );
}

#[test]
fn a_decision_maps_to_its_refusal_by_resource_kind() {
    assert_eq!(Decision::Allowed.refusal(ResourceKind::Chat), None);
    assert_eq!(Decision::Blocked.refusal(ResourceKind::Chat), Some(REFUSAL));
    assert_eq!(
        Decision::Blocked.refusal(ResourceKind::Channel),
        Some(REFUSAL)
    );
    assert_eq!(
        Decision::Blocked.refusal(ResourceKind::Calendar),
        Some(REFUSAL_CALENDAR_PEOPLE)
    );
    assert_eq!(
        Decision::Blocked.refusal(ResourceKind::Directory),
        Some(REFUSAL_CALENDAR_PEOPLE)
    );
    assert_eq!(
        Decision::Blocked.refusal(ResourceKind::Meeting),
        Some(REFUSAL_MEETING)
    );
    assert_eq!(
        Decision::Blocked.refusal(ResourceKind::SelfStatus),
        Some(REFUSAL_SELF_STATUS)
    );
    for k in [
        ResourceKind::Chat,
        ResourceKind::Channel,
        ResourceKind::Meeting,
        ResourceKind::Calendar,
        ResourceKind::Directory,
        ResourceKind::SelfStatus,
    ] {
        assert_eq!(
            Decision::MasterPaused.refusal(k),
            Some(REFUSAL_MASTER_PAUSED)
        );
        assert_eq!(
            Decision::AccountPaused.refusal(k),
            Some(REFUSAL_ACCOUNT_PAUSED)
        );
        assert_eq!(
            Decision::SettingsUnreadable.refusal(k),
            Some(REFUSAL_UNREADABLE)
        );
    }
}

#[test]
fn the_refusal_for_a_blocked_conversation_says_nothing_about_it() {
    // One text for every blocked, unknown, malformed or channel id, and it holds no id.
    assert!(!REFUSAL.contains("19:"));
    assert!(!REFUSAL.contains('{'));
    // The meeting refusal is one text for a blocked meeting and for one with no chat.
    assert!(!REFUSAL_MEETING.contains("19:") && !REFUSAL_MEETING.contains('{'));
}

#[test]
fn the_footer_is_pinned_and_carries_one_number() {
    assert_eq!(
        footer(199, false),
        "199 other conversations are not shared with AI; the user can allow them in IronTeams."
    );
    assert_eq!(
        footer(100, true),
        "At least 100 other conversations are not shared with AI; the user can allow them in IronTeams."
    );
    assert_eq!(
        footer(1, false),
        "1 other conversation is not shared with AI; the user can allow it in IronTeams."
    );
    assert_eq!(
        footer(0, false),
        "0 other conversations are not shared with AI; the user can allow them in IronTeams."
    );
}

#[test]
fn two_footers_differ_only_in_the_number() {
    let strip = |s: String| {
        s.chars()
            .filter(|c| !c.is_ascii_digit())
            .collect::<String>()
    };
    assert_eq!(strip(footer(3, false)), strip(footer(250, false)));
}

// --- the tool table and the call-level decision ---

fn gate(kind: ResourceKind, need: Need) -> Gate {
    Gate::Resource { kind, need }
}

#[test]
fn every_tool_has_exactly_one_gate_and_the_table_is_pinned() {
    use Need::{Draft, Read};
    use ResourceKind::*;
    let expected = [
        ("list_chats", gate(Chat, Read)),
        ("read_chat", gate(Chat, Read)),
        ("catch_up", gate(Chat, Read)),
        ("search_messages", gate(Chat, Read)),
        ("find_people", gate(Directory, Read)),
        ("get_presence", gate(Directory, Read)),
        ("calendar_agenda", gate(Calendar, Read)),
        ("meeting_detail", gate(Meeting, Read)),
        ("find_free_time", gate(Calendar, Read)),
        ("draft_message", gate(Chat, Draft)),
        ("draft_reaction", gate(Chat, Draft)),
        ("draft_mark_read", gate(Chat, Draft)),
        ("draft_presence", gate(SelfStatus, Draft)),
        ("draft_status_note", gate(SelfStatus, Draft)),
        ("draft_meeting_response", gate(Meeting, Draft)),
        ("send_draft", Gate::FromDraft),
        ("discard_draft", Gate::Session),
    ];
    assert_eq!(TOOL_GATES.len(), expected.len());
    for (tool, g) in expected {
        assert_eq!(gate_of(tool), Some(g), "{tool}");
    }
    let mut names: Vec<_> = TOOL_GATES.iter().map(|t| t.tool).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), TOOL_GATES.len(), "a tool is listed twice");
    assert_eq!(gate_of("not_a_tool"), None);
}

#[test]
fn a_session_decision_ignores_the_resource_but_not_the_switches() {
    let on = policy(|_| {});
    assert_eq!(decide_session(&on, &id()), Decision::Allowed);
    // An account with no scope at all is still a live session: its resources are Blocked.
    assert_eq!(decide_session(&Policy::default(), &id()), Decision::Allowed);
    let mut paused = policy(|s| s.paused = true);
    assert_eq!(decide_session(&paused, &id()), Decision::AccountPaused);
    paused.master = Master::Paused;
    assert_eq!(decide_session(&paused, &id()), Decision::MasterPaused);
    assert_eq!(
        decide_session(&Policy::deny_all("x"), &id()),
        Decision::SettingsUnreadable
    );
}

#[test]
fn a_scope_id_comes_from_the_label_tenant_and_self_mri() {
    let s = ScopeId::from_identity("work", "tenant-1", "8:orgid:0123-abcd").unwrap();
    assert_eq!(s.key, "work@tenant-1");
    assert_eq!(s.user_oid, "0123-abcd");
    for bad in ["", "8:orgid:", "8:live:someone", "48:notes", "orgid:abc"] {
        assert!(
            ScopeId::from_identity("work", "t", bad).is_none(),
            "{bad:?}"
        );
    }
}
