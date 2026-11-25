//! Answering an incoming 1:1 call, with or without the caller's offer in the ring.
//!
//! Live 2026-11-19: a ring from a Teams web guest came with `callNotification.mediaContent: null`
//! (and `links.mediaAnswer: "cc://ma"`, a placeholder no client can POST to). The second live
//! test got no further than the attach: no SDP was exchanged, the ring was forwarded to
//! voicemail, and the voicemail leg ended the call with `SdpParsingFailure` (7106). What happens
//! when the ring has no offer is chosen by [`NoOffer`]:
//!
//! - [`NoOffer::FetchOffer`] (default; rusty-teams `engine.rs` `ring_task`, MIT): `attach` with
//!   `requireMediaContent: true`. An offer in the attach answer is answered (the answerer path);
//!   without one we offer ourselves, as [`NoOffer::NewOffer`] does.
//! - [`NoOffer::NewOffer`]: attach as the web client does (`requireMediaContent: false`), then
//!   post OUR offer as a `mediaNegotiation` to the attach answer's `newOffer` link (the shape of
//!   rusty-teams' own client offers), accept without media, and apply the caller's answer.
//! - [`NoOffer::AcceptOffer`]: our offer in the acceptance itself. That slot holds an answer
//!   everywhere else, and an answer with offer-only lines draws `SdpParsingFailure`
//!   (teams-lite), so this is only a mode to try.
//! - [`NoOffer::OfferInAttach`]: our offer inside the attach, then accept without media.
//!
//! Every offer of ours is built as `place_call` builds it (verified live for outgoing calls):
//! [`MediaSession::create_offer`], `application/sdp-ngc-1.0`, plain SDP, `nonByPass`. The answer
//! comes inline or on our `call/mediaAnswer/` callback and is applied with
//! [`MediaSession::set_answer`]. All offerer paths are UNVERIFIED live.
//!
//! [`Answering`] is the pure state machine; [`answer_invitation`] drives it against the service
//! and reports each step, and every SDP blob before it is decoded, to its observer.

use crate::error::{Error, Result};
use crate::invitation::CallEvent;
use crate::media::MediaSession;
use crate::signaling::{Attached, CallSession, NgcClient};
use crate::types::{CallInvitation, Links, Modality, SdpBlob};
use std::time::Duration;
use tokio::sync::mpsc;

/// What the banner says when the caller gave up before our answer got through.
pub const ENDED_BEFORE_ANSWER: &str = "The call ended before you answered.";

/// Plain words for a failed answer. The service refuses an attach to a ring that is over with a
/// 412 (or 487), whose body is an `operationFailure` with `code` 487 ("canceled"): that is the
/// caller giving up, not a fault. The raw status and body belong in the log only.
pub fn answer_error_text(e: &Error) -> String {
    match e {
        Error::Status { op, status, body } => {
            let over = matches!(status, 412 | 487)
                || serde_json::from_str::<serde_json::Value>(body)
                    .is_ok_and(|v| v.pointer("/operationFailure/code") == Some(&487.into()));
            if over {
                ENDED_BEFORE_ANSWER.to_owned()
            } else {
                format!("{op}: HTTP {status}")
            }
        }
        other => other.to_string(),
    }
}

/// How long we wait for the caller's answer to our offer.
pub const ANSWER_WAIT: Duration = Duration::from_secs(20);
/// How long an attach may take (one that asks for media must not hold a ringing call).
pub const ATTACH_WAIT: Duration = Duration::from_secs(15);

/// What to do when the ring carries no offer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NoOffer {
    /// Ask for the caller's offer with the attach; offer ourselves when none comes.
    #[default]
    FetchOffer,
    /// Attach without asking, then offer on the attach answer's `newOffer` link.
    NewOffer,
    /// Attach without asking, then accept with our offer as the acceptance's media.
    AcceptOffer,
    /// Send our offer inside the attach, then accept without media.
    OfferInAttach,
}

impl NoOffer {
    /// `fetch`, `new-offer`, `accept-offer` or `attach-offer` (the CLI's `--no-offer` and the
    /// GUI's `IRONTEAMS_CALL_NO_OFFER`).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "fetch" => NoOffer::FetchOffer,
            "new-offer" => NoOffer::NewOffer,
            "accept-offer" => NoOffer::AcceptOffer,
            "attach-offer" => NoOffer::OfferInAttach,
            _ => return None,
        })
    }
}

/// Where our own offer goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferVia {
    /// A `mediaNegotiation` POSTed to this link (the attach answer's `newOffer`).
    NewOffer(String),
    /// The acceptance's `mediaContent`.
    Acceptance,
    /// It already went out with the attach.
    Attach,
}

