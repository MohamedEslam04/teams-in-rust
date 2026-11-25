//! IronTeams's own trouter connection for calling, registered only as `NextGenCalling`.
//!
//! Call invitations are pushed to `<surl>NGCallManagerWin`; the callbacks of a call in progress
//! arrive under `<surl>callAgent/...`, URLs we publish ourselves (see
//! [`crate::signaling::CallbackLinks`]). Every push is acked; a `call/acceptance` callback is
//! acked with a `callAcceptanceAcknowledgement` body, without which Teams times the acceptance out
//! (mautrix-teams `calls.go`, 408/10056; rusty-teams `engine.rs` does the same).
//!
//! The allocation/connect flow is teams-proto's `trouter.rs` (same project and licence), which
//! keeps it private; the frame helpers are reused from it.

use crate::signaling::CallbackLinks;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;
use teams_proto::Client;
use teams_proto::client_info::{CLIENT_VERSION, TROUTER_TCCV, USER_AGENT};
use teams_proto::trouter::{Frame, activity_event, parse_frame, registration_body, retry_after};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{
    Message as WsMessage, client::IntoClientRequest, http::HeaderValue,
};

/// `clientDescription.appId` of the calling registration.
pub const APP_ID: &str = "NextGenCalling";
/// `templateKey`: ost registers `DesktopNgc_2.5:SkypeNgc`; teams-proto and mautrix-teams send
/// `DesktopNgc_2.3:SkypeNgc`. Which one the live service prefers is unconfirmed.
pub const TEMPLATE_KEY: &str = "DesktopNgc_2.5:SkypeNgc";
/// Appended to the surl to form the registration's transport path.
pub const PATH_SUFFIX: &str = "NGCallManagerWin";
/// Registration lifetime (seconds); re-registered ten minutes before it lapses.
pub const REGISTRATION_TTL: u64 = 86400;
const REGISTRAR_URL: &str = "https://teams.microsoft.com/registrar/prod/V2/registrations";
/// Pushed bodies (a 1000-person roster is ~4.5 MB) are capped when inflated.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// Base64 of gzip's magic and method (1f 8b 08). No JSON starts with `H`.
const GZIP_BASE64_MAGIC: &str = "H4sI";

/// The registration body for our calling endpoint.
pub fn registration(surl: &str, registration_id: &str) -> Value {
    registration_body(
        APP_ID,
        TEMPLATE_KEY,
        &format!("{surl}{PATH_SUFFIX}"),
        registration_id,
    )
}

/// A request pushed to us, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct CallPush {
    pub id: i64,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    /// The JSON body: plain, gzip+base64 (with or without the content-encoding header), with any
    /// `cp` (deflated) / `gp` (base64) wrapper unpacked. `Null` when empty or unreadable.
    pub body: Value,
}

fn b64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .ok()
}

fn read_capped(r: impl Read) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    r.take(MAX_BODY_BYTES + 1).read_to_end(&mut out).ok()?;
    (out.len() as u64 <= MAX_BODY_BYTES).then_some(out)
}

fn gunzip(data: &[u8]) -> Option<Vec<u8>> {
    read_capped(flate2::read::GzDecoder::new(data))
}

// Adapted from rusty-teams (MIT), crates/calling/src/push.rs `inflated_json`.
/// `cp` has been seen as zlib, raw deflate and gzip.
fn inflate_json(data: &[u8]) -> Option<Value> {
    let parse = |bytes: Option<Vec<u8>>| serde_json::from_slice::<Value>(&bytes?).ok();
    parse(read_capped(flate2::read::ZlibDecoder::new(data)))
        .or_else(|| parse(read_capped(flate2::read::DeflateDecoder::new(data))))
        .or_else(|| parse(gunzip(data)))
}

