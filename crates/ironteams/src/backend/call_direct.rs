//! 1:1 calls (plans/calls.md K1.5, K2.1, K3.2): the incoming-call toast and the outgoing ringing
//! screen, up to the moment the call connects. Then the call manager (`super::call`, which also
//! owns the calling trouter) adopts the call and shows the in-call window.
//!
//! Incoming: the manager hands us each `callInvitation`. The toast rings (a generated tone through
//! cpal) and, when the window is not in front, the desktop's banner replaces the toast (one at a
//! time, swapped as the window's focus changes: `sync_ring_banner`). Accept builds a
//! media session and runs `calling::incoming::answer_invitation`: attach first when the
//! invitation is forked (asking for the caller's offer when the ring has none, as seen live
//! 2026-11-19), then ringing, then accept with our answer; or, when no offer comes at all, accept
//! with our own offer and apply the caller's answer from our `mediaAnswer` callback. Decline posts
//! to the invitation's rejection link (or to the leg `attach` hands back). The caller cancelling
//! closes the toast.
//!
//! Call waiting: an invitation while a call runs rings too, with a short beep instead of the
//! ringtone, and the toast offers Hold and accept, End and accept, and Decline. The call in
//! progress is held (or left) once the new one is answered ([`CallManager::adopt`]); the manager
//! keeps both, the held one on the call window's "On hold" bar. A third call, while one is on
//! hold, is ignored (left to ring on the other clients).
//!
//! Outgoing: Call in a 1:1 chat's header (or on a profile card) places an audio call with
//! `place_call`; the ringing screen follows the callbacks of our session until the far end
//! accepts (adopt), declines, does not answer, or we cancel.
//!
//! The signalling is ported from rusty-teams and mautrix-teams and is UNVERIFIED live. In demo
//! mode and in tests (the engine is offline) a fake backend stands in: nothing leaves the process,
//! and the ringtone and notifications stay silent (`crate::external::suppressed`).

use super::Engine;
use super::call::{self, CallKind, CallManager, CurrentCall};
use super::ringtone::RingtoneChoice;
use crate::data::{Pixels, color_for, initials};
use anyhow::Context;
use calling::incoming::NoOffer;
use calling::invitation::{CallEvent, NgcPush, classify};
use calling::media::MediaSession;
use calling::ngc_trouter::CallPush;
use calling::signaling::bodies::reject_request;
use calling::signaling::{CallSession, NgcClient};
use calling::{CallInvitation, Modality, Participant, SdpBlob};
use serde_json::Value;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// An outgoing call that is not answered within this is cancelled ("No answer").
pub const OUTGOING_RING: Duration = Duration::from_secs(60);
/// An incoming call still ringing after this is treated as missed (the caller's cancel normally
/// comes first).
pub const INCOMING_RING: Duration = Duration::from_secs(60);
/// How long an ended outgoing call's screen ("Declined", "No answer") stays up.
const OVER_SCREEN: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------------------------
// The flows as pure state machines.

/// An incoming call, from the toast's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingState {
    Ringing,
    /// Accept was pressed; the answer is being built and sent.
    Accepting,
    /// Answered and handed to the call manager.
    Connected,
    Declined,
    CallerCancelled,
    /// Rang out without an answer.
    Missed,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingEvent {
    Accept,
    Decline,
    CallerCancelled,
    RingTimeout,
    Connected,
    Failed(String),
}

impl IncomingState {
    pub fn on(&self, ev: IncomingEvent) -> IncomingState {
        use IncomingEvent as E;
        use IncomingState as S;
        match (self, ev) {
            (S::Ringing, E::Accept) => S::Accepting,
            (S::Ringing, E::Decline) => S::Declined,
            (S::Ringing, E::RingTimeout) => S::Missed,
            (S::Ringing | S::Accepting, E::CallerCancelled) => S::CallerCancelled,
            (S::Accepting, E::Connected) => S::Connected,
            (S::Ringing | S::Accepting, E::Failed(e)) => S::Failed(e),
            (s, _) => s.clone(),
        }
    }

    /// The toast is gone for good.
    pub fn is_over(&self) -> bool {
        !matches!(self, IncomingState::Ringing | IncomingState::Accepting)
    }
}

/// An outgoing call, from the ringing screen's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutgoingState {
    /// Placing the call (building media, posting to the conversation service).
    Placing,
    Ringing,
    /// Answered and handed to the call manager.
    Connected,
    Cancelled,
    Rejected,
    NoAnswer,
    Ended,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutgoingEvent {
    Ringing,
    Accepted,
    Rejected,
    Cancel,
    RingTimeout,
    Ended,
    Failed(String),
}

impl OutgoingState {
    pub fn on(&self, ev: OutgoingEvent) -> OutgoingState {
        use OutgoingEvent as E;
        use OutgoingState as S;
        match (self, ev) {
            (S::Placing | S::Ringing, E::Ringing) => S::Ringing,
            (S::Placing | S::Ringing, E::Accepted) => S::Connected,
            (S::Placing | S::Ringing, E::Rejected) => S::Rejected,
            (S::Placing | S::Ringing, E::Cancel) => S::Cancelled,
            (S::Placing | S::Ringing, E::RingTimeout) => S::NoAnswer,
            (S::Placing | S::Ringing | S::Connected, E::Ended) => S::Ended,
            (S::Placing | S::Ringing, E::Failed(e)) => S::Failed(e),
            (s, _) => s.clone(),
        }
    }

    /// Still ringing (or about to).
    pub fn is_live(&self) -> bool {
        matches!(self, OutgoingState::Placing | OutgoingState::Ringing)
    }

    /// The line under the callee's name.
    pub fn label(&self) -> String {
        match self {
            OutgoingState::Placing => "Calling…".into(),
            OutgoingState::Ringing => "Ringing…".into(),
            OutgoingState::Connected => "Connected".into(),
            OutgoingState::Cancelled => "Cancelled".into(),
            OutgoingState::Rejected => "Declined".into(),
            OutgoingState::NoAnswer => "No answer".into(),
            OutgoingState::Ended => "Call ended".into(),
            OutgoingState::Failed(e) => format!("Call failed: {e}"),
        }
    }
}

/// SIP-style codes a `call/end` may carry when the callee refused: decline and busy.
const REFUSED_CODES: [i64; 2] = [603, 486];

/// What a callback of our outgoing call means for the ringing screen.
pub fn outgoing_event(ev: &CallEvent) -> Option<OutgoingEvent> {
    match ev {
        CallEvent::Progress { .. } => Some(OutgoingEvent::Ringing),
        CallEvent::Accepted { .. } => Some(OutgoingEvent::Accepted),
        CallEvent::Rejected(_) => Some(OutgoingEvent::Rejected),
        CallEvent::Ended(r) if REFUSED_CODES.contains(&r.code) => Some(OutgoingEvent::Rejected),
        CallEvent::Ended(_) => Some(OutgoingEvent::Ended),
        _ => None,
    }
}

/// Keys of a body that ends or cancels a call.
const END_KEYS: [&str; 6] = [
    "callEnd",
    "conversationEnd",
    "callTransactionEnd",
    "callCancellation",
    "cancellation",
    "cancelation",
];

fn has_key(v: &Value, keys: &[&str]) -> bool {
    match v {
        Value::Object(m) => m
            .iter()
            .any(|(k, v)| keys.contains(&k.as_str()) || has_key(v, keys)),
        Value::Array(a) => a.iter().any(|v| has_key(v, keys)),
        _ => false,
    }
}

