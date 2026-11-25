//! Trouter: Teams' real-time push channel (socket.io v0 over a WebSocket).
//!
//! Flow: allocate an endpoint (`/v4/a`), socket.io handshake, open the WebSocket, authenticate,
//! register three apps with the registrar, then acknowledge every pushed request.

use crate::client::Client;
use crate::client_info::{CLIENT_VERSION, TROUTER_TCCV, USER_AGENT};
use crate::error::{Error, Result};
use crate::model::{Event, events_from_payload, presence_events};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::{
    Message as WsMessage, client::IntoClientRequest, http::HeaderValue,
};

/// What the trouter task reports to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum TrouterEvent {
    Connected,
    /// The server dropped events; refetch recent history.
    Resync,
    Disconnected(String),
    Event(Box<Event>),
}

/// A socket.io v0 frame (`type:id:endpoint:data`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Disconnect,
    Connect,
    Heartbeat,
    /// `3` — a pushed request (JSON with id/url/headers/body).
    Message(String),
    /// `5` — an event (JSON with name/args).
    Event(String),
    /// `6` — ack of one of our sequenced frames.
    Ack(String),
    Error(String),
    Noop,
}

/// Parses one socket.io v0 frame. Data may itself contain `:`.
pub fn parse_frame(s: &str) -> Option<Frame> {
    let mut parts = s.splitn(4, ':');
    let kind = parts.next()?;
    let _id = parts.next().unwrap_or_default();
    let _endpoint = parts.next().unwrap_or_default();
    let data = parts.next().unwrap_or_default().to_string();
    Some(match kind {
        "0" => Frame::Disconnect,
        "1" => Frame::Connect,
        "2" => Frame::Heartbeat,
        "3" => Frame::Message(data),
        "5" => Frame::Event(data),
        "6" => Frame::Ack(data),
        "7" => Frame::Error(data),
        "8" => Frame::Noop,
        _ => return None,
    })
}

/// A pushed request after decoding.
#[derive(Debug, Clone, PartialEq)]
pub struct PushRequest {
    pub id: i64,
    pub url: String,
    pub body: Value,
}

fn gunzip(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![];
    flate2::read::GzDecoder::new(data)
        .take(32 * 1024 * 1024)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

fn b64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .ok()
}

/// Decodes a `3:::` frame's JSON. Handles gzip content encoding and `cp`/`gp` wrapped bodies.
pub fn decode_push(data: &str) -> Option<PushRequest> {
    let v: Value = serde_json::from_str(data).ok()?;
    let id = v.get("id")?.as_i64()?;
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let raw = v.get("body").and_then(Value::as_str).unwrap_or_default();
    let gz = v
        .get("headers")
        .and_then(|h| h.as_object())
        .and_then(|h| {
            h.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("X-Microsoft-Skype-Content-Encoding"))
        })
        .and_then(|(_, val)| val.as_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gzip"));
    let text = if gz {
        String::from_utf8(gunzip(&b64(raw)?)?).ok()?
    } else {
        raw.to_string()
    };
    let mut body: Value = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).ok()?
    };
    if let Some(cp) = body.get("cp").and_then(Value::as_str) {
        body = serde_json::from_slice(&gunzip(&b64(cp)?)?).ok()?;
    } else if let Some(gp) = body.get("gp").and_then(Value::as_str) {
        body = serde_json::from_slice(&b64(gp)?).ok()?;
    }
    Some(PushRequest { id, url, body })
}

/// Ack frame for a pushed request.
pub fn ack_frame(id: i64) -> String {
    format!("3:::{}", json!({"id": id, "status": 200, "body": ""}))
}

/// Events carried by a decoded push.
pub fn events_from_push(p: &PushRequest) -> Vec<Event> {
    let path = p.url.rsplit('/').next().unwrap_or_default();
    if path == "messaging" {
        events_from_payload(&p.body)
    } else if path.contains("resence") {
        presence_events(&p.body)
    } else {
        vec![]
    }
}

