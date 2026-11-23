//! `ironteams-cli call …`: exercise the native calling crate from a terminal.
//!
//! ```text
//! ironteams-cli call probe        authz calling endpoints, TURN credentials, calling trouter (no call)
//!     --turn-tls     find the TURN relay over TLS on port 443 instead of UDP
//! ironteams-cli call meetings     meetings with an active call, as the CSA chat list shows them
//! ironteams-cli call history [n]  the last n calls (default 20) from the call log: time (UTC), direction, outcome, duration, party
//! ironteams-cli call join [opts]   join the newest active meeting (or --link URL), send audio, record what arrives
//!     --link <url>   join this meeting link instead of the newest active meeting
//!     --mic          send the microphone (default: a 440 Hz tone)
//!     --speaker      play what arrives (default: write it to --out)
//!     --no-aec       send the raw microphone (default with --mic: echo cancellation, noise
//!                    suppression and AGC, fed by what --speaker plays)
//!     --out <wav>    where received audio goes (default: ironteams-call.wav)
//!     --secs <n>     stay this long, then leave (default: 30)
//!     --relay <host> TURN gateway to start from (default: chosen from the authz region)
//!     --relay-only   use relayed (TURN) candidates only, to prove the relay path
//!     --turn-tls     reach the TURN relay over TLS on port 443 (for networks that block UDP)
//!     --dump <dir>   write every SDP exchanged to this directory (keep it out of the repo)
//!     --mute-at <s>        mute us <s> seconds after the join (stop sending the mic, publish
//!                          isMuted); repeatable, like the three below
//!     --unmute-at <s>      unmute us <s> seconds after the join
//!     --hand-at <s>        raise our hand <s> seconds after the join
//!     --lower-hand-at <s>  lower our hand <s> seconds after the join
//! ironteams-cli call listen [opts]  report what the calling trouter pushes; answers nothing by default
//!     --secs <n>     listen this long (default: 120)
//!     --dump <dir>   write every push as JSON to this directory (keep it out of the repo)
//!     --ring-only    on the first incoming call: attach and report ringing, never answer
//!     --decline      on the first incoming call: decline it
//!     --accept-audio on the first incoming call: answer with audio, run it like `join` for
//!                    --secs (default 30; --mic, --speaker, --out, --turn-tls ... apply), hang up
//!     --accept       the same (video is not supported yet: answers with audio only)
//!     --no-offer <mode>  when the ring carries no offer: `fetch` (default: attach asking for
//!                    the caller's offer, else offer ours on `newOffer`), `new-offer`,
//!                    `accept-offer` or `attach-offer` (see `calling::incoming::NoOffer`;
//!                    all but the answerer path are UNVERIFIED). `--offer-in-attach` is
//!                    `--no-offer attach-offer`. With --dump, every request IronTeams sends and
//!                    its answer, each step and every SDP are written there too
//!     --wait <n>     with an action: wait this long for the first call (default: 120)
//! ironteams-cli call dial <mri|email> [opts]  place a 1:1 audio call; prints ringing/connected/ended
//!     --secs <n>       stay this long after it connects, then hang up (default: 30)
//!     --ring-secs <n>  cancel when not answered within this (default: 60); Ctrl-C cancels too
//!     (the audio, TURN and --dump flags of `join` apply)
//! ```
//!
//! `join` prints the roster (count, display names, role, mute and hand flags) whenever it
//! changes. Names go to the terminal only, never to a file.
//!
//! Live tests may only join meetings the user hosts on their own personal account; see the
//! project's live-test rules.

use anyhow::{Context, Result, bail};
use calling::audio::{
    AudioProcessor, Capture, Paced, Playback, ToneSource, WavSink, echo_cancelled,
    spawn_capture_encoder, spawn_decoder_playback,
};
use calling::incoming::NoOffer;
use calling::invitation::{CallEvent, NgcPush, classify};
use calling::media::{MediaEvents, MediaOptions, MediaSession, MediaState};
use calling::ngc_trouter::{NgcEvent, NgcTrouter};
use calling::relay::tls::TlsShim;
use calling::signaling::bodies::reject_request;
use calling::signaling::meeting::{MeetingTarget, meeting_from_csa_chat, meeting_from_join_url};
use calling::signaling::roster::roster_snapshot;
use calling::signaling::{CallSession, Controls, NgcClient, Roster};
use calling::{CallInvitation, IceServer, Links, Modality, Participant, SdpBlob};
use serde_json::Value;
use std::time::Duration;
use teams_proto::Client;

const CSA_ME: &str = "https://teams.microsoft.com/api/csa/api/v1/teams/users/me?isPrefetch=false&enableMembershipSummary=true";

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "?".into())
}

/// A calling-trouter endpoint id: stable per install and distinct from the chat trouter's.
pub fn calling_epid(chat_epid: &str) -> String {
    let tail = chat_epid.get(8..).unwrap_or(chat_epid);
    format!("ca11ca11{tail}")
}

/// The CSA chat entries of meetings whose call is running, with their join target.
pub async fn active_meetings(client: &Client) -> Result<Vec<(Value, MeetingTarget)>> {
    let v = client
        .json("csa", reqwest::Method::GET, CSA_ME, None)
        .await?;
    let chats = v["chats"].as_array().cloned().unwrap_or_default();
    Ok(chats
        .into_iter()
        .filter(|c| c["activeMeetup"]["status"] == "Active")
        .filter_map(|c| meeting_from_csa_chat(&c).map(|t| (c, t)))
        .collect())
}

