//! A 1:1 call IronTeams answered with the camera and sharing lines, then re-offered (hold, camera,
//! share): the re-offer must keep the negotiated m-lines, in order, with their mids, kinds and
//! labels, the declined `x-data` line included. Offline (host candidates only); the caller's offer
//! is synthetic, shaped like a browser's (Teams web).

use calling::media::sdp::with_role_direction;
use calling::media::{MediaOptions, MediaSession};
use calling::signaling::call_ops::{hold_offer, keep_media_running};
use calling::video::VideoRole;

/// mid0 audio sendrecv, mid1 `main-video` recvonly, mid2 `applicationsharing-video` inactive,
/// mid3 `x-data` sendrecv; BUNDLE 0 1 2 3.
const OFFER: &str = include_str!("fixtures/media/teams_web_video_offer.sdp");

fn camera_and_share() -> MediaOptions {
    MediaOptions {
        udp_addrs: vec!["127.0.0.1:0".into()],
        video: vec![VideoRole::MainVideo, VideoRole::ApplicationSharing],
        ..Default::default()
    }
}

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

/// Each section as (kind, mid, label, declined).
fn layout(sdp: &str) -> Vec<(String, String, String, bool)> {
    sections(sdp)
        .iter()
        .map(|s| {
            let mut m = s[0].trim_start_matches("m=").split(' ');
            let kind = m.next().unwrap().to_owned();
            let declined = m.next() == Some("0");
            let mid = value(s, "mid").unwrap_or_default().to_owned();
            let label = value(s, "label").unwrap_or_default().to_owned();
            (kind, mid, label, declined)
        })
        .collect()
}

fn bundle(sdp: &str) -> &str {
    sdp.lines()
        .find_map(|l| l.strip_prefix("a=group:BUNDLE "))
        .unwrap_or_default()
}

fn negotiated() -> Vec<(String, String, String, bool)> {
    [
        ("audio", "0", "main-audio", false),
        ("video", "1", "main-video", false),
        ("video", "2", "applicationsharing-video", false),
        ("x-data", "3", "data", true),
    ]
    .iter()
    .map(|(k, m, l, d)| ((*k).into(), (*m).into(), (*l).into(), *d))
    .collect()
}

#[tokio::test]
async fn the_answer_to_a_browser_s_video_offer_keeps_its_lines() {
    let (session, _events) = MediaSession::with_options(vec![], camera_and_share())
        .await
        .unwrap();
    let answer = session.accept_offer(OFFER).await.unwrap();
    assert_eq!(layout(&answer), negotiated(), "{answer}");
    assert_eq!(bundle(&answer), "0 1 2", "{answer}");
    let s = sections(&answer);
    assert!(s[1].contains(&"a=sendonly"), "{answer}");
    assert!(s[2].contains(&"a=inactive"), "{answer}");
    assert_eq!(s[3][0], "m=x-data 0 RTP/SAVP 34");
    session.close().await.unwrap();
}