/// Endpoint allocation returned by `/v4/a`.
#[derive(Debug, Clone)]
struct Allocation {
    socketio: String,
    surl: String,
    ccid: String,
    connectparams: serde_json::Map<String, Value>,
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn connect_query(a: &Allocation, epid: &str) -> String {
    let mut q = String::from("v=v4&");
    for (k, v) in &a.connectparams {
        let val = v
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string());
        q.push_str(&format!("{}={}&", enc(k), enc(&val)));
    }
    let tc =
        json!({"cv": TROUTER_TCCV, "ua": "TeamsCDL", "hr": "", "v": CLIENT_VERSION}).to_string();
    let con_num = format!("{}_1", crate::time::now_ms());
    q.push_str(&format!(
        "tc={}&con_num={}&epid={}&ccid={}&auth=true&timeout=40&",
        enc(&tc),
        enc(&con_num),
        enc(epid),
        enc(&a.ccid)
    ));
    q
}

/// Registration body for one app.
pub fn registration_body(app_id: &str, template: &str, path: &str, registration_id: &str) -> Value {
    json!({
        "clientDescription": {
            "appId": app_id, "aesKey": "", "languageId": "en-US", "platform": "edge",
            "templateKey": template, "platformUIVersion": CLIENT_VERSION
        },
        "registrationId": registration_id,
        "nodeId": "",
        "transports": {"TROUTER": [{"context": "", "path": path, "ttl": 86400}]}
    })
}

async fn allocate(client: &Client) -> Result<Allocation> {
    let url = format!(
        "https://go.trouter.teams.microsoft.com/v4/a?epid={}",
        enc(client.epid())
    );
    let v = client
        .json("trouter_alloc", reqwest::Method::POST, &url, None)
        .await?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Ok(Allocation {
        socketio: s("socketio").unwrap_or_else(|| "https://go.trouter.teams.microsoft.com/".into()),
        surl: s("surl").ok_or_else(|| Error::Protocol("trouter allocation has no surl".into()))?,
        ccid: s("ccid").unwrap_or_default(),
        connectparams: v
            .get("connectparams")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
    })
}

/// Parses `retryAfter` (`"00:02:52.0065811"`) from a throttling response.
pub fn retry_after(body: &str) -> Option<Duration> {
    let v: Value = serde_json::from_str(body).ok()?;
    let s = v.get("retryAfter")?.as_str()?;
    let mut parts = s.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(Duration::from_secs_f64(h * 3600.0 + m * 60.0 + sec))
}

/// Registers, returning how long to wait before retrying when it fails (never fatal: the socket
/// stays open and registration is retried).
async fn register_or_retry(client: &Client, surl: &str) -> Option<Duration> {
    match register(client, surl).await {
        Ok(()) => None,
        Err(Error::Status {
            status: 429, body, ..
        }) => {
            let wait =
                retry_after(&body).unwrap_or(Duration::from_secs(120)) + Duration::from_secs(2);
            tracing::warn!(?wait, "trouter registration throttled");
            Some(wait)
        }
        Err(e) => {
            tracing::warn!(%e, "trouter registration failed");
            Some(Duration::from_secs(60))
        }
    }
}

async fn register(client: &Client, surl: &str) -> Result<()> {
    let regs = [
        (
            "NextGenCalling",
            "DesktopNgc_2.3:SkypeNgc",
            format!("{surl}NGCallManagerWin"),
            uuid::Uuid::new_v4().to_string(),
        ),
        (
            "SkypeSpacesWeb",
            "SkypeSpacesWeb_2.3",
            format!("{surl}SkypeSpacesWeb"),
            uuid::Uuid::new_v4().to_string(),
        ),
        // Must be last: this registration delivers chat events.
        (
            "TeamsCDLWebWorker",
            "TeamsCDLWebWorker_2.1",
            surl.to_string(),
            client.epid().to_string(),
        ),
    ];
    for (app, template, path, reg_id) in regs {
        let body = registration_body(app, template, &path, &reg_id);
        client
            .json(
                "trouter_register",
                reqwest::Method::POST,
                "https://teams.microsoft.com/registrar/prod/V2/registrations",
                Some(&body),
            )
            .await?;
    }
    Ok(())
}

