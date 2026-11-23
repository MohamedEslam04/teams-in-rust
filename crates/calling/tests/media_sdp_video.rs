//! The SDP rewrites for a session with video lines (screen share; the camera later): which
//! sections webrtc-rs is given, how a media server's re-offer is bundled onto the audio transport,
//! and the directions and labels Teams sees.

use calling::media::sdp::{
    answer_direction, from_teams, from_teams_with, mid_of_role, source_stream_ids,
    to_teams_answer_with, to_teams_offer, to_teams_reoffer, with_role_direction,
};
use calling::video::VideoRole;

/// The media servers' re-offer after a lobby admission (shape from a live ironteams run).
const FULL_REOFFER: &str = include_str!("fixtures/media/teams_meeting_full_reoffer.sdp");
const SHARE: &[VideoRole] = &[VideoRole::ApplicationSharing];

fn sections(sdp: &str) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    for line in sdp.lines().filter(|l| !l.is_empty()) {
        if line.starts_with("m=") {
            out.push(vec![line]);
        } else if let Some(s) = out.last_mut() {
            s.push(line);
        }
    }
    out
}

fn value<'a>(section: &[&'a str], name: &str) -> Option<&'a str> {
    section
        .iter()
        .find_map(|l| l.strip_prefix(&format!("a={name}:")))
}

fn has(section: &[&str], line: &str) -> bool {
    section.contains(&line)
}

#[test]
fn audio_only_sessions_still_drop_every_video_line() {
    let browser = from_teams(FULL_REOFFER).unwrap();
    assert!(
        sections(&browser)
            .iter()
            .all(|s| s[0].starts_with("m=audio"))
    );
}

#[test]
fn a_share_session_keeps_audio_and_the_first_sharing_line_only() {
    let browser = from_teams_with(FULL_REOFFER, SHARE, true).unwrap();
    let kept = sections(&browser);
    assert_eq!(kept.len(), 2, "{browser}");
    assert!(kept[0][0].starts_with("m=audio"));
    assert!(kept[1][0].starts_with("m=video"));
    assert_eq!(value(&kept[1], "label"), Some("applicationsharing-video"));
    assert_eq!(value(&kept[1], "mid"), Some("3"));
    assert!(browser.contains("a=group:BUNDLE 1 3\r\n"), "{browser}");
}

#[test]
fn the_sharing_line_is_bundled_onto_the_audio_transport() {
    let browser = from_teams_with(FULL_REOFFER, SHARE, true).unwrap();
    let kept = sections(&browser);
    let audio_ufrag = value(&kept[0], "ice-ufrag").unwrap();
    assert_eq!(value(&kept[1], "ice-ufrag"), Some(audio_ufrag));
    assert_eq!(value(&kept[1], "ice-pwd"), value(&kept[0], "ice-pwd"));
    assert!(
        !kept[1].iter().any(|l| l.starts_with("a=candidate:")),
        "the sharing transport's own candidates go"
    );
    assert!(!kept[1].iter().any(|l| l.starts_with("a=rtcp:")));
    assert!(kept[0].iter().any(|l| l.starts_with("a=candidate:")));
}

#[test]
fn a_sendonly_sharing_offer_is_read_as_sendrecv_so_we_can_send_later() {
    let browser = from_teams_with(FULL_REOFFER, SHARE, true).unwrap();
    let share = &sections(&browser)[1];
    assert!(has(share, "a=sendrecv"), "{share:?}");
    assert!(!has(share, "a=sendonly"));
    // An answer is taken as it is.
    let answer = from_teams_with(FULL_REOFFER, SHARE, false).unwrap();
    assert!(has(&sections(&answer)[1], "a=sendonly"));
}

#[test]
fn the_camera_role_takes_the_first_main_video_line() {
    let browser = from_teams_with(FULL_REOFFER, &[VideoRole::MainVideo], true).unwrap();
    let kept = sections(&browser);
    assert_eq!(kept.len(), 2);
    assert_eq!(value(&kept[1], "mid"), Some("2"));
    assert_eq!(value(&kept[1], "label"), Some("main-video"));
}

fn ssrc_lines(section: &[&str]) -> Vec<String> {
    section
        .iter()
        .filter(|l| l.starts_with("a=ssrc:"))
        .map(|l| (*l).to_owned())
        .collect()
}

#[test]
fn a_one_ssrc_range_is_declared_as_that_ssrc_and_a_wider_one_is_not() {
    // webrtc-rs routes an undeclared SSRC only with a single RTP line: next to the camera line
    // the audio line's SSRC must be declared, or every packet of it is dropped.
    let browser = from_teams_with(FULL_REOFFER, &[VideoRole::MainVideo], false).unwrap();
    let kept = sections(&browser);
    assert_eq!(kept.len(), 2, "{browser}");
    assert_eq!(
        ssrc_lines(&kept[0]),
        ["a=ssrc:1000 cname:1000"],
        "{browser}"
    );
    // The camera line's range is 2200-2299: no one SSRC to name.
    assert!(ssrc_lines(&kept[1]).is_empty(), "{browser}");
    // A section that declares its own SSRC keeps just that one.
    let own = FULL_REOFFER.replace(
        "a=x-ssrc-range:1000-1000",
        "a=x-ssrc-range:1000-1000\na=ssrc:1000 cname:own",
    );
    let browser = from_teams_with(&own, &[VideoRole::MainVideo], false).unwrap();
    assert_eq!(
        ssrc_lines(&sections(&browser)[0]),
        ["a=ssrc:1000 cname:own"]
    );
}

