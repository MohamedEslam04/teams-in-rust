//! A meeting join with video lines, offline: the offer's layout and attributes (the web client's),
//! its data line, the join body (declaring the lines only when asked to), and an answer that
//! refuses every video line (port 0), after which neither the camera nor the share is live.
//!
//! Live 2026-11-20: a join offering audio, camera, sharing, nine gallery lines and data, declared
//! `callModalities: ["Audio"]` with no `mediaDescriptions`, was answered with every video line at
//! port 0; sharing then posted `UpdateMediaDescriptions` for the refused line (400, 1047 "Invalid
//! MediaDescriptions") and the service ended the call (410). Every value here is synthetic.

use calling::media::sdp::video_mids;
use calling::media::{MediaOptions, MediaSession, VideoLayout};
use calling::signaling::bodies::{declared_video_mids, join_meeting_body};
use calling::signaling::video::JOIN_REQUEST_ID;
use calling::signaling::{CallSession, CallbackLinks, LocalEndpoint, MeetingInfo};
use calling::video::VideoRole;
use calling::{Modality, SdpBlob};
use serde_json::json;

const FULL_REOFFER: &str = include_str!("fixtures/media/teams_meeting_full_reoffer.sdp");
const MEETING_ANSWER: &str = include_str!("fixtures/media/teams_meeting_answer.sdp");
const SIGNALING_FB: &str = "a=x-signaling-fb:* x-message app send:src recv:src,vc";

fn join_options(layout: VideoLayout) -> MediaOptions {
    MediaOptions {
        udp_addrs: vec!["127.0.0.1:0".into()],
        video: vec![VideoRole::MainVideo, VideoRole::ApplicationSharing],
        receive_slots: 9,
        layout,
        ..Default::default()
    }
}

fn sections(sdp: &str) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    for line in sdp.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        if line.starts_with("m=") {
            out.push(vec![line]);
        } else if let Some(s) = out.last_mut() {
            s.push(line);
        }
    }
    out
}

fn attr<'a>(section: &[&'a str], name: &str) -> Option<&'a str> {
    section
        .iter()
        .find_map(|l| l.strip_prefix("a=")?.strip_prefix(name)?.strip_prefix(':'))
}