/// True when a calling push tells a ringing invitation (`call_id`) that the caller gave up: an
/// end/cancel body that names the call. The exact shape is UNVERIFIED (no capture yet), so this
/// reads any push that carries an end reason and the call id; `ironteams-cli call listen --ring-only
/// --dump DIR` records the real one.
pub fn cancels_invitation(call_id: &str, url: &str, body: &Value) -> bool {
    if call_id.is_empty() {
        return false;
    }
    if let NgcPush::Invitation(_) = classify(url, body) {
        return false;
    }
    has_key(body, &END_KEYS) && body.to_string().contains(call_id)
}

/// A 1:1 chat id (`19:<a>_<b>@unq.gbl.spaces`): its header offers Call and Video.
pub fn is_direct_chat(id: &str) -> bool {
    super::util::one_on_one_members(id).len() == 2
}

// ---------------------------------------------------------------------------------------------
// Commands from the UI.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallCmd {
    /// Accept the ringing call; `video` when "Accept with video" was pressed (the camera starts
    /// once the call runs, unless `IRONTEAMS_CAMERA=0`).
    Accept {
        video: bool,
    },
    /// Call waiting: "Hold and accept" (`CurrentCall::Hold`) or "End and accept".
    AcceptWaiting(CurrentCall),
    Decline,
    /// Call the other person of this 1:1 chat.
    DialChat(String),
    /// The chat header's "Share screen": call the other person of this 1:1 chat and share once
    /// connected (the screen and window picker opens).
    ShareScreen(String),
    /// Call this MRI (profile card).
    DialPerson(String),
    /// Cancel the outgoing call while it rings.
    Cancel,
    /// Close the screen of an outgoing call that is over.
    Close,
}

impl CallCmd {
    pub fn parse(name: &str, arg: &str) -> Option<Self> {
        Some(match name {
            "accept-video" => CallCmd::Accept { video: true },
            "accept-audio" => CallCmd::Accept { video: false },
            "hold-accept" => CallCmd::AcceptWaiting(CurrentCall::Hold),
            "end-accept" => CallCmd::AcceptWaiting(CurrentCall::End),
            "decline" => CallCmd::Decline,
            "dial" if !arg.is_empty() => CallCmd::DialChat(arg.to_owned()),
            "share-screen" if !arg.is_empty() => CallCmd::ShareScreen(arg.to_owned()),
            "dial-person" if !arg.is_empty() => CallCmd::DialPerson(arg.to_owned()),
            "cancel" => CallCmd::Cancel,
            "close" => CallCmd::Close,
            _ => return None,
        })
    }
}

/// What the toast or the ringing screen shows.
#[derive(Clone, Default)]
pub struct CallView {
    pub open: bool,
    pub name: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    pub video: bool,
    pub status: String,
    pub over: bool,
    /// The call rings while another runs: the toast offers Hold and accept and End and accept.
    pub waiting: bool,
}

// ---------------------------------------------------------------------------------------------
// Engine state.

struct IncomingCall {
    id: u64,
    call_id: String,
    mri: String,
    name: String,
    video: bool,
    /// It rang while another call ran (call waiting).
    waiting: bool,
    state: IncomingState,
    /// `None` for the fake (demo and test) invitation.
    inv: Option<Box<CallInvitation>>,
    /// Our session (endpoint) id once Accept started: its callbacks go to `events`.
    session: Option<String>,
    events: Option<mpsc::UnboundedSender<CallEvent>>,
    /// The desktop's banner rings it, not the toast in the window ([`Engine::sync_ring_banner`]).
    global: bool,
    /// Its card on the desktop (`desktop_cards`), taken away when it stops ringing or the window
    /// takes the ring over.
    card: Option<u32>,
    /// Its system notification: dropping this (or sending on it) takes the notification away.
    system: Option<std::sync::mpsc::Sender<()>>,
}

struct OutgoingCall {
    id: u64,
    mri: String,
    name: String,
    state: OutgoingState,
    /// Our session (endpoint) id: callbacks under it belong to this call.
    session: Option<String>,
    events: Option<mpsc::UnboundedSender<CallEvent>>,
    cancel: Option<oneshot::Sender<()>>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    incoming: Option<IncomingCall>,
    outgoing: Option<OutgoingCall>,
    ringtone: Option<Ringtone>,
    /// The soft tone pair our own call plays while it rings.
    ringback: Option<super::sounds::Loop>,
    /// What the fake backend was asked to do, oldest first.
    log: Vec<String>,
}

/// The 1:1 call flows of one engine.
#[derive(Default)]
pub struct Calls {
    inner: Mutex<Inner>,
}

/// The banner's words for a failed answer: plain text for what the service refused (the raw
/// answer stays in the log), else the error as it reads.
fn answer_failure_text(err: &anyhow::Error) -> String {
    err.chain()
        .find_map(|e| e.downcast_ref::<calling::Error>())
        .map_or_else(|| err.to_string(), calling::incoming::answer_error_text)
}

/// Runs `f` in the background, except in tests (they drive every step themselves) and outside a
/// runtime.
fn later(f: impl Future<Output = ()> + Send + 'static) {
    if cfg!(test) {
        return;
    }
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(f);
    }
}