fn activity_cv() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{}.0.1", &id[..22])
}

/// Whether the user is at the keyboard, as the trouter connections tell the presence service.
/// This device's presence endpoint is registered with `activityReporting: "Transport"`
/// (`presence::endpoint_body`), so the service takes its activity from the trouter's
/// `user.activity` events, as Teams web does (rusty-teams `realtime/native.rs`): an inactive here
/// is what turns the user Away. One value for the process: the keyboard belongs to the machine.
static USER_ACTIVE: LazyLock<watch::Sender<bool>> = LazyLock::new(|| watch::Sender::new(true));

/// Records whether the user is at the keyboard; a change goes out on every trouter connection at
/// once, and the current value on every (re)connect. The same value again sends nothing.
pub fn set_user_active(active: bool) {
    USER_ACTIVE.send_if_modified(|now| std::mem::replace(now, active) != active);
}

/// Follows [`set_user_active`] (for the calling crate's own trouter connection).
pub fn user_activity() -> watch::Receiver<bool> {
    USER_ACTIVE.subscribe()
}

/// The `user.activity` event telling the service whether the user is active.
pub fn activity_event(active: bool) -> Value {
    let state = if active { "active" } else { "inactive" };
    json!({"name": "user.activity", "args": [{"state": state, "cv": activity_cv()}]})
}

