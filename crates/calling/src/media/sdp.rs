//! SDP as the Teams calling service carries it, and the rewrites between its dialect and what
//! webrtc-rs reads and writes.
//!
//! # The `{contentType, blob}` envelope
//!
//! `application/sdp-ngc-1.0` is a label on ordinary WebRTC SDP, not a different language: the
//! Teams web client (a browser) and mautrix-teams both send the plain SDP text under it, and
//! mautrix-teams sends `application/sdp` for calls through Teams' media servers (teams-lite
//! `NATIVE-CALLING.md` §2.5; mautrix-teams `pkg/msteams/calls.go` `directContentType`). A blob
//! that does not start with `v=` is `base64(raw deflate(sdp))`, and the native clients deflate
//! with a preset dictionary taken from Microsoft's client library (eisbaw/ost
//! `src/calling/sdp_compress.rs`; mautrix-teams `pkg/teamsmedia/sdp.go` `ErrCompressedSDP`).
//! That dictionary is Microsoft's and is not shipped here: such a blob is a named error, and
//! [`SdpBlob::decode_with_dictionary`] reads it when the caller has a dictionary of its own.
//!
//! # The dialect
//!
//! The web client rewrites every SDP it sends (`toMsSdp`) and every one it receives
//! (`fromMsSdp`), and the service refuses SDP that has not been through it; teams-lite measured
//! each rule against the live service (`NATIVE-CALLING.md` §2.5, facts only):
//! - every m-line's profile is `RTP/SAVP` on the wire (`410 UnrecognizedTransportProfile`
//!   otherwise) and is restored to `UDP/TLS/RTP/SAVPF` for a DTLS section on the way back;
//! - ICE-TCP candidates fold their role into the transport (`tcp-act`, `tcp-pass`, `tcp-so`),
//!   and IPv6 candidates may arrive as `a=x-candidate-ipv6`;
//! - two header-extension URIs are spelled with backslashes;
//! - an offer carries `b=CT:4000` before `t=`, `a=msid-semantic: WMS *`, the bundle's real port
//!   and address on each m-line and `c=`, `a=rtcp:<port>` and the fingerprint per section; an
//!   answer carries none of that (`SdpParsingFailure`);
//! - each section states `a=label` and its SSRC as `a=x-ssrc-range:N-N` (Teams drops RTP from
//!   SSRCs outside the advertised range, mautrix-teams `pkg/teamsmedia/sdp.go`);
//! - every video line of an offer also states `a=x-signaling-fb:* x-message app send:src
//!   recv:src,vc`, and a receive-only one the range `1-1` (the web client's meeting join and
//!   mautrix-teams `writeVideoLine`).
//!
//! On the way in, candidates are reduced to what webrtc-rs uses: UDP, component 1 (rtcp-mux makes
//! the RTCP component redundant), without vendor extras such as `MTURNID`.

use std::collections::HashMap;
use std::net::IpAddr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use flate2::{Decompress, FlushDecompress, Status};

use crate::video::VideoRole;
use crate::{Error, Result, SdpBlob};

/// Plain SDP, as mautrix-teams labels calls through Teams' media servers.
pub const CONTENT_TYPE_SDP: &str = "application/sdp";
/// The label the web client puts on its SDP, and that direct (peer-to-peer) calls use.
pub const CONTENT_TYPE_NGC: &str = "application/sdp-ngc-1.0";

const MS_PROFILE: &str = "RTP/SAVP";
const DTLS_PROFILE: &str = "UDP/TLS/RTP/SAVPF";
const SESSION_BANDWIDTH: &str = "b=CT:4000";
/// The DTLS roles of SDP that states no `a=setup`, as a native Teams client's 1:1 offer does (it
/// offers SDES and a fingerprint): RFC 4145 §4.1, "active" in an offer and "passive" in an
/// answer. Live 2026-11-25, a native caller ended the call (406/4122 `FinalAnswerError`) 0.3 s
/// after our `a=setup:active` answer, before ICE could finish.
const OFFER_DEFAULT_SETUP: &str = "a=setup:active";
const ANSWER_DEFAULT_SETUP: &str = "a=setup:passive";
/// What every video line of the web client's offer (and mautrix-teams') declares; the media
/// servers answer theirs with `send:src,csrc,vc recv:src` (mautrix-teams openapi `TeamsSdp`).
const VIDEO_SIGNALING_FB: &str = "a=x-signaling-fb:* x-message app send:src recv:src,vc";
/// The SSRC range the web client gives a receive-only video line.
const NO_SSRC_RANGE: &str = "1-1";
/// The payload a declined section of an offer names (the web client's own stub).
const STUB_PAYLOAD: &str = "34";
const ENCODED_EXTENSIONS: [&str; 2] = [
    "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time",
    "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01",
];
/// `tcptype` roles and the transport names Teams folds them into.
const TCP_TYPES: [(&str, &str); 3] = [
    ("active", "tcp-act"),
    ("passive", "tcp-pass"),
    ("so", "tcp-so"),
];
/// Candidate extensions the web client deletes before a candidate goes out.
const CANDIDATE_EXTRAS: [&str; 3] = ["generation", "network-id", "network-cost"];
const MAX_INFLATED: usize = 1 << 20;
const DIRECTIONS: [&str; 4] = ["sendrecv", "sendonly", "recvonly", "inactive"];
// The sections handed to webrtc-rs: every audio section, and for each video role the session has
// (`MediaOptions::video`) the first video section labelled for it. The rest would only be
// declined, `x-data` fails the whole description (`SdpInvalidValue`; with the data channel it is
// rewritten as an SCTP section instead, see `DATA_KIND`), and the gallery's nine
// receive-only `main-video` lines need receivers IronTeams does not have. The rest are answered as
// stubs. The media servers give each kind its own ICE credentials although one BUNDLE names
// every mid, which webrtc-rs refuses ("multiple conflicting ice-ufrag values"): kept video
// sections take the first kept section's credentials, i.e. they are bundled onto the audio
// transport, as mautrix-teams answers (`teamsmedia/sdp.go` `answerSDP`).
/// Profiles Teams writes for SRTP lines; with a fingerprint each is DTLS-SRTP.
const MS_PROFILES: [&str; 4] = [MS_PROFILE, "RTP/SAVPF", "RTP/AVP", "RTP/AVPF"];
// The data line. Teams writes SCTP over DTLS as `m=x-data <port> RTP/SAVP 127 126` with
// `a=x-data-protocol:sctp`, `x-data`/`rtx` rtpmaps and an SSRC range; WebRTC writes it as
// `m=application <port> UDP/DTLS/SCTP webrtc-datachannel` with `a=sctp-port`. rusty-teams
// `sdp.rs` (`signal_data_line`, `browser_data_from_teams`; MIT, facts only) maps one onto the
// other, and its ROADMAP says live captions arrive that way.
const DATA_KIND: &str = "x-data";
const SCTP_KIND: &str = "application";
const SCTP_PROFILE: &str = "UDP/DTLS/SCTP";
const SCTP_FORMAT: &str = "webrtc-datachannel";
const SCTP_PORT: &str = "5000";
const SCTP_MAX_MESSAGE_SIZE: &str = "262144";
const DATA_PAYLOADS: &str = "127 126";
const DATA_LINES: [&str; 4] = [
    "a=x-data-protocol:sctp",
    "a=rtpmap:127 x-data/90000",
    "a=rtpmap:126 rtx/90000",
    "a=fmtp:126 apt=127",
];