pub async fn run(client: &Client, args: &[String]) -> Result<()> {
    match args.get(1).map(String::as_str) {
        Some("probe") => {
            let ngc = NgcClient::discover(client.clone())
                .await
                .context("authz (calling endpoints)")?;
            let ep = ngc.endpoints();
            println!(
                "conversation service: {} (region {}, partition {}, ring {})",
                host(&ep.conversation_service_url),
                ep.region,
                ep.partition,
                ep.ring
            );
            let turn = if args.iter().any(|a| a == "--turn-tls") {
                calling::relay::resolve_tls_ice_servers(client, &ep.region, opt(args, "--relay"))
                    .await
                    .map(|(ice, expires, _shim)| (ice, expires, " over TLS via a loopback shim"))
            } else {
                calling::relay::resolve_ice_servers(client, &ep.region, opt(args, "--relay"))
                    .await
                    .map(|(ice, expires)| (ice, expires, ""))
            };
            match turn {
                Ok((ice, expires, how)) => println!(
                    "TURN: {}{how} ({} credential(s), expire in {expires:?})",
                    ice.first()
                        .and_then(|s| s.urls.first())
                        .map(String::as_str)
                        .unwrap_or("-"),
                    ice.len(),
                ),
                Err(e) => println!("TURN: failed: {e}"),
            }
            let (trouter, mut rx) = NgcTrouter::spawn(client.clone(), calling_epid(client.epid()));
            let wait = tokio::time::timeout(Duration::from_secs(30), async {
                while let Some(ev) = rx.recv().await {
                    match ev {
                        NgcEvent::Connected { surl } => return Ok(surl),
                        NgcEvent::Disconnected(r) => println!("calling trouter: disconnected: {r}"),
                        NgcEvent::Push(p) => println!("calling trouter: push {}", p.url),
                    }
                }
                bail!("calling trouter task ended")
            })
            .await;
            match wait {
                Ok(Ok(surl)) => println!("calling trouter: registered on {}", host(&surl)),
                Ok(Err(e)) => println!("calling trouter: {e}"),
                Err(_) => println!("calling trouter: not registered within 30 s"),
            }
            drop(trouter);
        }
        Some("meetings") => {
            let ms = active_meetings(client).await?;
            println!("{} meeting(s) with an active call", ms.len());
            for (chat, t) in &ms {
                println!(
                    "{}  [{}] code={} tenant={} organizer={} conversationUrl={}",
                    chat["title"].as_str().unwrap_or("?"),
                    chat["productContext"].as_str().unwrap_or("-"),
                    t.meeting_code.is_some(),
                    t.tenant_id.is_some(),
                    t.organizer_id.is_some(),
                    t.conversation_url.as_deref().map(host).unwrap_or_default(),
                );
                println!("  id: {}", t.thread_id.as_deref().unwrap_or("?"));
            }
        }
        Some("history") => history(client, &args[2..]).await?,
        Some("join") => join(client, &args[2..]).await?,
        Some("listen") => listen(client, &args[2..]).await?,
        Some("dial") => dial(client, &args[2..]).await?,
        _ => bail!("usage: ironteams-cli call <probe|meetings|history|join|listen|dial>"),
    }
    Ok(())
}

/// `ironteams-cli call history [n]`: one line per call, newest first.
async fn history(client: &Client, args: &[String]) -> Result<()> {
    let n: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(20);
    for r in calling::history::call_history(client, n).await? {
        println!("{}", history_line(&r));
    }
    Ok(())
}

fn history_line(r: &calling::history::CallRecord) -> String {
    use calling::history::{CallDirection, CallOutcome};
    let secs = r.start_ms.div_euclid(1000);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let tod = secs.rem_euclid(86_400);
    let outcome = match (r.outcome, r.direction) {
        (CallOutcome::Missed, CallDirection::Incoming) => "missed",
        (CallOutcome::Missed, CallDirection::Outgoing) => "no answer",
        (CallOutcome::Accepted, _) => "answered",
        (CallOutcome::Declined, _) => "declined",
        (CallOutcome::Voicemail, _) => "voicemail",
    };
    let direction = match r.direction {
        CallDirection::Incoming => "in ",
        CallDirection::Outgoing => "out",
    };
    let duration = r.duration_secs.map_or("-".to_owned(), |s| {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    });
    let party = match (&r.other_name, &r.other_mri) {
        (Some(name), _) => name.clone(),
        (None, Some(mri)) => mri.clone(),
        (None, None) => "-".to_owned(),
    };
    let group = if r.is_group {
        format!(" [group, {} others]", r.participants.len())
    } else {
        String::new()
    };
    format!(
        "{y:04}-{mo:02}-{d:02} {:02}:{:02} {direction} {outcome:<9} {duration:>8}  {party}{group}",
        tod / 3600,
        tod % 3600 / 60
    )
}

/// Year, month, day of a day count since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// The first `{contentType, blob}` object anywhere in `v` (a join answer may carry the media
/// answer inline).
fn find_blob(v: &Value) -> Option<SdpBlob> {
    match v {
        Value::Object(m) => {
            if let (Some(ct), Some(b)) = (
                m.get("contentType").and_then(Value::as_str),
                m.get("blob").and_then(Value::as_str),
            ) && !b.is_empty()
            {
                return Some(SdpBlob {
                    content_type: ct.to_owned(),
                    blob: b.to_owned(),
                });
            }
            m.values().find_map(find_blob)
        }
        Value::Array(a) => a.iter().find_map(find_blob),
        _ => None,
    }
}

/// An in-meeting control `join` can run on a schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Mute,
    Unmute,
    RaiseHand,
    LowerHand,
}

const CONTROL_FLAGS: [(&str, Control); 4] = [
    ("--mute-at", Control::Mute),
    ("--unmute-at", Control::Unmute),
    ("--hand-at", Control::RaiseHand),
    ("--lower-hand-at", Control::LowerHand),
];

/// Every `--<control>-at <seconds>` in `args`, earliest first.
fn scheduled_controls(args: &[String]) -> Result<Vec<(Duration, Control)>> {
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate() {
        let Some((flag, control)) = CONTROL_FLAGS.iter().find(|(f, _)| f == a) else {
            continue;
        };
        let secs: f64 = args
            .get(i + 1)
            .and_then(|v| v.parse().ok())
            .filter(|s: &f64| s.is_finite() && *s >= 0.0)
            .with_context(|| format!("{flag} needs a number of seconds"))?;
        out.push((Duration::from_secs_f64(secs), *control));
    }
    out.sort_by_key(|(at, _)| *at);
    Ok(out)
}

fn print_roster(roster: &Roster, own_mri: &str) {
    println!("roster: {}", roster.summary(own_mri));
}

fn dump(dir: Option<&str>, name: &str, sdp: &str) {
    if let Some(d) = dir {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let _ = std::fs::write(format!("{d}/{n}-{name}.sdp"), sdp);
    }
}

