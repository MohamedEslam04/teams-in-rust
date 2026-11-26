//! One re-offer of ours at a time: hold, resume, and a 1:1 call's camera and share all go through
//! [`Reoffers`]. Nothing is sent while one waits for its answer (or the media moves to a new
//! session); changes made meanwhile are said together, once, when it is answered, refused or
//! superseded, or after [`ANSWER_WAIT`]. Live 2026-11-26: a re-offer sent while another's answer
//! was due got that answer applied to it, and the camera went off by itself.
//!
//! The request runs off the run loop (it can take ~15 s before a connection reset); its result
//! comes back as a [`Posted`].

use super::*;
use calling::signaling::video::{negotiation_tag, video_modalities};
use calling::video::VideoRole;

/// How long the answer to a re-offer of ours is waited for before the next one goes out anyway
/// (the official client answered a camera re-offer in 7 to 9 seconds, live 2026-11-26).
pub(crate) const ANSWER_WAIT: Duration = Duration::from_secs(30);

/// What our re-offers tell the other side: the call held, our camera sending, our screen shared.
/// A meeting's re-offers carry only the hold (its camera and share go by
/// `UpdateMediaDescriptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Said {
    pub held: bool,
    pub camera: bool,
    pub share: bool,
}

impl Said {
    /// What changed since `from`.
    pub(crate) fn changes(self, from: Said) -> Vec<Reoffer> {
        let mut changes = Vec::new();
        if from.held != self.held {
            changes.push(Reoffer::Hold(self.held));
        }
        if from.camera != self.camera {
            changes.push(Reoffer::Camera(self.camera));
        }
        if from.share != self.share {
            changes.push(Reoffer::Share(self.share));
        }
        changes
    }
}

/// A re-offer of ours whose answer is due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sent {
    /// Which one: its request's result comes back with it ([`Posted`]).
    pub id: u64,
    /// What the other side was told before it.
    pub from: Said,
    /// What it says.
    pub to: Said,
    /// Its `negotiationTag` (none on a hold or resume).
    pub tag: Option<String>,
    /// Our offer as the media session made it (a hold's answer is mirrored from it).
    pub ours: String,
    pub at: Instant,
}

impl Sent {
    pub(crate) fn changes(&self) -> Vec<Reoffer> {
        self.to.changes(self.from)
    }

    /// A hold or resume, or anything said while held: its answer is applied with our own
    /// directions mirrored back (`keep_media_running`) and acknowledged, as a hold's always was.
    pub(crate) fn holds(&self) -> bool {
        self.from.held || self.to.held
    }

    /// What the other side is taken to know once it was refused: the hold as it was, the camera
    /// and the share off where it changed them (turned on: stopped; turned off: it stays off and
    /// is not offered again).
    pub(crate) fn refused_to(&self) -> Said {
        Said {
            held: self.from.held,
            camera: self.from.camera && self.to.camera,
            share: self.from.share && self.to.share,
        }
    }
}

/// A re-offer's request came back: the answer when it came inline (else it comes on
/// `call/mediaAnswer/`), or why it failed.
pub(crate) struct Posted {
    pub id: u64,
    pub result: Result<Option<SdpBlob>, String>,
}

/// What a `call/mediaAnswer` answers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Answered {
    /// Our pending re-offer.
    Pending(Sent),
    /// One their offer superseded: changes nothing.
    Superseded(Sent),
    /// A negotiation whose tag is none of ours: ignored.
    Unknown,
    /// No re-offer of ours is out: the call's own answer (to our join's offer).
    Call,
}

/// A call's re-offers: at most one out ([`Reoffers::pending`]), and what the other side was last
/// told ([`Reoffers::told`]), by our re-offer or our answer to its offer.
#[derive(Debug, Default)]
pub(crate) struct Reoffers {
    pub told: Said,
    pub pending: Option<Sent>,
    /// One their offer (a media restart, or glare) left behind ([`supersede`]): its answer or
    /// rejection changes nothing, but holds the next re-offer back until it comes (or
    /// [`ANSWER_WAIT`]), so it is never taken for that one's.
    pub superseded: Option<Sent>,
    sent: u64,
}

impl Reoffers {
    /// The re-offer to send now: `want` when it differs from what the other side was told, no
    /// re-offer of ours waits for its answer (one waiting [`ANSWER_WAIT`] or longer no longer
    /// holds it back) and the media is not `moving` to a new session.
    pub(crate) fn due(&self, want: Said, moving: bool, now: Instant) -> Option<Said> {
        let waits = |s: &Option<Sent>| {
            s.as_ref()
                .is_some_and(|s| now.saturating_duration_since(s.at) < ANSWER_WAIT)
        };
        let held_back = moving || waits(&self.pending) || waits(&self.superseded);
        (!held_back && want != self.told).then_some(want)
    }