/// The desktop's notification for a call, shown on a blocking thread. On Linux, `stop` takes it
/// away again: a message on it, or its sender dropped, closes the notification (it closes by
/// itself after 30 s otherwise).
fn system_call_notification(
    summary: String,
    body: String,
    stop: Option<std::sync::mpsc::Receiver<()>>,
) {
    tokio::task::spawn_blocking(move || {
        let shown = notify_rust::Notification::new()
            .appname("IronTeams")
            .summary(&summary)
            .body(&body)
            .icon("call-start")
            .timeout(notify_rust::Timeout::Milliseconds(30_000))
            .show();
        #[cfg(target_os = "linux")]
        if let (Ok(handle), Some(stop)) = (shown, stop)
            && !matches!(
                stop.recv_timeout(Duration::from_secs(30)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            )
        {
            handle.close();
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (shown, stop);
    });
}

/// What Accept does with a ring that carries no offer: `IRONTEAMS_CALL_NO_OFFER=fetch|new-offer|
/// accept-offer|attach-offer` (see `calling::incoming::NoOffer`), `fetch` by default. For trying
/// the unverified paths live without a rebuild.
fn no_offer_mode() -> NoOffer {
    let mode = compat::var("IRONTEAMS_CALL_NO_OFFER").ok();
    match mode.as_deref().map(NoOffer::parse) {
        Some(Some(m)) => m,
        Some(None) => {
            tracing::warn!("IRONTEAMS_CALL_NO_OFFER: unknown mode; using fetch");
            NoOffer::default()
        }
        None => NoOffer::default(),
    }
}

fn manager() -> anyhow::Result<Arc<CallManager>> {
    call::manager().context("calling is not ready yet")
}

/// Declines an invitation: on its own `reject`/`rejection` link, else on the leg `attach` hands
/// back (as `ironteams-cli call listen --decline` does). UNVERIFIED live.
async fn decline_invitation(
    ngc: &NgcClient,
    s: &CallSession,
    inv: &CallInvitation,
) -> calling::Result<()> {
    if reject_request(&inv.links).is_some() {
        return ngc.reject(s, &inv.links).await;
    }
    let links = ngc.attach(s, inv).await?;
    ngc.reject(s, &links).await
}

/// Ends a call we placed: its `leave` or `callLeg` link, else the conversation controller.
async fn hang_up_placed(
    ngc: &NgcClient,
    s: &CallSession,
    links: &calling::Links,
    controller: &str,
) {
    let r = if links.get("leave").is_some() || links.get("callLeg").is_some() {
        ngc.hang_up(s, links).await
    } else {
        ngc.leave_controller(s, controller).await
    };
    if let Err(err) = r {
        tracing::warn!(%err, "cancelling the call failed");
    }
}

impl Engine {
    fn direct(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.direct_calls
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn person_name(&self, mri: &str) -> String {
        self.store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| mri.to_owned())
    }

    fn call_view(self: &Arc<Self>, mri: &str, name: &str) -> CallView {
        CallView {
            open: true,
            name: name.to_owned(),
            initials: initials(name),
            color: color_for(mri),
            avatar: self.avatar(mri),
            ..CallView::default()
        }
    }

    /// Listens for incoming calls and ringing-phase callbacks once the call manager runs.
    pub(super) fn start_calls(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            let m = loop {
                if let Some(m) = call::manager() {
                    break m;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            };
            let mut invitations = m.subscribe_invitations();
            let mut unmatched = m.subscribe_unmatched();
            loop {
                tokio::select! {
                    Some(inv) = invitations.recv() => { e.on_invitation(inv); }
                    Some(push) = unmatched.recv() => e.on_unmatched(&push),
                    else => break,
                }
            }
        });
    }

    /// Demo mode: `IRONTEAMS_DEMO_CALL=1` rings a fake incoming call a few seconds after start.
    pub(super) fn start_calls_demo(self: &Arc<Self>) {
        if compat::var_os("IRONTEAMS_DEMO_CALL").is_none() {
            return;
        }
        let e = self.clone();
        later(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            // The other person of the demo's first 1:1 chat.
            let mri = "8:orgid:00000000-0000-0000-0000-000000000002".to_owned();
            let name = e.person_name(&mri);
            e.ring_incoming(mri, name, false, "demo-call".into(), None);
        });
    }

    /// Runs in order with the other call commands (`in_order`): nothing here waits on the network.
    pub(super) fn direct_call_cmd(self: &Arc<Self>, c: CallCmd) -> anyhow::Result<()> {
        match c {
            CallCmd::Accept { video } => self.accept_incoming(video, None),
            CallCmd::AcceptWaiting(current) => self.accept_incoming(false, Some(current)),
            CallCmd::Decline => self.decline_incoming(),
            CallCmd::DialChat(chat) => self.dial_chat(&chat, false),
            CallCmd::ShareScreen(chat) => self.dial_chat(&chat, true),
            CallCmd::DialPerson(mri) => {
                if mri == self.me {
                    anyhow::bail!("You can't call yourself");
                }
                let name = self.person_name(&mri);
                self.dial(mri, name, false)
            }
            CallCmd::Cancel => {
                self.cancel_outgoing();
                Ok(())
            }
            CallCmd::Close => {
                let mut c = self.direct();
                if c.outgoing.as_ref().is_some_and(|o| !o.state.is_live()) {
                    c.outgoing = None;
                    drop(c);
                    self.ui.call_outgoing(CallView::default());
                }
                Ok(())
            }
        }
    }

    // --- incoming ------------------------------------------------------------------------------

    /// A `callInvitation` from the manager. Ignored (left to ring on the other clients) while
    /// another call rings or connects here, or while two calls already run (one on hold).
    pub(crate) fn on_invitation(self: &Arc<Self>, inv: Box<CallInvitation>) -> bool {
        let mri = inv.from.mri.clone();
        let name = inv
            .from
            .display_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.person_name(&mri));
        let video = inv.modalities.contains(&Modality::Video);
        let call_id = inv.call_id.clone();
        self.ring_incoming(mri, name, video, call_id, Some(inv))
    }

    fn ring_incoming(
        self: &Arc<Self>,
        mri: String,
        name: String,
        video: bool,
        call_id: String,
        inv: Option<Box<CallInvitation>>,
    ) -> bool {
        // Call waiting: a call runs in the call window. Two already (one on hold) is the limit.
        let waiting = self.calls.in_call();
        let full = waiting && self.calls.has_held();
        let mut c = self.direct();
        let busy = c.incoming.as_ref().is_some_and(|i| !i.state.is_over())
            || c.outgoing.as_ref().is_some_and(|o| o.state.is_live())
            || full;
        if busy {
            tracing::info!("incoming call ignored: another call is in progress");
            return false;
        }
        c.next_id += 1;
        let id = c.next_id;
        c.incoming = Some(IncomingCall {
            id,
            call_id,
            mri: mri.clone(),
            name: name.clone(),
            video,
            waiting,
            state: IncomingState::Ringing,
            inv,
            session: None,
            events: None,
            global: false,
            card: None,
            system: None,
        });
        // Over a call, a short beep rather than the full ringtone.
        c.ringtone = if waiting {
            Ringtone::waiting()
        } else {
            let prefs = super::settings::calls_prefs();
            Ringtone::start(prefs.ringtone, prefs.ringtone_file.as_deref())
        };
        drop(c);
        let mut view = self.call_view(&mri, &name);
        view.video = video;
        view.waiting = waiting;
        // One banner rings it: the toast here, or the desktop's while the window is away.
        self.ui.call_incoming(view);
        self.sync_ring_banner();
        let e = self.clone();
        later(async move {
            // The photo may arrive after the toast opened.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            e.refresh_incoming(id);
            tokio::time::sleep(INCOMING_RING).await;
            e.incoming_event(id, IncomingEvent::RingTimeout);
        });
        true
    }

    fn refresh_incoming(self: &Arc<Self>, id: u64) {
        let c = self.direct();
        let Some(i) = c
            .incoming
            .as_ref()
            .filter(|i| i.id == id && !i.state.is_over())
        else {
            return;
        };
        let mut view = self.call_view(&i.mri, &i.name);
        view.video = i.video;
        view.waiting = i.waiting;
        if i.state == IncomingState::Accepting {
            view.status = "Connecting…".into();
        }
        drop(c);
        self.ui.call_incoming(view);
    }

    /// Moves the incoming call `id` on; closes the toast and stops the ringtone once it is over.
    fn incoming_event(self: &Arc<Self>, id: u64, ev: IncomingEvent) -> Option<IncomingState> {
        let mut c = self.direct();
        let call = c.incoming.as_mut().filter(|i| i.id == id)?;
        let before = call.state.clone();
        call.state = call.state.on(ev);
        let (state, name, video) = (call.state.clone(), call.name.clone(), call.video);
        if state == before {
            return Some(state);
        }
        // Answered, declined or over: its desktop card and notification go.
        let (card, system) = if state != IncomingState::Ringing {
            call.global = false;
            (call.card.take(), call.system.take())
        } else {
            (None, None)
        };
        if state != IncomingState::Ringing {
            c.ringtone = None;
        }
        if state.is_over() {
            c.incoming = None;
        }
        drop(c);
        if let Some(card) = card {
            crate::desktop_cards::client::dismiss(card);
        }
        drop(system);
        match &state {
            IncomingState::Accepting => {
                // "Connecting…" shows in the window, whichever banner rang.
                self.ui.call_incoming_shown(true);
                self.refresh_incoming(id)
            }
            IncomingState::Connected => {
                self.ui.call_incoming(CallView::default());
                // Only with the camera switched off (`IRONTEAMS_CAMERA=0`).
                if video && !call::camera::wanted() {
                    self.ui
                        .banner("Video isn't available yet: the call is audio only.".into());
                }
            }
            IncomingState::CallerCancelled | IncomingState::Missed => {
                self.ui.call_incoming(CallView::default());
                // The service writes the missed call into the call log (`48:calllogs`), which
                // IronTeams only reads (`calling::history`); IronTeams adds no entry of its own.
                self.ui.banner(format!("Missed call from {name}"));
                self.notify_missed(&name);
            }
            IncomingState::Failed(e) => {
                self.ui.call_incoming(CallView::default());
                self.ui.banner(format!("Couldn't answer the call: {e}"));
            }
            IncomingState::Declined | IncomingState::Ringing => {
                self.ui.call_incoming(CallView::default());
            }
        }
        Some(state)
    }

    /// Accept. `current`: what happens to the call in progress, if one runs (call waiting); a
    /// plain Accept over a call holds it, as Teams does.
    fn accept_incoming(
        self: &Arc<Self>,
        video: bool,
        current: Option<CurrentCall>,
    ) -> anyhow::Result<()> {
        call::camera::accept_with_video(video);
        let current = self
            .calls
            .in_call()
            .then(|| current.unwrap_or(CurrentCall::Hold));
        let (id, inv, name, mri) = {
            let c = self.direct();
            let Some(i) = c
                .incoming
                .as_ref()
                .filter(|i| i.state == IncomingState::Ringing)
            else {
                return Ok(());
            };
            (i.id, i.inv.clone(), i.name.clone(), i.mri.clone())
        };
        self.incoming_event(id, IncomingEvent::Accept);
        let Some(inv) = inv else {
            // Fake invitation (demo and tests).
            self.direct().log.push(format!(
                "{}accept audio{}",
                match current {
                    Some(CurrentCall::Hold) => "hold and ",
                    Some(CurrentCall::End) => "end and ",
                    None => "",
                },
                if video { " (video asked)" } else { "" }
            ));
            self.incoming_event(id, IncomingEvent::Connected);
            // Over a call, the fake call joins the call window so both can be seen.
            if current.is_some() {
                let chat_id = super::util::one_on_one_chat_id(&self.me, &mri).unwrap_or_default();
                self.calls.adopt_without_engine(
                    CallKind::Direct {
                        peer_name: name,
                        chat_id,
                    },
                    current,
                );
                return Ok(());
            }
            if self.offline.load(Ordering::Relaxed) && !cfg!(test) {
                self.ui.banner(format!(
                    "Demo: call with {name} accepted. Calls need a signed-in account."
                ));
            }
            return Ok(());
        };
        let e = self.clone();
        later(async move {
            let r = e.accept_live(id, &inv, name, current).await;
            if let Err(err) = r {
                tracing::warn!(%err, "answering a call failed");
                e.incoming_event(id, IncomingEvent::Failed(answer_failure_text(&err)));
            }
        });
        Ok(())
    }

    /// Answers `inv` with audio and hands the call to the manager. UNVERIFIED live.
    async fn accept_live(
        self: &Arc<Self>,
        id: u64,
        inv: &CallInvitation,
        name: String,
        current: Option<CurrentCall>,
    ) -> anyhow::Result<()> {
        let m = manager()?;
        let ngc = m.ngc().await?;
        let surl = m.surl().context("the calling connection is down")?;
        let mut s = CallSession::for_invitation(&surl, &self.me, &self.my_name, inv);
        // Callbacks under our session (the caller's media answer) come to this flow.
        let (tx, mut events) = mpsc::unbounded_channel();
        if let Some(i) = self.direct().incoming.as_mut().filter(|i| i.id == id) {
            i.session = Some(s.callbacks.session_id().to_owned());
            i.events = Some(tx);
        }
        let ice =
            match calling::relay::resolve_ice_servers(&self.client, &ngc.endpoints().region, None)
                .await
            {
                Ok((ice, _)) => ice,
                Err(err) => {
                    tracing::warn!(%err, "no TURN relay; host candidates only");
                    vec![]
                }
            };
        let (media, media_events) =
            MediaSession::with_options(ice, call::share::media_options()).await?;
        let observe = |name: &str, text: &str| call::live::dump_answer(name, text);
        let links = match calling::incoming::answer_invitation(
            &ngc,
            &mut s,
            inv,
            &media,
            &mut events,
            no_offer_mode(),
            &observe,
        )
        .await
        {
            Ok(l) => l,
            Err(e) => {
                let _ = media.close().await;
                return Err(e.into());
            }
        };
        // The caller may have given up while we answered.
        if self.incoming_event(id, IncomingEvent::Connected) != Some(IncomingState::Connected) {
            let _ = ngc.hang_up(&s, &links).await;
            let _ = media.close().await;
            return Ok(());
        }
        // The ring names the 1:1 chat; else it follows from both of us.
        let chat_id = inv.raw["groupChat"]["threadId"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| super::util::one_on_one_chat_id(&self.me, &inv.from.mri))
            .unwrap_or_default();
        m.adopt(
            s,
            links,
            media,
            media_events,
            CallKind::Direct {
                peer_name: name,
                chat_id,
            },
            current,
        )
    }

    fn decline_incoming(self: &Arc<Self>) -> anyhow::Result<()> {
        let (id, inv) = {
            let c = self.direct();
            let Some(i) = c
                .incoming
                .as_ref()
                .filter(|i| i.state == IncomingState::Ringing)
            else {
                return Ok(());
            };
            (i.id, i.inv.clone())
        };
        self.incoming_event(id, IncomingEvent::Decline);
        let Some(inv) = inv else {
            self.direct().log.push("decline".into());
            return Ok(());
        };
        let e = self.clone();
        later(async move {
            let r = async {
                let m = manager()?;
                let ngc = m.ngc().await?;
                let surl = m.surl().context("the calling connection is down")?;
                let s = CallSession::for_invitation(&surl, &e.me, &e.my_name, &inv);
                decline_invitation(&ngc, &s, &inv).await?;
                anyhow::Ok(())
            }
            .await;
            if let Err(err) = r {
                tracing::warn!(%err, "declining a call failed");
            }
        });
        Ok(())
    }

    /// A calling push no adopted call claimed: our outgoing call's callbacks, or the caller
    /// cancelling a ringing invitation.
    pub(crate) fn on_unmatched(self: &Arc<Self>, push: &CallPush) {
        if let NgcPush::Call { session_id, event } = classify(&push.url, &push.body) {
            let c = self.direct();
            // The call we are answering: it waits for the caller's media answer. An end still
            // falls through to the cancel check below.
            if let Some(i) = c
                .incoming
                .as_ref()
                .filter(|i| i.session.as_deref() == Some(session_id.as_str()))
                && let Some(tx) = &i.events
            {
                let _ = tx.send(event.clone());
            }
            if let Some(o) = c
                .outgoing
                .as_ref()
                .filter(|o| o.session.as_deref() == Some(session_id.as_str()))
                && let Some(tx) = &o.events
            {
                let _ = tx.send(event);
                return;
            }
        }
        let cancelled = {
            let c = self.direct();
            c.incoming
                .as_ref()
                .filter(|i| cancels_invitation(&i.call_id, &push.url, &push.body))
                .map(|i| i.id)
        };
        if let Some(id) = cancelled {
            self.incoming_event(id, IncomingEvent::CallerCancelled);
        }
    }

    /// Rings the incoming call on one banner only: the toast in the window while it is in sight
    /// and focused, else the desktop's (a card with Accept and Decline, or the system
    /// notification). Called when the call starts ringing and at every focus or visibility change
    /// of the window while it rings: the banner that is not wanted is taken away and the other
    /// put up. The ringtone and the call's state are not touched, so a swap loses nothing.
    pub(super) fn sync_ring_banner(self: &Arc<Self>) {
        let global_ok =
            !crate::external::suppressed() && !crate::tray_wiring::notifications_muted();
        let which = super::banner::ring_banner(
            self.is_visible(),
            self.focused.load(Ordering::Relaxed),
            global_ok,
        );
        let want = which == super::banner::RingBanner::Global;
        let mut c = self.direct();
        let Some(i) = c
            .incoming
            .as_mut()
            .filter(|i| i.state == IncomingState::Ringing)
        else {
            return;
        };
        let (id, mri, name, video) = (i.id, i.mri.clone(), i.name.clone(), i.video);
        let was = std::mem::replace(&mut i.global, want);
        let gone = (was && !want).then(|| (i.card.take(), i.system.take()));
        drop(c);
        self.ui.call_incoming_shown(!want);
        if let Some((card, system)) = gone {
            if let Some(card) = card {
                crate::desktop_cards::client::dismiss(card);
            }
            drop(system);
        }
        if want && !was {
            let what = if video {
                "Incoming video call"
            } else {
                "Incoming call"
            };
            self.ring_on_desktop(id, &mri, &name, what);
        }
    }

    /// The desktop's banner for the ringing call `id`: a call card when the cards are on
    /// (`cards.rs`), else the system notification.
    fn ring_on_desktop(self: &Arc<Self>, id: u64, mri: &str, name: &str, what: &str) {
        let e = self.clone();
        let (n, w) = (name.to_owned(), what.to_owned());
        let Some(card) = self.call_card(mri, name, what, move || e.ring_system(id, &n, &w)) else {
            return self.ring_system(id, name, what);
        };
        // The window may have taken the ring back meanwhile: then this card is not wanted.
        let wanted = match self.direct().incoming.as_mut() {
            Some(i) if i.id == id && i.global => {
                i.card = Some(card);
                true
            }
            _ => false,
        };
        if !wanted {
            crate::desktop_cards::client::dismiss(card);
        }
    }

    /// The system notification of the ringing call `id`, while it still rings on the desktop.
    /// It is taken away again when the window takes the ring over or the call stops ringing.
    fn ring_system(self: &Arc<Self>, id: u64, name: &str, what: &str) {
        let (stop, listen) = std::sync::mpsc::channel();
        match self.direct().incoming.as_mut() {
            Some(i) if i.id == id && i.global && i.state == IncomingState::Ringing => {
                i.system = Some(stop);
            }
            _ => return,
        }
        system_call_notification(name.to_owned(), what.to_owned(), Some(listen));
    }

    /// "Missed call" as a desktop card, or the system notification.
    fn notify_missed(self: &Arc<Self>, name: &str) {
        if crate::external::suppressed()
            || self.focused.load(Ordering::Relaxed)
            || crate::tray_wiring::notifications_muted()
        {
            return;
        }
        let e = self.clone();
        let n = name.to_owned();
        if !self.missed_call_card(name, move || e.notify_call(&n, "Missed call")) {
            self.notify_call(name, "Missed call");
        }
    }

    fn notify_call(&self, name: &str, what: &str) {
        if crate::external::suppressed()
            || self.focused.load(Ordering::Relaxed)
            || crate::tray_wiring::notifications_muted()
        {
            return;
        }
        system_call_notification(name.to_owned(), what.to_owned(), None);
    }

    // --- outgoing ------------------------------------------------------------------------------

    /// Calls the other person of the 1:1 chat `chat`; `share`: share the screen once connected.
    fn dial_chat(self: &Arc<Self>, chat: &str, share: bool) -> anyhow::Result<()> {
        let peer = super::util::one_on_one_members(chat)
            .into_iter()
            .find(|m| *m != self.me)
            .context("not a 1:1 chat")?;
        let title = self
            .store
            .lock()
            .unwrap()
            .conversation(chat)
            .ok()
            .flatten()
            .map(|c| c.title)
            .filter(|t| !t.is_empty());
        let name = title.unwrap_or_else(|| self.person_name(&peer));
        self.dial(peer, name, share)
    }

    fn dial(self: &Arc<Self>, mri: String, name: String, share: bool) -> anyhow::Result<()> {
        let mut c = self.direct();
        let busy = c.incoming.as_ref().is_some_and(|i| !i.state.is_over())
            || c.outgoing.as_ref().is_some_and(|o| o.state.is_live());
        if busy {
            anyhow::bail!("Another call is in progress");
        }
        c.next_id += 1;
        let id = c.next_id;
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        c.outgoing = Some(OutgoingCall {
            id,
            mri: mri.clone(),
            name: name.clone(),
            state: OutgoingState::Placing,
            session: None,
            events: Some(ev_tx),
            cancel: Some(cancel_tx),
        });
        drop(c);
        self.push_outgoing(id);
        if self.offline.load(Ordering::Relaxed) {
            self.direct().log.push(format!("dial {mri}"));
            if share {
                self.direct().log.push("share when connected".into());
            }
            let e = self.clone();
            // Demo: rings, then connects (there is no in-call window without an account).
            later(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                e.outgoing_event(id, OutgoingEvent::Ringing);
                tokio::time::sleep(Duration::from_secs(4)).await;
                if e.outgoing_event(id, OutgoingEvent::Accepted) == Some(OutgoingState::Connected) {
                    e.ui.banner(format!(
                        "Demo: {name} answered. Calls need a signed-in account."
                    ));
                }
            });
            return Ok(());
        }
        let e = self.clone();
        later(async move {
            if let Err(err) = e.dial_live(id, mri, name, share, cancel_rx, ev_rx).await {
                tracing::warn!(%err, "placing a call failed");
                e.outgoing_event(id, OutgoingEvent::Failed(err.to_string()));
            }
        });
        Ok(())
    }

    /// Places the call and follows it until it is answered (adopt), refused or cancelled.
    /// UNVERIFIED live.
    async fn dial_live(
        self: &Arc<Self>,
        id: u64,
        mri: String,
        name: String,
        share: bool,
        mut cancel: oneshot::Receiver<()>,
        mut events: mpsc::UnboundedReceiver<CallEvent>,
    ) -> anyhow::Result<()> {
        let m = manager()?;
        let ngc = m.ngc().await?;
        let surl = m.surl().context("the calling connection is down")?;
        let ice =
            match calling::relay::resolve_ice_servers(&self.client, &ngc.endpoints().region, None)
                .await
            {
                Ok((ice, _)) => ice,
                Err(err) => {
                    tracing::warn!(%err, "no TURN relay; host candidates only");
                    vec![]
                }
            };
        let (media, media_events) =
            MediaSession::with_options(ice, call::share::media_options()).await?;
        // The camera is off and nothing is shared yet: the camera line receives only and the
        // sharing line is inactive, as the web client offers them (webrtc-rs says both send).
        let camera = media
            .video_sender(calling::video::VideoRole::MainVideo)
            .is_some()
            .then_some(false);
        let offer = call::share::direct_offer(&media.create_offer().await?, false, camera);
        tracing::info!(ours = %calling::media::sdp::directions(&offer), "1:1 call offered");
        let s = CallSession::new(&surl, &self.me, &self.my_name);
        // Callbacks under our session come to this call from now on.
        if let Some(o) = self.direct().outgoing.as_mut().filter(|o| o.id == id) {
            o.session = Some(s.callbacks.session_id().to_owned());
        }
        let chat_id = super::util::one_on_one_chat_id(&self.me, &mri).unwrap_or_default();
        let callee = Participant {
            mri,
            display_name: Some(name.clone()),
            ..Participant::default()
        };
        let conv = ngc
            .place_call(
                &s,
                &[callee],
                None,
                &SdpBlob::ngc(offer),
                &[Modality::Audio],
            )
            .await?;
        let mut links = conv.links.clone();
        let ring = tokio::time::sleep(OUTGOING_RING);
        tokio::pin!(ring);
        let mut answered = false;
        loop {
            if !self.outgoing_live(id) {
                // Cancelled while the call was being placed.
                hang_up_placed(&ngc, &s, &links, &conv.controller).await;
                let _ = media.close().await;
                return Ok(());
            }
            tokio::select! {
                _ = &mut cancel => {
                    hang_up_placed(&ngc, &s, &links, &conv.controller).await;
                    let _ = media.close().await;
                    return Ok(());
                }
                _ = &mut ring => {
                    self.outgoing_event(id, OutgoingEvent::RingTimeout);
                    hang_up_placed(&ngc, &s, &links, &conv.controller).await;
                    let _ = media.close().await;
                    return Ok(());
                }
                ev = events.recv() => {
                    let Some(ev) = ev else {
                        // The call was dropped here (cancelled): end it on the service too.
                        hang_up_placed(&ngc, &s, &links, &conv.controller).await;
                        let _ = media.close().await;
                        return Ok(());
                    };
                    match &ev {
                        CallEvent::Accepted { answer, links: l, .. } => {
                            links.0.extend(l.0.clone());
                            if let (Some(a), false) = (answer, answered) {
                                media.set_answer(&a.decode()?).await?;
                                answered = true;
                            }
                        }
                        CallEvent::MediaAnswer { answer, links: l } => {
                            links.0.extend(l.0.clone());
                            if !answered {
                                media.set_answer(&answer.decode()?).await?;
                                answered = true;
                            }
                        }
                        _ => {}
                    }
                    let Some(oe) = outgoing_event(&ev) else { continue };
                    match self.outgoing_event(id, oe) {
                        Some(OutgoingState::Connected) => {
                            m.adopt(s, links, media, media_events, CallKind::Direct { peer_name: name, chat_id }, None)?;
                            if share {
                                m.share_when_ready();
                            }
                            return Ok(());
                        }
                        Some(st) if !st.is_live() => {
                            let _ = media.close().await;
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn outgoing_live(&self, id: u64) -> bool {
        self.direct()
            .outgoing
            .as_ref()
            .is_some_and(|o| o.id == id && o.state.is_live())
    }

    fn push_outgoing(self: &Arc<Self>, id: u64) {
        let c = self.direct();
        let Some(o) = c.outgoing.as_ref().filter(|o| o.id == id) else {
            return;
        };
        let mut view = self.call_view(&o.mri, &o.name);
        view.status = o.state.label();
        view.over = !o.state.is_live();
        drop(c);
        self.ui.call_outgoing(view);
    }

    /// Moves the outgoing call `id` on. Connected and Cancelled close the screen at once; the
    /// other ends show their reason for a moment.
    fn outgoing_event(self: &Arc<Self>, id: u64, ev: OutgoingEvent) -> Option<OutgoingState> {
        let mut c = self.direct();
        let o = c.outgoing.as_mut().filter(|o| o.id == id)?;
        let before = o.state.clone();
        o.state = o.state.on(ev);
        let state = o.state.clone();
        if state == before {
            return Some(state);
        }
        if matches!(state, OutgoingState::Connected | OutgoingState::Cancelled) {
            if let Some(cancel) = o.cancel.take()
                && state == OutgoingState::Cancelled
            {
                let _ = cancel.send(());
            }
            c.outgoing = None;
            c.ringback = None;
            drop(c);
            self.ui.call_outgoing(CallView::default());
            return Some(state);
        }
        // Rings while the far end rings, and stops with any other state (answered, declined, over).
        c.ringback = (state == OutgoingState::Ringing)
            .then(super::sounds::start_ringback)
            .flatten();
        drop(c);
        self.push_outgoing(id);
        if !state.is_live() {
            let e = self.clone();
            later(async move {
                tokio::time::sleep(OVER_SCREEN).await;
                let mut c = e.direct();
                if c.outgoing.as_ref().is_some_and(|o| o.id == id) {
                    c.outgoing = None;
                    drop(c);
                    e.ui.call_outgoing(CallView::default());
                }
            });
        }
        Some(state)
    }

    fn cancel_outgoing(self: &Arc<Self>) {
        let id = self
            .direct()
            .outgoing
            .as_ref()
            .filter(|o| o.state.is_live())
            .map(|o| o.id);
        if let Some(id) = id {
            if self.offline.load(Ordering::Relaxed) {
                self.direct().log.push("cancel".into());
            }
            self.outgoing_event(id, OutgoingEvent::Cancel);
        }
    }
}

/// Test and demo hooks: a fake invitation, fake progress and what the fake backend was asked.
#[cfg(test)]
impl Engine {
    pub(crate) fn fake_incoming_call(self: &Arc<Self>, mri: &str, name: &str, video: bool) {
        self.ring_incoming(mri.into(), name.into(), video, "fake-call".into(), None);
    }

    pub(crate) fn fake_caller_cancel(self: &Arc<Self>) {
        let id = self.direct().incoming.as_ref().map(|i| i.id);
        if let Some(id) = id {
            self.incoming_event(id, IncomingEvent::CallerCancelled);
        }
    }

    pub(crate) fn fake_outgoing(self: &Arc<Self>, ev: OutgoingEvent) -> Option<OutgoingState> {
        let id = self.direct().outgoing.as_ref().map(|o| o.id)?;
        self.outgoing_event(id, ev)
    }

    pub(crate) fn incoming_call_state(&self) -> Option<IncomingState> {
        self.direct().incoming.as_ref().map(|i| i.state.clone())
    }

    pub(crate) fn outgoing_call_state(&self) -> Option<OutgoingState> {
        self.direct().outgoing.as_ref().map(|o| o.state.clone())
    }

    pub(crate) fn call_log(&self) -> Vec<String> {
        self.direct().log.clone()
    }

    pub(crate) fn ringtone_playing(&self) -> bool {
        self.direct().ringtone.is_some()
    }
}

// ---------------------------------------------------------------------------------------------
// The ringtone: a generated two-note chime through cpal, no asset.

const RING_RATE: u64 = calling::SAMPLE_RATE as u64;
/// One ring: two 400 ms notes, then silence, every 3 s.
const RING_CYCLE: u64 = RING_RATE * 3;
const NOTE: u64 = RING_RATE * 2 / 5;
const SECOND_NOTE_AT: u64 = RING_RATE / 2;
/// A ringtone stops by itself after this, whatever happens to the call.
const RING_MAX: Duration = Duration::from_secs(90);

/// Sample `n` (48 kHz mono) of the ringtone.
pub fn ring_sample(n: u64) -> i16 {
    let t = n % RING_CYCLE;
    let (start, freq) = if t < NOTE {
        (0, 784.0)
    } else if (SECOND_NOTE_AT..SECOND_NOTE_AT + NOTE).contains(&t) {
        (SECOND_NOTE_AT, 1046.5)
    } else {
        return 0;
    };
    let i = (t - start) as f64;
    // 10 ms fades keep the notes from clicking.
    let fade = (i / 480.0).min((NOTE as f64 - i) / 480.0).min(1.0);
    let x = 2.0 * std::f64::consts::PI * freq * n as f64 / RING_RATE as f64;
    let s = (x.sin() + 0.3 * (2.0 * x).sin()) / 1.3;
    (0.2 * 32767.0 * fade * s) as i16
}

/// The call-waiting beep (a call rings over another): two short 440 Hz beeps every 4 s,
/// quieter than the ringtone, since it plays over the call.
const WAIT_CYCLE: u64 = RING_RATE * 4;
const BEEP: u64 = RING_RATE * 3 / 20;
const SECOND_BEEP_AT: u64 = RING_RATE / 4;

/// Sample `n` (48 kHz mono) of the call-waiting beep.
pub fn waiting_sample(n: u64) -> i16 {
    let t = n % WAIT_CYCLE;
    let start = if t < BEEP {
        0
    } else if (SECOND_BEEP_AT..SECOND_BEEP_AT + BEEP).contains(&t) {
        SECOND_BEEP_AT
    } else {
        return 0;
    };
    let i = (t - start) as f64;
    let fade = (i / 480.0).min((BEEP as f64 - i) / 480.0).min(1.0);
    let x = 2.0 * std::f64::consts::PI * 440.0 * n as f64 / RING_RATE as f64;
    (0.12 * 32767.0 * fade * x.sin()) as i16
}

impl RingtoneChoice {
    /// The generator of the older ringtones; the Teams and IronTeams sounds and a file are made in
    /// `sounds`.
    fn sample(self) -> Option<fn(u64) -> i16> {
        match self {
            RingtoneChoice::Chime => Some(ring_sample),
            RingtoneChoice::Soft => Some(soft_sample),
            RingtoneChoice::Pulse => Some(pulse_sample),
            RingtoneChoice::Teams
            | RingtoneChoice::Original(_)
            | RingtoneChoice::IronTeams
            | RingtoneChoice::Custom => None,
        }
    }
}

/// The "Soft" ringtone: one 800 ms note of 660 Hz with slow fades, every 3 s.
const SOFT_NOTE: u64 = RING_RATE * 4 / 5;

/// Sample `n` (48 kHz mono) of the soft ringtone.
pub fn soft_sample(n: u64) -> i16 {
    let t = n % RING_CYCLE;
    if t >= SOFT_NOTE {
        return 0;
    }
    let i = t as f64;
    let fade = (i / 4800.0).min((SOFT_NOTE as f64 - i) / 4800.0).min(1.0);
    let x = 2.0 * std::f64::consts::PI * 660.0 * n as f64 / RING_RATE as f64;
    (0.18 * 32767.0 * fade * x.sin()) as i16
}

/// The "Pulse" ringtone: three 120 ms beeps of 880 Hz, 250 ms apart, every 3 s.
const PULSE_BEEP: u64 = RING_RATE * 3 / 25;
const PULSE_GAP: u64 = RING_RATE / 4;

/// Sample `n` (48 kHz mono) of the pulse ringtone.
pub fn pulse_sample(n: u64) -> i16 {
    let t = n % RING_CYCLE;
    let beep = t / PULSE_GAP;
    let i = t % PULSE_GAP;
    if beep > 2 || i >= PULSE_BEEP {
        return 0;
    }
    let i = i as f64;
    let fade = (i / 240.0).min((PULSE_BEEP as f64 - i) / 240.0).min(1.0);
    let x = 2.0 * std::f64::consts::PI * 880.0 * n as f64 / RING_RATE as f64;
    (0.18 * 32767.0 * fade * x.sin()) as i16
}

/// Plays a ringtone for three seconds, for the Settings preview. Silent in tests and demo mode.
pub(crate) fn preview_ringtone(choice: RingtoneChoice) {
    if let Some(tone) = Ringtone::start(choice, None) {
        let _ = std::thread::Builder::new()
            .name("ironteams-ringtone-preview".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                drop(tone);
            });
    }
}

/// Plays while it lives. Never plays in tests or demo mode.
pub(crate) struct Ringtone {
    stop: Arc<AtomicBool>,
    /// The Default or a chosen file, played by `sounds`; it stops when this is dropped.
    _tone: Option<super::sounds::Loop>,
}

impl Ringtone {
    fn start(choice: RingtoneChoice, file: Option<&str>) -> Option<Ringtone> {
        match choice.sample() {
            Some(sample) => Self::play(sample),
            None => super::sounds::start_ringtone(choice, file).map(|tone| Ringtone {
                stop: Arc::new(AtomicBool::new(false)),
                _tone: Some(tone),
            }),
        }
    }

    /// The call-waiting beep instead of the ringtone.
    fn waiting() -> Option<Ringtone> {
        Self::play(waiting_sample)
    }

    fn play(sample: fn(u64) -> i16) -> Option<Ringtone> {
        if crate::external::suppressed() {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("ironteams-ringtone".into())
            .spawn(move || play_ring(&flag, sample));
        match spawned {
            Ok(_) => Some(Ringtone { stop, _tone: None }),
            Err(err) => {
                tracing::warn!(%err, "ringtone not started");
                None
            }
        }
    }
}

impl Drop for Ringtone {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn play_ring(stop: &AtomicBool, sample: fn(u64) -> i16) {
    use calling::audio::FrameSink;
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            tracing::warn!(%err, "ringtone runtime");
            return;
        }
    };
    let mut out = match calling::audio::Playback::start(None) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(%err, "no speaker for the ringtone");
            return;
        }
    };
    let frame = calling::FRAME_SAMPLES as u64;
    let started = Instant::now();
    let mut next = started;
    let mut n = 0u64;
    while !stop.load(Ordering::Relaxed) && started.elapsed() < RING_MAX {
        let samples: Vec<i16> = (n..n + frame).map(sample).collect();
        n += frame;
        if rt.block_on(out.write_frame(&samples)).is_err() {
            break;
        }
        next += Duration::from_millis(20);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calling::invitation::EndReason;
    use serde_json::json;

    #[test]
    fn a_late_answer_reads_as_the_call_having_ended_even_under_context() {
        let late = calling::Error::Status {
            op: "attach",
            status: 412,
            body: r#"{"operationFailure":{"code":487,"subCode":10021}}"#.into(),
        };
        let plain: anyhow::Error = late.into();
        assert_eq!(
            answer_failure_text(&plain),
            "The call ended before you answered."
        );
        let wrapped = plain.context("answering the call");
        assert_eq!(
            answer_failure_text(&wrapped),
            "The call ended before you answered.",
            "found through the context chain"
        );
        assert_eq!(
            answer_failure_text(&anyhow::anyhow!("the calling connection is down")),
            "the calling connection is down"
        );
    }

    #[test]
    fn incoming_accept_then_connect() {
        let s = IncomingState::Ringing.on(IncomingEvent::Accept);
        assert_eq!(s, IncomingState::Accepting);
        assert!(!s.is_over());
        let s = s.on(IncomingEvent::Connected);
        assert_eq!(s, IncomingState::Connected);
        assert!(s.is_over());
        // Nothing moves a connected call back.
        assert_eq!(s.on(IncomingEvent::Decline), IncomingState::Connected);
    }

    #[test]
    fn incoming_decline_cancel_timeout_and_failure() {
        assert_eq!(
            IncomingState::Ringing.on(IncomingEvent::Decline),
            IncomingState::Declined
        );
        assert_eq!(
            IncomingState::Ringing.on(IncomingEvent::CallerCancelled),
            IncomingState::CallerCancelled
        );
        assert_eq!(
            IncomingState::Accepting.on(IncomingEvent::CallerCancelled),
            IncomingState::CallerCancelled
        );
        assert_eq!(
            IncomingState::Ringing.on(IncomingEvent::RingTimeout),
            IncomingState::Missed
        );
        // A timeout while answering does not drop the call.
        assert_eq!(
            IncomingState::Accepting.on(IncomingEvent::RingTimeout),
            IncomingState::Accepting
        );
        assert_eq!(
            IncomingState::Accepting.on(IncomingEvent::Failed("x".into())),
            IncomingState::Failed("x".into())
        );
        // Connected is only reached through Accept.
        assert_eq!(
            IncomingState::Ringing.on(IncomingEvent::Connected),
            IncomingState::Ringing
        );
        assert_eq!(
            IncomingState::Declined.on(IncomingEvent::Accept),
            IncomingState::Declined
        );
    }

    #[test]
    fn outgoing_ringing_then_connected() {
        let s = OutgoingState::Placing;
        assert_eq!(s.label(), "Calling…");
        let s = s.on(OutgoingEvent::Ringing);
        assert_eq!(s, OutgoingState::Ringing);
        assert_eq!(s.label(), "Ringing…");
        assert!(s.is_live());
        let s = s.on(OutgoingEvent::Accepted);
        assert_eq!(s, OutgoingState::Connected);
        assert!(!s.is_live());
        assert_eq!(s.on(OutgoingEvent::Cancel), OutgoingState::Connected);
        assert_eq!(s.on(OutgoingEvent::Ended), OutgoingState::Ended);
    }

    #[test]
    fn outgoing_cancelled_rejected_unanswered_failed() {
        assert_eq!(
            OutgoingState::Ringing.on(OutgoingEvent::Cancel),
            OutgoingState::Cancelled
        );
        assert_eq!(
            OutgoingState::Placing.on(OutgoingEvent::Cancel),
            OutgoingState::Cancelled
        );
        let r = OutgoingState::Ringing.on(OutgoingEvent::Rejected);
        assert_eq!(r, OutgoingState::Rejected);
        assert_eq!(r.label(), "Declined");
        let n = OutgoingState::Ringing.on(OutgoingEvent::RingTimeout);
        assert_eq!(n.label(), "No answer");
        assert_eq!(
            OutgoingState::Placing.on(OutgoingEvent::Failed("no relay".into())),
            OutgoingState::Failed("no relay".into())
        );
        // A late answer after a cancel changes nothing.
        assert_eq!(
            OutgoingState::Cancelled.on(OutgoingEvent::Accepted),
            OutgoingState::Cancelled
        );
    }

    fn ended(code: i64) -> CallEvent {
        CallEvent::Ended(EndReason {
            code,
            ..EndReason::default()
        })
    }

    #[test]
    fn callbacks_map_to_outgoing_events() {
        assert_eq!(
            outgoing_event(&CallEvent::Progress {
                status: "ringing".into()
            }),
            Some(OutgoingEvent::Ringing)
        );
        assert_eq!(
            outgoing_event(&CallEvent::Rejected(EndReason::default())),
            Some(OutgoingEvent::Rejected)
        );
        assert_eq!(outgoing_event(&ended(603)), Some(OutgoingEvent::Rejected));
        assert_eq!(outgoing_event(&ended(486)), Some(OutgoingEvent::Rejected));
        assert_eq!(outgoing_event(&ended(0)), Some(OutgoingEvent::Ended));
        assert_eq!(outgoing_event(&CallEvent::RosterUpdate(json!({}))), None);
    }

    #[test]
    fn a_cancel_must_name_the_ringing_call() {
        let body = json!({"callEnd": {"code": 487}, "debugContent": {"callId": "abc-123"}});
        assert!(cancels_invitation(
            "abc-123",
            "https://t/NGCallManagerWin",
            &body
        ));
        assert!(!cancels_invitation(
            "other",
            "https://t/NGCallManagerWin",
            &body
        ));
        assert!(!cancels_invitation("", "https://t/NGCallManagerWin", &body));
        let no_end = json!({"debugContent": {"callId": "abc-123"}});
        assert!(!cancels_invitation("abc-123", "https://t/x", &no_end));
    }

    #[test]
    fn commands_parse_from_the_store() {
        assert_eq!(
            CallCmd::parse("accept-video", ""),
            Some(CallCmd::Accept { video: true })
        );
        assert_eq!(
            CallCmd::parse("accept-audio", ""),
            Some(CallCmd::Accept { video: false })
        );
        assert_eq!(CallCmd::parse("decline", ""), Some(CallCmd::Decline));
        assert_eq!(
            CallCmd::parse("dial", "19:a_b@unq.gbl.spaces"),
            Some(CallCmd::DialChat("19:a_b@unq.gbl.spaces".into()))
        );
        assert_eq!(CallCmd::parse("dial", ""), None);
        assert_eq!(
            CallCmd::parse("share-screen", "19:a_b@unq.gbl.spaces"),
            Some(CallCmd::ShareScreen("19:a_b@unq.gbl.spaces".into()))
        );
        assert_eq!(CallCmd::parse("share-screen", ""), None);
        assert_eq!(
            CallCmd::parse("dial-person", "8:orgid:x"),
            Some(CallCmd::DialPerson("8:orgid:x".into()))
        );
        assert_eq!(
            CallCmd::parse("hold-accept", ""),
            Some(CallCmd::AcceptWaiting(CurrentCall::Hold))
        );
        assert_eq!(
            CallCmd::parse("end-accept", ""),
            Some(CallCmd::AcceptWaiting(CurrentCall::End))
        );
        assert_eq!(CallCmd::parse("cancel", ""), Some(CallCmd::Cancel));
        assert_eq!(CallCmd::parse("close", ""), Some(CallCmd::Close));
        assert_eq!(CallCmd::parse("video", ""), None);
    }

    #[test]
    fn only_one_on_one_chats_are_direct() {
        assert!(is_direct_chat("19:aaa_bbb@unq.gbl.spaces"));
        assert!(!is_direct_chat("19:abc@thread.v2"));
        assert!(!is_direct_chat("48:notes"));
        assert!(!is_direct_chat("19:abc@thread.tacv2"));
    }

    #[test]
    fn the_ringtone_rings_twice_then_rests() {
        assert_ne!(ring_sample(RING_RATE / 10), 0, "first note");
        assert_ne!(
            ring_sample(SECOND_NOTE_AT + RING_RATE / 10),
            0,
            "second note"
        );
        assert_eq!(ring_sample(NOTE + 10), 0, "gap between the notes");
        assert_eq!(ring_sample(RING_RATE * 2), 0, "rest");
        // It starts from silence (no click) and repeats every cycle.
        assert_eq!(ring_sample(0), 0);
        assert_eq!(ring_sample(1234), ring_sample(RING_CYCLE + 1234));
        let peak = (0..RING_CYCLE).map(|n| ring_sample(n).unsigned_abs()).max();
        assert!(peak.unwrap() < 8000, "quiet enough: {peak:?}");
    }

    #[test]
    fn every_ringtone_choice_sounds_repeats_and_stays_quiet() {
        for choice in RingtoneChoice::all() {
            // The Default and a file are played by `sounds`, which tests its own.
            let Some(f) = choice.sample() else { continue };
            let peak = (0..RING_CYCLE).map(|n| f(n).unsigned_abs()).max().unwrap();
            assert!(peak > 1000, "{choice:?} is audible: {peak}");
            assert!(peak < 8000, "{choice:?} is quiet enough: {peak}");
            assert_eq!(f(0), 0, "{choice:?} starts from silence");
            assert_eq!(
                f(RING_RATE / 10),
                f(RING_CYCLE + RING_RATE / 10),
                "{choice:?} repeats"
            );
            assert_eq!(f(RING_RATE * 2), 0, "{choice:?} rests");
            assert_eq!(RingtoneChoice::from_slug(choice.slug()), Some(choice));
        }
        // The pulse beeps three times, not four.
        assert_ne!(
            pulse_sample(PULSE_GAP * 2 + PULSE_BEEP / 2),
            0,
            "third beep"
        );
        assert_eq!(pulse_sample(PULSE_GAP * 3 + PULSE_BEEP / 2), 0, "no fourth");
    }

    #[test]
    fn tests_never_ring() {
        assert!(Ringtone::start(RingtoneChoice::default(), None).is_none());
        assert!(Ringtone::start(RingtoneChoice::Chime, None).is_none());
        assert!(Ringtone::start(RingtoneChoice::Custom, Some("/x.wav")).is_none());
        assert!(Ringtone::waiting().is_none());
    }

    #[test]
    fn the_waiting_beep_is_two_short_quiet_beeps_then_a_long_rest() {
        assert_ne!(waiting_sample(BEEP / 2 + 7), 0, "first beep");
        assert_ne!(
            waiting_sample(SECOND_BEEP_AT + BEEP / 2 + 7),
            0,
            "second beep"
        );
        assert_eq!(waiting_sample(BEEP + 10), 0, "gap");
        assert_eq!(waiting_sample(RING_RATE), 0, "rest");
        assert_eq!(waiting_sample(0), 0, "starts from silence");
        assert_eq!(waiting_sample(77), waiting_sample(WAIT_CYCLE + 77));
        let peak = (0..WAIT_CYCLE)
            .map(|n| waiting_sample(n).unsigned_abs())
            .max();
        let ring = (0..RING_CYCLE).map(|n| ring_sample(n).unsigned_abs()).max();
        assert!(
            peak < ring,
            "quieter than the ringtone: {peak:?} < {ring:?}"
        );
        // Under a second of sound in every four.
        let loud = (0..WAIT_CYCLE).filter(|n| waiting_sample(*n) != 0).count() as u64;
        assert!(loud < RING_RATE / 2, "{loud}");
    }
}