/// TURN for one call: the relay over UDP (or over TLS with `--turn-tls`), or host candidates only
/// when none answers. The TLS shim must outlive every media session built from the servers.
async fn ice_servers(
    client: &Client,
    ngc: &NgcClient,
    args: &[String],
) -> (Vec<IceServer>, Option<TlsShim>) {
    let region = ngc.endpoints().region.clone();
    let (ice, shim) = if args.iter().any(|a| a == "--turn-tls") {
        match calling::relay::resolve_tls_ice_servers(client, &region, opt(args, "--relay")).await {
            Ok((ice, _, shim)) => (ice, Some(shim)),
            Err(e) => {
                println!("TURN over TLS unavailable ({e}); host candidates only");
                (vec![], None)
            }
        }
    } else {
        match calling::relay::resolve_ice_servers(client, &region, opt(args, "--relay")).await {
            Ok((ice, _)) => (ice, None),
            Err(e) => {
                println!("TURN unavailable ({e}); host candidates only");
                (vec![], None)
            }
        }
    };
    if let Some(url) = ice.first().and_then(|s| s.urls.first()) {
        println!("TURN: {url}");
    }
    (ice, shim)
}

fn media_options(args: &[String]) -> MediaOptions {
    MediaOptions {
        relay_only: args.iter().any(|a| a == "--relay-only"),
        ..MediaOptions::default()
    }
}

/// How a call runs once it is placed, joined or accepted (the flags `join` documents).
struct CallOpts {
    /// Stay this long after the call connects, then hang up.
    secs: u64,
    /// An outgoing call that is not answered within this is cancelled.
    ring_timeout: Option<Duration>,
    schedule: Vec<(Duration, Control)>,
    dump_dir: Option<String>,
    mic: bool,
    speaker: bool,
    no_aec: bool,
    out: String,
}

impl CallOpts {
    fn from_args(args: &[String], default_secs: u64) -> Result<Self> {
        Ok(CallOpts {
            secs: opt(args, "--secs")
                .and_then(|s| s.parse().ok())
                .unwrap_or(default_secs),
            ring_timeout: None,
            schedule: scheduled_controls(args)?,
            dump_dir: opt(args, "--dump").map(str::to_owned),
            mic: args.iter().any(|a| a == "--mic"),
            speaker: args.iter().any(|a| a == "--speaker"),
            no_aec: args.iter().any(|a| a == "--no-aec"),
            out: opt(args, "--out")
                .unwrap_or("ironteams-call.wav")
                .to_owned(),
        })
    }
}

/// One call in progress, as `run_call` drives it.
struct LiveCall {
    ngc: NgcClient,
    s: CallSession,
    /// The conversation controller (meetings and placed calls); an answered call has none.
    controller: Option<String>,
    links: Links,
    media: MediaSession,
    events: MediaEvents,
    ice: Vec<IceServer>,
    media_opts: MediaOptions,
    /// Our offer's answer is applied (always true for an answered incoming call).
    answered: bool,
    /// Joined or answered; false while an outgoing call rings.
    connected: bool,
    roster: Roster,
}

