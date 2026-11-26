//! Switching our video lines (screen share; the camera later) on and off with the service.
//!
//! - In a meeting the media server never renegotiates for it: the client flips lines it already
//!   negotiated with `UpdateMediaDescriptions` on the call leg's `updateMediaDescriptions` link
//!   (mautrix-teams `docs/openapi/teams-webclient.yaml` `/updateMediaDescriptions`, captured from
//!   the web client; mautrix-teams `video.go` `SetVideo`; rusty-teams `video_layout.rs`).
//! - In a 1:1 (direct) call the client sends a new offer, a `mediaNegotiation` on the other
//!   side's `mediaRenegotiation` link, with `ScreenSharer` among the `callModalities`
//!   (mautrix-teams `calls.go` `Renegotiate`; rusty-teams `hold.rs` `offer_modalities`).
//! - The media controller asks the sender for a size, a rate and keyframes on
//!   `call/controlVideoStreaming/` (mautrix-teams `video.go` `handleControl`).
//! - In a meeting we ask for a participant's camera or share on one of our receive lines with
//!   `applyChannelParameters` (a `controlVideoStreaming` subscription: the participant's MSI on
//!   that line's `x-source-streamid`; rusty-teams `video_layout.rs`, mautrix-teams `video.go`).
//!   UNVERIFIED live.
//!
//! Facts only from mautrix-teams (AGPL); see `docs/protocol-calls.md` (Screen sharing).

use serde_json::{Value, json};

use super::{CallSession, NgcClient};
use crate::error::{Error, Result};
use crate::signaling::bodies;
use crate::types::{Links, Modality, SdpBlob};
use crate::video::VideoRole;

/// The `requestId` of the media descriptions a meeting join declares; every later
/// `UpdateMediaDescriptions` counts on from it.
pub const JOIN_REQUEST_ID: u64 = 1;

/// One line in an `UpdateMediaDescriptions`: its mid, its direction and, while it sends, its
/// label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaDescription {
    pub mid: String,
    pub direction: &'static str,
    pub label: Option<&'static str>,
}

impl MediaDescription {
    /// The line `mid` that carries `role`: sending (its role's direction and label) or
    /// receive-only.
    pub fn line(mid: impl Into<String>, role: VideoRole, sending: bool) -> Self {
        MediaDescription {
            mid: mid.into(),
            direction: if sending {
                role.sending_direction()
            } else {
                "recvonly"
            },
            label: sending.then(|| role.label()),
        }
    }

    /// The line `mid`, receive-only.
    pub fn receive_only(mid: impl Into<String>) -> Self {
        MediaDescription {
            mid: mid.into(),
            direction: "recvonly",
            label: None,
        }
    }

    pub fn wire(&self) -> Value {
        let mut v = json!({"mid": self.mid, "direction": self.direction});
        if let Some(label) = self.label {
            v["label"] = json!(label);
        }
        v
    }
}

/// The descriptions of every one of our `live` video lines (mids, in order) for an
/// `UpdateMediaDescriptions`, as the web client and mautrix-teams (`SetVideo`) post them: the
/// lines in `sending` (mid, role) send, every other one is receive-only. A mid that is not live
/// is never named.
pub fn meeting_descriptions(
    live: &[String],
    sending: &[(String, VideoRole)],
) -> Vec<MediaDescription> {
    live.iter()
        .map(
            |mid| match sending.iter().find(|(m, _)| m == mid).map(|(_, r)| *r) {
                Some(role) => MediaDescription::line(mid.clone(), role, true),
                None => MediaDescription::receive_only(mid.clone()),
            },
        )
        .collect()
}

/// `<participantId>;ss_1` (share), `;v_2` (camera on) or `;v_1` (camera off).
pub fn negotiation_tag(s: &CallSession, role: VideoRole, sending: bool) -> String {
    format!(
        "{};{}",
        s.me.participant_id,
        role.negotiation_suffix(sending)
    )
}

/// `POST <updateMediaDescriptions>`. `request_id` grows by one per change.
pub fn update_media_descriptions_body(
    descriptions: &[MediaDescription],
    negotiation_tag: &str,
    request_id: u64,
) -> Value {
    json!({"UpdateMediaDescriptions": {"mediaDescriptions": {
        "descriptions": descriptions.iter().map(MediaDescription::wire).collect::<Vec<_>>(),
        "negotiationTag": negotiation_tag,
        "requestId": request_id,
    }}})
}

