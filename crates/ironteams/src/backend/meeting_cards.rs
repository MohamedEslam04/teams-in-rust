//! Meeting events drawn as cards, the way Teams draws them in a meeting chat: one card per call
//! ("Meeting started", then "Meeting ended · 1h 3m 12s" with who took part), one card per
//! recording in its latest state (Play, or "Recording…", or "Recording failed") and a small
//! transcript row. Teams sends several messages for each (the call's start and end, a recording's
//! status updates); the earlier ones are left out of the chat. The messages are parsed by
//! `teams_proto::meeting_events`; anything it cannot read stays the grey line it was.

use crate::data::{color_for, initials, MeetingCardData, MeetingPersonData};
use std::collections::HashMap;
use store::MessageRow;
use teams_proto::meeting_events::{
    parse_meeting_event, CallEvent, MeetingEvent, RecordingEvent, RecordingState,
};

/// At most this many people are drawn on a call card.
const PEOPLE_SHOWN: usize = 5;

/// What each meeting-event row shows: `Some(card)`, or `None` when another row of the same call,
/// recording or transcript says it instead (the row is left out of the chat). Rows that are not
/// meeting events are not in the map.
pub(crate) type Plan = HashMap<String, Option<MeetingCardData>>;

/// The rows a chat draws (no `Control/` messages, no meeting event another row says instead) and
/// the cards of the meeting events among them.
pub(crate) fn visible_rows(rows: &[MessageRow]) -> (Vec<&MessageRow>, Plan) {
    let rows: Vec<&MessageRow> = rows
        .iter()
        .filter(|r| !r.message_type.starts_with("Control/"))
        .collect();
    let plan = plan(&rows);
    let shown = rows
        .into_iter()
        .filter(|r| !matches!(plan.get(&r.id), Some(None)))
        .collect();
    (shown, plan)
}

/// The chat's meeting is going on: its latest call card (`rows` as `visible_rows` returns them)
/// is a started one, no ended one after it. The header's Join button is filled on it.
pub(crate) fn is_live(rows: &[&MessageRow], plan: &Plan) -> bool {
    rows.iter()
        .rev()
        .find_map(|r| match plan.get(&r.id) {
            Some(Some(c)) if c.kind == "call" => Some(c.state == "started"),
            _ => None,
        })
        .unwrap_or(false)
}

/// When the chat's running call started, by the server's clock: the time of its "Meeting started"
/// message, among the chat's newest `rows`. `None` when no call is running, as in [`is_live`].
pub(crate) fn live_call_start_ms(rows: &[MessageRow]) -> Option<i64> {
    let (shown, plan) = visible_rows(rows);
    let (row, card) = shown.iter().rev().find_map(|r| match plan.get(&r.id) {
        Some(Some(c)) if c.kind == "call" => Some((r, c)),
        _ => None,
    })?;
    (card.state == "started").then_some(row.compose_ms)
}

fn is_meeting_chat(id: &str) -> bool {
    id.starts_with("19:meeting_")
}

fn noun(conv: &str) -> &'static str {
    if is_meeting_chat(conv) {
        "Meeting"
    } else {
        "Call"
    }
}