/// (mid, label, direction) of each video section.
fn video_layout(offer: &str) -> Vec<(String, String, String)> {
    sections(offer)
        .iter()
        .filter(|s| s[0].starts_with("m=video "))
        .map(|s| {
            let direction = ["sendrecv", "sendonly", "recvonly", "inactive"]
                .into_iter()
                .find(|d| s.contains(&format!("a={d}").as_str()))
                .unwrap_or("sendrecv");
            (
                attr(s, "mid").unwrap_or_default().to_owned(),
                attr(s, "label").unwrap_or_default().to_owned(),
                direction.to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_meeting_join_offers_the_web_client_layout() {
    let (session, _events) =
        MediaSession::with_options(vec![], join_options(VideoLayout::WebClient))
            .await
            .unwrap();
    let offer = session.create_offer().await.unwrap();
    let mut want = vec![(
        "1".to_owned(),
        "main-video".to_owned(),
        "sendrecv".to_owned(),
    )];
    for mid in 2..=10 {
        want.push((mid.to_string(), "main-video".into(), "recvonly".into()));
    }
    want.push((
        "11".into(),
        "applicationsharing-video".into(),
        "sendrecv".into(),
    ));
    assert_eq!(video_layout(&offer), want, "{offer}");
    assert_eq!(
        session
            .video_mid(VideoRole::ApplicationSharing)
            .await
            .as_deref(),
        Some("11")
    );
    assert_eq!(
        session.video_mid(VideoRole::MainVideo).await.as_deref(),
        Some("1")
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn every_video_line_declares_signaling_feedback_and_an_ssrc_range() {
    let (session, _events) =
        MediaSession::with_options(vec![], join_options(VideoLayout::WebClient))
            .await
            .unwrap();
    let offer = session.create_offer().await.unwrap();
    let video: Vec<_> = sections(&offer)
        .into_iter()
        .filter(|s| s[0].starts_with("m=video "))
        .collect();
    assert_eq!(video.len(), 11, "{offer}");
    for s in &video {
        assert!(s.contains(&SIGNALING_FB), "{s:?}");
        assert_eq!(
            s.iter()
                .filter(|l| l.starts_with("a=x-ssrc-range:"))
                .count(),
            1,
            "{s:?}"
        );
    }
    // The gallery's receive-only lines send nothing: the web client's placeholder range.
    let gallery = &video[1];
    assert_eq!(attr(gallery, "x-ssrc-range"), Some("1-1"), "{gallery:?}");
    // Audio is unchanged: it was always accepted.
    let audio = &sections(&offer)[0];
    assert!(!audio.iter().any(|l| l.starts_with("a=x-signaling-fb:")));
    session.close().await.unwrap();
}

#[tokio::test]
async fn the_sharing_second_layout_keeps_the_apps_first_order() {
    let (session, _events) =
        MediaSession::with_options(vec![], join_options(VideoLayout::SharingSecond))
            .await
            .unwrap();
    let offer = session.create_offer().await.unwrap();
    let layout = video_layout(&offer);
    assert_eq!(layout[1].0, "2");
    assert_eq!(layout[1].1, "applicationsharing-video");
    assert_eq!(layout[2].1, "main-video");
    assert_eq!(layout.len(), 11);
    session.close().await.unwrap();
}

fn session_ids() -> CallSession {
    CallSession {
        me: LocalEndpoint {
            mri: "8:orgid:00000000-0000-0000-0000-0000000000b2".into(),
            display_name: "Me Example".into(),
            endpoint_id: "sess-1".into(),
            participant_id: "part-1".into(),
            language_id: "en-US".into(),
        },
        callbacks: CallbackLinks::new("https://trouter.example.invalid/v4/f/AbCd/", "sess-1"),
        chain_id: "chain-1".into(),
        media_leg_id: "LEG1".into(),
    }
}

fn meeting() -> MeetingInfo {
    MeetingInfo {
        thread_id: "19:meeting_U3ludGhldGlj@thread.v2".into(),
        tenant_id: "00000000-0000-0000-0000-000000000000".into(),
        organizer_id: "00000000-0000-0000-0000-0000000000c3".into(),
    }
}

/// Live 2026-11-20 on b1d38a7: a join declaring every video mid (1-11) `recvonly` in
/// `mediaDescriptions`, with `ScreenViewer` and a bandwidth seed, was taken (201) and ended within
/// 1.5 s: 410, 301999 "InvalidFinalAnswerMediaDescriptions". Unless asked to, a join with video
/// lines declares nothing, as the join that kept audio up did.
#[tokio::test]
async fn a_join_with_video_lines_declares_nothing_by_default() {
    let (session, _events) =
        MediaSession::with_options(vec![], join_options(VideoLayout::WebClient))
            .await
            .unwrap();
    let offer = session.create_offer().await.unwrap();
    let b = join_meeting_body(
        &session_ids(),
        &meeting(),
        &SdpBlob::ngc(offer),
        &[Modality::Audio],
    );
    let inv = &b["callInvitation"];
    assert_eq!(inv["callModalities"], json!(["Audio"]));
    assert!(inv["mediaContent"].get("mediaDescriptions").is_none());
    assert!(inv["mediaContent"].get("applyChannelParameters").is_none());
    session.close().await.unwrap();
}

/// Asked to (`ScreenViewer` among the modalities: `IRONTEAMS_MEETING_VIDEO_DESCRIBE=1`), a join
/// declares only its send-capable video lines, as the web client's captured join does (mids 1-6
/// and 11, the lines its SDP offers `sendrecv`; never its `recvonly` 7-10): here the camera (1)
/// and the sharing line (11), not the receive-only gallery (2-10).
#[tokio::test]
async fn a_join_asked_to_declares_only_its_send_capable_video_lines() {
    let (session, _events) =
        MediaSession::with_options(vec![], join_options(VideoLayout::WebClient))
            .await
            .unwrap();
    let offer = session.create_offer().await.unwrap();
    let b = join_meeting_body(
        &session_ids(),
        &meeting(),
        &SdpBlob::ngc(offer),
        &[Modality::Audio, Modality::ScreenViewer],
    );
    let inv = &b["callInvitation"];
    assert_eq!(inv["callModalities"], json!(["Audio", "ScreenViewer"]));
    let media = &inv["mediaContent"];
    assert_eq!(
        media["mediaDescriptions"],
        json!({
            "descriptions": [
                {"mid": "1", "direction": "recvonly"},
                {"mid": "11", "direction": "recvonly"},
            ],
            "requestId": JOIN_REQUEST_ID,
        })
    );
    assert_eq!(
        media["applyChannelParameters"]["multiChannelParameter"]["mids"],
        json!(["*"])
    );
    session.close().await.unwrap();
}

/// What a join declared is what a meeting may later switch with `UpdateMediaDescriptions`: the
/// mids of its `mediaDescriptions`, none by default. Live 2026-11-20: after a default join (no
/// descriptions), an update naming the video lines was refused (400, 1047 "Invalid
/// MediaDescriptions") and the call ended (410, 301999 "InvalidUpdatedMediaDescriptions").
#[tokio::test]
async fn the_declared_mids_are_the_join_bodys_descriptions() {
    let (session, _events) =
        MediaSession::with_options(vec![], join_options(VideoLayout::WebClient))
            .await
            .unwrap();
    let offer = session.create_offer().await.unwrap();
    assert!(declared_video_mids(&offer, &[Modality::Audio]).is_empty());
    let asked = [Modality::Audio, Modality::ScreenViewer];
    let declared = declared_video_mids(&offer, &asked);
    assert_eq!(declared, ["1", "11"]);
    let b = join_meeting_body(&session_ids(), &meeting(), &SdpBlob::ngc(offer), &asked);
    let named: Vec<_> = b["callInvitation"]["mediaContent"]["mediaDescriptions"]["descriptions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["mid"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(named, declared);
    assert!(declared_video_mids(MEETING_ANSWER, &asked).is_empty());
    session.close().await.unwrap();
}

/// A join with receive-only video lines alone, asked to declare them, declares an empty list, as
/// rusty-teams joins a meeting (`descriptions: []`).
#[tokio::test]
async fn a_receive_only_join_asked_to_declares_an_empty_list() {
    let options = MediaOptions {
        udp_addrs: vec!["127.0.0.1:0".into()],
        receive_slots: 2,
        ..Default::default()
    };
    let (session, _events) = MediaSession::with_options(vec![], options).await.unwrap();
    let offer = session.create_offer().await.unwrap();
    let b = join_meeting_body(
        &session_ids(),
        &meeting(),
        &SdpBlob::ngc(offer),
        &[Modality::Audio, Modality::ScreenViewer],
    );
    assert_eq!(
        b["callInvitation"]["mediaContent"]["mediaDescriptions"],
        json!({"descriptions": [], "requestId": JOIN_REQUEST_ID})
    );
    session.close().await.unwrap();
}

/// An audio-only offer asked to declare video still joins with audio alone (mautrix-teams
/// `videoMedia`: no video mids, `["Audio"]`).
#[test]
fn an_audio_only_join_asked_to_declares_nothing() {
    let b = join_meeting_body(
        &session_ids(),
        &meeting(),
        &SdpBlob::ngc(MEETING_ANSWER),
        &[Modality::Audio, Modality::ScreenViewer],
    );
    let inv = &b["callInvitation"];
    assert_eq!(inv["callModalities"], json!(["Audio"]));
    assert!(inv["mediaContent"].get("mediaDescriptions").is_none());
}

/// The meeting join's data line (live captions) is still offered with the web client's video
/// layout: in Teams' `m=x-data` form, after every video line (mid 12, as the web client's).
#[tokio::test]
async fn a_meeting_join_still_offers_the_data_line() {
    let options = MediaOptions {
        data_channel: true,
        ..join_options(VideoLayout::WebClient)
    };
    let (session, _events) = MediaSession::with_options(vec![], options).await.unwrap();
    let offer = session.create_offer().await.unwrap();
    let all = sections(&offer);
    let data: Vec<_> = all
        .iter()
        .filter(|s| s[0].starts_with("m=x-data "))
        .collect();
    assert_eq!(data.len(), 1, "{offer}");
    let data = data[0];
    assert!(!data[0].starts_with("m=x-data 0 "), "{offer}");
    assert_eq!(attr(data, "mid"), Some("12"), "{offer}");
    assert_eq!(attr(data, "label"), Some("data"), "{offer}");
    assert!(data.contains(&"a=x-data-protocol:sctp"), "{data:?}");
    assert!(!offer.contains("m=application"), "{offer}");
    assert_eq!(all.last().map(|s| s[0]), Some(data[0]), "{offer}");
    session.close().await.unwrap();
}

#[test]
fn an_audio_only_join_stays_as_it_was_verified() {
    let b = join_meeting_body(
        &session_ids(),
        &meeting(),
        &SdpBlob::ngc(MEETING_ANSWER),
        &[Modality::Audio],
    );
    let inv = &b["callInvitation"];
    assert_eq!(inv["callModalities"], json!(["Audio"]));
    assert!(inv["mediaContent"].get("mediaDescriptions").is_none());
    assert!(inv["mediaContent"].get("applyChannelParameters").is_none());
}

/// The media servers' answer to `offer` as the live run had it: audio accepted, every video line
/// refused with a bare port-0 line (the camera and sharing lines keep their label, no mid).
fn refusing_answer(offer: &str) -> String {
    let answer = sections(MEETING_ANSWER);
    let head: Vec<&str> = MEETING_ANSWER
        .lines()
        .map(str::trim_end)
        .take_while(|l| !l.starts_with("m="))
        .filter(|l| !l.is_empty())
        .collect();
    let mut out: Vec<String> = head.iter().map(|l| (*l).to_owned()).collect();
    out.extend(answer[0].iter().map(|l| (*l).to_owned()));
    for s in sections(offer).iter().skip(1) {
        let label = attr(s, "label").unwrap_or_default();
        if s[0].starts_with("m=video ") {
            if label == "main-video" && s.contains(&"a=recvonly") {
                out.push("m=video 0 RTP/SAVP 107 108".into());
            } else {
                out.push("m=video 0 RTP/AVP 107".into());
                out.push(format!("a=label:{label}"));
            }
        }
    }
    out.join("\r\n") + "\r\n"
}

#[tokio::test]
async fn an_answer_refusing_every_video_line_leaves_neither_camera_nor_share_live() {
    let (session, _events) =
        MediaSession::with_options(vec![], join_options(VideoLayout::WebClient))
            .await
            .unwrap();
    let offer = session.create_offer().await.unwrap();
    let answer = refusing_answer(&offer);
    assert!(video_mids(&answer).is_empty(), "{answer}");
    session.set_answer(&answer).await.unwrap();
    // The lines exist on our side, but the server refused them.
    assert!(session.video_mid(VideoRole::MainVideo).await.is_some());
    assert_eq!(session.live_video_mid(VideoRole::MainVideo).await, None);
    assert_eq!(
        session.live_video_mid(VideoRole::ApplicationSharing).await,
        None
    );
    assert!(session.live_video_mids().is_empty());
    session.close().await.unwrap();
}

#[tokio::test]
async fn a_line_the_media_server_offered_and_we_answered_is_live() {
    let options = MediaOptions {
        udp_addrs: vec!["127.0.0.1:0".into()],
        video: vec![VideoRole::MainVideo, VideoRole::ApplicationSharing],
        ..Default::default()
    };
    let (session, _events) = MediaSession::with_options(vec![], options).await.unwrap();
    assert_eq!(session.live_video_mid(VideoRole::MainVideo).await, None);
    session.accept_offer(FULL_REOFFER).await.unwrap();
    assert_eq!(
        session
            .live_video_mid(VideoRole::ApplicationSharing)
            .await
            .as_deref(),
        Some("3")
    );
    assert_eq!(
        session
            .live_video_mid(VideoRole::MainVideo)
            .await
            .as_deref(),
        Some("2")
    );
    assert_eq!(session.live_video_mids(), vec!["2", "3"]);
    session.close().await.unwrap();
}

/// The media restart after lobby admission is answered on a fresh session built as the join's
/// (live 2026-11-25: one built without the gallery lines logged `slots=0`, nothing subscribed).
/// webrtc-rs gives the offer's `main-video` lines to the receive-only slots first, in order, the
/// last one to the camera; the sharing line stays the sharing line.
#[tokio::test]
async fn a_fresh_meeting_session_places_its_gallery_lines_on_the_restart_offer() {
    let (session, _events) =
        MediaSession::with_options(vec![], join_options(VideoLayout::WebClient))
            .await
            .unwrap();
    session.accept_offer(FULL_REOFFER).await.unwrap();
    assert_eq!(
        session.slot_mids().await,
        ["2", "5", "6", "7", "8", "9", "10", "11", "12"]
    );
    assert_eq!(
        session
            .video_mid(VideoRole::ApplicationSharing)
            .await
            .as_deref(),
        Some("3")
    );
    assert_eq!(
        session.video_mid(VideoRole::MainVideo).await.as_deref(),
        Some("13")
    );
    session.close().await.unwrap();
}

#[test]
fn an_update_names_every_live_line_and_only_live_ones() {
    use calling::signaling::video::meeting_descriptions;
    let live: Vec<String> = ["1", "2", "3", "11"].map(String::from).to_vec();
    let lines = meeting_descriptions(
        &live,
        &[
            ("11".into(), VideoRole::ApplicationSharing),
            // A refused line (not live) is never named, even while it would send.
            ("4".into(), VideoRole::MainVideo),
        ],
    );
    let wire: Vec<_> = lines.iter().map(|l| l.wire()).collect();
    assert_eq!(
        wire,
        vec![
            json!({"mid": "1", "direction": "recvonly"}),
            json!({"mid": "2", "direction": "recvonly"}),
            json!({"mid": "3", "direction": "recvonly"}),
            json!({"mid": "11", "direction": "sendonly", "label": "applicationsharing-video"}),
        ]
    );
    assert!(meeting_descriptions(&[], &[("11".into(), VideoRole::ApplicationSharing)]).is_empty());
}