impl SdpBlob {
    /// A blob of plain SDP labelled `application/sdp`.
    pub fn sdp(sdp: impl Into<String>) -> Self {
        Self {
            content_type: CONTENT_TYPE_SDP.to_owned(),
            blob: sdp.into(),
        }
    }

    /// A blob of plain SDP labelled `application/sdp-ngc-1.0`, as the web client sends it.
    pub fn ngc(sdp: impl Into<String>) -> Self {
        Self {
            content_type: CONTENT_TYPE_NGC.to_owned(),
            blob: sdp.into(),
        }
    }

    /// The SDP text of a blob of either content type: plain text, or base64 raw deflate.
    pub fn decode(&self) -> Result<String> {
        decode(&self.blob, None)
    }

    /// [`Self::decode`], also reading a blob deflated with a preset dictionary.
    pub fn decode_with_dictionary(&self, dictionary: &[u8]) -> Result<String> {
        decode(&self.blob, Some(dictionary))
    }
}

fn media(msg: impl Into<String>) -> Error {
    Error::Media(msg.into())
}

fn decode(blob: &str, dictionary: Option<&[u8]>) -> Result<String> {
    let text = blob.trim_start();
    if text.starts_with("v=") {
        return Ok(text.to_owned());
    }
    let raw = B64
        .decode(text.trim_end())
        .map_err(|e| media(format!("SDP blob is neither SDP text nor base64: {e}")))?;
    if let Some(sdp) = inflate(&raw, None) {
        return Ok(sdp);
    }
    match dictionary {
        Some(dict) => inflate(&raw, Some(dict))
            .ok_or_else(|| media("SDP blob does not inflate with the given dictionary")),
        None => Err(media(
            "SDP blob is deflated with Teams' preset dictionary, which IronTeams does not ship",
        )),
    }
}

/// Raw inflate, with an optional preset dictionary; `None` unless the result is SDP text.
fn inflate(data: &[u8], dictionary: Option<&[u8]>) -> Option<String> {
    let mut d = Decompress::new(false);
    if let Some(dict) = dictionary {
        d.set_dictionary(dict).ok()?;
    }
    let mut out = Vec::with_capacity(data.len() * 4 + 256);
    loop {
        let (before_in, before_out) = (d.total_in(), d.total_out());
        let consumed = usize::try_from(before_in).ok()?;
        let status = d
            .decompress_vec(&data[consumed..], &mut out, FlushDecompress::Finish)
            .ok()?;
        if status == Status::StreamEnd {
            break;
        }
        if out.len() >= MAX_INFLATED {
            return None;
        }
        if out.len() == out.capacity() {
            out.reserve(out.capacity());
        } else if d.total_in() == before_in && d.total_out() == before_out {
            // Truncated stream: no progress with room to spare.
            return None;
        }
    }
    String::from_utf8(out).ok().filter(|s| s.starts_with("v="))
}

/// An SDP split into its session lines and its media sections (each starting with `m=`).
struct Parsed {
    session: Vec<String>,
    sections: Vec<Vec<String>>,
}

impl Parsed {
    fn new(sdp: &str) -> Self {
        let mut session = Vec::new();
        let mut sections: Vec<Vec<String>> = Vec::new();
        for line in sdp.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
            if line.starts_with("m=") {
                sections.push(vec![line.to_owned()]);
            } else if let Some(section) = sections.last_mut() {
                section.push(line.to_owned());
            } else {
                session.push(line.to_owned());
            }
        }
        Self { session, sections }
    }
}

/// `m=<kind> <port> <profile> <formats>`.
struct MediaLine<'a> {
    kind: &'a str,
    port: &'a str,
    formats: &'a str,
}

impl<'a> MediaLine<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let mut parts = line.strip_prefix("m=")?.splitn(4, ' ');
        let kind = parts.next()?;
        let port = parts.next()?;
        let _profile = parts.next()?;
        Some(Self {
            kind,
            port,
            formats: parts.next().unwrap_or(""),
        })
    }

    fn with(&self, port: &str, profile: &str) -> String {
        if self.formats.is_empty() {
            format!("m={} {port} {profile}", self.kind)
        } else {
            format!("m={} {port} {profile} {}", self.kind, self.formats)
        }
    }
}

fn join(lines: Vec<String>) -> String {
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

fn attr<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.strip_prefix("a=")?
        .strip_prefix(name)?
        .strip_prefix(':')
}

// --- inbound ---

/// Teams' SDP (an offer or an answer) rewritten so webrtc-rs accepts it, audio only. Fails on
/// SDP keyed with SDES alone: webrtc-rs only does DTLS-SRTP.
pub fn from_teams(sdp: &str) -> Result<String> {
    from_teams_with(sdp, &[], false)
}