/// `callModalities` of a re-offer: audio, plus the modality of each line that sends.
pub fn video_modalities(sending: &[VideoRole]) -> Vec<Modality> {
    let mut m = vec![Modality::Audio];
    for role in sending {
        let modality = role.modality();
        if !m.contains(&modality) {
            m.push(modality);
        }
    }
    m
}

/// `POST <mediaRenegotiation>` (the other side's link): our new offer on a running direct call,
/// as the web client switches its share (mautrix-teams `Renegotiate`): `mediaNegotiation` with
/// our `mediaAnswer`/`rejection` links, `requiredFeatures: nonByPass`, the negotiation tag, and
/// `newOffer: false` (rusty-teams, a re-offer to the same peer).
pub fn video_renegotiation_body(
    s: &CallSession,
    modalities: &[Modality],
    offer: &SdpBlob,
    negotiation_tag: &str,
) -> Value {
    let mut body = bodies::negotiation_body(s, modalities, offer);
    let media = &mut body["mediaNegotiation"]["mediaContent"];
    media["newOffer"] = json!(false);
    media["negotiationTag"] = json!(negotiation_tag);
    body
}

/// What the media controller asks of one of our video sources (`controlVideoStreaming`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SenderControl {
    /// The `sourceId` the request is for (our line's `x-source-streamid`).
    pub source_id: u64,
    /// The raw `fmtParams` (`max-fs=…;max-fps=…;max-br=…;KeyFrame=1`).
    pub fmt_params: String,
}

impl SenderControl {
    pub fn keyframe(&self) -> bool {
        self.param("KeyFrame") == Some("1")
    }

    /// One `name=value` of `fmt_params`.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.fmt_params
            .split([';', ' ', ','])
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
    }
}

/// The requests in a `controlVideoStreaming` callback body.
pub fn parse_sender_controls(body: &Value) -> Vec<SenderControl> {
    let holder = body.get("controlVideoStreaming").unwrap_or(body);
    holder["controlInfo"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| SenderControl {
            source_id: c["sourceId"].as_u64().unwrap_or_default(),
            fmt_params: c["fmtParams"].as_str().unwrap_or_default().to_owned(),
        })
        .collect()
}

/// What a subscription asks the media server for: the largest picture (`max-fs`, macroblocks),
/// the rate (`max-fps`, hundredths) and the macroblock rate (`max-mbps`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiveLimits {
    pub max_fs: u32,
    pub max_fps: u32,
    pub max_mbps: u32,
}

impl ReceiveLimits {
    /// The limits of a `width` x `height` picture at `fps` frames a second: `max-fs` in 16x16
    /// macroblocks (each side rounded up), `max-fps` in hundredths, `max-mbps` the macroblocks a
    /// second. 640x360 at 30 is [`TILE_LIMITS`]; 1280x720 at 30 is 3600 (the 720p30 the media
    /// server's answer allows); 320x180 at 15 is 240. The caller decides which size a line asks
    /// for (this crate knows no layout).
    pub const fn for_size(width: u32, height: u32, fps: u32) -> Self {
        let max_fs = width.div_ceil(16) * height.div_ceil(16);
        ReceiveLimits {
            max_fs,
            max_fps: fps * 100,
            max_mbps: max_fs * fps,
        }
    }
}

/// A gallery tile: 640x360 at 30 fps (rusty-teams' small tile).
pub const TILE_LIMITS: ReceiveLimits = ReceiveLimits {
    max_fs: 920,
    max_fps: 3000,
    max_mbps: 27_600,
};

/// A shared screen: up to 1920x1080 at 15 fps (rusty-teams' screen request).
pub const SCREEN_LIMITS: ReceiveLimits = ReceiveLimits {
    max_fs: 8160,
    max_fps: 1500,
    max_mbps: 135_000,
};

/// The profile a subscription names (as rusty-teams sends it).
const SUBSCRIBE_PROFILE: &str = "64001f";
/// `sourceId` of a subscription that stops a line.
const NO_SOURCE: i64 = -1;

/// One `applyChannelParameters` subscription: show `source` (a participant's MSI; `None`
/// unsubscribes) on our receive line `mid`, whose `x-source-streamid` is `stream_msid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRequest {
    pub mid: String,
    pub stream_msid: u32,
    pub source: Option<u32>,
    pub limits: ReceiveLimits,
}