/// The next thing the callee does.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// POST the ring's `attach` link; `require_media` asks for the caller's offer and
    /// `with_offer` sends ours along.
    Attach {
        require_media: bool,
        with_offer: bool,
    },
    /// Answer the caller's offer and accept with the answer (the answerer path).
    AnswerOffer(SdpBlob),
    /// Nobody offered: send our offer this way, then accept.
    SendOffer(OfferVia),
    /// Our offer is out and the call accepted; wait for the caller's answer.
    AwaitAnswer,
    /// Accepted and negotiated: apply [`Answering::remote_answer`] (offerer path) and hand the
    /// call to the call manager.
    Adopt,
    Failed(String),
}

/// The callee's side of answering one invitation, as a state machine.
#[derive(Debug, Clone)]
pub struct Answering {
    step: Step,
    no_offer: NoOffer,
    /// The ring's own offer, kept until the attach answer is in.
    ring_offer: Option<SdpBlob>,
    links: Links,
    media_leg_id: Option<String>,
    answer: Option<SdpBlob>,
}

impl Answering {
    pub fn new(inv: &CallInvitation, no_offer: NoOffer) -> Self {
        let mut a = Answering {
            step: Step::Adopt,
            no_offer,
            ring_offer: inv.offer.clone(),
            links: Links::default(),
            media_leg_id: None,
            answer: None,
        };
        a.step = match (&inv.offer, inv.links.get("acceptance")) {
            // An invitation that is not forked: answer it straight away.
            (Some(offer), Some(_)) => {
                a.links = inv.links.clone();
                Step::AnswerOffer(offer.clone())
            }
            (Some(_), None) => Step::Attach {
                require_media: false,
                with_offer: false,
            },
            (None, _) => Step::Attach {
                require_media: no_offer == NoOffer::FetchOffer,
                with_offer: no_offer == NoOffer::OfferInAttach,
            },
        };
        a
    }

    pub fn step(&self) -> &Step {
        &self.step
    }

    /// The links gathered so far (attach, then acceptance).
    pub fn links(&self) -> &Links {
        &self.links
    }

    pub fn into_links(self) -> Links {
        self.links
    }

    /// The media leg id the attach answer named for the caller's offer.
    pub fn media_leg_id(&self) -> Option<&str> {
        self.media_leg_id.as_deref()
    }

    /// The caller's answer to our offer (offerer path only).
    pub fn remote_answer(&self) -> Option<&SdpBlob> {
        self.answer.as_ref()
    }

    /// We are the offerer on this call.
    pub fn offering(&self) -> bool {
        matches!(self.step, Step::SendOffer(_) | Step::AwaitAnswer)
            || (self.step == Step::Adopt && self.answer.is_some())
    }

    pub fn fail(&mut self, why: impl Into<String>) {
        self.step = Step::Failed(why.into());
    }

    /// The attach answer is in.
    pub fn on_attached(&mut self, a: Attached) {
        let Step::Attach { with_offer, .. } = self.step else {
            return;
        };
        self.links = a.links;
        if self.links.get("acceptance").is_none() {
            return self.fail("the attach answer has no acceptance link");
        }
        self.media_leg_id = a.media_leg_id;
        if with_offer {
            // Our offer is already out on this peer: stay the offerer.
            self.step = Step::SendOffer(OfferVia::Attach);
            return;
        }
        self.step = match self.ring_offer.take().or(a.offer) {
            Some(o) => Step::AnswerOffer(o),
            None => Step::SendOffer(self.offer_via()),
        };
    }

    fn offer_via(&self) -> OfferVia {
        match (self.no_offer, self.links.get("newOffer")) {
            (NoOffer::AcceptOffer, _) | (_, None) => OfferVia::Acceptance,
            (_, Some(url)) => OfferVia::NewOffer(url.to_owned()),
        }
    }

    /// The acceptance went through; `inline_answer` is an SDP its answer (or our offer's)
    /// carried.
    pub fn on_accepted(&mut self, links: Links, inline_answer: Option<SdpBlob>) {
        self.links.0.extend(links.0);
        match self.step {
            Step::AnswerOffer(_) => self.step = Step::Adopt,
            Step::SendOffer(_) => {
                if self.answer.is_none() {
                    self.answer = inline_answer;
                }
                self.step = if self.answer.is_some() {
                    Step::Adopt
                } else {
                    Step::AwaitAnswer
                };
            }
            _ => {}
        }
    }