/// [`from_teams`] for a session that also has video lines for `roles`. `offer` is whether `sdp`
/// is the other side's offer: a kept video section it offers `sendonly` (a media server sending
/// to us) or `inactive` (a browser's sharing line before anyone shares) is read as `sendrecv`, so
/// our transceiver takes the line and stays able to send on it (webrtc-rs gives an `inactive`
/// line a receiver of its own, and a later offer then moves ours to a new mid); the media servers
/// switch sending with `UpdateMediaDescriptions`, not with SDP directions (mautrix-teams openapi).
/// [`to_teams_answer`] puts the RFC direction back on the wire.
pub fn from_teams_with(sdp: &str, roles: &[VideoRole], offer: bool) -> Result<String> {
    from_teams_with_data(sdp, roles, offer, false)
}

/// [`from_teams_with`] for a session that also has the data channel (`data`): the first
/// `m=x-data` section becomes a WebRTC `m=application` SCTP section instead of being removed.
pub fn from_teams_with_data(
    sdp: &str,
    roles: &[VideoRole],
    offer: bool,
    data: bool,
) -> Result<String> {
    let parsed = Parsed::new(sdp);
    if parsed.sections.is_empty() {
        return Err(media("remote SDP has no media section"));
    }
    let dtls = parsed
        .session
        .iter()
        .chain(parsed.sections.iter().flatten())
        .any(|l| l.starts_with("a=fingerprint:"));
    if !dtls {
        return Err(media(
            "remote SDP has no DTLS fingerprint (SDES-only keying); webrtc-rs supports DTLS-SRTP only",
        ));
    }
    // A native client states no `a=setup`; webrtc-rs would read that as `actpass`. Say the
    // role RFC 4145 gives it, so webrtc-rs takes the other one ([`to_teams_answer`] then says so).
    let default_setup = (!has_setup(&parsed)).then_some(if offer {
        OFFER_DEFAULT_SETUP
    } else {
        ANSWER_DEFAULT_SETUP
    });
    // Sections webrtc-rs is not given go, mids and all; `to_teams_answer` writes them back as
    // stubs from the offer.
    let keep = kept_sections(&parsed, roles, data);
    let removed: Vec<&str> = parsed
        .sections
        .iter()
        .zip(&keep)
        .filter(|(_, k)| !**k)
        .filter_map(|(s, _)| s.iter().find_map(|l| attr(l, "mid")))
        .collect();
    let bundle = BundleTransport::of(&parsed, &keep);
    let mut out = Vec::new();
    for line in &parsed.session {
        match line.strip_prefix("a=group:BUNDLE") {
            Some(mids) => {
                let kept: Vec<&str> = mids
                    .split_whitespace()
                    .filter(|mid| !removed.contains(mid))
                    .collect();
                out.push(format!("a=group:BUNDLE {}", kept.join(" ")));
            }
            None => out.extend(inbound(line)),
        }
    }
    for (index, section) in parsed.sections.iter().enumerate().filter(|(i, _)| keep[*i]) {
        let m = MediaLine::parse(&section[0]).ok_or_else(|| media("malformed m= line"))?;
        let rebundled = bundle.as_ref().filter(|b| b.rewrites(index, section));
        if m.kind == DATA_KIND {
            out.extend(sctp_from_teams(&m, section, rebundled));
            continue;
        }
        let profile = section[0].split(' ').nth(2).unwrap_or_default();
        if MS_PROFILES.contains(&profile) {
            out.push(m.with(m.port, DTLS_PROFILE));
        } else {
            out.push(section[0].clone());
        }
        for line in &section[1..] {
            if rebundled.is_some() && is_transport_line(line) {
                continue;
            }
            if offer && m.kind == "video" && (line == "a=sendonly" || line == "a=inactive") {
                out.push("a=sendrecv".to_owned());
                continue;
            }
            out.extend(inbound(line));
        }
        if let Some(b) = rebundled {
            out.extend(b.credentials.iter().cloned());
        }
        // Native clients leave the direction out (RFC 4566 default: sendrecv); webrtc-rs then
        // drops the section from its answer.
        let directed = section[1..]
            .iter()
            .any(|l| DIRECTIONS.iter().any(|d| l.strip_prefix("a=") == Some(*d)));
        if !directed && m.port != "0" {
            out.push("a=sendrecv".to_owned());
        }
        if let Some(setup) = default_setup.filter(|_| m.port != "0") {
            out.push(setup.to_owned());
        }
        // webrtc-rs routes an SSRC that no `a=ssrc` declares only while the session has a single
        // RTP line; with the camera or sharing line next to audio it dropped every packet of the
        // other side's audio. Teams declares the SSRC as a one-SSRC range instead.
        if let Some(ssrc) = single_ssrc(section).filter(|_| m.port != "0") {
            out.push(format!("a=ssrc:{ssrc} cname:{ssrc}"));
        }
    }
    Ok(join(out))
}

/// Whether any line of `parsed` states the DTLS role (`a=setup`).
fn has_setup(parsed: &Parsed) -> bool {
    parsed
        .session
        .iter()
        .chain(parsed.sections.iter().flatten())
        .any(|l| attr(l, "setup").is_some())
}

/// The SSRC a section sends from when it declares no `a=ssrc` of its own and its
/// `a=x-ssrc-range` names exactly one (the audio line's `N-N`). A wider range (a video line's
/// hundred) and the placeholder `1-1` of a line that sends nothing name none.
fn single_ssrc(section: &[String]) -> Option<u32> {
    if section.iter().any(|l| attr(l, "ssrc").is_some()) {
        return None;
    }
    let range = section.iter().find_map(|l| attr(l, "x-ssrc-range"))?;
    if range == NO_SSRC_RANGE {
        return None;
    }
    let (first, last) = range.split_once('-')?;
    let first: u32 = first.trim().parse().ok()?;
    (last.trim().parse::<u32>().ok()? == first).then_some(first)
}