    /// Our re-offer saying `to` goes out: its id.
    pub(crate) fn send(
        &mut self,
        to: Said,
        tag: Option<String>,
        ours: String,
        now: Instant,
    ) -> u64 {
        self.sent += 1;
        let sent = Sent {
            id: self.sent,
            from: self.told,
            to,
            tag,
            ours,
            at: now,
        };
        self.told = to;
        if let Some(lost) = self.pending.replace(sent) {
            tracing::info!(to = ?lost.to, "no answer to our re-offer in time: the next one went out");
        }
        self.sent
    }

    /// A `call/mediaAnswer` naming negotiation `tag` (when it names one): the re-offer it answers.
    /// A tag only rules out a re-offer that has another one; without, the pending one goes first.
    pub(crate) fn answer(&mut self, tag: Option<&str>) -> Answered {
        let answers = |s: &Option<Sent>| {
            s.as_ref().is_some_and(|s| match (tag, s.tag.as_deref()) {
                (Some(theirs), Some(ours)) => theirs == ours,
                _ => true,
            })
        };
        if answers(&self.pending)
            && let Some(s) = self.pending.take()
        {
            return Answered::Pending(s);
        }
        if answers(&self.superseded)
            && let Some(s) = self.superseded.take()
        {
            return Answered::Superseded(s);
        }
        if self.pending.is_some() || self.superseded.is_some() {
            Answered::Unknown
        } else {
            Answered::Call
        }
    }

    /// Their offer met our pending re-offer: theirs wins ([`supersede`]). What our answer to it
    /// says is set in [`Reoffers::told`] by the caller.
    pub(crate) fn supersede(&mut self) {
        supersede(&mut self.pending, &mut self.superseded);
    }

    /// A `call/rejection` while a re-offer of ours is out: which one it refuses
    /// ([`refused_reoffer`]). A refused pending one leaves [`Sent::refused_to`] told.
    pub(crate) fn rejected(&mut self) -> Option<Refused<Sent>> {
        let r = refused_reoffer(&mut self.superseded, &mut self.pending);
        if let Some(Refused::Pending(s)) = &r {
            self.told = s.refused_to();
        }
        r
    }

    /// Re-offer `id`'s request came back with an inline answer, or `failed`: the pending re-offer
    /// when it is still that one (a failed one leaves [`Sent::refused_to`] told). One superseded
    /// meanwhile is done with: nothing to apply or undo.
    pub(crate) fn returned(&mut self, id: u64, failed: bool) -> Option<Sent> {
        if self.superseded.as_ref().is_some_and(|s| s.id == id) {
            self.superseded = None;
            return None;
        }
        let sent = self.pending.take_if(|s| s.id == id)?;
        if failed {
            self.told = sent.refused_to();
        }
        Some(sent)
    }
}

/// The line whose change names a 1:1 re-offer's `negotiationTag` (`;ss_1` for the share, else
/// `;v_2`/`;v_1` for the camera) and whether it sends. `None` for a hold or resume, or anything
/// said while held: sent without a tag, as a hold always was.
pub(crate) fn tag_line(from: Said, to: Said) -> Option<(VideoRole, bool)> {
    if from.held || to.held {
        None
    } else if from.share != to.share {
        Some((VideoRole::ApplicationSharing, to.share))
    } else {
        Some((VideoRole::MainVideo, to.camera))
    }
}

/// What our re-offer (`ours`, the media session's own offer) says on the wire for `to`: in a 1:1
/// call the camera line sends only while the camera is on (`camera_line`: the call has one) and
/// the sharing line only while we share ([`share::direct_offer`]); while held every line is
/// `inactive` ([`hold_offer`]). A meeting's lines stay as the session made them.
pub(crate) fn reoffer_sdp(ours: &str, to: Said, meeting: bool, camera_line: bool) -> String {
    let sdp = if meeting {
        ours.to_owned()
    } else {
        share::direct_offer(ours, to.share, camera_line.then_some(to.camera))
    };
    if to.held { hold_offer(&sdp) } else { sdp }
}

/// The `negotiationTag` a `call/mediaAnswer` names, if any (UNVERIFIED that the service echoes
/// ours; when it does, the answer is matched by it: [`Reoffers::answer`]).
pub(crate) fn answer_tag(body: &serde_json::Value) -> Option<String> {
    [
        "/mediaAnswer/mediaContent/negotiationTag",
        "/mediaAnswer/negotiationTag",
    ]
    .iter()
    .find_map(|p| body.pointer(p)?.as_str())
    .map(str::to_owned)
}