/// Sends audio (a tone, or the microphone with `--mic`), plays or records what arrives, and
/// follows the call's callbacks until `--secs` after it connected, the far end hangs up, the media
/// fails, the ring times out or Ctrl-C; then hangs up unless the service ended the call.
async fn run_call(
    call: LiveCall,
    pushes: &mut tokio::sync::mpsc::UnboundedReceiver<NgcEvent>,
    o: CallOpts,
) -> Result<()> {
    let LiveCall {
        ngc,
        s,
        controller,
        mut links,
        mut media,
        events,
        ice,
        media_opts,
        mut answered,
        connected,
        mut roster,
    } = call;
    let dump_dir = o.dump_dir.as_deref();
    let mut schedule = o.schedule;
    let modalities = [Modality::Audio];
    let mut state_rx = events.state;
    let started = tokio::time::Instant::now();
    let mut connected_at = connected.then_some(started);
    let mut controls = Controls::default();

    // Audio: source -> encoder -> media; media -> decoder -> sink. The microphone goes through
    // the echo canceller, whose reference is what the speaker plays.
    let mut echo_reference = None;
    let mut encoded = if o.mic {
        let mic = Capture::start(None)?;
        if o.no_aec {
            spawn_capture_encoder(mic)
        } else {
            let (mic, reference) = echo_cancelled(mic, AudioProcessor::new());
            echo_reference = Some(reference);
            spawn_capture_encoder(mic)
        }
    } else {
        spawn_capture_encoder(Paced::new(ToneSource::endless(440.0)))
    };
    // The media session can be replaced mid-call (the service restarts ICE when the lobby
    // admits us), so the sender follows a watch and every session's audio feeds one forwarder.
    let (sender_tx, sender_rx) = tokio::sync::watch::channel(media.audio_sender());
    let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sent_count = sent.clone();
    // While muted the microphone's packets are replaced here by digital silence: muting is also
    // local, not only the state the service shows, yet the media keeps flowing (a call that sends
    // no RTP at all is ended by the service as `MediaConnectivityFailure`).
    let mic_muted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let send_muted = mic_muted.clone();
    let send_task = tokio::spawn(async move {
        while let Some(mut p) = encoded.recv().await {
            if send_muted.load(std::sync::atomic::Ordering::Relaxed) {
                p = calling::audio::OPUS_SILENCE.to_vec();
            }
            let sender = sender_rx.borrow().clone();
            if sender.send(p).await.is_ok() {
                sent_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
    let (fwd_tx, fwd_rx) = tokio::sync::mpsc::channel(256);
    let received = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let rx_total = received.clone();
    // Owns the only sender of the playback queue: dropping it (and the forwarders) ends playback.
    let forward = move |mut incoming: tokio::sync::mpsc::Receiver<calling::OpusPacket>| {
        let (fwd_tx, rx_count) = (fwd_tx.clone(), rx_total.clone());
        tokio::spawn(async move {
            while let Some(p) = incoming.recv().await {
                rx_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if fwd_tx.send(p).await.is_err() {
                    break;
                }
            }
        })
    };
    let mut forwarders = vec![forward(events.audio)];
    let playback = if o.speaker {
        let speaker = match echo_reference {
            Some(reference) => Playback::start_with_tap(None, move |f| reference.push(f))?,
            None => Playback::start(None)?,
        };
        spawn_decoder_playback(fwd_rx, speaker)
    } else {
        spawn_decoder_playback(fwd_rx, WavSink::create(&o.out)?)
    };

    let mut keep_alive: Option<tokio::task::JoinHandle<()>> = None;
    let mut ended = false;
    let far = started + Duration::from_secs(365 * 86_400);
    loop {
        let deadline = connected_at.map(|t| t + Duration::from_secs(o.secs));
        let ring_deadline = o.ring_timeout.map(|r| started + r);
        let next_control =
            connected_at.unwrap_or(far) + schedule.first().map(|(at, _)| *at).unwrap_or_default();
        tokio::select! {
            _ = tokio::time::sleep_until(deadline.unwrap_or(far)), if deadline.is_some() => break,
            _ = tokio::time::sleep_until(ring_deadline.unwrap_or(far)), if connected_at.is_none() && ring_deadline.is_some() => {
                println!("no answer within {} s; cancelling", o.ring_timeout.unwrap_or_default().as_secs());
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                println!("interrupted");
                break;
            }
            _ = tokio::time::sleep_until(next_control), if connected_at.is_some() && !schedule.is_empty() => {
                let (_, control) = schedule.remove(0);
                let Some(controller) = controller.as_deref() else {
                    println!("control {control:?} skipped: this call has no conversation controller");
                    continue;
                };
                let result = match control {
                    Control::Mute | Control::Unmute => {
                        let muted = control == Control::Mute;
                        mic_muted.store(muted, std::sync::atomic::Ordering::Relaxed);
                        ngc.set_muted(&s, &links, controller, &mut controls, muted).await
                    }
                    Control::RaiseHand => ngc
                        .raise_hand(&s, &links, controller, &mut controls)
                        .await
                        .map(|r| println!("hand raised (queue #{})", r.rank.map_or("?".into(), |n| n.to_string()))),
                    Control::LowerHand => {
                        let own = roster.hand_of(&s.me.mri).map(|h| h.state_id.clone());
                        ngc.lower_hand(&s, &links, controller, &mut controls, own.as_deref())
                            .await
                            .map(|lowered| if !lowered { println!("no raised hand to lower") })
                    }
                };
                match result {
                    Ok(()) => println!("control {control:?}: ok"),
                    Err(e) => println!("control {control:?} failed: {e}"),
                }
            }
            st = state_rx.recv() => match st {
                Some(st) => {
                    println!("media: {st:?}");
                    if matches!(st, MediaState::Failed | MediaState::Closed) { break; }
                }
                None => break,
            },
            ev = pushes.recv() => {
                let Some(ev) = ev else { break };
                let NgcEvent::Push(p) = ev else {
                    println!("calling trouter: {ev:?}");
                    continue;
                };
                match classify(&p.url, &p.body) {
                    NgcPush::Call { event, .. } => match event {
                        CallEvent::Accepted { answer, links: l, keep_alive_interval_s, .. } => {
                            if connected_at.is_none() {
                                println!("connected");
                                connected_at = Some(tokio::time::Instant::now());
                            } else {
                                println!("accepted");
                            }
                            links.0.extend(l.0);
                            if let (Some(a), false) = (answer, answered) {
                                let sdp = a.decode().unwrap_or_else(|e| { println!("undecodable SDP: {e}"); String::new() });
                                dump(dump_dir, "service-answer", &sdp);
                                match media.set_answer(&sdp).await {
                                    Ok(()) => {
                                        answered = true;
                                        println!("media answer applied");
                                    }
                                    Err(e) => println!("media answer refused: {e}"),
                                }
                            }
                            if keep_alive.is_none() {
                                let (ngc, s, l) = (ngc.clone(), s.clone(), links.clone());
                                keep_alive = Some(tokio::spawn(async move {
                                    if let Err(e) = ngc.run_keep_alive(&s, &l, keep_alive_interval_s).await {
                                        println!("keepAlive stopped: {e}");
                                    }
                                }));
                            }
                        }
                        CallEvent::MediaAnswer { answer, links: l } => {
                            links.0.extend(l.0);
                            if !answered {
                                let sdp = answer.decode().unwrap_or_else(|e| { println!("undecodable SDP: {e}"); String::new() });
                                dump(dump_dir, "service-answer", &sdp);
                                match media.set_answer(&sdp).await {
                                    Ok(()) => {
                                        answered = true;
                                        println!("media answer applied");
                                    }
                                    Err(e) => println!("media answer refused: {e}"),
                                }
                            }
                        }
                        CallEvent::MediaRenegotiation { offer, links: l } => {
                            println!("renegotiation offered");
                            let sdp = offer.decode().unwrap_or_else(|e| { println!("undecodable SDP: {e}"); String::new() });
                            dump(dump_dir, "service-reoffer", &sdp);
                            let answer = match media.accept_offer(&sdp).await {
                                Ok(ans) => Ok(ans),
                                Err(e) => {
                                    // A re-offer that restarts ICE (new mids/ufrags after the lobby)
                                    // is answered by a fresh peer connection.
                                    println!("existing session refused the re-offer ({e}); answering on a new session");
                                    match MediaSession::with_options(ice.clone(), media_opts.clone()).await {
                                        Ok((fresh, ev)) => match fresh.accept_offer(&sdp).await {
                                            Ok(ans) => {
                                                let old = std::mem::replace(&mut media, fresh);
                                                let _ = old.close().await;
                                                let _ = sender_tx.send(media.audio_sender());
                                                state_rx = ev.state;
                                                forwarders.push(forward(ev.audio));
                                                Ok(ans)
                                            }
                                            Err(e) => Err(e),
                                        },
                                        Err(e) => Err(e),
                                    }
                                }
                            };
                            match answer {
                                Ok(ans) => {
                                    dump(dump_dir, "our-reanswer", &ans);
                                    if let Some(url) = l.get("mediaAnswer") {
                                        match ngc.send_media_answer(&s, url, &modalities, &SdpBlob::ngc(ans)).await {
                                            Ok(_) => {
                                                println!("renegotiation answered");
                                                // Teams ignored the first state update after admission
                                                // (mautrix-teams), so publish the mute state again.
                                                if let Some(controller) = controller.as_deref()
                                                    && let Err(e) = ngc.repeat_muted(&s, &links, controller, &mut controls).await
                                                {
                                                    println!("mute state not repeated: {e}");
                                                }
                                            }
                                            Err(e) => println!("renegotiation answer refused: {e}"),
                                        }
                                    }
                                }
                                Err(e) => println!("renegotiation not answered: {e}"),
                            }
                            links.0.extend(l.0);
                        }
                        CallEvent::Rejected(r) => {
                            println!("rejected by the far end: {r:?}");
                            ended = true;
                            break;
                        }
                        CallEvent::Ended(r) => {
                            println!("ended by service: {r:?}");
                            ended = true;
                            break;
                        }
                        CallEvent::Progress { status } => println!("progress: {status}"),
                        CallEvent::MediaAcknowledgement { .. } => println!("media acknowledged"),
                        CallEvent::RosterUpdate(delta) => {
                            if roster.apply(&delta) {
                                print_roster(&roster, &s.me.mri);
                            }
                        }
                        CallEvent::ConversationUpdate(body) => {
                            let keys: Vec<&str> = body.as_object().map(|m| m.keys().map(String::as_str).collect()).unwrap_or_default();
                            println!("conversation update: {}", keys.join(","));
                            if let Some(snapshot) = roster_snapshot(&body)
                                && roster.apply_snapshot(snapshot)
                            {
                                print_roster(&roster, &s.me.mri);
                            }
                        }
                        CallEvent::Other { scope, event, .. } => println!("callback {scope}/{event}"),
                    },
                    NgcPush::Invitation(_) => println!("(ignored an incoming call invitation)"),
                    NgcPush::Other { url, .. } => println!("push {}", url.rsplit('/').take(3).collect::<Vec<_>>().join("<")),
                }
            }
        }
    }

    if !ended {
        let r = if links.get("leave").is_some() || links.get("callLeg").is_some() {
            ngc.hang_up(&s, &links).await
        } else if let Some(controller) = controller.as_deref() {
            ngc.leave_controller(&s, controller).await
        } else {
            Err(calling::Error::Protocol("no link to hang up on".into()))
        };
        match r {
            Ok(()) => println!("hung up"),
            Err(e) => println!("hang-up failed: {e}"),
        }
    }
    if let Some(k) = keep_alive {
        k.abort();
    }
    media.close().await?;
    drop(media);
    // The tone source never ends: stop the sender, then let the playback drain and finish.
    send_task.abort();
    // Dropping the forwarders closes the playback's input, so the WAV is finalised.
    drop(forward);
    for f in forwarders {
        f.abort();
    }
    let sent = sent.load(std::sync::atomic::Ordering::Relaxed);
    if tokio::time::timeout(Duration::from_secs(5), playback)
        .await
        .is_err()
    {
        println!("playback did not finish in 5 s");
    }
    println!(
        "audio packets: sent {sent}, received {}",
        received.load(std::sync::atomic::Ordering::Relaxed)
    );
    if !o.speaker {
        println!("received audio written to {}", o.out);
    }
    Ok(())
}

/// Spawns the calling trouter and waits (30 s at most) for its surl.
async fn calling_trouter(
    client: &Client,
) -> Result<(
    NgcTrouter,
    tokio::sync::mpsc::UnboundedReceiver<NgcEvent>,
    String,
)> {
    let (trouter, pushes) = NgcTrouter::spawn(client.clone(), calling_epid(client.epid()));
    let surl = tokio::time::timeout(Duration::from_secs(30), trouter.connected_surl())
        .await
        .ok()
        .flatten()
        .context("calling trouter did not register")?;
    println!("calling trouter registered");
    Ok((trouter, pushes, surl))
}

async fn join(client: &Client, args: &[String]) -> Result<()> {
    let o = CallOpts::from_args(args, 30)?;
    let dump_dir = o.dump_dir.clone();
    let target = match opt(args, "--link") {
        Some(link) => meeting_from_join_url(link).context("not a meeting link")?,
        None => {
            let mut ms = active_meetings(client).await?;
            ms.sort_by_key(|(c, _)| c["lastJoinAt"].as_str().unwrap_or_default().to_owned());
            let (chat, t) = ms.pop().context("no meeting with an active call")?;
            println!(
                "meeting: {} [{}]",
                chat["title"].as_str().unwrap_or("?"),
                chat["productContext"].as_str().unwrap_or("-")
            );
            t
        }
    };

    let ngc = NgcClient::discover(client.clone()).await?;
    let (trouter, mut pushes, surl) = calling_trouter(client).await?;
    // The TLS shim must outlive every media session built from `ice`.
    let (ice, _turn_shim) = ice_servers(client, &ngc, args).await;
    let media_opts = media_options(args);
    let (media, events) = MediaSession::with_options(ice.clone(), media_opts.clone()).await?;
    let offer = media.create_offer().await?;
    dump(dump_dir.as_deref(), "our-offer", &offer);
    println!("offer ready ({} bytes)", offer.len());

    let me = client.self_mri().await?;
    let s = CallSession::new(&surl, &me, "ironteams");
    let conv = ngc
        .join(&s, &target, &SdpBlob::ngc(offer), &[Modality::Audio])
        .await
        .context("join")?;
    println!("joined: controller {}", host(&conv.controller));
    let mut roster = Roster::default();
    if let Some(snapshot) = roster_snapshot(&conv.raw)
        && roster.apply_snapshot(snapshot)
    {
        print_roster(&roster, &s.me.mri);
    }
    let answered = apply_inline_answer(&media, &conv.raw, dump_dir.as_deref()).await;
    let call = LiveCall {
        ngc,
        s,
        controller: Some(conv.controller.clone()),
        links: conv.links.clone(),
        media,
        events,
        ice,
        media_opts,
        answered,
        connected: true,
        roster,
    };
    run_call(call, &mut pushes, o).await?;
    drop(trouter);
    Ok(())
}

/// Applies a media answer the service sent inline in a join or place answer. True when applied.
async fn apply_inline_answer(media: &MediaSession, raw: &Value, dump_dir: Option<&str>) -> bool {
    let Some(ans) = find_blob(raw) else {
        return false;
    };
    let sdp = match ans.decode() {
        Ok(sdp) => sdp,
        Err(e) => {
            println!("undecodable inline SDP: {e}");
            return false;
        }
    };
    dump(dump_dir, "service-answer-inline", &sdp);
    match media.set_answer(&sdp).await {
        Ok(()) => {
            println!("media answer (inline in the answer) applied");
            true
        }
        Err(e) => {
            println!("inline media answer refused: {e}");
            false
        }
    }
}

/// What `call listen` does with the first incoming call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListenAction {
    /// Report pushes only (the default): nothing is answered.
    Report,
    /// Attach and report ringing, then keep reporting; never answer.
    RingOnly,
    Decline,
    /// Answer with audio (`--accept` and `--accept-audio`: video is not supported yet).
    Accept,
}

fn listen_action(args: &[String]) -> Result<ListenAction> {
    let mut picked: Vec<ListenAction> = args
        .iter()
        .filter_map(|a| match a.as_str() {
            "--ring-only" => Some(ListenAction::RingOnly),
            "--decline" => Some(ListenAction::Decline),
            "--accept" | "--accept-audio" => Some(ListenAction::Accept),
            _ => None,
        })
        .collect();
    picked.dedup();
    match picked.as_slice() {
        [] => Ok(ListenAction::Report),
        [one] => Ok(*one),
        _ => bail!("pick one of --accept, --accept-audio, --decline and --ring-only"),
    }
}

/// Declines an invitation: on its own `reject`/`rejection` link, else on the leg `attach` hands
/// back. UNVERIFIED live.
async fn decline(ngc: &NgcClient, s: &CallSession, inv: &CallInvitation) -> calling::Result<()> {
    if reject_request(&inv.links).is_some() {
        return ngc.reject(s, &inv.links).await;
    }
    let links = ngc.attach(s, inv).await?;
    ngc.reject(s, &links).await
}

fn invitation_line(n: u32, inv: &CallInvitation) -> String {
    format!(
        "#{n} incoming call: modalities {:?}, caller name {}, offer {}, links [{}]",
        inv.modalities,
        if inv.from.display_name.is_some() {
            "present"
        } else {
            "missing"
        },
        if inv.offer.is_some() {
            "present"
        } else {
            "missing"
        },
        inv.links.0.keys().cloned().collect::<Vec<_>>().join(", "),
    )
}

/// `call listen [--secs N] [--dump DIR]`: registers the calling trouter and reports what it pushes
/// (incoming call invitations and call callbacks) without answering anything.
///
/// With `--ring-only`, `--decline`, `--accept` or `--accept-audio` it acts on the first invitation
/// that arrives within `--wait` seconds (default 120): `--ring-only` attaches and reports ringing
/// and keeps reporting; `--decline` declines; `--accept[-audio]` attaches, rings, answers the
/// caller's offer with audio (or, when the ring has none, fetches it with the attach, else offers
/// ours; see `calling::incoming`) and runs the call like `join` for `--secs` (default 30, the audio
/// flags of `join` apply), then hangs up.
async fn listen(client: &Client, args: &[String]) -> Result<()> {
    let action = listen_action(args)?;
    let secs: u64 = if action == ListenAction::Report {
        opt(args, "--secs")
            .map(str::parse)
            .transpose()?
            .unwrap_or(120)
    } else {
        opt(args, "--wait")
            .map(str::parse)
            .transpose()?
            .unwrap_or(120)
    };
    let dump = opt(args, "--dump").map(std::path::PathBuf::from);
    if let Some(d) = &dump {
        std::fs::create_dir_all(d)?;
    }
    if action == ListenAction::Accept && args.iter().any(|a| a == "--accept") {
        println!("note: video is not supported yet; --accept answers with audio only");
    }
    let (trouter, mut rx) = NgcTrouter::spawn(client.clone(), calling_epid(client.epid()));
    let mut n = 0u32;
    let mut surl = None;
    let run = tokio::time::timeout(Duration::from_secs(secs), async {
        while let Some(ev) = rx.recv().await {
            match ev {
                NgcEvent::Connected { surl: u } => {
                    println!("listening on {} for {secs} s", host(&u));
                    surl = Some(u);
                }
                NgcEvent::Disconnected(r) => println!("calling trouter: disconnected: {r}"),
                NgcEvent::Push(p) => {
                    n += 1;
                    if let Some(d) = &dump {
                        let f = d.join(format!("push-{n:03}.json"));
                        let doc = serde_json::json!({"url": p.url, "body": p.body});
                        std::fs::write(&f, serde_json::to_vec_pretty(&doc)?)?;
                    }
                    match classify(&p.url, &p.body) {
                        NgcPush::Invitation(inv) => {
                            println!("{}", invitation_line(n, &inv));
                            if action != ListenAction::Report {
                                return anyhow::Ok(Some(inv));
                            }
                        }
                        NgcPush::Call { event, .. } => println!("#{n} call callback: {event:?}"),
                        NgcPush::Other { url, .. } => println!("#{n} other push: {}", host(&url)),
                    }
                }
            }
        }
        anyhow::Ok(None)
    })
    .await;
    let inv = match run {
        Ok(r) => r?,
        Err(_) => {
            println!("stopped after {secs} s, {n} push(es)");
            None
        }
    };
    if let (Some(inv), Some(surl)) = (inv, surl) {
        answer_invitation(client, args, action, &inv, &surl, &mut rx).await?;
    }
    drop(trouter);
    Ok(())
}

/// Prints the calling pushes that arrive within `secs` (or until the call ends or Ctrl-C).
async fn report_pushes(rx: &mut tokio::sync::mpsc::UnboundedReceiver<NgcEvent>, secs: u64) {
    let _ = tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => { println!("interrupted"); return; }
                ev = rx.recv() => {
                    let Some(ev) = ev else { return };
                    let NgcEvent::Push(p) = ev else {
                        println!("calling trouter: {ev:?}");
                        continue;
                    };
                    match classify(&p.url, &p.body) {
                        NgcPush::Call { event, .. } => {
                            let end = matches!(event, CallEvent::Ended(_) | CallEvent::Rejected(_));
                            println!("call callback: {event:?}");
                            if end { return; }
                        }
                        NgcPush::Invitation(inv) => println!("{}", invitation_line(0, &inv)),
                        NgcPush::Other { url, body } => {
                            println!("other push: {} keys [{}]", host(&url), body.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>().join(",")).unwrap_or_default());
                        }
                    }
                }
            }
        }
    })
    .await;
}