/// A Teams `x-data` section as the WebRTC SCTP section webrtc-rs reads: its transport lines
/// (or the bundle's credentials), and the SCTP port WebRTC assumes. The RTP-only lines go.
fn sctp_from_teams(
    m: &MediaLine<'_>,
    section: &[String],
    rebundled: Option<&BundleTransport>,
) -> Vec<String> {
    let mut out = vec![format!(
        "m={SCTP_KIND} {} {SCTP_PROFILE} {SCTP_FORMAT}",
        m.port
    )];
    let rtp_only = |l: &str| {
        [
            "a=rtpmap:",
            "a=fmtp:",
            "a=rtcp",
            "a=x-",
            "a=label:",
            "a=ssrc",
        ]
        .iter()
        .any(|p| l.starts_with(p))
            || DIRECTIONS.iter().any(|d| l.strip_prefix("a=") == Some(*d))
    };
    for line in &section[1..] {
        if rtp_only(line.as_str()) || (rebundled.is_some() && is_transport_line(line)) {
            continue;
        }
        out.extend(inbound(line));
    }
    if let Some(b) = rebundled {
        out.extend(b.credentials.iter().cloned());
    }
    if !section.iter().any(|l| attr(l, "sctp-port").is_some()) {
        out.push(format!("a=sctp-port:{SCTP_PORT}"));
    }
    if !section
        .iter()
        .any(|l| attr(l, "max-message-size").is_some())
    {
        out.push(format!("a=max-message-size:{SCTP_MAX_MESSAGE_SIZE}"));
    }
    out
}

/// The video role a section serves, by its `a=label` (a video section without one is
/// `main-video`, as Teams' default).
fn role_of(section: &[String]) -> Option<VideoRole> {
    let m = MediaLine::parse(&section[0])?;
    if m.kind != "video" {
        return None;
    }
    let label = section
        .iter()
        .find_map(|l| attr(l, "label"))
        .unwrap_or(VideoRole::MainVideo.label());
    VideoRole::from_label(label)
}

/// Which sections webrtc-rs is given (see the note on the kept sections above). A role named
/// n times in `roles` keeps its first n sections (the camera line, then the receive slots).
fn kept_sections(parsed: &Parsed, roles: &[VideoRole], data: bool) -> Vec<bool> {
    let mut taken: Vec<VideoRole> = Vec::new();
    let mut data_taken = !data;
    let count = |list: &[VideoRole], r: VideoRole| list.iter().filter(|x| **x == r).count();
    parsed
        .sections
        .iter()
        .map(|s| {
            let Some(m) = MediaLine::parse(&s[0]) else {
                return false;
            };
            if m.kind == "audio" {
                return true;
            }
            // The data line is kept even when declined (port 0): an answer to our offer must
            // answer every section we offered.
            if m.kind == DATA_KIND {
                return !std::mem::replace(&mut data_taken, true);
            }
            match role_of(s) {
                Some(r) if count(&taken, r) < count(roles, r) && m.port != "0" => {
                    taken.push(r);
                    true
                }
                _ => false,
            }
        })
        .collect()
}

/// ICE lines that belong to a section's own transport.
fn is_transport_line(line: &str) -> bool {
    [
        "ice-ufrag",
        "ice-pwd",
        "candidate",
        "x-candidate-ipv6",
        "rtcp",
    ]
    .iter()
    .any(|name| attr(line, name).is_some())
}

/// The first kept section's ICE credentials, which later kept sections with other credentials
/// take instead of their own (their candidates go: they belong to another transport).
struct BundleTransport {
    first: usize,
    ufrag: String,
    credentials: Vec<String>,
}

impl BundleTransport {
    fn of(parsed: &Parsed, keep: &[bool]) -> Option<Self> {
        let first = keep.iter().position(|k| *k)?;
        let section = &parsed.sections[first];
        let ufrag = section
            .iter()
            .find_map(|l| attr(l, "ice-ufrag"))?
            .to_owned();
        let credentials = section
            .iter()
            .filter(|l| attr(l, "ice-ufrag").is_some() || attr(l, "ice-pwd").is_some())
            .cloned()
            .collect();
        Some(BundleTransport {
            first,
            ufrag,
            credentials,
        })
    }

    fn rewrites(&self, index: usize, section: &[String]) -> bool {
        index != self.first
            && section
                .iter()
                .find_map(|l| attr(l, "ice-ufrag"))
                .is_some_and(|u| u != self.ufrag)
    }
}

fn inbound(line: &str) -> Option<String> {
    if let Some(c) = attr(line, "candidate").or_else(|| attr(line, "x-candidate-ipv6")) {
        return candidate_from_teams(c).map(|c| format!("a=candidate:{c}"));
    }
    if line.starts_with("a=extmap:") {
        return Some(line.replace('\\', "/"));
    }
    Some(line.to_owned())
}

/// A UDP RTP candidate in standard form; `None` for TCP, RTCP-component and unreadable ones.
fn candidate_from_teams(value: &str) -> Option<String> {
    let f: Vec<&str> = value.split_whitespace().collect();
    if f.len() < 8 || f[6] != "typ" || f[1] != "1" || !f[2].eq_ignore_ascii_case("udp") {
        return None;
    }
    let mut out = format!("{} 1 udp {} {} {} typ {}", f[0], f[3], f[4], f[5], f[7]);
    let extra: HashMap<&str, &str> = f[8..]
        .chunks(2)
        .filter_map(|kv| Some((kv[0], *kv.get(1)?)))
        .collect();
    if let (Some(addr), Some(port)) = (extra.get("raddr"), extra.get("rport"))
        && addr.parse::<IpAddr>().is_ok()
        && port.parse::<u16>().is_ok()
    {
        out.push_str(&format!(" raddr {addr} rport {port}"));
    }
    Some(out)
}

// --- outbound ---

/// Our (webrtc-rs) offer rewritten into the dialect the Teams calling service reads.
pub fn to_teams_offer(sdp: &str) -> String {
    to_teams(sdp, None, None, &[], false)
}

/// [`to_teams_offer`] for a re-offer on a negotiated session: one section per section of
/// `negotiated` (the Teams SDP that last settled the session, ours or theirs), in its order, with
/// its mids, kinds and labels. A line webrtc-rs no longer offers (the declined `x-data` line it
/// was never given) comes back as a port-0 stub; a line beyond the negotiated set is dropped, and
/// the BUNDLE group with it. The other side refuses a re-offer whose lines moved
/// (`CallEndReasonMediaAnswerError`, 488/3102).
pub fn to_teams_reoffer(sdp: &str, negotiated: &str) -> String {
    to_teams(sdp, None, Some(negotiated), &[], false)
}

/// Our (webrtc-rs) answer to `remote_offer` rewritten into the Teams dialect: one section per
/// offered section, in the offer's order (those [`from_teams`] removed come back as stubs), each
/// keeping the `a=label` the offer gave it.
pub fn to_teams_answer(sdp: &str, remote_offer: &str) -> String {
    to_teams(sdp, Some(remote_offer), None, &[], false)
}