impl SourceRequest {
    /// `POST <applyChannelParameters>`: the subscription as a JSON string in `mediaParameter`.
    /// `sequence_number` grows by one per request of the call.
    pub fn body(&self, sequence_number: u64) -> Value {
        let l = self.limits;
        let control = json!({"controlVideoStreaming": {
            "sequenceNumber": sequence_number,
            "controlInfo": {
                "sourceId": self.source.map_or(NO_SOURCE, i64::from),
                "streamMsid": self.stream_msid,
                "fmtParams": [{
                    "max-fs": l.max_fs,
                    "max-mbps": l.max_mbps,
                    "max-fps": l.max_fps,
                    "profile-level-id": SUBSCRIBE_PROFILE,
                }],
            },
        }});
        json!({"applyChannelParameters": {"multiChannelParameter": {
            "mids": [self.mid],
            "mediaParameter": control.to_string(),
        }}})
    }
}

/// Who each receive slot shows next: a slot keeps the person it shows while they are still
/// `wanted` (same MSI); the free slots take the wanted ones not shown yet, in `wanted`'s order.
/// Only the first `current.len()` of `wanted` get a slot.
pub fn plan_receive(
    current: &[Option<(String, u32)>],
    wanted: &[(String, u32)],
) -> Vec<Option<(String, u32)>> {
    let wanted = &wanted[..wanted.len().min(current.len())];
    let mut next: Vec<Option<(String, u32)>> = current
        .iter()
        .map(|held| held.clone().filter(|h| wanted.contains(h)))
        .collect();
    for w in wanted {
        if next.iter().flatten().any(|h| h == w) {
            continue;
        }
        if let Some(free) = next.iter_mut().find(|s| s.is_none()) {
            *free = Some(w.clone());
        }
    }
    next
}