#[test]
fn a_role_named_n_times_keeps_its_first_n_lines() {
    let roles = [
        VideoRole::MainVideo,
        VideoRole::MainVideo,
        VideoRole::MainVideo,
    ];
    let browser = from_teams_with(FULL_REOFFER, &roles, false).unwrap();
    let kept = sections(&browser);
    let mids: Vec<_> = kept[1..].iter().map(|s| value(s, "mid").unwrap()).collect();
    assert_eq!(mids, ["2", "5", "6"]);
}

#[test]
fn each_line_s_source_stream_id_is_read_by_mid() {
    let answer = "v=0\r\ns=-\r\nt=0 0\r\n\
m=audio 3478 RTP/SAVP 111\r\na=mid:0\r\na=x-source-streamid:100\r\n\
m=video 3478 RTP/SAVP 107\r\na=mid:1\r\na=label:main-video\r\na=x-source-streamid: 2001\r\n\
m=video 3478 RTP/SAVP 107\r\na=mid:3\r\na=label:main-video\r\n\
m=video 3478 RTP/SAVP 107\r\na=mid:4\r\na=label:main-video\r\na=x-source-streamid:2004\r\n";
    let ids = source_stream_ids(answer);
    assert_eq!(ids.get("1"), Some(&2001));
    assert_eq!(ids.get("4"), Some(&2004));
    assert_eq!(ids.get("3"), None, "no x-source-streamid");
    assert_eq!(ids.get("0"), Some(&100));
}

#[test]
fn answer_directions_follow_what_the_offer_allows() {
    assert_eq!(answer_direction("sendonly", "sendrecv"), "recvonly");
    assert_eq!(answer_direction("recvonly", "sendrecv"), "sendonly");
    assert_eq!(answer_direction("sendrecv", "sendrecv"), "sendrecv");
    assert_eq!(answer_direction("inactive", "sendrecv"), "inactive");
    assert_eq!(answer_direction("sendrecv", "recvonly"), "recvonly");
}

/// A webrtc-rs answer to the share session's reading of `FULL_REOFFER` (shape only).
const OUR_ANSWER: &str = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
a=fingerprint:sha-256 AA:BB\r\na=group:BUNDLE 1 3\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=setup:active\r\na=mid:1\r\n\
a=ice-ufrag:ours\r\na=ice-pwd:ourpassword\r\na=rtcp-mux\r\na=rtpmap:111 opus/48000/2\r\n\
a=ssrc:1111 cname:ironteams\r\na=sendrecv\r\n\
a=candidate:1 1 udp 2130706431 192.0.2.10 50000 typ host\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 107\r\nc=IN IP4 0.0.0.0\r\na=setup:active\r\na=mid:3\r\n\
a=ice-ufrag:ours\r\na=ice-pwd:ourpassword\r\na=rtcp-mux\r\na=rtpmap:107 H264/90000\r\n\
a=fmtp:107 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f\r\n\
a=msid:applicationsharing-video ironteams-share\r\na=ssrc:2222 cname:ironteams\r\na=sendrecv\r\n";

#[test]
fn the_answer_has_every_offered_line_and_a_recvonly_sharing_line() {
    let teams = to_teams_answer_with(OUR_ANSWER, FULL_REOFFER, SHARE);
    let ours = sections(&teams);
    assert_eq!(ours.len(), sections(FULL_REOFFER).len(), "{teams}");
    let share = ours
        .iter()
        .find(|s| value(s, "mid") == Some("3"))
        .expect("sharing line answered");
    assert!(!share[0].starts_with("m=video 0 "), "{share:?}");
    assert!(has(share, "a=recvonly"), "{share:?}");
    assert_eq!(value(share, "label"), Some("applicationsharing-video"));
    assert_eq!(value(share, "x-ssrc-range"), Some("2222-2222"));
    // The gallery lines are declined.
    let gallery = ours.iter().find(|s| value(s, "mid") == Some("2")).unwrap();
    assert!(gallery[0].starts_with("m=video 0 "));
}