async fn answer_invitation(
    client: &Client,
    args: &[String],
    action: ListenAction,
    inv: &CallInvitation,
    surl: &str,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<NgcEvent>,
) -> Result<()> {
    let mut ngc = NgcClient::discover(client.clone())
        .await
        .context("authz (calling endpoints)")?;
    if let Some(dir) = opt(args, "--dump").map(str::to_owned) {
        ngc = ngc.with_observer(std::sync::Arc::new(
            move |e: &calling::signaling::Exchange<'_>| dump_exchange(&dir, e),
        ));
    }
    let me = client.self_mri().await?;
    let s = CallSession::for_invitation(surl, &me, "ironteams", inv);
    match action {
        ListenAction::Report => {}
        ListenAction::RingOnly => {
            let links = if inv.links.get("acceptance").is_some() {
                inv.links.clone()
            } else {
                // Without an offer in the ring, ask for it: the answer shows whether it comes.
                let a = ngc
                    .attach_with(&s, inv, inv.offer.is_none())
                    .await
                    .context("attach")?;
                println!(
                    "attached; leg links [{}]; offer in the attach answer: {}",
                    a.links.0.keys().cloned().collect::<Vec<_>>().join(", "),
                    if a.offer.is_some() {
                        "present"
                    } else {
                        "missing"
                    },
                );
                if let Some(offer) = a.offer.as_ref().and_then(|o| o.decode().ok()) {
                    dump(opt(args, "--dump"), "caller-offer-attach", &offer);
                }
                a.links
            };
            match ngc.send_ringing(&s, &links).await {
                Ok(()) => println!("ringing reported; not answering (Ctrl-C to stop)"),
                Err(e) => println!("ringing not reported: {e}"),
            }
            let secs = opt(args, "--secs")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60);
            report_pushes(rx, secs).await;
        }
        ListenAction::Decline => {
            match decline(&ngc, &s, inv).await {
                Ok(()) => println!("declined"),
                Err(e) => println!("decline failed: {e}"),
            }
            report_pushes(rx, 5).await;
        }
        ListenAction::Accept => {
            let o = CallOpts::from_args(args, 30)?;
            let no_offer = no_offer_mode(args)?;
            if inv.offer.is_none() {
                println!("the ring carries no offer: {no_offer:?}");
            }
            let (ice, _turn_shim) = ice_servers(client, &ngc, args).await;
            let media_opts = media_options(args);
            let (media, events) =
                MediaSession::with_options(ice.clone(), media_opts.clone()).await?;
            let mut s = s;
            let links = answer_with_pushes(&ngc, &mut s, inv, &media, rx, no_offer, &o).await?;
            println!(
                "accepted; links [{}]",
                links.0.keys().cloned().collect::<Vec<_>>().join(", ")
            );
            let call = LiveCall {
                ngc,
                s,
                controller: calling::invitation::conversation_controller(inv).map(str::to_owned),
                links,
                media,
                events,
                ice,
                media_opts,
                answered: true,
                connected: true,
                roster: Roster::default(),
            };
            run_call(call, rx, o).await?;
        }
    }
    Ok(())
}