/// [`to_teams_answer`] for a session with video lines for `roles` (as [`from_teams_with`] read
/// the offer). Each answered section's direction is made one the offer allows: a line offered
/// `sendonly` is answered `recvonly` even though webrtc-rs, given `sendrecv`, answered that.
pub fn to_teams_answer_with(sdp: &str, remote_offer: &str, roles: &[VideoRole]) -> String {
    to_teams(sdp, Some(remote_offer), None, roles, false)
}

/// [`to_teams_answer_with`] for a session that also has the data channel: the offer's `x-data`
/// section is answered with our SCTP section (in Teams' `x-data` form) instead of a stub.
pub fn to_teams_answer_with_data(
    sdp: &str,
    remote_offer: &str,
    roles: &[VideoRole],
    data: bool,
) -> String {
    to_teams(sdp, Some(remote_offer), None, roles, data)
}

/// The direction an answer may give a line offered `offered`, given what we would do (`ours`).
pub fn answer_direction(offered: &str, ours: &str) -> &'static str {
    let we_send = matches!(ours, "sendrecv" | "sendonly");
    let we_receive = matches!(ours, "sendrecv" | "recvonly");
    let they_send = matches!(offered, "sendrecv" | "sendonly");
    let they_receive = matches!(offered, "sendrecv" | "recvonly");
    match (we_send && they_receive, we_receive && they_send) {
        (true, true) => "sendrecv",
        (true, false) => "sendonly",
        (false, true) => "recvonly",
        (false, false) => "inactive",
    }
}

/// `sdp` (Teams dialect, ours) with the direction of the section labelled for `role` set to
/// `direction`. A 1:1 re-offer states the sharing line `sendonly` while sharing and `inactive`
/// otherwise (mautrix-teams `teamsmedia/direct.go`).
pub fn with_role_direction(sdp: &str, role: VideoRole, direction: &str) -> String {
    let mut parsed = Parsed::new(sdp);
    for section in &mut parsed.sections {
        if role_of(section) == Some(role) {
            set_direction(section, direction);
        }
    }
    let mut lines = parsed.session;
    lines.extend(parsed.sections.into_iter().flatten());
    join(lines)
}

fn set_direction(section: &mut Vec<String>, direction: &str) {
    section.retain(|l| !DIRECTIONS.iter().any(|d| l.strip_prefix("a=") == Some(*d)));
    let at = section
        .iter()
        .position(|l| attr(l, "mid").is_some())
        .map_or(section.len(), |i| i + 1);
    section.insert(at, format!("a={direction}"));
}

/// Our answer (Teams dialect) to the other side's 1:1 `offer`, saying on each video line what we
/// send: the camera line sends only while `camera_on`, the sharing line only while `sharing`,
/// each within what the offer allows ([`answer_direction`]). webrtc-rs answers every line it has
/// a track for `sendonly`/`sendrecv`, so a 1:1 answer said our camera sent while it was off, and
/// Teams web then shows our tile as an empty video box. Declined (port 0) lines stay as they are.
pub fn direct_answer(answer: &str, offer: &str, camera_on: bool, sharing: bool) -> String {
    let offered: HashMap<String, String> = Parsed::new(offer)
        .sections
        .iter()
        .filter_map(|s| {
            let mid = s.iter().find_map(|l| attr(l, "mid"))?;
            Some((mid.to_owned(), direction_of(s).to_owned()))
        })
        .collect();
    let mut parsed = Parsed::new(answer);
    for section in &mut parsed.sections {
        if MediaLine::parse(&section[0]).is_none_or(|m| m.port == "0") {
            continue;
        }
        let ours = match role_of(section) {
            Some(VideoRole::MainVideo) if camera_on => "sendrecv",
            Some(VideoRole::ApplicationSharing) if sharing => "sendonly",
            Some(_) => "recvonly",
            None => continue,
        };
        let Some(theirs) = section
            .iter()
            .find_map(|l| attr(l, "mid"))
            .and_then(|mid| offered.get(mid))
        else {
            continue;
        };
        let direction = answer_direction(theirs, ours);
        set_direction(section, direction);
    }
    let mut lines = parsed.session;
    lines.extend(parsed.sections.into_iter().flatten());
    join(lines)
}