/// A webrtc-rs offer of a share session (shape only).
const OUR_OFFER: &str = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
a=fingerprint:sha-256 AA:BB\r\na=group:BUNDLE 0 1\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=setup:actpass\r\na=mid:0\r\n\
a=ice-ufrag:ours\r\na=ice-pwd:ourpassword\r\na=rtcp-mux\r\na=rtpmap:111 opus/48000/2\r\n\
a=msid:ironteams-1111 ironteams-audio\r\na=ssrc:1111 cname:ironteams\r\na=sendrecv\r\n\
a=candidate:1 1 udp 2130706431 192.0.2.10 50000 typ host\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 107\r\nc=IN IP4 0.0.0.0\r\na=setup:actpass\r\na=mid:1\r\n\
a=ice-ufrag:ours\r\na=ice-pwd:ourpassword\r\na=rtcp-mux\r\na=rtpmap:107 H264/90000\r\n\
a=msid:applicationsharing-video ironteams-share\r\na=ssrc:2222 cname:ironteams\r\na=sendrecv\r\n";

#[test]
fn our_offer_labels_the_sharing_line_from_its_stream() {
    let teams = to_teams_offer(OUR_OFFER);
    let ours = sections(&teams);
    assert_eq!(value(&ours[0], "label"), Some("main-audio"));
    assert_eq!(value(&ours[1], "label"), Some("applicationsharing-video"));
    assert_eq!(value(&ours[1], "x-ssrc-range"), Some("2222-2222"));
    assert!(
        ours[1][0].starts_with("m=video 50000 RTP/SAVP 107"),
        "{:?}",
        ours[1]
    );
    assert_eq!(
        mid_of_role(&teams, VideoRole::ApplicationSharing).as_deref(),
        Some("1")
    );
    assert_eq!(mid_of_role(&teams, VideoRole::MainVideo), None);
}

#[test]
fn a_re_offer_states_the_sharing_direction() {
    let teams = to_teams_offer(OUR_OFFER);
    let sharing = with_role_direction(&teams, VideoRole::ApplicationSharing, "sendonly");
    let ours = sections(&sharing);
    assert!(has(&ours[1], "a=sendonly") && !has(&ours[1], "a=sendrecv"));
    assert!(has(&ours[0], "a=sendrecv"), "audio untouched");
    let stopped = with_role_direction(&sharing, VideoRole::ApplicationSharing, "inactive");
    assert!(has(&sections(&stopped)[1], "a=inactive"));
    assert_eq!(stopped.lines().count(), sharing.lines().count());
}

/// A webrtc-rs offer as one went out live on a 1:1 call ironteams answered: an extra `main-video`
/// receiver took mid 2, the sharing line moved to mid 3, and the declined `x-data` is missing.
const MOVED_REOFFER: &str = "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
a=fingerprint:sha-256 AA:BB\r\na=group:BUNDLE 0 1 2 3\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\na=sendrecv\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 107\r\nc=IN IP4 0.0.0.0\r\na=mid:1\r\n\
a=msid:main-video ironteams-1\r\na=sendonly\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 107\r\nc=IN IP4 0.0.0.0\r\na=mid:2\r\na=recvonly\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 107\r\nc=IN IP4 0.0.0.0\r\na=mid:3\r\n\
a=msid:applicationsharing-video ironteams-2\r\na=sendrecv\r\n";

/// Our answer to the browser's offer: mid 0-2 bundled, the `x-data` line declined.
const NEGOTIATED: &str = "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
a=group:BUNDLE 0 1 2\r\n\
m=audio 9 RTP/SAVP 111\r\na=mid:0\r\na=sendrecv\r\na=label:main-audio\r\n\
m=video 9 RTP/SAVP 107\r\na=mid:1\r\na=sendonly\r\na=label:main-video\r\n\
m=video 9 RTP/SAVP 107\r\na=mid:2\r\na=inactive\r\na=label:applicationsharing-video\r\n\
m=x-data 0 RTP/SAVP 34\r\nc=IN IP4 0.0.0.0\r\na=mid:3\r\na=label:data\r\n";

#[test]
fn a_re_offer_keeps_the_negotiated_lines_and_their_labels() {
    let offer = to_teams_reoffer(MOVED_REOFFER, NEGOTIATED);
    let s = sections(&offer);
    let shape: Vec<(&str, Option<&str>, Option<&str>)> = s
        .iter()
        .map(|x| {
            (
                x[0].split(' ').next().unwrap(),
                value(x, "mid"),
                value(x, "label"),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("m=audio", Some("0"), Some("main-audio")),
            ("m=video", Some("1"), Some("main-video")),
            ("m=video", Some("2"), Some("applicationsharing-video")),
            ("m=x-data", Some("3"), Some("data")),
        ],
        "{offer}"
    );
    assert_eq!(s[3][0], "m=x-data 0 RTP/SAVP 34");
    assert!(offer.contains("a=group:BUNDLE 0 1 2\r\n"), "{offer}");
}

#[test]
fn a_first_offer_has_no_negotiated_lines_to_keep() {
    let offer = to_teams_offer(MOVED_REOFFER);
    assert_eq!(sections(&offer).len(), 4, "{offer}");
    assert!(offer.contains("a=group:BUNDLE 0 1 2 3\r\n"), "{offer}");
}