/// Runs [`calling::incoming::answer_invitation`] while forwarding our session's callbacks to it
/// (the caller's `mediaAnswer` when we offer) and writing every push and SDP to `--dump`.
async fn answer_with_pushes(
    ngc: &NgcClient,
    s: &mut CallSession,
    inv: &CallInvitation,
    media: &MediaSession,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<NgcEvent>,
    no_offer: NoOffer,
    o: &CallOpts,
) -> Result<Links> {
    let session = s.callbacks.session_id().to_owned();
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let dump_dir = o.dump_dir.clone();
    let observe = move |name: &str, text: &str| {
        if name == "step" {
            println!("answer: {text}");
            dump_text(dump_dir.as_deref(), "step", text);
        } else if name.ends_with("-blob") {
            dump_text(dump_dir.as_deref(), name, text);
        } else {
            println!("{name} ({} bytes)", text.len());
            dump(dump_dir.as_deref(), name, text);
        }
    };
    let drive =
        calling::incoming::answer_invitation(ngc, s, inv, media, &mut events, no_offer, &observe);
    tokio::pin!(drive);
    let mut n = 0u32;
    loop {
        tokio::select! {
            r = &mut drive => return r.context("answer"),
            ev = rx.recv() => {
                let Some(NgcEvent::Push(p)) = ev else { continue };
                n += 1;
                dump_push(o.dump_dir.as_deref(), &format!("answer-{n:03}"), &p.url, &p.body);
                match classify(&p.url, &p.body) {
                    NgcPush::Call { session_id, event } if session_id == session => {
                        println!("call callback: {event:?}");
                        let _ = tx.send(event);
                    }
                    NgcPush::Call { event, .. } => println!("other call's callback: {event:?}"),
                    NgcPush::Invitation(inv) => println!("{}", invitation_line(0, &inv)),
                    NgcPush::Other { url, .. } => println!("other push: {}", host(&url)),
                }
            }
        }
    }
}