impl NgcClient {
    /// Sends one subscription (see [`SourceRequest::body`]) to the call leg's
    /// `applyChannelParameters` link `url`.
    pub async fn apply_channel_parameters(
        &self,
        s: &CallSession,
        url: &str,
        request: &SourceRequest,
        sequence_number: u64,
    ) -> Result<()> {
        let body = request.body(sequence_number);
        self.send(
            "apply_channel_parameters",
            s,
            reqwest::Method::POST,
            url,
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// Flips our video lines in a meeting (see [`update_media_descriptions_body`]).
    pub async fn update_media_descriptions(
        &self,
        s: &CallSession,
        links: &Links,
        descriptions: &[MediaDescription],
        negotiation_tag: &str,
        request_id: u64,
    ) -> Result<()> {
        let url = links
            .get("updateMediaDescriptions")
            .ok_or_else(|| Error::Protocol("no 'updateMediaDescriptions' link".into()))?;
        let body = update_media_descriptions_body(descriptions, negotiation_tag, request_id);
        self.send(
            "update_media_descriptions",
            s,
            reqwest::Method::POST,
            url,
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// Sends our re-offer on a direct call (see [`video_renegotiation_body`]). The answer comes
    /// inline or later on `call/mediaAnswer/`.
    pub async fn renegotiate_video(
        &self,
        s: &CallSession,
        links: &Links,
        modalities: &[Modality],
        offer: &SdpBlob,
        negotiation_tag: &str,
    ) -> Result<Option<SdpBlob>> {
        let url = links
            .get("mediaRenegotiation")
            .ok_or_else(|| Error::Protocol("no 'mediaRenegotiation' link".into()))?;
        let body = video_renegotiation_body(s, modalities, offer, negotiation_tag);
        let r = self
            .send(
                "media_renegotiation",
                s,
                reqwest::Method::POST,
                url,
                Some(&body),
            )
            .await?;
        let answer = bodies::parse_inline_answer(&r.body);
        // What we told the other side we send, line by line, and what it answered (when inline;
        // else `call/mediaAnswer/`): a live 1:1 run shows whether a line still said it sends.
        let ours = offer.decode().map(|o| crate::media::sdp::directions(&o));
        let theirs = answer
            .as_ref()
            .and_then(|a| a.decode().ok())
            .map(|a| crate::media::sdp::directions(&a));
        tracing::info!(
            ?modalities,
            negotiation_tag,
            status = r.status,
            ours = %ours.unwrap_or_default(),
            theirs = %theirs.unwrap_or_else(|| "on call/mediaAnswer".into()),
            "1:1 re-offer sent"
        );
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(mri: &str, msi: u32) -> (String, u32) {
        (mri.to_owned(), msi)
    }

    #[test]
    fn a_subscription_names_the_source_on_the_line_s_stream() {
        let r = SourceRequest {
            mid: "3".into(),
            stream_msid: 4001,
            source: Some(202),
            limits: TILE_LIMITS,
        };
        let body = r.body(7);
        let channel = &body["applyChannelParameters"]["multiChannelParameter"];
        assert_eq!(channel["mids"], json!(["3"]));
        let inner: Value =
            serde_json::from_str(channel["mediaParameter"].as_str().unwrap()).unwrap();
        let c = &inner["controlVideoStreaming"];
        assert_eq!(c["sequenceNumber"], 7);
        assert_eq!(c["controlInfo"]["sourceId"], 202);
        assert_eq!(c["controlInfo"]["streamMsid"], 4001);
        assert_eq!(c["controlInfo"]["fmtParams"][0]["max-fs"], 920);
        assert_eq!(
            c["controlInfo"]["fmtParams"][0]["profile-level-id"],
            "64001f"
        );
    }

    #[test]
    fn no_source_unsubscribes() {
        let r = SourceRequest {
            mid: "11".into(),
            stream_msid: 9,
            source: None,
            limits: SCREEN_LIMITS,
        };
        let body = r.body(1);
        let inner: Value = serde_json::from_str(
            body["applyChannelParameters"]["multiChannelParameter"]["mediaParameter"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            inner["controlVideoStreaming"]["controlInfo"]["sourceId"],
            -1
        );
    }

    fn fmt_params(body: &Value) -> Value {
        let inner: Value = serde_json::from_str(
            body["applyChannelParameters"]["multiChannelParameter"]["mediaParameter"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        inner["controlVideoStreaming"]["controlInfo"].clone()
    }

    #[test]
    fn limits_follow_the_picture_size_in_macroblocks() {
        assert_eq!(ReceiveLimits::for_size(640, 360, 30), TILE_LIMITS);
        let stage = ReceiveLimits::for_size(1280, 720, 30);
        assert_eq!(
            (stage.max_fs, stage.max_fps, stage.max_mbps),
            (3600, 3000, 108_000)
        );
        let strip = ReceiveLimits::for_size(320, 180, 15);
        assert_eq!(
            (strip.max_fs, strip.max_fps, strip.max_mbps),
            (240, 1500, 3600)
        );
    }

    /// A large (stage) tile asks for 720p; a line nobody sees asks for no source at all.
    #[test]
    fn a_720p_line_asks_for_3600_macroblocks_and_an_unseen_one_for_no_source() {
        let mut r = SourceRequest {
            mid: "4".into(),
            stream_msid: 4002,
            source: Some(311),
            limits: ReceiveLimits::for_size(1280, 720, 30),
        };
        let c = fmt_params(&r.body(3));
        assert_eq!(c["sourceId"], 311);
        assert_eq!(c["fmtParams"][0]["max-fs"], 3600);
        assert_eq!(c["fmtParams"][0]["max-fps"], 3000);
        assert_eq!(c["fmtParams"][0]["max-mbps"], 108_000);
        r.source = None;
        assert_eq!(fmt_params(&r.body(4))["sourceId"], -1);
    }

    #[test]
    fn a_slot_keeps_its_person_and_free_slots_take_the_next_ones() {
        let empty = vec![None; 2];
        let first = plan_receive(&empty, &[who("a", 1), who("b", 2), who("c", 3)]);
        assert_eq!(first, vec![Some(who("a", 1)), Some(who("b", 2))]);
        // a left, c came first: b stays where it is, c takes a's slot.
        let next = plan_receive(&first, &[who("c", 3), who("b", 2)]);
        assert_eq!(next, vec![Some(who("c", 3)), Some(who("b", 2))]);
        // A new MSI for b (camera restarted) is a new subscription.
        let again = plan_receive(&next, &[who("b", 5)]);
        assert_eq!(again, vec![Some(who("b", 5)), None]);
        assert_eq!(plan_receive(&again, &[]), vec![None, None]);
    }
}