/// `1h 3m 12s`, `4m 5s`, `9s`.
pub(crate) fn long_duration(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// `1h 3m`, `4m`, `9s`: the chat list's shorter form.
pub(crate) fn short_duration(secs: i64) -> String {
    let (h, m) = (secs / 3600, secs % 3600 / 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

/// The one line a meeting event reads as (the chat list's preview, the row's copied text);
/// `None` when the message is not one or cannot be read.
pub(crate) fn summary(message_type: &str, content: &str, conv: &str) -> Option<String> {
    Some(match parse_meeting_event(message_type, content)? {
        MeetingEvent::Call(c) => match c.duration_secs.filter(|_| c.ended) {
            Some(secs) => format!("{} ended · {}", noun(conv), short_duration(secs)),
            None if c.ended => format!("{} ended", noun(conv)),
            None => format!("{} started", noun(conv)),
        },
        MeetingEvent::Recording(r) => match r.state {
            RecordingState::Ready if !r.title.is_empty() => format!("Recording: {}", r.title),
            RecordingState::Ready => "Recording".into(),
            RecordingState::InProgress => "Recording…".into(),
            RecordingState::Failed => "Recording failed".into(),
        },
        MeetingEvent::Transcript { .. } => "Transcription started".into(),
    })
}

fn person(p: &teams_proto::meeting_events::CallPart) -> MeetingPersonData {
    MeetingPersonData {
        name: p.name.clone(),
        initials: initials(&p.name),
        color: color_for(&p.mri),
    }
}

fn call_card(c: &CallEvent, started: &MessageRow) -> MeetingCardData {
    let conv = &started.conversation_id;
    let (title, detail) = if c.ended {
        let title = match c.duration_secs {
            Some(s) => format!("{} ended · {}", noun(conv), long_duration(s)),
            None => format!("{} ended", noun(conv)),
        };
        let detail = match c.count {
            0 => String::new(),
            1 => "1 participant".into(),
            n => format!("{n} participants"),
        };
        (title, detail)
    } else {
        let at = appearance::formats::local(started.compose_ms)
            .map(|t| appearance::formats::time_of(&t))
            .unwrap_or_default();
        (format!("{} started", noun(conv)), at)
    };
    let people: Vec<MeetingPersonData> = c.people.iter().take(PEOPLE_SHOWN).map(person).collect();
    MeetingCardData {
        kind: "call",
        title,
        detail,
        more: c.people.len().saturating_sub(people.len()) as i32,
        people,
        state: if c.ended { "ended" } else { "started" },
        url: String::new(),
        recap_chat: if c.ended && is_meeting_chat(conv) {
            conv.clone()
        } else {
            String::new()
        },
        join_chat: if !c.ended && is_meeting_chat(conv) {
            conv.clone()
        } else {
            String::new()
        },
    }
}

fn recording_card(r: &RecordingEvent, names: &HashMap<String, String>) -> MeetingCardData {
    let detail = match r.state {
        RecordingState::Ready => {
            let mut parts = Vec::new();
            if let Some(ms) = r.duration_ms {
                parts.push(long_duration(ms / 1000));
            }
            if let Some(name) = r.initiator.as_ref().and_then(|m| names.get(m)) {
                parts.push(format!("Recorded by {name}"));
            }
            parts.join(" · ")
        }
        RecordingState::InProgress => "Recording…".into(),
        RecordingState::Failed => "Recording failed".into(),
    };
    MeetingCardData {
        kind: "recording",
        title: if r.title.is_empty() {
            "Meeting recording".into()
        } else {
            r.title.clone()
        },
        detail,
        people: Vec::new(),
        more: 0,
        state: match r.state {
            RecordingState::Ready => "ready",
            RecordingState::InProgress => "progress",
            RecordingState::Failed => "failed",
        },
        url: r
            .url
            .clone()
            .filter(|_| r.state == RecordingState::Ready)
            .unwrap_or_default(),
        recap_chat: String::new(),
        join_chat: String::new(),
    }
}

/// The cards of the meeting events among `rows` (in chat order), see [`Plan`].
pub(crate) fn plan(rows: &[&MessageRow]) -> Plan {
    let events: Vec<(&MessageRow, MeetingEvent)> = rows
        .iter()
        .filter_map(|r| Some((*r, parse_meeting_event(&r.message_type, &r.content_html)?)))
        .collect();
    let mut plan = Plan::new();
    if events.is_empty() {
        return plan;
    }
    // Who is who, for "Recorded by": the people of the calls, then the senders.
    let mut names: HashMap<String, String> = HashMap::new();
    for (r, e) in &events {
        if let MeetingEvent::Call(c) = e {
            for p in &c.people {
                names.insert(p.mri.clone(), p.name.clone());
            }
        }
        if !r.sender_name.is_empty() {
            names
                .entry(r.sender_mri.clone())
                .or_insert_with(|| r.sender_name.clone());
        }
    }
    // A call: one card where it started, showing how it ended.
    let call_key = |r: &MessageRow, c: &CallEvent| -> String {
        if c.call_id.is_empty() {
            r.id.clone()
        } else {
            c.call_id.clone()
        }
    };
    let mut calls: Vec<(String, Vec<(&MessageRow, &CallEvent)>)> = Vec::new();
    for (r, e) in &events {
        if let MeetingEvent::Call(c) = e {
            let key = call_key(r, c);
            match calls.iter_mut().find(|(k, _)| *k == key) {
                Some((_, list)) => list.push((*r, c)),
                None => calls.push((key, vec![(*r, c)])),
            }
        }
    }
    for (_, list) in &calls {
        let host = list
            .iter()
            .find(|(_, c)| !c.ended)
            .or_else(|| list.iter().rev().find(|(_, c)| c.ended))
            .unwrap_or(&list[0]);
        let shown = list.iter().rev().find(|(_, c)| c.ended).unwrap_or(host);
        for (r, _) in list {
            plan.insert(r.id.clone(), None);
        }
        plan.insert(host.0.id.clone(), Some(call_card(shown.1, host.0)));
    }
    // A recording: only its latest state; a finished one stays unless the same one comes again.
    let recordings: Vec<(&MessageRow, &RecordingEvent)> = events
        .iter()
        .filter_map(|(r, e)| match e {
            MeetingEvent::Recording(x) => Some((*r, x)),
            _ => None,
        })
        .collect();
    for (i, (r, rec)) in recordings.iter().enumerate() {
        let replaced = recordings[i + 1..].iter().any(|(_, later)| {
            later.key() == rec.key()
                && (rec.state != RecordingState::Ready
                    || (later.state == RecordingState::Ready
                        && later.document_id == rec.document_id))
        });
        plan.insert(
            r.id.clone(),
            (!replaced).then(|| recording_card(rec, &names)),
        );
    }
    // A transcript: one row per call.
    let transcripts: Vec<(&MessageRow, &str)> = events
        .iter()
        .filter_map(|(r, e)| match e {
            MeetingEvent::Transcript { call_id } => Some((*r, call_id.as_str())),
            _ => None,
        })
        .collect();
    for (i, (r, call)) in transcripts.iter().enumerate() {
        let again = transcripts[i + 1..]
            .iter()
            .any(|(_, c)| !call.is_empty() && c == call);
        let done = recordings.iter().any(|(_, x)| {
            x.state == RecordingState::Ready
                && x.transcript
                && (call.is_empty() || x.call_id.as_deref() == Some(*call))
        });
        let card = MeetingCardData {
            kind: "transcript",
            title: if done {
                "Transcript available".into()
            } else {
                "Transcription started".into()
            },
            detail: String::new(),
            people: Vec::new(),
            more: 0,
            state: if done { "ready" } else { "progress" },
            url: String::new(),
            recap_chat: if is_meeting_chat(&r.conversation_id) {
                r.conversation_id.clone()
            } else {
                String::new()
            },
            join_chat: String::new(),
        };
        plan.insert(r.id.clone(), (!again).then_some(card));
    }
    plan
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const CHAT: &str = "19:meeting_made-up-chat@thread.v2";

    pub(crate) fn row(id: &str, at: i64, message_type: &str, content: &str) -> MessageRow {
        MessageRow {
            id: id.into(),
            conversation_id: CHAT.into(),
            parent_id: None,
            sender_mri: "8:orgid:aaaa-1".into(),
            sender_name: String::new(),
            compose_ms: 1_795_000_000_000 + at * 1000,
            edited_ms: None,
            deleted: false,
            message_type: message_type.into(),
            content_html: content.into(),
            client_message_id: None,
            reactions_json: "{}".into(),
        }
    }

    pub(crate) fn started() -> String {
        "<partlist alt =\"\"></partlist><callEventType>callStarted</callEventType>\n<callId>call-1</callId>".into()
    }

    pub(crate) fn ended() -> String {
        let parts: String = [
            ("aaaa-1", "Ada Lovelace"),
            ("bbbb-2", "Grace Hopper"),
            ("cccc-3", "Alan Turing"),
            ("dddd-4", "Edsger Dijkstra"),
            ("eeee-5", "Barbara Liskov"),
            ("ffff-6", "Donald Knuth"),
        ]
        .iter()
        .map(|(id, name)| {
            format!(
                "<part identity=\"8:orgid:{id}\"><name>8:orgid:{id}</name><displayName>{name}</displayName><duration>3792</duration></part>"
            )
        })
        .collect();
        format!(
            "<ended/><partlist alt=\"\" count=\"7\">{parts}<part identity=\"28:bot\"><name>28:bot</name><displayName></displayName><duration>3792</duration></part></partlist><meetingDetails><meetingDetails><isRecapEnabled>true</isRecapEnabled></meetingDetails></meetingDetails><callEventType>callEnded</callEventType>\n<callId>call-1</callId>"
        )
    }

    pub(crate) fn recording(status: &str, types: &str) -> String {
        let (href, duration) = if status == "Success" {
            (
                "https://contoso-my.sharepoint.invalid/:v:/g/rec",
                "0:58:35.6",
            )
        } else {
            ("", "0:00:00")
        };
        format!(
            r#"<URIObject type="Video.2/CallRecording.1" uri="" version="1.0"><RecordingStatus status="{status}" code="0"><amsErrorResult /></RecordingStatus><Title>Weekly sync</Title><a href="{href}">Play</a><OriginalName v="Weekly sync-20261110_1230-Meeting Recording.mp4" /><RecordingInitiatorId value="8:orgid:bbbb-2" isAutoRecording="False" /><Identifiers><Id type="callId" value="call-1" /><Id type="AMSDocumentID" value="" /></Identifiers><RecordingContent contentTypes="{types}" duration="{duration}"><item type="onedriveForBusinessVideo" uri="{href}" /></RecordingContent></URIObject>"#
        )
    }

    pub(crate) fn transcript() -> String {
        r#"{\"scopeId\":\"call-1\",\"callId\":\"call-1\",\"isExportedToOdsp\":true}"#.into()
    }

    /// A whole meeting as Teams sends it: the call starts, recording and transcription start,
    /// someone talks, the call ends, the recording finishes processing.
    pub(crate) fn meeting() -> Vec<MessageRow> {
        const REC: &str = "RichText/Media_CallRecording";
        vec![
            row("c1", 0, "Event/Call", &started()),
            row("t1", 60, "RichText/Media_CallTranscript", &transcript()),
            row("r1", 61, REC, &recording("Initial", "Recording")),
            row("r2", 61, REC, &recording("Initial", "Transcript")),
            row("h1", 120, "RichText/Html", "<p>hello</p>"),
            row("r3", 3800, REC, &recording("ChunkFinished", "Transcript")),
            row("r4", 3800, REC, &recording("ChunkFinished", "Recording")),
            row("c2", 3800, "Event/Call", &ended()),
            row(
                "r5",
                3900,
                REC,
                &recording("Success", "Recording+Transcript"),
            ),
        ]
    }

    #[test]
    fn a_meeting_collapses_to_one_call_card_one_recording_and_one_transcript_row() {
        let rows = meeting();
        let (shown, plan) = visible_rows(&rows);
        let ids: Vec<&str> = shown.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["c1", "t1", "h1", "r5"]);

        let call = plan["c1"].clone().expect("the call card");
        assert_eq!(call.kind, "call");
        assert_eq!(call.title, "Meeting ended · 1h 3m 12s");
        assert_eq!(call.detail, "7 participants");
        assert_eq!(call.people.len(), 5);
        assert_eq!(call.people[0].name, "Ada Lovelace");
        assert_eq!(call.people[0].initials, "AL");
        assert_eq!(call.more, 1);
        assert_eq!(call.recap_chat, CHAT);

        let rec = plan["r5"].clone().expect("the recording card");
        assert_eq!(rec.kind, "recording");
        assert_eq!(rec.state, "ready");
        assert_eq!(rec.title, "Weekly sync-20261110_1230-Meeting Recording");
        assert_eq!(rec.detail, "58m 35s · Recorded by Grace Hopper");
        assert_eq!(rec.url, "https://contoso-my.sharepoint.invalid/:v:/g/rec");

        let tr = plan["t1"].clone().expect("the transcript row");
        assert_eq!(tr.title, "Transcript available");
        assert_eq!(tr.recap_chat, CHAT);
    }

    #[test]
    fn a_meeting_still_going_shows_started_and_recording() {
        let rows: Vec<MessageRow> = meeting().into_iter().take(5).collect();
        let (shown, plan) = visible_rows(&rows);
        let ids: Vec<&str> = shown.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["c1", "t1", "r2", "h1"]);
        let call = plan["c1"].clone().unwrap();
        assert_eq!(call.title, "Meeting started");
        assert_eq!(call.state, "started");
        assert!(call.recap_chat.is_empty());
        let rec = plan["r2"].clone().unwrap();
        assert_eq!(rec.state, "progress");
        assert_eq!(rec.detail, "Recording…");
        assert!(rec.url.is_empty());
        assert_eq!(plan["t1"].clone().unwrap().title, "Transcription started");
    }

    #[test]
    fn a_started_call_card_in_a_meeting_chat_offers_join() {
        let rows: Vec<MessageRow> = meeting().into_iter().take(1).collect();
        let (_, plan) = visible_rows(&rows);
        let call = plan["c1"].clone().unwrap();
        assert_eq!(call.state, "started");
        assert_eq!(call.join_chat, CHAT);
    }

    #[test]
    fn an_ended_call_card_or_a_call_in_another_chat_offers_no_join() {
        let rows = meeting();
        let (_, plan) = visible_rows(&rows);
        assert!(plan["c1"].clone().unwrap().join_chat.is_empty());
        // Recordings and transcripts never offer it.
        assert!(plan["r5"].clone().unwrap().join_chat.is_empty());
        assert!(plan["t1"].clone().unwrap().join_chat.is_empty());

        let mut group = row("g1", 0, "Event/Call", &started());
        group.conversation_id = "19:made-up-group@thread.v2".into();
        let call = super::plan(&[&group])["g1"].clone().unwrap();
        assert_eq!(call.state, "started");
        assert!(call.join_chat.is_empty());
    }

    /// `is_live` of `rows` as the chat draws them.
    fn live(rows: &[MessageRow]) -> bool {
        let (shown, plan) = visible_rows(rows);
        is_live(&shown, &plan)
    }

    #[test]
    fn a_meeting_is_live_from_its_start_until_its_end() {
        assert!(!live(&[]));
        assert!(!live(&[row("h1", 0, "RichText/Html", "<p>hello</p>")]));
        let all = meeting();
        assert!(live(&all[..1]), "started");
        assert!(live(&all[..5]), "started, still talking");
        assert!(!live(&all[..8]), "ended");
        assert!(!live(&all), "ended, recording processed");
    }

    use super::live_call_start_ms as live_start;

    #[test]
    fn the_running_call_started_when_its_started_message_says() {
        assert_eq!(live_start(&[]), None);
        let all = meeting();
        let at = all[0].compose_ms;
        assert_eq!(live_start(&all[..1]), Some(at), "started");
        assert_eq!(live_start(&all[..5]), Some(at), "still talking");
        assert_eq!(live_start(&all[..8]), None, "ended");
        // A later occurrence of the meeting counts from its own start.
        let mut rows = all.clone();
        let second = |id: &str, at: i64, body: String| {
            row(id, at, "Event/Call", &body.replace("call-1", "call-2"))
        };
        rows.push(second("c3", 90_000, started()));
        assert_eq!(live_start(&rows), Some(rows.last().unwrap().compose_ms));
    }

    #[test]
    fn only_the_latest_call_decides_if_the_meeting_is_live() {
        let second = |id: &str, at: i64, body: String| {
            row(id, at, "Event/Call", &body.replace("call-1", "call-2"))
        };
        let mut rows = meeting();
        rows.push(second("c3", 90_000, started()));
        assert!(live(&rows), "a later occurrence started");
        rows.push(second("c4", 93_600, ended()));
        assert!(!live(&rows), "and ended");
        // A start that never got its end, followed by a later call that ended.
        let mut lost = vec![row("c1", 0, "Event/Call", &started())];
        lost.push(second("c3", 90_000, ended()));
        assert!(!live(&lost));
    }

    #[test]
    fn an_ended_call_without_its_start_is_drawn_where_it_ended() {
        let rows = vec![row("c2", 0, "Event/Call", &ended())];
        let (shown, plan) = visible_rows(&rows);
        assert_eq!(shown.len(), 1);
        assert_eq!(
            plan["c2"].clone().unwrap().title,
            "Meeting ended · 1h 3m 12s"
        );
    }

    #[test]
    fn a_failed_recording_says_so() {
        let failed = row(
            "r1",
            0,
            "RichText/Media_CallRecording",
            &recording("Failure", "Recording"),
        );
        let card = plan(&[&failed])["r1"].clone().unwrap();
        assert_eq!(card.state, "failed");
        assert_eq!(card.detail, "Recording failed");
    }

    #[test]
    fn unreadable_events_are_left_as_they_were() {
        let rows = vec![
            row("x1", 0, "Event/Call", "<something/>"),
            row("x2", 1, "RichText/Media_CallRecording", "<p>?</p>"),
        ];
        let (shown, plan) = visible_rows(&rows);
        assert_eq!(shown.len(), 2);
        assert!(plan.is_empty());
    }

    #[test]
    fn the_summary_is_the_chat_list_line() {
        assert_eq!(
            summary("Event/Call", &ended(), CHAT).as_deref(),
            Some("Meeting ended · 1h 3m")
        );
        assert_eq!(
            summary("Event/Call", &started(), "19:abc@thread.v2").as_deref(),
            Some("Call started")
        );
        assert_eq!(
            summary(
                "RichText/Media_CallRecording",
                &recording("Success", "Recording"),
                CHAT
            )
            .as_deref(),
            Some("Recording: Weekly sync-20261110_1230-Meeting Recording")
        );
        assert_eq!(
            summary(
                "RichText/Media_CallRecording",
                &recording("ChunkFinished", "Recording"),
                CHAT
            )
            .as_deref(),
            Some("Recording…")
        );
        assert_eq!(summary("RichText/Html", "<p>hi</p>", CHAT), None);
    }

    #[test]
    fn durations_read_like_teams() {
        assert_eq!(long_duration(3792), "1h 3m 12s");
        assert_eq!(long_duration(245), "4m 5s");
        assert_eq!(long_duration(9), "9s");
        assert_eq!(short_duration(3792), "1h 3m");
        assert_eq!(short_duration(245), "4m");
        assert_eq!(short_duration(9), "9s");
    }
}