#[tokio::test]
async fn the_sharing_line_the_caller_offered_inactive_is_ours() {
    let (session, _events) = MediaSession::with_options(vec![], camera_and_share())
        .await
        .unwrap();
    session.accept_offer(OFFER).await.unwrap();
    assert_eq!(
        session.video_mid(VideoRole::MainVideo).await.as_deref(),
        Some("1")
    );
    assert_eq!(
        session
            .video_mid(VideoRole::ApplicationSharing)
            .await
            .as_deref(),
        Some("2"),
        "webrtc-rs answered the sharing line with a receiver of its own"
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn a_re_offer_after_answering_keeps_the_negotiated_lines() {
    let (session, _events) = MediaSession::with_options(vec![], camera_and_share())
        .await
        .unwrap();
    session.accept_offer(OFFER).await.unwrap();
    let ours = session.create_offer().await.unwrap();
    // Hold, camera on/off and share (the GUI's `live::reoffers::reoffer_sdp`) only rewrite
    // directions of this offer.
    let hold = hold_offer(&ours);
    let camera = with_role_direction(
        &with_role_direction(&ours, VideoRole::MainVideo, "recvonly"),
        VideoRole::ApplicationSharing,
        "inactive",
    );
    let share = with_role_direction(&ours, VideoRole::ApplicationSharing, "sendonly");
    for offer in [&ours, &hold, &camera, &share] {
        assert_eq!(layout(offer), negotiated(), "{offer}");
        assert_eq!(bundle(offer), "0 1 2", "{offer}");
        let s = sections(offer);
        assert_eq!(s[3][0], "m=x-data 0 RTP/SAVP 34", "{offer}");
    }
    assert!(
        sections(&hold)
            .iter()
            .take(3)
            .all(|s| s.contains(&"a=inactive"))
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn a_refused_re_offer_rolls_back_and_the_caller_s_offer_is_answered() {
    let (session, _events) = MediaSession::with_options(vec![], camera_and_share())
        .await
        .unwrap();
    session.accept_offer(OFFER).await.unwrap();
    session.create_offer().await.unwrap();
    // `call/rejection` to our re-offer, then the browser offers again to recover.
    session.rollback_offer().await.unwrap();
    let answer = session.accept_offer(OFFER).await.unwrap();
    assert_eq!(layout(&answer), negotiated(), "{answer}");
    // Nothing pending: a second rollback is a no-op.
    session.rollback_offer().await.unwrap();
    session.close().await.unwrap();
}

/// The browser's answer to one of our re-offers as a live 1:1 call had it (2026-11-20, after a
/// hold): audio and mid1 `main-video` accepted (`audio`, `video` directions), the sharing line and
/// `x-data` refused with bare port-0 stubs that name no mid, only a label. On the caller's own
/// transport (the fixture's).
fn answer_refusing_the_sharing_line(audio: &str, video: &str) -> String {
    let offer = sections(OFFER);
    let mut out: Vec<String> = OFFER
        .lines()
        .take_while(|l| !l.starts_with("m="))
        .map(|l| match l {
            l if l.starts_with("a=group:BUNDLE") => "a=group:BUNDLE 0 1".to_owned(),
            l => l.to_owned(),
        })
        .collect();
    for (s, direction) in offer.iter().take(2).zip([audio, video]) {
        for l in s {
            let d = ["a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive"].contains(l);
            out.push(if d {
                format!("a={direction}")
            } else {
                (*l).to_owned()
            });
        }
    }
    out.extend(
        [
            "m=video 0 RTP/SAVP 34",
            "a=label:applicationsharing-video",
            "m=x-data 0 RTP/SAVP 34",
            "a=label:data",
        ]
        .map(String::from),
    );
    out.join("\r\n") + "\r\n"
}

#[tokio::test]
async fn a_share_after_the_sharing_line_was_refused_offers_it_again_on_its_own_mid() {
    let (session, _events) = MediaSession::with_options(vec![], camera_and_share())
        .await
        .unwrap();
    session.accept_offer(OFFER).await.unwrap();
    // Hold: the browser refuses the sharing line.
    let ours = session.create_offer().await.unwrap();
    let hold = answer_refusing_the_sharing_line("inactive", "inactive");
    session
        .set_answer(&keep_media_running(&hold, &ours))
        .await
        .unwrap();
    // Resume: still refused.
    let ours = session.create_offer().await.unwrap();
    let resume = answer_refusing_the_sharing_line("sendrecv", "recvonly");
    session
        .set_answer(&keep_media_running(&resume, &ours))
        .await
        .unwrap();
    // Share.
    let ours = session.create_offer().await.unwrap();
    let share = with_role_direction(&ours, VideoRole::ApplicationSharing, "sendonly");
    assert_eq!(
        layout(&share),
        [
            ("audio", "0", "main-audio", false),
            ("video", "1", "main-video", false),
            ("video", "2", "applicationsharing-video", false),
            ("x-data", "", "data", true),
        ]
        .iter()
        .map(|(k, m, l, d)| ((*k).into(), (*m).into(), (*l).into(), *d))
        .collect::<Vec<(String, String, String, bool)>>(),
        "{share}"
    );
    assert_eq!(bundle(&share), "0 1 2", "{share}");
    let line = &sections(&share)[2];
    assert!(line.contains(&"a=sendonly"), "{line:?}");
    assert!(
        line.iter()
            .any(|l| l.starts_with("a=rtpmap:") && l.ends_with(" H264/90000")),
        "{line:?}"
    );
    assert!(
        line.iter()
            .any(|l| l.starts_with("a=msid:applicationsharing-video ")),
        "the screen goes out on the sharing line: {line:?}"
    );
    session.close().await.unwrap();
}