/// `--no-offer <mode>` (or `--offer-in-attach`).
fn no_offer_mode(args: &[String]) -> Result<NoOffer> {
    if args.iter().any(|a| a == "--offer-in-attach") {
        return Ok(NoOffer::OfferInAttach);
    }
    match opt(args, "--no-offer") {
        None => Ok(NoOffer::default()),
        Some(m) => NoOffer::parse(m).with_context(|| {
            format!("--no-offer {m}: pick fetch, new-offer, accept-offer or attach-offer")
        }),
    }
}

fn dump_text(dir: Option<&str>, name: &str, text: &str) {
    if let Some(d) = dir {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let _ = std::fs::write(format!("{d}/{n}-{name}.txt"), text);
    }
}

/// One request to the conversation service and its answer, as `<ms>-ngc-<op>.json`.
fn dump_exchange(dir: &str, e: &calling::signaling::Exchange<'_>) {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let doc = serde_json::json!({
        "op": e.op, "method": e.method, "url": e.url, "status": e.status, "request": e.request,
        "response": serde_json::from_str::<Value>(e.response)
            .unwrap_or_else(|_| Value::String(e.response.to_owned())),
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
        let _ = std::fs::write(format!("{dir}/{n}-ngc-{}.json", e.op), bytes);
    }
}

fn dump_push(dir: Option<&str>, name: &str, url: &str, body: &Value) {
    if let Some(d) = dir {
        let doc = serde_json::json!({"url": url, "body": body});
        if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
            let _ = std::fs::write(format!("{d}/push-{name}.json"), bytes);
        }
    }
}

/// The MRI and name to call: an MRI as given, or the directory entry whose email is `target`.
async fn resolve_callee(client: &Client, target: &str) -> Result<(String, Option<String>)> {
    if is_mri(target) {
        return Ok((target.to_owned(), None));
    }
    if !target.contains('@') {
        bail!("call dial takes an MRI (8:orgid:<oid>) or an email address");
    }
    let me = client.self_mri().await?;
    let people = client.search_people(&me, target).await?;
    let p = people
        .into_iter()
        .find(|p| {
            p.email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(target))
        })
        .with_context(|| format!("nobody in the directory has the email {target}"))?;
    Ok((p.mri, Some(p.display_name)))
}

/// `8:orgid:<oid>`, `8:live:...`, `28:<bot>`: a type number, a colon, then the id.
fn is_mri(s: &str) -> bool {
    s.split_once(':').is_some_and(|(t, rest)| {
        !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty()
    })
}