    /// A callback of our session arrived (it may come before the acceptance's own answer).
    pub fn on_event(&mut self, ev: &CallEvent) {
        if !matches!(self.step, Step::SendOffer(_) | Step::AwaitAnswer) {
            return;
        }
        let (answer, links) = match ev {
            CallEvent::MediaAnswer { answer, links } => (Some(answer), links),
            CallEvent::Accepted { answer, links, .. } => (answer.as_ref(), links),
            CallEvent::Rejected(r) | CallEvent::Ended(r) => {
                return self.fail(format!(
                    "the call ended before the media was negotiated ({} {}: {})",
                    r.code, r.sub_code, r.phrase
                ));
            }
            _ => return,
        };
        self.links.0.extend(links.0.clone());
        if let Some(a) = answer
            && self.answer.is_none()
        {
            self.answer = Some(a.clone());
        }
        if self.step == Step::AwaitAnswer && self.answer.is_some() {
            self.step = Step::Adopt;
        }
    }

    /// [`ANSWER_WAIT`] passed without an answer.
    pub fn on_timeout(&mut self) {
        if self.step == Step::AwaitAnswer {
            self.fail("the caller never answered our media offer");
        }
    }
}

/// Watches the exchange, for `--dump` / `IRONTEAMS_CALL_DUMP`: called with a name and a text.
/// `step` names a step taken (one line); `*-blob` is an SDP envelope as received (content type,
/// then the blob, before any decoding); the rest (`caller-offer`, `our-answer`, `our-offer`,
/// `caller-answer`) are SDPs.
pub type Observe<'a> = &'a (dyn Fn(&str, &str) + Send + Sync);

/// Answers `inv` with audio on `media`: attach (when forked), report ringing, then either answer
/// the caller's offer or offer ourselves and apply the caller's answer. `events` carries the
/// callbacks of `s`'s session (the caller's `mediaAnswer`). Returns the call's links, ready for
/// the call manager; `s.media_leg_id` follows the offer's leg.
pub async fn answer_invitation(
    ngc: &NgcClient,
    s: &mut CallSession,
    inv: &CallInvitation,
    media: &MediaSession,
    events: &mut mpsc::UnboundedReceiver<CallEvent>,
    no_offer: NoOffer,
    observe: Observe<'_>,
) -> Result<Links> {
    step(
        observe,
        &format!(
            "ring: {}, mode {no_offer:?}",
            crate::invitation::ring_summary(inv)
        ),
    );
    let r = drive(ngc, s, inv, media, events, no_offer, observe).await;
    match &r {
        Ok(_) => observe("step", "adopt"),
        Err(e) => observe("step", &format!("failed: {e}")),
    }
    r
}

fn step(observe: Observe<'_>, what: &str) {
    tracing::info!(step = what, "answering a call");
    observe("step", what);
}

/// Our offer of a call being answered: the camera is off and nothing is shared yet, so the camera
/// line receives only and the sharing line is inactive, as the web client offers them
/// (webrtc-rs says both send).
fn idle_video_offer(offer: &str) -> String {
    use crate::media::sdp::with_role_direction;
    use crate::video::VideoRole;
    let offer = with_role_direction(offer, VideoRole::MainVideo, "recvonly");
    with_role_direction(&offer, VideoRole::ApplicationSharing, "inactive")
}

/// Dumps a blob as it came, then decodes it.
fn decode(observe: Observe<'_>, name: &str, blob: &SdpBlob) -> Result<String> {
    observe(
        &format!("{name}-blob"),
        &format!("contentType: {}\n\n{}", blob.content_type, blob.blob),
    );
    let sdp = blob.decode()?;
    observe(name, &sdp);
    Ok(sdp)
}