/// Each section of `sdp` as `<mid> <label> <direction> <x-ssrc-range>` (`-` when missing; a
/// declined section is `<mid> <label> declined`), for the live log of a 1:1 negotiation: what
/// each side says it sends on which line.
pub fn directions(sdp: &str) -> String {
    Parsed::new(sdp)
        .sections
        .iter()
        .map(|s| {
            let m = MediaLine::parse(&s[0]);
            let found = |name| s.iter().find_map(|l| attr(l, name)).unwrap_or("-");
            let label = s
                .iter()
                .find_map(|l| attr(l, "label"))
                .or(m.as_ref().map(|m| m.kind))
                .unwrap_or("-");
            if m.is_some_and(|m| m.port == "0") {
                format!("{} {label} declined", found("mid"))
            } else {
                format!(
                    "{} {label} {} {}",
                    found("mid"),
                    direction_of(s),
                    found("x-ssrc-range")
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The transport of an SDP's audio section, for the live log of a renegotiation:
/// `mid <mid> ufrag <ufrag> setup <role> candidates [<type> <transport>, ...]` (`-` when missing),
/// then ` ice-lite` when the SDP says so.
/// Enough to tell a restart (new mid or ufrag), who is the DTLS client (`setup`), and which
/// paths ICE had to try; no password, no address.
pub fn transport_summary(sdp: &str) -> String {
    let parsed = Parsed::new(sdp);
    let Some(audio) = parsed
        .sections
        .iter()
        .find(|s| MediaLine::parse(&s[0]).is_some_and(|m| m.kind == "audio"))
    else {
        return "no audio".to_owned();
    };
    let value = |name: &str| {
        audio
            .iter()
            .chain(&parsed.session)
            .find_map(|l| attr(l, name))
            .unwrap_or("-")
            .to_owned()
    };
    let candidates: Vec<String> = audio
        .iter()
        .filter_map(|l| attr(l, "candidate"))
        .map(|c| {
            let f: Vec<&str> = c.split_whitespace().collect();
            let kind = f.get(7).copied().unwrap_or("?");
            let transport = f.get(2).copied().unwrap_or("?").to_ascii_lowercase();
            format!("{kind} {transport}")
        })
        .collect();
    // A lite agent on their side makes us the controlling one (RFC 8445 6.1.1; webrtc-rs reads it).
    let lite = if parsed.session.iter().any(|l| l == "a=ice-lite") {
        " ice-lite"
    } else {
        ""
    };
    format!(
        "mid {} ufrag {} setup {} candidates [{}]{lite}",
        value("mid"),
        value("ice-ufrag"),
        value("setup"),
        candidates.join(", ")
    )
}

/// Each section's `a=x-source-streamid` by mid: what a meeting's subscription names as the
/// line's `streamMsid` (rusty-teams `sdp.rs` `stream_lines`). Sections without one are left out.
pub fn source_stream_ids(sdp: &str) -> HashMap<String, u32> {
    Parsed::new(sdp)
        .sections
        .iter()
        .filter_map(|s| {
            let mid = s.iter().find_map(|l| attr(l, "mid"))?;
            let id = s
                .iter()
                .find_map(|l| attr(l, "x-source-streamid"))?
                .trim()
                .parse()
                .ok()?;
            Some((mid.to_owned(), id))
        })
        .collect()
}

/// The mids of `sdp`'s video sections that are not declined (port 0), in order.
pub fn video_mids(sdp: &str) -> Vec<String> {
    Parsed::new(sdp)
        .sections
        .iter()
        .filter(|s| MediaLine::parse(&s[0]).is_some_and(|m| m.kind == "video" && m.port != "0"))
        .filter_map(|s| s.iter().find_map(|l| attr(l, "mid")).map(str::to_owned))
        .collect()
}

/// [`video_mids`] of the lines that can send (`sendrecv` or `sendonly`): the lines a meeting
/// join's `mediaDescriptions` may name (the web client's names its `sendrecv` mids 1-6 and 11,
/// never its `recvonly` 7-10; mautrix-teams openapi `TeamsSdp`).
pub fn send_capable_video_mids(sdp: &str) -> Vec<String> {
    Parsed::new(sdp)
        .sections
        .iter()
        .filter(|s| MediaLine::parse(&s[0]).is_some_and(|m| m.kind == "video" && m.port != "0"))
        .filter(|s| matches!(direction_of(s), "sendrecv" | "sendonly"))
        .filter_map(|s| s.iter().find_map(|l| attr(l, "mid")).map(str::to_owned))
        .collect()
}

/// The mid of the first section labelled for `role`.
pub fn mid_of_role(sdp: &str, role: VideoRole) -> Option<String> {
    Parsed::new(sdp)
        .sections
        .iter()
        .find(|s| role_of(s) == Some(role))
        .and_then(|s| s.iter().find_map(|l| attr(l, "mid")))
        .map(str::to_owned)
}

fn direction_of(section: &[String]) -> &str {
    section
        .iter()
        .find_map(|l| {
            DIRECTIONS
                .iter()
                .find(|d| l.strip_prefix("a=") == Some(**d))
        })
        .copied()
        .unwrap_or("sendrecv")
}

/// One section per offered section, in the offer's order: ours where we answered a kept section
/// (matched by mid), else a port-0 stand-in built from the offer's.
fn in_offer_order(
    ours: Vec<Vec<String>>,
    remote_offer: &Parsed,
    roles: &[VideoRole],
    data: bool,
) -> Vec<Vec<String>> {
    let mid_of = |s: &[String]| s.iter().find_map(|l| attr(l, "mid")).map(str::to_owned);
    let mut ours: Vec<Option<Vec<String>>> = ours.into_iter().map(Some).collect();
    let keep = kept_sections(remote_offer, roles, data);
    let mut out = Vec::new();
    for (theirs, kept) in remote_offer.sections.iter().zip(keep) {
        let mid = mid_of(theirs);
        let answered = kept
            .then(|| {
                ours.iter_mut()
                    .find(|s| s.as_ref().is_some_and(|s| mid_of(s) == mid))
                    .and_then(Option::take)
            })
            .flatten();
        if let Some(mut section) = answered {
            let offered = direction_of(theirs).to_owned();
            if let Some(i) = section
                .iter()
                .position(|l| DIRECTIONS.iter().any(|d| l.strip_prefix("a=") == Some(*d)))
            {
                let ours = section[i].trim_start_matches("a=").to_owned();
                section[i] = format!("a={}", answer_direction(&offered, &ours));
            }
            out.push(section);
        } else {
            out.extend(stub_of(theirs));
        }
    }
    out
}

/// A port-0 stand-in for `section`: its kind, mid and label only.
fn stub_of(section: &[String]) -> Option<Vec<String>> {
    let m = MediaLine::parse(&section[0])?;
    let mut stub = vec![format!("m={} 0 {MS_PROFILE} {STUB_PAYLOAD}", m.kind)];
    for name in ["mid", "label"] {
        stub.extend(
            section
                .iter()
                .find_map(|l| attr(l, name))
                .map(|v| format!("a={name}:{v}")),
        );
    }
    Some(stub)
}

/// The label a section of our own (webrtc-rs) SDP carries: its `a=label`, else the stream id
/// of its `a=msid` when that names a video role.
fn own_label(section: &[String]) -> Option<&str> {
    section.iter().find_map(|l| attr(l, "label")).or_else(|| {
        section
            .iter()
            .find_map(|l| attr(l, "msid"))
            .and_then(|v| v.split_whitespace().next())
            .and_then(VideoRole::from_label)
            .map(VideoRole::label)
    })
}

/// Our offer's sections laid out as `negotiated`'s: ours where a section has the same mid and
/// kind, else a stub; ours beyond them go. A section the other side declined with a stub that
/// names no mid (a browser's answer to a line it refused, live 2026-11-20) is ours again when
/// one of our sections of its kind and label has a mid `negotiated` does not name: the line is
/// offered anew on its own mid instead of being lost (a share re-offer then has no sharing line).
fn in_negotiated_order(ours: Vec<Vec<String>>, negotiated: &Parsed) -> Vec<Vec<String>> {
    let kind = |s: &[String]| MediaLine::parse(&s[0]).map(|m| m.kind.to_owned());
    let mid = |s: &[String]| s.iter().find_map(|l| attr(l, "mid")).map(str::to_owned);
    let named: Vec<String> = negotiated.sections.iter().filter_map(|s| mid(s)).collect();
    let mut ours: Vec<Option<Vec<String>>> = ours.into_iter().map(Some).collect();
    negotiated
        .sections
        .iter()
        .filter_map(|theirs| {
            let matches = |s: &Vec<String>| match mid(theirs) {
                Some(m) => kind(s) == kind(theirs) && mid(s) == Some(m),
                None => {
                    kind(s) == kind(theirs)
                        && mid(s).is_some_and(|m| !named.contains(&m))
                        && own_label(s).is_some()
                        && own_label(s) == theirs.iter().find_map(|l| attr(l, "label"))
                }
            };
            ours.iter_mut()
                .find(|s| s.as_ref().is_some_and(matches))
                .and_then(Option::take)
                .or_else(|| stub_of(theirs))
        })
        .collect()
}

/// `session` with its BUNDLE group cut to the mids of `sections` that are not declined.
fn bundle_of(session: &mut [String], sections: &[Vec<String>]) {
    let live: Vec<&str> = sections
        .iter()
        .filter(|s| MediaLine::parse(&s[0]).is_some_and(|m| m.port != "0"))
        .filter_map(|s| s.iter().find_map(|l| attr(l, "mid")))
        .collect();
    for line in session.iter_mut() {
        if let Some(mids) = line.strip_prefix("a=group:BUNDLE") {
            let kept: Vec<&str> = mids
                .split_whitespace()
                .filter(|m| live.contains(m))
                .collect();
            *line = format!("a=group:BUNDLE {}", kept.join(" "));
        }
    }
}

fn labels_by_mid(parsed: &Parsed) -> HashMap<String, String> {
    parsed
        .sections
        .iter()
        .filter_map(|s| {
            let mid = s.iter().find_map(|l| attr(l, "mid"))?;
            let label = s.iter().find_map(|l| attr(l, "label"))?;
            Some((mid.to_owned(), label.to_owned()))
        })
        .collect()
}

fn default_label(kind: &str) -> Option<&'static str> {
    match kind {
        "audio" => Some("main-audio"),
        "video" => Some("main-video"),
        DATA_KIND => Some("data"),
        _ => None,
    }
}

/// The port and `c=` value of the bundle: the first section's first UDP host candidate (else its
/// first UDP candidate), which the web client writes on the m-line in place of `9`/`0.0.0.0`.
fn bundle_transport(sections: &[Vec<String>]) -> Option<(String, String)> {
    let section = sections
        .iter()
        .find(|s| s.iter().any(|l| attr(l, "candidate").is_some()))?;
    let udp: Vec<Vec<&str>> = section
        .iter()
        .filter_map(|l| attr(l, "candidate"))
        .map(|c| c.split_whitespace().collect::<Vec<_>>())
        .filter(|f| f.len() >= 8 && f[1] == "1" && f[2].eq_ignore_ascii_case("udp"))
        .collect();
    let best = udp.iter().find(|f| f[7] == "host").or(udp.first())?;
    let family = if best[4].contains(':') { "IP6" } else { "IP4" };
    Some((best[5].to_owned(), format!("IN {family} {}", best[4])))
}

fn to_teams(
    sdp: &str,
    remote_offer: Option<&str>,
    negotiated: Option<&str>,
    roles: &[VideoRole],
    data: bool,
) -> String {
    let offer = remote_offer.is_none();
    // webrtc-rs answers `active` whatever the offer says. To an offer with no `a=setup` (the
    // offerer is `active`, and `from_teams` made webrtc-rs the DTLS server) we are `passive`.
    let passive = remote_offer.is_some_and(|o| !has_setup(&Parsed::new(o)));
    let mut parsed = Parsed::new(sdp);
    for section in &mut parsed.sections {
        sctp_to_teams(section);
    }
    let labels = match (remote_offer.map(Parsed::new), negotiated.map(Parsed::new)) {
        (Some(theirs), _) => {
            parsed.sections = in_offer_order(parsed.sections, &theirs, roles, data);
            labels_by_mid(&theirs)
        }
        (None, Some(layout)) => {
            parsed.sections = in_negotiated_order(parsed.sections, &layout);
            bundle_of(&mut parsed.session, &parsed.sections);
            labels_by_mid(&layout)
        }
        (None, None) => HashMap::new(),
    };
    let fingerprint = parsed
        .session
        .iter()
        .find(|l| l.starts_with("a=fingerprint:"))
        .cloned();
    let transport = bundle_transport(&parsed.sections);
    let mut out = Vec::new();
    for line in &parsed.session {
        if offer && line.starts_with("b=") {
            continue;
        }
        if offer && line.starts_with("t=") {
            out.push(SESSION_BANDWIDTH.to_owned());
        }
        if offer && line.starts_with("a=msid-semantic:") {
            out.push("a=msid-semantic: WMS *".to_owned());
            continue;
        }
        out.push(line.clone());
    }
    if offer
        && !parsed
            .session
            .iter()
            .any(|l| l.starts_with("a=msid-semantic:"))
    {
        out.push("a=msid-semantic: WMS *".to_owned());
    }
    for section in &parsed.sections {
        let Some(m) = MediaLine::parse(&section[0]) else {
            out.extend(section.iter().cloned());
            continue;
        };
        let mid = section.iter().find_map(|l| attr(l, "mid"));
        // Our own video lines say what they carry through their stream id (`a=msid:<label> …`).
        let label = mid
            .and_then(|mid| labels.get(mid).map(String::as_str))
            .or_else(|| {
                section
                    .iter()
                    .find_map(|l| attr(l, "msid"))
                    .and_then(|v| v.split_whitespace().next())
                    .and_then(VideoRole::from_label)
                    .map(VideoRole::label)
            })
            .or_else(|| default_label(m.kind));
        // A declined section is a stub, offer or answer: the web client keeps the kind, port,
        // profile, mid and label, names payload 34, and the service refuses anything more
        // (`SdpParsingFailure`). `c=` because the session level has none.
        if m.port == "0" {
            // A stub keeps the label it came with (the negotiated line's).
            let label = section.iter().find_map(|l| attr(l, "label")).or(label);
            out.push(format!("m={} 0 {MS_PROFILE} {STUB_PAYLOAD}", m.kind));
            out.push("c=IN IP4 0.0.0.0".to_owned());
            out.extend(mid.map(|mid| format!("a=mid:{mid}")));
            out.extend(label.map(|label| format!("a=label:{label}")));
            continue;
        }
        let placed = transport.as_ref().filter(|_| m.port != "0");
        match placed {
            Some((port, connection)) => {
                out.push(m.with(port, MS_PROFILE));
                out.push(format!("c={connection}"));
                if offer && m.kind != DATA_KIND {
                    out.push(format!("a=rtcp:{port}"));
                }
            }
            None => out.push(m.with(m.port, MS_PROFILE)),
        }
        let has_fingerprint = section.iter().any(|l| l.starts_with("a=fingerprint:"));
        if offer && !has_fingerprint {
            out.extend(fingerprint.clone());
        }
        let mut has_label = false;
        let mut has_range = false;
        let mut ssrc = None;
        for line in &section[1..] {
            if (placed.is_some() && line.starts_with("c=")) || line.starts_with("a=rtcp:") {
                continue;
            }
            has_label |= line.starts_with("a=label:");
            has_range |= line.starts_with("a=x-ssrc-range:");
            if ssrc.is_none() {
                ssrc = attr(line, "ssrc")
                    .and_then(|v| v.split(' ').next())
                    .map(str::to_owned);
            }
            if let Some(c) = attr(line, "candidate") {
                // rtcp-mux: the RTCP component's twin (webrtc-rs writes one) goes nowhere.
                if c.split_whitespace().nth(1) == Some("1") {
                    out.push(format!("a=candidate:{}", candidate_to_teams(c)));
                }
            } else if line.starts_with("a=extmap:") {
                out.push(extmap_to_teams(line));
            } else if passive && attr(line, "setup").is_some() {
                out.push(ANSWER_DEFAULT_SETUP.to_owned());
            } else {
                out.push(line.clone());
            }
        }
        if let (false, Some(label)) = (has_label, label) {
            out.push(format!("a=label:{label}"));
        }
        if let (false, Some(ssrc)) = (has_range, &ssrc) {
            out.push(format!("a=x-ssrc-range:{ssrc}-{ssrc}"));
        }
        if offer && m.kind == "video" {
            // As the web client and mautrix-teams offer every video line: a line that sends
            // nothing states the placeholder range `1-1`, and each declares the signalling
            // feedback the media controller drives video with (source requests, `vc`).
            if !has_range && ssrc.is_none() {
                out.push(format!("a=x-ssrc-range:{NO_SSRC_RANGE}"));
            }
            if !section.iter().any(|l| attr(l, "x-signaling-fb").is_some()) {
                out.push(VIDEO_SIGNALING_FB.to_owned());
            }
        }
    }
    join(out)
}

/// Our WebRTC SCTP section (`m=application`) rewritten in place as Teams' `x-data` line; the
/// rest of [`to_teams`] then places it on the bundle like any other line. Other sections are
/// left alone.
fn sctp_to_teams(section: &mut Vec<String>) {
    let Some(m) = MediaLine::parse(&section[0]) else {
        return;
    };
    if m.kind != SCTP_KIND {
        return;
    }
    let port = m.port.to_owned();
    section[0] = format!("m={DATA_KIND} {port} {MS_PROFILE} {DATA_PAYLOADS}");
    if port == "0" {
        return;
    }
    let ssrc = rand::random::<u32>() >> 1;
    let mut head: Vec<String> = DATA_LINES.iter().map(|l| (*l).to_owned()).collect();
    head.insert(1, format!("a=x-ssrc-range:{ssrc}-{ssrc}"));
    if !section.iter().any(|l| l == "a=rtcp-mux") {
        head.push("a=rtcp-mux".to_owned());
    }
    if !section[1..]
        .iter()
        .any(|l| DIRECTIONS.iter().any(|d| l.strip_prefix("a=") == Some(*d)))
    {
        head.push("a=sendrecv".to_owned());
    }
    // After `c=` (which `to_teams` replaces with the bundle's) so the m-line keeps it next.
    let at = section
        .iter()
        .position(|l| l.starts_with("c="))
        .map_or(1, |i| i + 1);
    let tail = section.split_off(at);
    section.extend(head);
    section.extend(tail);
}

fn candidate_to_teams(value: &str) -> String {
    let mut f: Vec<&str> = value.split_whitespace().collect();
    if f.len() > 2 {
        if f[2].eq_ignore_ascii_case("udp") {
            f[2] = "UDP";
        } else if f[2].eq_ignore_ascii_case("tcp")
            && let Some(at) = f.iter().position(|x| *x == "tcptype")
            && let Some(&(_, ms)) = f
                .get(at + 1)
                .and_then(|role| TCP_TYPES.iter().find(|(jsep, _)| jsep == role))
        {
            f[2] = ms;
            f.drain(at..at + 2);
        }
    }
    let mut out = Vec::with_capacity(f.len());
    let mut i = 0;
    while i < f.len() {
        if i >= 8 && CANDIDATE_EXTRAS.contains(&f[i]) {
            i += 2;
            continue;
        }
        out.push(f[i]);
        i += 1;
    }
    out.join(" ")
}

fn extmap_to_teams(line: &str) -> String {
    for uri in ENCODED_EXTENSIONS {
        if let Some(head) = line.strip_suffix(uri)
            && head.ends_with(' ')
        {
            return format!("{head}{}", uri.replace('/', "\\"));
        }
    }
    line.to_owned()
}

/// The mid and ICE ufrag of an SDP's first audio section (the ufrag may be session-level).
pub(crate) fn audio_transport(sdp: &str) -> Option<(String, String)> {
    let parsed = Parsed::new(sdp);
    let audio = parsed
        .sections
        .iter()
        .find(|s| MediaLine::parse(&s[0]).is_some_and(|m| m.kind == "audio"))?;
    let mid = audio.iter().find_map(|l| attr(l, "mid"))?;
    let ufrag = audio
        .iter()
        .chain(&parsed.session)
        .find_map(|l| attr(l, "ice-ufrag"))?;
    Some((mid.to_owned(), ufrag.to_owned()))
}