impl Call {
    /// What our re-offers should say now: the hold, and in a 1:1 call the camera and the share.
    fn said_now(&self, held: bool) -> Said {
        Said {
            held,
            camera: !self.meeting && self.camera.on(),
            share: !self.meeting && self.share.sharing(),
        }
    }

    /// Sends the re-offer due now ([`Reoffers::due`]). The run loop asks after every step, so a
    /// hold, camera or share change made while a re-offer waits for its answer goes out with the
    /// others, once. The request runs off the loop and comes back as a [`Posted`]
    /// ([`Call::reoffer_posted`]).
    pub(super) async fn sync_reoffer(
        &mut self,
        audio: &Audio,
        posted: &mpsc::UnboundedSender<Posted>,
        sink: &EventSink,
    ) {
        let now = Instant::now();
        let want = self.said_now(audio.held.load(Ordering::Relaxed));
        let Some(to) = self.reoffers.due(want, self.next.is_some(), now) else {
            return;
        };
        let from = self.reoffers.told;
        let ours = match self.media.create_offer().await {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(%e, "re-offer not made");
                let id = self.reoffers.send(to, None, String::new(), now);
                if let Some(sent) = self.reoffers.returned(id, true) {
                    self.refused(&sent, audio, sink).await;
                }
                return;
            }
        };
        let camera_line = self.media.video_sender(VideoRole::MainVideo).is_some();
        let offer = reoffer_sdp(&ours, to, self.meeting, camera_line);
        let tag = tag_line(from, to).map(|(role, on)| negotiation_tag(&self.s, role, on));
        let mut sending = Vec::new();
        if to.camera {
            sending.push(VideoRole::MainVideo);
        }
        if to.share {
            sending.push(VideoRole::ApplicationSharing);
        }
        let modalities = match tag {
            Some(_) => video_modalities(&sending),
            None => MODALITIES.to_vec(),
        };
        tracing::info!(
            ?from,
            ?to,
            ?tag,
            offered = %calling::media::sdp::directions(&offer),
            transport = %calling::media::sdp::transport_summary(&offer),
            "re-offer sent"
        );
        let id = self.reoffers.send(to, tag.clone(), ours, now);
        let (ngc, s, links, tx) = (
            self.ngc.clone(),
            self.s.clone(),
            self.links.clone(),
            posted.clone(),
        );
        let blob = SdpBlob::ngc(offer);
        tokio::spawn(async move {
            let r = match tag {
                Some(tag) => {
                    ngc.renegotiate_video(&s, &links, &modalities, &blob, &tag)
                        .await
                }
                None => ngc.renegotiate(&s, &links, &modalities, &blob).await,
            };
            let result = r.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Posted { id, result });
        });
    }

    /// A re-offer's request came back: an inline answer is applied, a failure undoes it
    /// ([`Call::refused`]), when it is still the pending one. True when new lines came.
    pub(super) async fn reoffer_posted(
        &mut self,
        p: Posted,
        audio: &Audio,
        sink: &EventSink,
    ) -> bool {
        let answer = match p.result {
            // The answer comes on `call/mediaAnswer/`.
            Ok(None) => return false,
            Ok(Some(a)) => Some(a),
            Err(e) => {
                tracing::warn!(id = p.id, "re-offer not sent: {e}");
                None
            }
        };
        let Some(sent) = self.reoffers.returned(p.id, answer.is_none()) else {
            tracing::info!(id = p.id, "a re-offer no longer pending came back: ignored");
            return false;
        };
        match answer {
            Some(a) => self.apply_reoffer(&sent, &a, &Links::default(), sink).await,
            None => {
                self.refused(&sent, audio, sink).await;
                false
            }
        }
    }

    /// Applies the answer to our re-offer `sent`: a hold's (or one said while held) with our own
    /// directions mirrored back, and acknowledged; any other as the media answer it is. True
    /// when new lines came (the subscriptions follow).
    pub(super) async fn apply_reoffer(
        &mut self,
        sent: &Sent,
        answer: &SdpBlob,
        links: &Links,
        sink: &EventSink,
    ) -> bool {
        if sent.holds() {
            self.apply_reoffer_answer(answer, &sent.ours, links, sink)
                .await;
            return false;
        }
        // `apply_answer` takes one answer per offer: this one is our re-offer's.
        self.answered = false;
        let applied = self.apply_answer(answer).await;
        self.answered = true;
        applied
    }
}