/// `call dial <mri|email> [--secs N] [--ring-secs N]`: places an outgoing 1:1 audio call, prints
/// its progress (ringing, connected, ended), runs it like `join` for `--secs` after it connects
/// (the audio flags of `join` apply), then hangs up. An unanswered call is cancelled after
/// `--ring-secs` (default 60); Ctrl-C cancels or hangs up at any time. UNVERIFIED live.
async fn dial(client: &Client, args: &[String]) -> Result<()> {
    let target = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .context("usage: ironteams-cli call dial <mri|email> [--secs N] [--ring-secs N]")?;
    anyhow::ensure!(
        !calling::emergency::is_emergency_number(target),
        calling::emergency::REFUSAL
    );
    let mut o = CallOpts::from_args(args, 30)?;
    o.ring_timeout = Some(Duration::from_secs(
        opt(args, "--ring-secs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(60),
    ));
    let (mri, name) = resolve_callee(client, target).await?;
    println!("calling {}", name.as_deref().unwrap_or("the given MRI"));
    let ngc = NgcClient::discover(client.clone()).await?;
    let (trouter, mut pushes, surl) = calling_trouter(client).await?;
    let (ice, _turn_shim) = ice_servers(client, &ngc, args).await;
    let media_opts = media_options(args);
    let (media, events) = MediaSession::with_options(ice.clone(), media_opts.clone()).await?;
    let offer = media.create_offer().await?;
    dump(o.dump_dir.as_deref(), "our-offer", &offer);
    let me = client.self_mri().await?;
    let s = CallSession::new(&surl, &me, "ironteams");
    let callee = Participant {
        mri,
        display_name: name,
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
        .await
        .context("place call")?;
    println!(
        "placed: controller {}; waiting for an answer (Ctrl-C cancels)",
        host(&conv.controller)
    );
    let answered = apply_inline_answer(&media, &conv.raw, o.dump_dir.as_deref()).await;
    let call = LiveCall {
        ngc,
        s,
        controller: Some(conv.controller.clone()),
        links: conv.links.clone(),
        media,
        events,
        ice,
        media_opts,
        answered,
        connected: false,
        roster: Roster::default(),
    };
    run_call(call, &mut pushes, o).await?;
    drop(trouter);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn scheduled_controls_are_read_from_every_flag_earliest_first() {
        let got = scheduled_controls(&args(&[
            "--secs",
            "90",
            "--hand-at",
            "40",
            "--mute-at",
            "20",
            "--unmute-at",
            "30.5",
            "--mute-at",
            "50",
            "--lower-hand-at",
            "45",
        ]))
        .unwrap();
        assert_eq!(
            got,
            vec![
                (Duration::from_secs(20), Control::Mute),
                (Duration::from_secs_f64(30.5), Control::Unmute),
                (Duration::from_secs(40), Control::RaiseHand),
                (Duration::from_secs(45), Control::LowerHand),
                (Duration::from_secs(50), Control::Mute),
            ]
        );
        assert!(
            scheduled_controls(&args(&["--secs", "30"]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_control_flag_without_seconds_is_an_error() {
        assert!(scheduled_controls(&args(&["--mute-at"])).is_err());
        assert!(scheduled_controls(&args(&["--hand-at", "soon"])).is_err());
        assert!(scheduled_controls(&args(&["--unmute-at", "-1"])).is_err());
    }

    #[test]
    fn the_no_offer_mode_is_read_from_the_flags() {
        assert_eq!(no_offer_mode(&args(&[])).unwrap(), NoOffer::FetchOffer);
        assert_eq!(
            no_offer_mode(&args(&["--no-offer", "new-offer"])).unwrap(),
            NoOffer::NewOffer
        );
        assert_eq!(
            no_offer_mode(&args(&["--offer-in-attach"])).unwrap(),
            NoOffer::OfferInAttach
        );
        assert!(no_offer_mode(&args(&["--no-offer", "nope"])).is_err());
    }

    #[test]
    fn listen_takes_at_most_one_action() {
        assert_eq!(
            listen_action(&args(&["--secs", "60"])).unwrap(),
            ListenAction::Report
        );
        assert_eq!(
            listen_action(&args(&["--ring-only"])).unwrap(),
            ListenAction::RingOnly
        );
        assert_eq!(
            listen_action(&args(&["--decline"])).unwrap(),
            ListenAction::Decline
        );
        assert_eq!(
            listen_action(&args(&["--accept"])).unwrap(),
            ListenAction::Accept
        );
        assert_eq!(
            listen_action(&args(&["--accept-audio", "--secs", "20"])).unwrap(),
            ListenAction::Accept
        );
        assert!(listen_action(&args(&["--accept", "--decline"])).is_err());
    }

    #[test]
    fn a_callee_is_an_mri_or_an_email() {
        assert!(is_mri("8:orgid:00000000-0000-0000-0000-000000000001"));
        assert!(is_mri("8:live:.cid.abc"));
        assert!(is_mri("28:bot"));
        assert!(!is_mri("ada@example.com"));
        assert!(!is_mri("orgid:x"));
        assert!(!is_mri("8:"));
    }

    #[test]
    fn call_options_default_to_thirty_seconds_and_a_tone() {
        let o = CallOpts::from_args(&args(&["8:orgid:x"]), 30).unwrap();
        assert_eq!(o.secs, 30);
        assert!(!o.mic && !o.speaker && o.ring_timeout.is_none());
        assert_eq!(o.out, "ironteams-call.wav");
        let o = CallOpts::from_args(&args(&["--secs", "5", "--mic", "--speaker"]), 30).unwrap();
        assert_eq!(o.secs, 5);
        assert!(o.mic && o.speaker);
    }

    use calling::history::{CallDirection, CallOutcome, CallRecord};

    #[test]
    fn a_history_line_shows_time_direction_outcome_duration_and_party() {
        let r = CallRecord {
            id: "c".into(),
            start_ms: 1_794_912_696_849, // 2026-11-17 10:51:36 UTC
            end_ms: None,
            duration_secs: Some(7129),
            direction: CallDirection::Incoming,
            outcome: CallOutcome::Accepted,
            other_mri: Some("8:orgid:x".into()),
            other_name: Some("Ada".into()),
            is_group: false,
            thread_id: None,
            participants: vec![],
            voicemail_message_id: None,
        };
        assert_eq!(
            history_line(&r),
            "2026-11-17 10:51 in  answered   1:58:49  Ada"
        );
        let missed = CallRecord {
            direction: CallDirection::Outgoing,
            outcome: CallOutcome::Missed,
            duration_secs: None,
            other_name: None,
            ..r
        };
        assert_eq!(
            history_line(&missed),
            "2026-11-17 10:51 out no answer        -  8:orgid:x"
        );
    }
}