async fn drive(
    ngc: &NgcClient,
    s: &mut CallSession,
    inv: &CallInvitation,
    media: &MediaSession,
    events: &mut mpsc::UnboundedReceiver<CallEvent>,
    no_offer: NoOffer,
    observe: Observe<'_>,
) -> Result<Links> {
    let modalities = [Modality::Audio];
    let mut a = Answering::new(inv, no_offer);
    let mut rang = false;
    let mut our_offer: Option<String> = None;
    let mut deadline = None;
    loop {
        match a.step().clone() {
            Step::Attach {
                require_media,
                with_offer,
            } => {
                step(
                    observe,
                    &format!(
                        "attach requireMediaContent={require_media} withOurOffer={with_offer}"
                    ),
                );
                let attach = async {
                    if with_offer {
                        let offer = idle_video_offer(&media.create_offer().await?);
                        observe("our-offer", &offer);
                        let blob = SdpBlob::ngc(offer.as_str());
                        our_offer = Some(offer);
                        ngc.attach_with_offer(s, inv, &blob).await
                    } else {
                        ngc.attach_with(s, inv, require_media).await
                    }
                };
                let attached = tokio::time::timeout(ATTACH_WAIT, attach)
                    .await
                    .map_err(|_| {
                        Error::Protocol(format!(
                            "the attach got no answer within {ATTACH_WAIT:?} (requireMediaContent={require_media})"
                        ))
                    })??;
                step(
                    observe,
                    &format!(
                        "attached: offer {}, links [{}]",
                        if attached.offer.is_some() {
                            "present"
                        } else {
                            "missing"
                        },
                        attached
                            .links
                            .0
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                a.on_attached(attached);
                if let Some(leg) = a.media_leg_id() {
                    s.media_leg_id = leg.to_owned();
                }
            }
            Step::AnswerOffer(offer) => {
                step(observe, "answer the caller's offer");
                ring(ngc, s, a.links(), &mut rang).await;
                let sdp = decode(observe, "caller-offer", &offer)?;
                // The camera starts, and a share goes out, after the call runs (re-offers):
                // until then our video lines only receive.
                let answer = crate::media::sdp::direct_answer(
                    &media.accept_offer(&sdp).await?,
                    &sdp,
                    false,
                    false,
                );
                // What the other side's media stack must take as our final answer: the lines,
                // and each side's transport (BUNDLE, ICE lite, DTLS role, candidate kinds; no
                // address or password).
                let bundled = |text: &str| text.lines().any(|l| l.starts_with("a=group:BUNDLE"));
                tracing::info!(
                    offer_type = %offer.content_type,
                    theirs = %crate::media::sdp::directions(&sdp),
                    ours = %crate::media::sdp::directions(&answer),
                    theirs_transport = %crate::media::sdp::transport_summary(&sdp),
                    ours_transport = %crate::media::sdp::transport_summary(&answer),
                    theirs_bundle = bundled(&sdp),
                    ours_bundle = bundled(&answer),
                    "1:1 call answered"
                );
                observe("our-answer", &answer);
                let links = ngc
                    .accept(s, a.links(), &modalities, &SdpBlob::ngc(answer))
                    .await?;
                a.on_accepted(links, None);
            }
            Step::SendOffer(via) => {
                step(observe, &format!("no offer: we offer via {via:?}"));
                ring(ngc, s, a.links(), &mut rang).await;
                let offer = match our_offer.take() {
                    Some(o) => o,
                    None => {
                        let o = idle_video_offer(&media.create_offer().await?);
                        observe("our-offer", &o);
                        o
                    }
                };
                let blob = SdpBlob::ngc(offer);
                let (links, inline) = match &via {
                    OfferVia::Acceptance => {
                        ngc.accept_with_offer(s, a.links(), &modalities, &blob)
                            .await?
                    }
                    OfferVia::NewOffer(url) => {
                        let inline = ngc.send_offer(s, url, &modalities, &blob).await?;
                        let (links, acked) =
                            ngc.accept_without_media(s, a.links(), &modalities).await?;
                        (links, inline.or(acked))
                    }
                    OfferVia::Attach => ngc.accept_without_media(s, a.links(), &modalities).await?,
                };
                // An answer that raced the HTTP answers is already queued.
                while let Ok(ev) = events.try_recv() {
                    tracing::info!(event = %ev.summary(), "answering a call: callback");
                    a.on_event(&ev);
                }
                a.on_accepted(links, inline);
            }
            Step::AwaitAnswer => {
                let until = *deadline.get_or_insert_with(|| {
                    step(observe, "accepted; waiting for the caller's answer");
                    tokio::time::Instant::now() + ANSWER_WAIT
                });
                match tokio::time::timeout_at(until, events.recv()).await {
                    Ok(Some(ev)) => {
                        tracing::info!(event = %ev.summary(), "answering a call: callback");
                        a.on_event(&ev)
                    }
                    Ok(None) => a.fail("the calling connection closed"),
                    Err(_) => a.on_timeout(),
                }
            }
            Step::Adopt => {
                if let Some(answer) = a.remote_answer().cloned() {
                    let sdp = decode(observe, "caller-answer", &answer)?;
                    media.set_answer(&sdp).await?;
                    step(observe, "caller's answer applied");
                }
                return Ok(a.into_links());
            }
            Step::Failed(why) => return Err(Error::Protocol(why)),
        }
    }
}

/// Reports ringing once, when the leg has a `progress` link (a failure is only logged).
async fn ring(ngc: &NgcClient, s: &CallSession, links: &Links, rang: &mut bool) {
    if std::mem::replace(rang, true) || links.get("progress").is_none() {
        return;
    }
    if let Err(e) = ngc.send_ringing(s, links).await {
        tracing::warn!(%e, "reporting the call as ringing failed");
    }
}