/// Runs the trouter connection forever, reconnecting with backoff. Events go to `tx`; the task
/// ends when the receiver is dropped.
pub async fn run(client: Client, tx: mpsc::UnboundedSender<TrouterEvent>) {
    let mut backoff = 1u64;
    loop {
        let started = tokio::time::Instant::now();
        let reason = match connect_once(&client, &tx).await {
            Ok(()) => "closed".to_string(),
            Err(e) => e.to_string(),
        };
        if tx.send(TrouterEvent::Disconnected(reason.clone())).is_err() {
            return;
        }
        tracing::warn!(%reason, "trouter disconnected");
        if started.elapsed() > Duration::from_secs(120) {
            backoff = 1;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}

async fn connect_once(client: &Client, tx: &mpsc::UnboundedSender<TrouterEvent>) -> Result<()> {
    client.ensure_fresh().await?;
    let alloc = allocate(client).await?;
    let query = connect_query(&alloc, client.epid());
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
        return Err(Error::status("trouter_handshake", status, &hs));
    }
    let sid = hs.split(':').next().unwrap_or_default().to_string();
    if sid.is_empty() {
        return Err(Error::Protocol(
            "trouter handshake returned no session id".into(),
        ));
    }
    let ws_url = format!(
        "{}socket.io/1/websocket/{sid}?{query}",
        base.replacen("https://", "wss://", 1)
    );
    let mut req = ws_url
        .into_client_request()
        .map_err(|e| Error::Ws(Box::new(e)))?;
    let tokens = client.tokens().await;
    let hv = |s: &str| HeaderValue::from_str(s).map_err(|e| Error::Protocol(e.to_string()));
    req.headers_mut().insert("User-Agent", hv(USER_AGENT)?);
    if let Some(sk) = &tokens.skype {
        req.headers_mut().insert("X-Skypetoken", hv(sk)?);
    }
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| Error::Ws(Box::new(e)))?;
    let (mut sink, mut stream) = ws.split();
    let mut seq: u64 = 0;
    let mut next = |payload: Value| {
        seq += 1;
        format!("5:{seq}+::{payload}")
    };
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    ping.tick().await;
    let mut active = user_activity();
    // Activity goes out once authenticated; a change before then is sent with the connect.
    let mut ready = false;
    let mut last_loss_register: Option<tokio::time::Instant> = None;
    let reregister = tokio::time::sleep(Duration::from_secs(86400 - 600));
    tokio::pin!(reregister);
    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(msg) = msg else { return Ok(()) };
                let text = match msg.map_err(|e| Error::Ws(Box::new(e)))? {
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
                        sink.send(WsMessage::text(format!("5:::{auth}"))).await.map_err(|e| Error::Ws(Box::new(e)))?;
                        let now = *active.borrow_and_update();
                        sink.send(WsMessage::text(next(activity_event(now)))).await.map_err(|e| Error::Ws(Box::new(e)))?;
                        ready = true;
                        tracing::info!(active = now, "trouter: connected, user activity sent");
                        let wait = register_or_retry(client, &alloc.surl).await.unwrap_or(Duration::from_secs(86400 - 600));
                        reregister.as_mut().reset(tokio::time::Instant::now() + wait);
                        if tx.send(TrouterEvent::Connected).is_err() { return Ok(()); }
                    }
                    Some(Frame::Message(data)) => {
                        if let Some(push) = decode_push(&data) {
                            sink.send(WsMessage::text(ack_frame(push.id))).await.map_err(|e| Error::Ws(Box::new(e)))?;
                            for ev in events_from_push(&push) {
                                if tx.send(TrouterEvent::Event(Box::new(ev))).is_err() { return Ok(()); }
                            }
                        }
                    }
                    Some(Frame::Event(data)) if data.contains("trouter.message_loss") => {
                        // The server dropped events. Re-register at most once a minute (more trips
                        // the registrar's throttling) and ask the app to catch up.
                        if last_loss_register.is_none_or(|t: tokio::time::Instant| t.elapsed() > Duration::from_secs(60)) {
                            last_loss_register = Some(tokio::time::Instant::now());
                            let wait = register_or_retry(client, &alloc.surl).await.unwrap_or(Duration::from_secs(86400 - 600));
                            reregister.as_mut().reset(tokio::time::Instant::now() + wait);
                            if tx.send(TrouterEvent::Resync).is_err() { return Ok(()); }
                        }
                    }
                    Some(Frame::Disconnect) => return Ok(()),
                    _ => {}
                }
            }
            _ = ping.tick() => {
                sink.send(WsMessage::text(next(json!({"name": "ping"})))).await.map_err(|e| Error::Ws(Box::new(e)))?;
            }
            Ok(()) = active.changed(), if ready => {
                let now = *active.borrow_and_update();
                sink.send(WsMessage::text(next(activity_event(now)))).await.map_err(|e| Error::Ws(Box::new(e)))?;
                tracing::info!(active = now, "trouter: user activity sent");
            }
            _ = &mut reregister => {
                let wait = register_or_retry(client, &alloc.surl).await.unwrap_or(Duration::from_secs(86400 - 600));
                reregister.as_mut().reset(tokio::time::Instant::now() + wait);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn frames() {
        assert_eq!(parse_frame("1::"), Some(Frame::Connect));
        assert_eq!(parse_frame("2::"), Some(Frame::Heartbeat));
        assert_eq!(
            parse_frame("3:::{\"a\":\"b:c\"}"),
            Some(Frame::Message("{\"a\":\"b:c\"}".into()))
        );
        assert_eq!(
            parse_frame("5:12+::{\"name\":\"x\"}"),
            Some(Frame::Event("{\"name\":\"x\"}".into()))
        );
        assert_eq!(parse_frame("6:::3+[]"), Some(Frame::Ack("3+[]".into())));
        assert_eq!(parse_frame("0::"), Some(Frame::Disconnect));
        assert_eq!(parse_frame("x::"), None);
    }

    fn gz(s: &str) -> String {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(s.as_bytes()).unwrap();
        base64::engine::general_purpose::STANDARD.encode(e.finish().unwrap())
    }

    const PAYLOAD: &str = r#"{"resourceType":"NewMessage","resource":{"id":"1","conversationLink":"https://h/v1/users/ME/conversations/19:a@thread.v2","from":"https://h/contacts/8:orgid:x","imdisplayname":"Sara","composetime":"2026-11-18T11:24:38Z","messagetype":"RichText/Html","content":"<p>hi</p>"}}"#;

    #[test]
    fn decodes_plain_push() {
        let frame =
            json!({"id": 7, "url": "https://t/v4/f/x/messaging", "headers": {}, "body": PAYLOAD})
                .to_string();
        let p = decode_push(&frame).unwrap();
        assert_eq!(p.id, 7);
        let ev = events_from_push(&p);
        assert!(matches!(&ev[..], [Event::NewMessage(m)] if m.sender_name == "Sara"));
        assert_eq!(ack_frame(7), r#"3:::{"body":"","id":7,"status":200}"#);
    }

    #[test]
    fn decodes_gzip_header_push() {
        let frame = json!({"id": 1, "url": "https://t/messaging", "headers": {"X-Microsoft-Skype-Content-Encoding": "gzip"}, "body": gz(PAYLOAD)}).to_string();
        assert_eq!(events_from_push(&decode_push(&frame).unwrap()).len(), 1);
    }

    #[test]
    fn decodes_cp_and_gp_bodies() {
        let cp = json!({"cp": gz(PAYLOAD)}).to_string();
        let frame =
            json!({"id": 2, "url": "https://t/messaging", "headers": {}, "body": cp}).to_string();
        assert_eq!(events_from_push(&decode_push(&frame).unwrap()).len(), 1);
        let gp =
            json!({"gp": base64::engine::general_purpose::STANDARD.encode(PAYLOAD)}).to_string();
        let frame =
            json!({"id": 3, "url": "https://t/messaging", "headers": {}, "body": gp}).to_string();
        assert_eq!(events_from_push(&decode_push(&frame).unwrap()).len(), 1);
    }

    #[test]
    fn presence_push() {
        let body =
            json!({"presence": [{"mri": "8:orgid:a", "presence": {"availability": "Busy"}}]})
                .to_string();
        let frame = json!({"id": 4, "url": "https://t/x/TeamsUnifiedPresenceService", "headers": {}, "body": body}).to_string();
        assert_eq!(
            events_from_push(&decode_push(&frame).unwrap()),
            vec![Event::Presence {
                mri: "8:orgid:a".into(),
                availability: "Busy".into(),
                activity: String::new()
            }]
        );
    }

    #[test]
    fn garbage_push_is_ignored() {
        assert!(decode_push("not json").is_none());
        assert!(decode_push(r#"{"id":1,"url":"u","headers":{"X-Microsoft-Skype-Content-Encoding":"gzip"},"body":"!!!"}"#).is_none());
    }

    #[test]
    fn parses_retry_after() {
        let d = retry_after(
            r#"{"code":42901,"retryAfter":"00:02:52.0065811","message":"Request was throttled"}"#,
        )
        .unwrap();
        assert_eq!(d.as_secs(), 172);
        assert!(retry_after("{}").is_none());
    }

    #[test]
    fn query_and_registration_shapes() {
        let mut cp = serde_json::Map::new();
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
        let r = registration_body(
            "TeamsCDLWebWorker",
            "TeamsCDLWebWorker_2.1",
            "https://s/v4/f/x/",
            "ep",
        );
        assert_eq!(r["transports"]["TROUTER"][0]["ttl"], 86400);
        assert_eq!(r["registrationId"], "ep");
        assert_eq!(activity_cv().len(), 26);
    }

    #[test]
    fn the_activity_event_carries_active_or_inactive() {
        let inactive = activity_event(false);
        assert_eq!(inactive["name"], "user.activity");
        assert_eq!(inactive["args"][0]["state"], "inactive");
        assert_eq!(inactive["args"][0]["cv"].as_str().unwrap().len(), 26);
        assert_eq!(activity_event(true)["args"][0]["state"], "active");
    }

    #[test]
    fn the_user_activity_wakes_the_connections_only_when_it_changes() {
        let mut seen = user_activity();
        seen.borrow_and_update();
        set_user_active(false);
        assert!(seen.has_changed().unwrap());
        assert!(!*seen.borrow_and_update());
        // The same answer again is no news: nothing is sent.
        set_user_active(false);
        assert!(!seen.has_changed().unwrap());
        set_user_active(true);
        assert!(*seen.borrow_and_update());
    }
}