fn decode_body(raw: &str, gzip_header: bool) -> Option<Value> {
    let text = if gzip_header || raw.starts_with(GZIP_BASE64_MAGIC) {
        String::from_utf8(gunzip(&b64(raw)?)?).ok()?
    } else {
        raw.to_owned()
    };
    if text.trim().is_empty() {
        return Some(Value::Null);
    }
    let body: Value = serde_json::from_str(&text).ok()?;
    if let Some(cp) = body.get("cp").and_then(Value::as_str) {
        return inflate_json(&b64(cp)?);
    }
    if let Some(gp) = body.get("gp").and_then(Value::as_str) {
        return serde_json::from_slice(&b64(gp)?).ok();
    }
    Some(body)
}

/// Decodes a `3:::` frame's JSON. `None` only when it has no request id (nothing to ack).
pub fn decode_call_push(data: &str) -> Option<CallPush> {
    let v: Value = serde_json::from_str(data).ok()?;
    let id = v.get("id")?.as_i64()?;
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let headers: BTreeMap<String, String> = v
        .get("headers")
        .and_then(Value::as_object)
        .map(|h| {
            h.iter()
                .map(|(k, v)| {
                    let v = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
                    (k.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default();
    let gzip = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("X-Microsoft-Skype-Content-Encoding")
            && v.eq_ignore_ascii_case("gzip")
    });
    let raw = v.get("body").and_then(Value::as_str).unwrap_or_default();
    let body = decode_body(raw, gzip).unwrap_or_else(|| {
        tracing::warn!(url = %teams_proto::logsafe::url(&url), "calling push body could not be decoded");
        Value::Null
    });
    Some(CallPush {
        id,
        url,
        headers,
        body,
    })
}

fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The session id of one of our `call/acceptance` callback URLs.
fn acceptance_session(url: &str) -> Option<String> {
    let p = crate::invitation::parse_callback_path(url)?;
    (p.scope == "call" && p.event == "acceptance").then_some(p.session_id)
}

/// The reply to a push. Callbacks echo the routing headers (chain id, message id,
/// `trouter-request`, `MS-CV` + `.0`); a `call/acceptance` callback also carries our
/// acknowledgement with the links of the accepted call, built on the same session (rusty-teams
/// `native.rs` `echoed_headers` and mautrix-teams `acceptanceReply` agree on this).
pub fn reply_frame(push: &CallPush, surl: &str) -> String {
    if crate::invitation::parse_callback_path(&push.url).is_none() {
        return format!(
            "3:::{}",
            json!({"id": push.id, "status": 200, "headers": {}, "body": ""})
        );
    }
    let mut headers = Map::new();
    for name in [
        "X-Microsoft-Skype-Chain-ID",
        "X-Microsoft-Skype-Message-ID",
        "trouter-request",
    ] {
        if let Some(v) = header(&push.headers, name) {
            headers.insert(name.to_owned(), json!(v));
        }
    }
    if let Some(cv) = header(&push.headers, "MS-CV") {
        headers.insert("MS-CV".into(), json!(format!("{cv}.0")));
    }
    let body = acceptance_session(&push.url).map_or_else(String::new, |session| {
        let links = CallbackLinks::new(surl, &session).acceptance_links();
        json!({"callAcceptanceAcknowledgement": {"links": links}}).to_string()
    });
    format!(
        "3:::{}",
        json!({"id": push.id, "status": 200, "headers": headers, "body": body})
    )
}

/// The ack for a `5:<id>[+]::` event frame (`6:<id>::`, as ost and mautrix-teams send it); trouter
/// redelivers event frames that are not acked. `None` for frames without an id.
pub fn event_ack(frame: &str) -> Option<String> {
    let rest = frame.strip_prefix("5:")?;
    let id = rest.split(':').next()?.trim_end_matches('+');
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| format!("6:{id}::"))
}

/// What the calling trouter task reports.
#[derive(Debug, Clone, PartialEq)]
pub enum NgcEvent {
    /// Connected and registered; callback links must be built on this `surl`.
    Connected {
        surl: String,
    },
    Disconnected(String),
    /// A pushed request (already acked).
    Push(CallPush),
}

/// The running calling trouter. Dropping it stops the connection.
pub struct NgcTrouter {
    surl: watch::Receiver<Option<String>>,
    task: tokio::task::JoinHandle<()>,
}

impl NgcTrouter {
    /// Starts the connection on the current tokio runtime. `epid` is the endpoint id of this
    /// connection: keep it stable across runs (Teams caps endpoints per account) and distinct
    /// from the chat trouter's.
    pub fn spawn(client: Client, epid: String) -> (NgcTrouter, mpsc::UnboundedReceiver<NgcEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (surl_tx, surl) = watch::channel(None);
        let task = tokio::spawn(run(client, epid, surl_tx, tx));
        (NgcTrouter { surl, task }, rx)
    }

    /// The current surl, `None` while disconnected.
    pub fn surl(&self) -> Option<String> {
        self.surl.borrow().clone()
    }

    /// A watch on the surl (it changes on reconnect, which invalidates a live call's links).
    pub fn surl_watch(&self) -> watch::Receiver<Option<String>> {
        self.surl.clone()
    }

    /// Waits until connected and returns the surl. `None` if the task ended.
    pub async fn connected_surl(&self) -> Option<String> {
        let mut w = self.surl.clone();
        w.wait_for(Option::is_some).await.ok()?.clone()
    }
}

impl Drop for NgcTrouter {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Runs the connection forever, reconnecting with backoff (1 s doubling to 60 s, reset after a
/// connection that lasted two minutes). Ends when `tx`'s receiver is dropped.
pub async fn run(
    client: Client,
    epid: String,
    surl: watch::Sender<Option<String>>,
    tx: mpsc::UnboundedSender<NgcEvent>,
) {
    let mut backoff = 1u64;
    loop {
        let started = tokio::time::Instant::now();
        let reason = match connect_once(&client, &epid, &surl, &tx).await {
            Ok(()) => "closed".to_owned(),
            Err(e) => e.to_string(),
        };
        surl.send_replace(None);
        if tx.send(NgcEvent::Disconnected(reason.clone())).is_err() {
            return;
        }
        tracing::warn!(%reason, "calling trouter disconnected");
        if started.elapsed() > Duration::from_secs(120) {
            backoff = 1;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}

/// Endpoint allocation returned by `/v4/a`.
struct Allocation {
    socketio: String,
    surl: String,
    ccid: String,
    connectparams: Map<String, Value>,
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn connect_query(a: &Allocation, epid: &str) -> String {
    let mut q = String::from("v=v4&");
    for (k, v) in &a.connectparams {
        let val = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
        q.push_str(&format!("{}={}&", enc(k), enc(&val)));
    }
    let tc =
        json!({"cv": TROUTER_TCCV, "ua": "TeamsCDL", "hr": "", "v": CLIENT_VERSION}).to_string();
    let con_num = format!("{}_1", teams_proto::time::now_ms());
    q.push_str(&format!(
        "tc={}&con_num={}&epid={}&ccid={}&auth=true&timeout=40&",
        enc(&tc),
        enc(&con_num),
        enc(epid),
        enc(&a.ccid)
    ));
    q
}

async fn allocate(client: &Client, epid: &str) -> teams_proto::Result<Allocation> {
    let url = format!(
        "https://go.trouter.teams.microsoft.com/v4/a?epid={}",
        enc(epid)
    );
    let v = client
        .json("ngc_trouter_alloc", reqwest::Method::POST, &url, None)
        .await?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Ok(Allocation {
        socketio: s("socketio").unwrap_or_else(|| "https://go.trouter.teams.microsoft.com/".into()),
        surl: s("surl")
            .ok_or_else(|| teams_proto::Error::Protocol("trouter allocation has no surl".into()))?,
        ccid: s("ccid").unwrap_or_default(),
        connectparams: v
            .get("connectparams")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
    })
}

/// Registers; on failure returns how long to wait before retrying (never fatal).
async fn register_or_retry(client: &Client, surl: &str, registration_id: &str) -> Option<Duration> {
    let body = registration(surl, registration_id);
    match client
        .json(
            "ngc_register",
            reqwest::Method::POST,
            REGISTRAR_URL,
            Some(&body),
        )
        .await
    {
        Ok(_) => None,
        Err(teams_proto::Error::Status {
            status: 429, body, ..
        }) => {
            let wait =
                retry_after(&body).unwrap_or(Duration::from_secs(120)) + Duration::from_secs(2);
            tracing::warn!(?wait, "calling registration throttled");
            Some(wait)
        }
        Err(e) => {
            tracing::warn!(%e, "calling registration failed");
            Some(Duration::from_secs(60))
        }
    }
}

fn ws_err(e: tokio_tungstenite::tungstenite::Error) -> teams_proto::Error {
    teams_proto::Error::Ws(Box::new(e))
}

async fn connect_once(
    client: &Client,
    epid: &str,
    surl_tx: &watch::Sender<Option<String>>,
    tx: &mpsc::UnboundedSender<NgcEvent>,
) -> teams_proto::Result<()> {
    client.ensure_fresh().await?;
    let alloc = allocate(client, epid).await?;
    let query = connect_query(&alloc, epid);
    let base = if alloc.socketio.ends_with('/') {
        alloc.socketio.clone()
    } else {
        format!("{}/", alloc.socketio)
    };
    let (status, hs) = client
        .request(
            reqwest::Method::GET,
            &format!("{base}socket.io/1/?{query}"),
            None,
            &[],
        )
        .await?;
    if status != 200 {
        return Err(teams_proto::Error::status(
            "ngc_trouter_handshake",
            status,
            &hs,
        ));
    }
    let sid = hs.split(':').next().unwrap_or_default().to_owned();
    if sid.is_empty() {
        return Err(teams_proto::Error::Protocol(
            "trouter handshake returned no session id".into(),
        ));
    }
    let ws_url = format!(
        "{}socket.io/1/websocket/{sid}?{query}",
        base.replacen("https://", "wss://", 1)
    );
    let mut req = ws_url.into_client_request().map_err(ws_err)?;
    let tokens = client.tokens().await;
    let hv =
        |s: &str| HeaderValue::from_str(s).map_err(|e| teams_proto::Error::Protocol(e.to_string()));
    req.headers_mut().insert("User-Agent", hv(USER_AGENT)?);
    if let Some(sk) = &tokens.skype {
        req.headers_mut().insert("X-Skypetoken", hv(sk)?);
    }
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(ws_err)?;
    let (mut sink, mut stream) = ws.split();
    let registration_id = uuid::Uuid::new_v4().to_string();
    let mut seq: u64 = 0;
    let mut next = |payload: Value| {
        seq += 1;
        format!("5:{seq}+::{payload}")
    };
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    ping.tick().await;
    let mut active = teams_proto::trouter::user_activity();
    let mut ready = false;
    let renew = Duration::from_secs(REGISTRATION_TTL - 600);
    let mut last_loss_register: Option<tokio::time::Instant> = None;
    let reregister = tokio::time::sleep(renew);
    tokio::pin!(reregister);
    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(msg) = msg else { return Ok(()) };
                let text = match msg.map_err(ws_err)? {
                    WsMessage::Text(t) => t.to_string(),
                    WsMessage::Close(_) => return Ok(()),
                    _ => continue,
                };
                match parse_frame(&text) {
                    Some(Frame::Connect) => {
                        let auth = json!({"name": "user.authenticate", "args": [{
                            "headers": {"X-Ms-Test-User": "False", "Authorization": format!("Bearer {}", tokens.spaces.clone().unwrap_or_default()), "X-MS-Migration": "True"},
                            "connectparams": alloc.connectparams,
                        }]});
                        sink.send(WsMessage::text(format!("5:::{auth}"))).await.map_err(ws_err)?;
                        // The user's activity as the presence connection tells it (an "active"
                        // here forever would keep the user Available).
                        let now = *active.borrow_and_update();
                        sink.send(WsMessage::text(next(activity_event(now)))).await.map_err(ws_err)?;
                        ready = true;
                        let wait = register_or_retry(client, &alloc.surl, &registration_id).await.unwrap_or(renew);
                        reregister.as_mut().reset(tokio::time::Instant::now() + wait);
                        surl_tx.send_replace(Some(alloc.surl.clone()));
                        if tx.send(NgcEvent::Connected { surl: alloc.surl.clone() }).is_err() { return Ok(()); }
                    }
                    Some(Frame::Message(data)) => {
                        if let Some(push) = decode_call_push(&data) {
                            sink.send(WsMessage::text(reply_frame(&push, &alloc.surl))).await.map_err(ws_err)?;
                            if tx.send(NgcEvent::Push(push)).is_err() { return Ok(()); }
                        }
                    }
                    Some(Frame::Event(data)) => {
                        if let Some(ack) = event_ack(&text) {
                            sink.send(WsMessage::text(ack)).await.map_err(ws_err)?;
                        }
                        // The server dropped pushes: re-register, at most once a minute.
                        if data.contains("trouter.message_loss")
                            && last_loss_register.is_none_or(|t| t.elapsed() > Duration::from_secs(60))
                        {
                            last_loss_register = Some(tokio::time::Instant::now());
                            let wait = register_or_retry(client, &alloc.surl, &registration_id).await.unwrap_or(renew);
                            reregister.as_mut().reset(tokio::time::Instant::now() + wait);
                        }
                    }
                    Some(Frame::Disconnect) => return Ok(()),
                    _ => {}
                }
            }
            _ = ping.tick() => {
                sink.send(WsMessage::text(next(json!({"name": "ping"})))).await.map_err(ws_err)?;
            }
            Ok(()) = active.changed(), if ready => {
                let now = *active.borrow_and_update();
                sink.send(WsMessage::text(next(activity_event(now)))).await.map_err(ws_err)?;
                tracing::debug!(active = now, "calling trouter: user activity sent");
            }
            _ = &mut reregister => {
                let wait = register_or_retry(client, &alloc.surl, &registration_id).await.unwrap_or(renew);
                reregister.as_mut().reset(tokio::time::Instant::now() + wait);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teams_proto::trouter::ack_frame;

    #[test]
    fn connect_query_shape() {
        let mut cp = Map::new();
        cp.insert("sr".into(), json!("abc"));
        let a = Allocation {
            socketio: "https://s/".into(),
            surl: "https://s/v4/f/x/".into(),
            ccid: "cc".into(),
            connectparams: cp,
        };
        let q = connect_query(&a, "ep");
        assert!(q.starts_with("v=v4&sr=abc&tc="));
        assert!(q.contains("&epid=ep&ccid=cc&auth=true&timeout=40&"));
    }

    #[test]
    fn plain_ack_matches_teams_proto_shape() {
        // Our reply for a non-callback push carries the same fields as teams-proto's ack.
        let ours: Value = serde_json::from_str(
            reply_frame(
                &CallPush {
                    id: 4,
                    url: "https://t/NGCallManagerWin".into(),
                    headers: BTreeMap::new(),
                    body: Value::Null,
                },
                "https://t/",
            )
            .strip_prefix("3:::")
            .unwrap(),
        )
        .unwrap();
        let theirs: Value =
            serde_json::from_str(ack_frame(4).strip_prefix("3:::").unwrap()).unwrap();
        for k in ["id", "status", "body"] {
            assert_eq!(ours[k], theirs[k]);
        }
    }
}
