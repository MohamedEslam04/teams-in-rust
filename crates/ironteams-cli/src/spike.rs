//! `ironteams-cli spike personal-login`: the Gate 0 personal-account (Teams free) sign-in spike.
//!
//! One interactive sign-in on the `/consumers` authority with a first-party public client, then
//! the consumer skypetoken exchange and a five-row conversation list. It answers "does this chain
//! work" and nothing else: it changes no product code path, stores nothing in the keyring or the
//! config, and sends each request once (the device-code poll is the protocol's own wait, not a
//! retry). See docs/spikes/personal-auth.md.
//!
//! Two sign-ins are offered: `--flow devicecode` (the default) and `--flow pkce`, an auth-code +
//! PKCE loopback flow for clients that refuse device code — the Teams web client refuses the
//! first-party consent device code needs, but accepts the loopback redirect. The PKCE flow holds
//! a verifier in memory for the one exchange and never prints or writes it.
//!
//! Output rule: tokens never leave memory. The terminal and the `--out` transcript get status
//! codes, header names, key names, field presence, scopes, lifetimes and service hosts only;
//! every line still goes through [`scrub`] in case a token slipped into an error text.

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;
use teams_proto::client_info::USER_AGENT;

pub const USAGE: &str = "ironteams-cli spike personal-login [--flow devicecode|pkce] [--client teamsweb|office|azcli] [--out FILE]";

/// The `/consumers` (personal Microsoft account) v2 authority.
pub const CONSUMERS_AUTHORITY: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
/// The audience Teams free exchanges for a skypetoken, plus a refresh token.
pub const SCOPE: &str = "https://auth.fl.teams.microsoft.com/teams.auth.readwrite offline_access";
/// The PKCE flow's scope: the Teams audience plus `offline_access` and `openid` (the Teams web
/// client's own request shape, which the unauthenticated authorize probe accepted).
pub const PKCE_SCOPE: &str =
    "https://auth.fl.teams.microsoft.com/teams.auth.readwrite offline_access openid";
/// The consumer skypetoken exchange (the work one is `authsvc/v1.0/authz`).
pub const AUTHZ_CONSUMER_URL: &str = "https://teams.live.com/api/auth/v2.0/authz/consumer";
/// The consumer chat service tried first.
pub const MSGAPI_BASE: &str = "https://msgapi.teams.live.com";
/// The tenant id Entra uses for every personal Microsoft account (a public constant).
pub const CONSUMER_TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";
/// The origin of the Teams free web app.
const LIVE_ORIGIN: &str = "https://teams.live.com";
/// The chat service path on teams.live.com that may receive the skypetoken.
const LIVE_CHATSVC_PATH: &str = "/api/chatsvc/consumer";

/// Request header names, for the transcript (the values are the token or constants).
const AUTHZ_HEADERS: [&str; 4] = ["user-agent", "authorization", "accept", "content-length"];
const CHAT_HEADERS: [&str; 6] = [
    "user-agent",
    "authentication",
    "x-skypetoken",
    "accept",
    "origin",
    "referer",
];

/// Which first-party public client signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpikeClient {
    /// The Teams web client, registered as an SPA. Refused the first-party consent device code
    /// needs, but the authorize probe accepted its loopback redirect (the PKCE flow's default).
    TeamsWeb,
    /// Microsoft Office, MSA-enabled and allowed device code (live probe 2026-11-25).
    Office,
    /// Azure CLI, the fallback.
    AzCli,
}

impl SpikeClient {
    pub fn id(self) -> &'static str {
        match self {
            SpikeClient::TeamsWeb => "4b3e8f46-56d3-427f-b1e2-d239b2ea6bca",
            SpikeClient::Office => "d3590ed6-52b3-4102-aeff-aad2292ab01c",
            SpikeClient::AzCli => "04b07795-8ddb-461a-bbee-02f9e1bf7b46",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SpikeClient::TeamsWeb => "teamsweb",
            SpikeClient::Office => "office",
            SpikeClient::AzCli => "azcli",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "teamsweb" => Some(SpikeClient::TeamsWeb),
            "office" => Some(SpikeClient::Office),
            "azcli" => Some(SpikeClient::AzCli),
            _ => None,
        }
    }
}

/// Which sign-in flow runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpikeFlow {
    /// Device code on `/consumers` (the default; the existing behaviour).
    DeviceCode,
    /// Auth-code + PKCE on a loopback redirect.
    Pkce,
}

impl SpikeFlow {
    pub fn name(self) -> &'static str {
        match self {
            SpikeFlow::DeviceCode => "devicecode",
            SpikeFlow::Pkce => "pkce",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "devicecode" => Some(SpikeFlow::DeviceCode),
            "pkce" => Some(SpikeFlow::Pkce),
            _ => None,
        }
    }

    /// The client used when `--client` is not given.
    fn default_client(self) -> SpikeClient {
        match self {
            SpikeFlow::DeviceCode => SpikeClient::Office,
            SpikeFlow::Pkce => SpikeClient::TeamsWeb,
        }
    }
}

/// Parsed command-line options.
#[derive(Debug, PartialEq, Eq)]
struct Opts {
    flow: SpikeFlow,
    client: SpikeClient,
    out: Option<PathBuf>,
}

fn parse_opts(args: &[String]) -> Result<Opts> {
    let mut flow = SpikeFlow::DeviceCode;
    let mut client: Option<SpikeClient> = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--flow" => {
                let v = it.next().map(String::as_str).unwrap_or_default();
                flow = SpikeFlow::parse(v)
                    .ok_or_else(|| anyhow::anyhow!("--flow devicecode|pkce, not {v:?}"))?;
            }
            "--client" => {
                let v = it.next().map(String::as_str).unwrap_or_default();
                client =
                    Some(SpikeClient::parse(v).ok_or_else(|| {
                        anyhow::anyhow!("--client teamsweb|office|azcli, not {v:?}")
                    })?);
            }
            "--out" => {
                let v = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--out needs a file path"))?;
                out = Some(PathBuf::from(v));
            }
            other => bail!("unknown argument {other:?}; usage: {USAGE}"),
        }
    }
    Ok(Opts {
        flow,
        client: client.unwrap_or_else(|| flow.default_client()),
        out,
    })
}

/// A form POST, built without sending so the tests can read it. No `Debug`: the poll body
/// carries the device code.
pub struct FormRequest {
    pub url: String,
    pub body: String,
}

fn form(url: String, pairs: &[(&str, &str)]) -> FormRequest {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    FormRequest {
        url,
        body: s.finish(),
    }
}

/// Starts the device-code flow.
pub fn device_code_request(client: SpikeClient) -> FormRequest {
    form(
        format!("{CONSUMERS_AUTHORITY}/devicecode"),
        &[("client_id", client.id()), ("scope", SCOPE)],
    )
}

/// One device-code poll of the token endpoint.
pub fn token_poll_request(client: SpikeClient, device_code: &str) -> FormRequest {
    form(
        format!("{CONSUMERS_AUTHORITY}/token"),
        &[
            ("client_id", client.id()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
        ],
    )
}

/// base64url without padding (the PKCE and the authorize-URL alphabet). Shared with the test
/// JWT builder.
fn b64url_encode(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let (mut out, mut acc, mut bits) = (String::new(), 0u32, 0u32);
    for &b in data {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(A[((acc >> bits) & 63) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(A[((acc << (6 - bits)) & 63) as usize] as char);
    }
    out
}

/// The PKCE S256 code challenge for a verifier: base64url(SHA-256(verifier)), no padding
/// (RFC 7636 §4.2).
pub fn pkce_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    b64url_encode(hasher.finalize().as_slice())
}

/// `n` random bytes as base64url: every character is a PKCE-unreserved character, so the result
/// is a valid verifier or state. 32 bytes give 43 characters (within the 43-128 PKCE range).
fn random_b64url(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("the OS CSPRNG must be available");
    b64url_encode(&buf)
}

/// A fresh PKCE code verifier (43 unreserved characters from 32 random bytes).
pub fn new_verifier() -> String {
    random_b64url(32)
}

/// A fresh anti-CSRF `state` (22 characters from 16 random bytes).
pub fn new_state() -> String {
    random_b64url(16)
}

/// The authorize URL: the consumers authority, `response_type=code`, the loopback redirect, the
/// PKCE scope, the S256 challenge, the state and `response_mode=query`. No secret is in it (the
/// challenge is a hash, the state a nonce), so it may be shown so it can be pasted.
pub fn authorize_url(
    client: SpikeClient,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    s.append_pair("client_id", client.id());
    s.append_pair("response_type", "code");
    s.append_pair("redirect_uri", redirect_uri);
    s.append_pair("scope", PKCE_SCOPE);
    s.append_pair("code_challenge", challenge);
    s.append_pair("code_challenge_method", "S256");
    s.append_pair("state", state);
    s.append_pair("response_mode", "query");
    format!("{CONSUMERS_AUTHORITY}/authorize?{}", s.finish())
}

/// The auth-code token exchange: `grant_type=authorization_code`, the code, the same redirect
/// and scope, and the PKCE verifier. No `client_secret` (a public client). No `Debug`: the body
/// carries the code and the verifier.
pub fn code_exchange_request(
    client: SpikeClient,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> FormRequest {
    form(
        format!("{CONSUMERS_AUTHORITY}/token"),
        &[
            ("client_id", client.id()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("scope", PKCE_SCOPE),
        ],
    )
}

/// What came back on the loopback redirect. No `Debug`: `code` is the authorization code.
pub struct Redirect {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Parses `code`, `state` and `error` from a redirect query string (no leading `?`).
pub fn parse_redirect_query(query: &str) -> Redirect {
    let mut r = Redirect {
        code: None,
        state: None,
        error: None,
    };
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" => r.code = Some(v.into_owned()),
            "state" => r.state = Some(v.into_owned()),
            "error" => r.error = Some(v.into_owned()),
            _ => {}
        }
    }
    r
}

/// The query of an HTTP request line like `GET /?code=…&state=… HTTP/1.1`, or `None`.
pub fn request_target_query(request_line: &str) -> Option<String> {
    let target = request_line.split_whitespace().nth(1)?;
    target.split_once('?').map(|(_, q)| q.to_string())
}

/// The authorization code from a redirect, once the state matches. Errors (never with the code
/// in the message) on an authorize error, a missing code, or a state mismatch.
pub fn accept_code(redirect: &Redirect, expected_state: &str) -> Result<String> {
    if let Some(e) = &redirect.error {
        bail!("the authorize endpoint returned error {}", safe_word(e));
    }
    match (&redirect.code, redirect.state.as_deref()) {
        (Some(_), Some(state)) if state != expected_state => bail!("state does not match"),
        (Some(code), Some(_)) => Ok(code.clone()),
        (_, None) => bail!("no state on the redirect"),
        (None, _) => bail!("no code on the redirect"),
    }
}

/// Whether a failed token answer is the SPA cross-origin refusal, so the exchange should be
/// retried once with an `Origin` header (AADSTS9002327).
fn needs_origin(body: &str) -> bool {
    let low = body.to_ascii_lowercase();
    low.contains("aadsts9002327") || low.contains("origin") || low.contains("cors")
}

/// The redirect URI reduced to host and a `PORT` placeholder for the transcript, e.g.
/// `http://localhost:PORT/`.
fn redirect_uri_shape(redirect_uri: &str) -> String {
    match url::Url::parse(redirect_uri) {
        Ok(u) => format!(
            "{}://{}:PORT/",
            u.scheme(),
            u.host_str().unwrap_or("localhost")
        ),
        Err(_) => "http://localhost:PORT/".to_string(),
    }
}

/// The first page (five rows) of conversations on a chat service base URL.
pub fn conversations_url(base: &str) -> String {
    format!(
        "{}/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=5",
        base.trim_end_matches('/')
    )
}

/// Whether the skypetoken may be sent to `base`: https on the default port, and exactly
/// `msgapi.teams.live.com`, or `teams.live.com` under `/api/chatsvc/consumer`. Exact matches
/// only, never a suffix, so a host named in the authz answer cannot pull the token elsewhere.
pub fn chat_host_allowed(base: &str) -> bool {
    let Ok(u) = url::Url::parse(base) else {
        return false;
    };
    if u.scheme() != "https" || u.port().is_some() || !u.username().is_empty() {
        return false;
    }
    match u.host_str() {
        Some("msgapi.teams.live.com") => true,
        Some("teams.live.com") => {
            let p = u.path().trim_end_matches('/');
            p == LIVE_CHATSVC_PATH || p.starts_with(&format!("{LIVE_CHATSVC_PATH}/"))
        }
        _ => false,
    }
}

/// A successful device-code grant. No `Debug`: it holds the access token. The refresh token is
/// not kept at all, only whether one came.
pub struct Grant {
    pub access_token: String,
    pub refresh_present: bool,
    pub scopes: Vec<String>,
    pub expires_in: Option<u64>,
    pub refresh_token_expires_in: Option<u64>,
    pub token_type: Option<String>,
    pub field_names: Vec<String>,
}

/// One token endpoint answer.
pub enum Poll {
    Pending,
    SlowDown,
    Done(Grant),
    /// Declined, expired or refused: the `error` name and the AADSTS number, nothing else.
    Failed {
        error: String,
        aadsts: Option<u32>,
    },
}

/// Numbers in Microsoft answers are sometimes strings.
fn num(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Key names, sorted (the order of the answer is not meaningful).
fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    k.sort();
    k
}

pub fn parse_poll(body: &str) -> Poll {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Poll::Failed {
            error: "unparsable answer".into(),
            aadsts: None,
        };
    };
    if let Some(err) = v["error"].as_str() {
        return match err {
            "authorization_pending" => Poll::Pending,
            "slow_down" => Poll::SlowDown,
            _ => Poll::Failed {
                error: safe_word(err),
                aadsts: v["error_description"]
                    .as_str()
                    .and_then(teams_proto::auth::aadsts_code)
                    .or_else(|| {
                        v["error_codes"][0]
                            .as_u64()
                            .and_then(|c| u32::try_from(c).ok())
                    }),
            },
        };
    }
    let Some(access_token) = v["access_token"].as_str().map(str::to_owned) else {
        return Poll::Failed {
            error: "no access_token".into(),
            aadsts: None,
        };
    };
    Poll::Done(Grant {
        access_token,
        refresh_present: v["refresh_token"].as_str().is_some_and(|s| !s.is_empty()),
        scopes: v["scope"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        expires_in: num(v.get("expires_in")),
        refresh_token_expires_in: num(v.get("refresh_token_expires_in")),
        token_type: v["token_type"].as_str().map(safe_word),
        field_names: keys(&v),
    })
}

/// Decodes base64url (with or without padding; the standard alphabet is accepted too).
fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// What the access token's payload says, without the token. `tid` and `aud` only: the other
/// claims (oid, puid, names, email) are personal and are listed by name.
pub struct JwtSummary {
    pub is_jwt: bool,
    pub tid: Option<String>,
    pub aud: Option<String>,
    pub claim_names: Vec<String>,
}

pub fn jwt_summary(token: &str) -> JwtSummary {
    let payload = token
        .split('.')
        .nth(1)
        .filter(|_| token.split('.').count() == 3)
        .and_then(b64url_decode)
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(Value::is_object);
    let Some(p) = payload else {
        return JwtSummary {
            is_jwt: false,
            tid: None,
            aud: None,
            claim_names: vec![],
        };
    };
    let aud = match &p["aud"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    };
    JwtSummary {
        is_jwt: true,
        tid: p["tid"].as_str().map(str::to_owned),
        aud,
        claim_names: keys(&p),
    }
}

/// The lines printed for the token stage. Pure, so a test can prove the token is not in them.
pub fn token_lines(g: &Grant, j: &JwtSummary) -> Vec<String> {
    let mut l = vec![
        format!("token answer fields: {}", g.field_names.join(", ")),
        format!(
            "scopes granted: {}",
            if g.scopes.is_empty() {
                "(none named)".to_string()
            } else {
                g.scopes.join(" ")
            }
        ),
        format!(
            "expires_in: {}, token_type: {}",
            g.expires_in.map_or("absent".into(), |s| format!("{s} s")),
            g.token_type.as_deref().unwrap_or("absent")
        ),
        format!(
            "refresh token present: {}, refresh_token_expires_in: {}",
            yes(g.refresh_present),
            g.refresh_token_expires_in
                .map_or("absent".into(), |s| format!("{s} s"))
        ),
    ];
    if j.is_jwt {
        l.push(format!(
            "access token is a JWT; tid: {}{}, aud: {}",
            j.tid.as_deref().unwrap_or("absent"),
            if j.tid.as_deref() == Some(CONSUMER_TENANT) {
                " (the personal-account tenant)"
            } else {
                ""
            },
            j.aud.as_deref().unwrap_or("absent")
        ));
        l.push(format!("claim names: {}", j.claim_names.join(", ")));
    } else {
        l.push("access token is not a JWT (opaque): no tid/aud to read".into());
    }
    l
}

/// A `regionGtms` entry: its key, the JSON type of its value, and for chat-service keys the URL
/// (a service host, not a secret).
pub struct GtmsEntry {
    pub key: String,
    pub shape: &'static str,
    pub chat_url: Option<String>,
}

/// The authz/consumer answer, reduced. No `Debug`: it holds the skypetoken.
pub struct AuthzSummary {
    pub top_keys: Vec<String>,
    pub skype_token: Option<String>,
    /// Where the token was found (`tokens.skypeToken` or `skypeToken.skypetoken`).
    pub token_path: Option<&'static str>,
    pub expires_in: Option<u64>,
    pub region_present: bool,
    pub gtms: Vec<GtmsEntry>,
}

/// A `regionGtms` URL: a string, or an object with `endpointUrl`.
fn gtms_url(v: &Value) -> Option<String> {
    v.as_str()
        .or_else(|| v["endpointUrl"].as_str())
        .map(|s| s.trim_end_matches('/').to_string())
}

fn is_chat_key(k: &str) -> bool {
    k.to_ascii_lowercase().contains("chat")
}

pub fn parse_authz_consumer(body: &str) -> Result<AuthzSummary> {
    let v: Value = serde_json::from_str(body)?;
    let (skype_token, token_path) = if let Some(t) = v["tokens"]["skypeToken"].as_str() {
        (Some(t.to_string()), Some("tokens.skypeToken"))
    } else if let Some(t) = v["skypeToken"]["skypetoken"].as_str() {
        (Some(t.to_string()), Some("skypeToken.skypetoken"))
    } else if let Some(t) = v["skypeToken"].as_str() {
        (Some(t.to_string()), Some("skypeToken"))
    } else {
        (None, None)
    };
    let expires_in = num(v["tokens"].get("expiresIn"))
        .or_else(|| num(v["skypeToken"].get("expiresIn")))
        .or_else(|| num(v["skypeToken"].get("expires_in")));
    let gtms = v["regionGtms"]
        .as_object()
        .map(|g| {
            g.iter()
                .map(|(k, val)| GtmsEntry {
                    key: k.clone(),
                    shape: match val {
                        Value::String(_) => "string",
                        Value::Object(_) => "object",
                        Value::Null => "null",
                        Value::Bool(_) => "bool",
                        Value::Number(_) => "number",
                        Value::Array(_) => "array",
                    },
                    chat_url: if is_chat_key(k) { gtms_url(val) } else { None },
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(AuthzSummary {
        top_keys: keys(&v),
        skype_token: skype_token.filter(|t| !t.is_empty()),
        token_path,
        expires_in,
        region_present: v.get("region").is_some(),
        gtms,
    })
}

/// Chat service bases to try, in order: msgapi, then `regionGtms.chatService`, then the other
/// chat keys. Duplicates dropped.
pub fn chat_candidates(a: &AuthzSummary) -> Vec<String> {
    let mut out = vec![MSGAPI_BASE.to_string()];
    let mut entries: Vec<&GtmsEntry> = a.gtms.iter().filter(|e| e.chat_url.is_some()).collect();
    entries.sort_by_key(|e| e.key != "chatService");
    for e in entries {
        let u = e.chat_url.clone().unwrap_or_default();
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

/// One conversation, reduced to its shape.
pub struct ConvRow {
    pub id_shape: String,
    pub thread_type: Option<String>,
    pub members: Option<u64>,
}

/// The shape of a conversation id with the unique part elided: `19:…@thread.v2`, `8:live:…`.
/// The fixed system ids (`48:notes`, `48:calllogs`) are kept.
pub fn id_shape(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("48:")
        && !rest.is_empty()
        && rest.len() < 20
        && rest.bytes().all(|b| b.is_ascii_lowercase())
    {
        return id.to_string();
    }
    let parts: Vec<&str> = id.splitn(3, ':').collect();
    let prefix = match parts.as_slice() {
        [kind, sub, _] if *kind == "8" && sub.bytes().all(|b| b.is_ascii_lowercase()) => {
            format!("8:{sub}:")
        }
        [kind, ..] if parts.len() > 1 && kind.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{kind}:")
        }
        _ => String::new(),
    };
    let suffix = id
        .rfind('@')
        .map(|i| &id[i..])
        .filter(|s| {
            s.len() < 32
                && s[1..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.')
        })
        .unwrap_or_default();
    format!("{prefix}…{suffix}")
}

/// An enum-like word (thread type, error name) or `<other>`.
fn safe_word(s: &str) -> String {
    if !s.is_empty()
        && s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        s.to_string()
    } else {
        "<other>".into()
    }
}

pub fn parse_conversations(body: &str) -> Result<(Vec<String>, Vec<ConvRow>)> {
    let v: Value = serde_json::from_str(body)?;
    let Some(list) = v["conversations"].as_array() else {
        bail!("no conversations array (keys: {})", keys(&v).join(", "));
    };
    let rows = list
        .iter()
        .take(5)
        .map(|c| {
            let tp = &c["threadProperties"];
            ConvRow {
                id_shape: id_shape(c["id"].as_str().unwrap_or_default()),
                thread_type: tp["threadType"].as_str().map(safe_word),
                members: num(tp.get("membercount"))
                    .or_else(|| num(tp.get("memberCount")))
                    .or_else(|| num(c.get("memberCount")))
                    .or_else(|| c["members"].as_array().map(|m| m.len() as u64)),
            }
        })
        .collect();
    Ok((keys(&v), rows))
}

/// Replaces anything token-like with `<redacted>`: a run of token characters that contains a JWT
/// start (`eyJ`), or is 32+ characters mixing upper case, lower case and digits (MSA tickets,
/// refresh tokens). URLs (runs starting `//`), hosts, GUIDs and short codes are kept.
pub fn scrub(text: &str) -> String {
    fn secret(run: &str) -> bool {
        if run.len() >= 16 && run.contains("eyJ") {
            return true;
        }
        run.len() >= 32
            && !run.starts_with("//")
            && run.bytes().any(|b| b.is_ascii_uppercase())
            && run.bytes().any(|b| b.is_ascii_lowercase())
            && run.bytes().any(|b| b.is_ascii_digit())
    }
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if secret(run) {
            out.push_str("<redacted>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || "-_.~+/=%".contains(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// An error body for the terminal: names, emails, MRIs and tokens redacted, then cut to 300
/// characters.
pub fn error_excerpt(body: &str) -> String {
    let clean = scrub(&diag::redact(body)).replace(['\n', '\r'], " ");
    if clean.chars().count() > 300 {
        format!("{}…", clean.chars().take(300).collect::<String>())
    } else {
        clean
    }
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn say(line: &str) {
    println!("{}", scrub(line));
}

/// One HTTP answer: status, header names, body.
struct Exchange {
    status: u16,
    header_names: Vec<String>,
    body: String,
}

async fn send(rb: reqwest::RequestBuilder) -> Result<Exchange> {
    let resp = rb.send().await?;
    let status = resp.status().as_u16();
    let mut header_names: Vec<String> = resp
        .headers()
        .keys()
        .map(|k| k.as_str().to_string())
        .collect();
    header_names.sort();
    header_names.dedup();
    let body = resp.text().await?;
    Ok(Exchange {
        status,
        header_names,
        body,
    })
}

async fn post_form(http: &reqwest::Client, r: &FormRequest) -> Result<Exchange> {
    send(
        http.post(&r.url)
            .header("User-Agent", USER_AGENT)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(r.body.clone()),
    )
    .await
}

fn body_keys(body: &str) -> Vec<String> {
    serde_json::from_str::<Value>(body)
        .map(|v| keys(&v))
        .unwrap_or_default()
}

/// The error name and AADSTS number of a token-service answer.
fn aad_error(body: &str) -> (Option<String>, Option<u32>) {
    let has_error = serde_json::from_str::<Value>(body).is_ok_and(|v| v["error"].is_string());
    match parse_poll(body) {
        Poll::Failed { error, aadsts } if has_error => (Some(error), aadsts),
        _ => (None, None),
    }
}

/// Stage 1: device code, then poll until signed in, declined or expired.
async fn stage_token(
    http: &reqwest::Client,
    client: SpikeClient,
    rec: &mut Map<String, Value>,
) -> Option<Grant> {
    say(&format!(
        "== 1. token: device code on /consumers with the {} client",
        client.name()
    ));
    let ex = match post_form(http, &device_code_request(client)).await {
        Ok(ex) => ex,
        Err(e) => {
            say(&format!("device code request failed: {e}"));
            rec.insert("device_code".into(), json!({"network_error": true}));
            return None;
        }
    };
    let (error, aadsts) = aad_error(&ex.body);
    rec.insert(
        "device_code".into(),
        json!({
            "status": ex.status,
            "response_headers": ex.header_names,
            "body_keys": body_keys(&ex.body),
            "error": error,
            "aadsts": aadsts,
        }),
    );
    if ex.status != 200 {
        say(&format!(
            "device code refused: HTTP {}, error {}, AADSTS {}",
            ex.status,
            error.as_deref().unwrap_or("?"),
            aadsts.map_or("?".into(), |c| c.to_string())
        ));
        return None;
    }
    let dc = match teams_proto::auth::parse_device_code(&ex.body) {
        Ok(dc) => dc,
        Err(e) => {
            say(&format!("device code answer not understood: {e}"));
            return None;
        }
    };
    say(&format!(
        "Open {} and enter the code {}",
        dc.verification_url, dc.user_code
    ));
    say(&format!(
        "Waiting for the sign-in (the code expires in {} min). Sign in once; do not rerun this in a loop.",
        dc.expires_in_s / 60
    ));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(dc.expires_in_s);
    let mut interval = dc.interval_s.max(1);
    let mut polls = 0u32;
    let (outcome, grant, last) = loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() > deadline {
            break ("expired (local deadline)".to_string(), None, None);
        }
        polls += 1;
        let ex = match post_form(http, &token_poll_request(client, &dc.device_code)).await {
            Ok(ex) => ex,
            Err(e) => {
                say(&format!("token poll failed: {e}"));
                break ("network error".to_string(), None, None);
            }
        };
        let meta = (ex.status, ex.header_names);
        match parse_poll(&ex.body) {
            Poll::Pending => {}
            Poll::SlowDown => interval += 5,
            Poll::Done(g) => break ("signed in".to_string(), Some(g), Some(meta)),
            Poll::Failed { error, aadsts } => {
                break (
                    format!(
                        "{error}{}",
                        aadsts.map_or(String::new(), |c| format!(" (AADSTS{c})"))
                    ),
                    None,
                    Some(meta),
                );
            }
        }
    };
    let mut t = json!({
        "polls": polls,
        "outcome": outcome,
        "status": last.as_ref().map(|m| m.0),
        "response_headers": last.map(|m| m.1).unwrap_or_default(),
    });
    match &grant {
        Some(g) => record_grant(&mut t, g),
        None => say(&format!("sign-in ended: {outcome}")),
    }
    rec.insert("token".into(), t);
    grant
}

/// Prints the shape-only token lines and folds the grant's field summary into a `token` record.
/// The access token and the claim values never reach `t`.
fn record_grant(t: &mut Value, g: &Grant) {
    let j = jwt_summary(&g.access_token);
    for l in token_lines(g, &j) {
        say(&l);
    }
    t["field_names"] = json!(g.field_names);
    t["scopes"] = json!(g.scopes);
    t["expires_in"] = json!(g.expires_in);
    t["refresh_token_present"] = json!(g.refresh_present);
    t["refresh_token_expires_in"] = json!(g.refresh_token_expires_in);
    t["token_type"] = json!(g.token_type);
    t["jwt"] = json!({
        "is_jwt": j.is_jwt,
        "claim_names": j.claim_names,
        "tid_is_consumer_tenant": j.tid.as_deref().map(|tid| tid == CONSUMER_TENANT),
        "aud": j.aud,
    });
}

/// Opens `url` in the user's browser. The URL carries no secret (the challenge is a hash, the
/// state a nonce). Returns whether a launcher started; it does not wait for the browser.
fn spawn_browser(url: &str) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        #[cfg(target_os = "linux")]
        let (cmd, pre): (&str, &[&str]) = ("xdg-open", &[]);
        #[cfg(target_os = "macos")]
        let (cmd, pre): (&str, &[&str]) = ("open", &[]);
        #[cfg(target_os = "windows")]
        let (cmd, pre): (&str, &[&str]) = ("cmd", &["/C", "start", ""]);
        std::process::Command::new(cmd)
            .args(pre)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = url;
        false
    }
}

/// Waits (up to five minutes) for one HTTP GET on the loopback listener, replies with a tiny
/// "you can close this tab" page, and returns the request's query string. Runs on a blocking
/// thread so it does not stall the async runtime.
async fn await_redirect(listener: std::net::TcpListener) -> Result<String> {
    use std::io::{BufRead, BufReader, Write};
    tokio::task::spawn_blocking(move || -> Result<String> {
        listener.set_nonblocking(true)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(300);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
                    let request_line = {
                        let mut reader = BufReader::new(stream.try_clone()?);
                        let mut line = String::new();
                        reader.read_line(&mut line)?;
                        line
                    };
                    let html = "<!doctype html><meta charset=\"utf-8\"><title>IronTeams sign-in</title>\
                        <body style=\"font-family:system-ui;margin:3rem\"><p>Signed in. You can close this tab.</p></body>";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                        html.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    return request_target_query(&request_line)
                        .ok_or_else(|| anyhow::anyhow!("the redirect request had no query"));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() > deadline {
                        bail!("timed out after 5 min waiting for the browser redirect");
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => return Err(e.into()),
            }
        }
    })
    .await?
}

/// POSTs the auth-code exchange, first without an `Origin` header; on an SPA cross-origin
/// refusal it retries once with `Origin: https://teams.live.com`. Returns the grant (if any),
/// whether the final attempt used `Origin`, and that attempt's status and header names.
async fn exchange_code(
    http: &reqwest::Client,
    req: &FormRequest,
) -> (Option<Grant>, bool, Option<u16>, Vec<String>) {
    // First attempt: no Origin header.
    match post_form(http, req).await {
        Ok(ex) => {
            if let Poll::Done(g) = parse_poll(&ex.body) {
                say("token exchange succeeded WITHOUT an Origin header.");
                return (Some(g), false, Some(ex.status), ex.header_names);
            }
            let (error, aadsts) = aad_error(&ex.body);
            say(&format!(
                "first token attempt (no Origin): HTTP {}, error {}, AADSTS {}",
                ex.status,
                error.as_deref().unwrap_or("?"),
                aadsts.map_or("?".into(), |c| c.to_string())
            ));
            if !needs_origin(&ex.body) {
                return (None, false, Some(ex.status), ex.header_names);
            }
            say(
                "answer looks like the SPA cross-origin refusal; retrying once with Origin: https://teams.live.com",
            );
        }
        Err(e) => {
            say(&format!("token exchange request failed: {e}"));
            return (None, false, None, vec![]);
        }
    }
    // Second attempt: with the Origin header.
    match send(
        http.post(&req.url)
            .header("User-Agent", USER_AGENT)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Origin", LIVE_ORIGIN)
            .body(req.body.clone()),
    )
    .await
    {
        Ok(ex) => {
            if let Poll::Done(g) = parse_poll(&ex.body) {
                say("token exchange succeeded WITH an Origin header.");
                (Some(g), true, Some(ex.status), ex.header_names)
            } else {
                let (error, aadsts) = aad_error(&ex.body);
                say(&format!(
                    "second token attempt (with Origin): HTTP {}, error {}, AADSTS {}",
                    ex.status,
                    error.as_deref().unwrap_or("?"),
                    aadsts.map_or("?".into(), |c| c.to_string())
                ));
                (None, true, Some(ex.status), ex.header_names)
            }
        }
        Err(e) => {
            say(&format!("second token attempt failed: {e}"));
            (None, true, None, vec![])
        }
    }
}

/// Stage 1 (PKCE): bind a loopback listener, open the authorize URL, take the redirect back, and
/// exchange the code for a token. Records which flow, the client, the redirect shape, whether an
/// `Origin` header was needed, and the grant's shape-only fields.
async fn stage_token_pkce(
    http: &reqwest::Client,
    client: SpikeClient,
    rec: &mut Map<String, Value>,
) -> Option<Grant> {
    say(&format!(
        "== 1. token: auth-code + PKCE loopback on /consumers with the {} client",
        client.name()
    ));
    // Bind first, so the redirect_uri names a real port.
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            say(&format!("could not bind a loopback port: {e}"));
            return None;
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            say(&format!("could not read the loopback port: {e}"));
            return None;
        }
    };
    // `localhost` is what the authorize probe used; 127.0.0.1 is a registered fallback.
    let redirect_uri = format!("http://localhost:{port}/");
    let shape = redirect_uri_shape(&redirect_uri);

    let verifier = new_verifier();
    let challenge = pkce_challenge(&verifier);
    let state = new_state();
    let url = authorize_url(client, &redirect_uri, &challenge, &state);

    say(&format!("redirect_uri: {shape}"));
    if spawn_browser(&url) {
        say("opening the sign-in page in your browser; sign in with your personal account once.");
        say(
            "if no window opened, paste this URL (it holds no token, only the public challenge and state):",
        );
    } else {
        say("could not launch a browser; paste this URL to sign in (it holds no token):");
    }
    // Printed raw, not via scrub(): the authorize URL carries only the public S256 challenge, the
    // anti-CSRF state and the loopback redirect. The verifier and the code are never in it.
    println!("{url}");
    say("waiting for the redirect back to localhost. Do not rerun this in a loop.");

    let mut pkce = json!({
        "authorize_endpoint": format!("{CONSUMERS_AUTHORITY}/authorize"),
        "redirect_uri_shape": shape,
        "response_type": "code",
        "code_challenge_method": "S256",
        "response_mode": "query",
    });

    let query = match await_redirect(listener).await {
        Ok(q) => q,
        Err(e) => {
            say(&format!("stopped waiting for the redirect: {e}"));
            pkce["redirect_received"] = json!(false);
            rec.insert("pkce".into(), pkce);
            return None;
        }
    };
    let redirect = parse_redirect_query(&query);
    pkce["redirect_received"] = json!(true);
    pkce["redirect_has_code"] = json!(redirect.code.is_some());
    pkce["redirect_has_state"] = json!(redirect.state.is_some());
    pkce["redirect_error"] = json!(redirect.error.as_deref().map(safe_word));

    let code = match accept_code(&redirect, &state) {
        Ok(c) => c,
        Err(e) => {
            say(&format!("redirect rejected: {e}"));
            pkce["state_verified"] = json!(false);
            rec.insert("pkce".into(), pkce);
            return None;
        }
    };
    pkce["state_verified"] = json!(true);
    say("redirect received, state matches; exchanging the code for a token.");

    let req = code_exchange_request(client, &code, &redirect_uri, &verifier);
    let (grant, origin_needed, status, headers) = exchange_code(http, &req).await;
    pkce["origin_needed"] = json!(origin_needed);
    rec.insert("pkce".into(), pkce);

    let mut t = json!({
        "status": status,
        "response_headers": headers,
        "origin_needed": origin_needed,
    });
    match &grant {
        Some(g) => record_grant(&mut t, g),
        None => say("token exchange failed; see the status above."),
    }
    rec.insert("token".into(), t);
    grant
}

/// Stage 2: the consumer skypetoken exchange, with the work `authz` headers.
async fn stage_authz(
    http: &reqwest::Client,
    g: &Grant,
    rec: &mut Map<String, Value>,
) -> Option<AuthzSummary> {
    say("== 2. skypetoken: POST authz/consumer");
    let ex = match send(
        http.post(AUTHZ_CONSUMER_URL)
            .header("User-Agent", USER_AGENT)
            .header("Authorization", format!("Bearer {}", g.access_token))
            .header("Accept", "application/json; ver=1.0")
            .header("Content-Length", "0"),
    )
    .await
    {
        Ok(ex) => ex,
        Err(e) => {
            say(&format!("authz request failed: {e}"));
            rec.insert("authz".into(), json!({"network_error": true}));
            return None;
        }
    };
    let mut r = json!({
        "url": AUTHZ_CONSUMER_URL,
        "request_headers": AUTHZ_HEADERS,
        "status": ex.status,
        "response_headers": ex.header_names,
        "body_keys": body_keys(&ex.body),
    });
    say(&format!("HTTP {}", ex.status));
    if !(200..300).contains(&ex.status) {
        say(&format!("error body: {}", error_excerpt(&ex.body)));
        rec.insert("authz".into(), r);
        return None;
    }
    let a = match parse_authz_consumer(&ex.body) {
        Ok(a) => a,
        Err(e) => {
            say(&format!("authz answer is not JSON: {e}"));
            rec.insert("authz".into(), r);
            return None;
        }
    };
    say(&format!("answer keys: {}", a.top_keys.join(", ")));
    say(&format!(
        "skypeToken present: {}{}, expiresIn: {}, region present: {}",
        yes(a.skype_token.is_some()),
        a.token_path.map_or(String::new(), |p| format!(" (at {p})")),
        a.expires_in.map_or("absent".into(), |s| format!("{s} s")),
        yes(a.region_present)
    ));
    say(&format!(
        "regionGtms keys: {}",
        a.gtms
            .iter()
            .map(|e| format!("{} ({})", e.key, e.shape))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for e in a.gtms.iter().filter(|e| e.chat_url.is_some()) {
        say(&format!(
            "  {} = {}",
            e.key,
            e.chat_url.as_deref().unwrap_or_default()
        ));
    }
    r["skype_token_present"] = json!(a.skype_token.is_some());
    r["skype_token_path"] = json!(a.token_path);
    r["expires_in"] = json!(a.expires_in);
    r["region_present"] = json!(a.region_present);
    r["region_gtms"] = Value::Array(
        a.gtms
            .iter()
            .map(|e| json!({"key": e.key, "shape": e.shape, "chat_url": e.chat_url}))
            .collect(),
    );
    rec.insert("authz".into(), r);
    Some(a)
}

/// Stage 3: the first five conversations, msgapi first, then the chat hosts from `regionGtms`.
/// Each host is asked once. Returns the number of rows on success.
async fn stage_chats(
    http: &reqwest::Client,
    a: &AuthzSummary,
    skype: &str,
    rec: &mut Map<String, Value>,
) -> Option<usize> {
    say("== 3. chat list: first 5 conversations");
    let mut attempts = vec![];
    let mut found = None;
    for base in chat_candidates(a) {
        if !chat_host_allowed(&base) {
            say(&format!(
                "skip {base}: not msgapi.teams.live.com or teams.live.com{LIVE_CHATSVC_PATH}, the skypetoken is not sent there"
            ));
            attempts.push(json!({"base": base, "skipped": "host not allowed"}));
            continue;
        }
        let ex = match send(
            http.get(conversations_url(&base))
                .header("User-Agent", USER_AGENT)
                .header("Authentication", format!("skypetoken={skype}"))
                .header("X-Skypetoken", skype)
                .header("Accept", "application/json; ver=1.0;")
                .header("Origin", LIVE_ORIGIN)
                .header("Referer", format!("{LIVE_ORIGIN}/")),
        )
        .await
        {
            Ok(ex) => ex,
            Err(e) => {
                say(&format!("{base}: request failed: {e}"));
                attempts.push(json!({"base": base, "network_error": true}));
                continue;
            }
        };
        say(&format!("{base}: HTTP {}", ex.status));
        let mut at = json!({
            "base": base,
            "query": ["view", "pageSize"],
            "request_headers": CHAT_HEADERS,
            "status": ex.status,
            "response_headers": ex.header_names,
        });
        if !(200..300).contains(&ex.status) {
            say(&format!("error body: {}", error_excerpt(&ex.body)));
            attempts.push(at);
            continue;
        }
        match parse_conversations(&ex.body) {
            Ok((top, rows)) => {
                for r in &rows {
                    say(&format!(
                        "  {:<24} type {:<10} members {}",
                        r.id_shape,
                        r.thread_type.as_deref().unwrap_or("?"),
                        r.members.map_or("?".into(), |m| m.to_string())
                    ));
                }
                at["body_keys"] = json!(top);
                at["rows"] = Value::Array(
                    rows.iter()
                        .map(|r| json!({"id_shape": r.id_shape, "thread_type": r.thread_type, "members": r.members}))
                        .collect(),
                );
                attempts.push(at);
                found = Some(rows.len());
                break;
            }
            Err(e) => {
                say(&format!("answer not understood: {e}"));
                at["body_keys"] = json!(body_keys(&ex.body));
                attempts.push(at);
            }
        }
    }
    rec.insert("chat_list".into(), json!({"attempts": attempts}));
    found
}

async fn personal_login(args: &[String]) -> Result<()> {
    let opts = parse_opts(args)?;
    // No redirects: a token must never follow one to another host.
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut rec = Map::new();
    rec.insert("tool".into(), json!("ironteams-cli spike personal-login"));
    rec.insert("fabricated".into(), json!(false));
    rec.insert("time_ms".into(), json!(teams_proto::time::now_ms()));
    rec.insert("flow".into(), json!(opts.flow.name()));
    rec.insert("client".into(), json!(opts.client.name()));
    rec.insert(
        "scope".into(),
        json!(match opts.flow {
            SpikeFlow::Pkce => PKCE_SCOPE,
            SpikeFlow::DeviceCode => SCOPE,
        }),
    );

    let grant = match opts.flow {
        SpikeFlow::DeviceCode => stage_token(&http, opts.client, &mut rec).await,
        SpikeFlow::Pkce => stage_token_pkce(&http, opts.client, &mut rec).await,
    };
    let authz = match &grant {
        Some(g) => stage_authz(&http, g, &mut rec).await,
        None => None,
    };
    let skype = authz.as_ref().and_then(|a| a.skype_token.clone());
    let chats = match (&authz, &skype) {
        (Some(a), Some(s)) => stage_chats(&http, a, s, &mut rec).await,
        _ => None,
    };

    let verdict = [
        ("token", grant.is_some(), "no access token"),
        ("skypetoken", skype.is_some(), "no skypetoken"),
        ("chat list", chats.is_some(), "no conversation list"),
    ];
    say("== result");
    for (stage, ok, why) in verdict {
        let detail = match (stage, ok) {
            ("chat list", true) => format!("{} conversations read", chats.unwrap_or(0)),
            (_, true) => "ok".into(),
            (_, false) => why.into(),
        };
        say(&format!(
            "{:<6} {stage:<11} {detail}",
            if ok { "GO" } else { "NO-GO" }
        ));
    }
    rec.insert(
        "verdict".into(),
        json!({
            "token": grant.is_some(),
            "skypetoken": skype.is_some(),
            "chat_list": chats.is_some(),
        }),
    );
    if let Some(out) = &opts.out {
        let text = scrub(&serde_json::to_string_pretty(&Value::Object(rec))?);
        std::fs::write(out, text + "\n")?;
        println!("Wrote the scrubbed transcript to {}", out.display());
    }
    // The tokens are dropped here: nothing was written to the keyring or the config.
    Ok(())
}

pub async fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("personal-login") => personal_login(&args[1..]).await,
        _ => bail!("usage: {USAGE}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64url(data: &[u8]) -> String {
        b64url_encode(data)
    }

    /// A fabricated JWT, built at run time so no token-looking literal sits in the source.
    fn fake_jwt(payload: &Value) -> String {
        format!(
            "{}.{}.{}",
            b64url(br#"{"alg":"none","typ":"JWT"}"#),
            b64url(payload.to_string().as_bytes()),
            b64url(b"fabricated-signature-not-a-real-one")
        )
    }

    fn form_pairs(body: &str) -> Vec<(String, String)> {
        url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect()
    }

    fn pair<'a>(p: &'a [(String, String)], k: &str) -> Option<&'a str> {
        p.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn base64url_round_trip() {
        for s in ["", "a", "ab", "abc", "abcd", "{\"tid\":\"x\"}"] {
            assert_eq!(b64url_decode(&b64url(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert!(b64url_decode("not base64!").is_none());
    }

    #[test]
    fn scrub_removes_a_jwt() {
        let jwt = fake_jwt(&json!({"tid": CONSUMER_TENANT, "aud": "x"}));
        assert!(jwt.starts_with("eyJ"));
        for text in [
            jwt.clone(),
            format!("Authorization: Bearer {jwt}"),
            format!("{{\"skypeToken\":\"{jwt}\"}}"),
            format!("skypetoken={jwt}; next"),
        ] {
            let out = scrub(&text);
            assert!(!out.contains(&jwt), "{out}");
            assert!(!out.contains("eyJ"), "{out}");
            // No piece of the payload segment survives either.
            let payload = jwt.split('.').nth(1).unwrap();
            assert!(!out.contains(payload), "{out}");
        }
        assert!(scrub("skypetoken=JWT; next").contains("next"));
    }

    #[test]
    fn scrub_removes_opaque_tickets_and_keeps_hosts_guids_and_codes() {
        let ticket = "EwBwA8l6BAAUbDba3x2OMJElkF7gJ4z0a2PxzbYAAc5mQ7Kk9z0vYgm9Ajk1bH8ZQ2";
        let out = scrub(&format!("refresh_token: {ticket} end"));
        assert!(!out.contains(ticket), "{out}");
        assert!(out.ends_with(" end"));
        for keep in [
            "https://msgapi.teams.live.com/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=5",
            "https://teams.live.com/api/chatsvc/consumer",
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode",
            CONSUMER_TENANT,
            "Open https://www.microsoft.com/link and enter the code GRUE55CDY",
            "https://auth.fl.teams.microsoft.com/teams.auth.readwrite offline_access",
        ] {
            assert_eq!(scrub(keep), keep);
        }
    }

    #[test]
    fn error_excerpt_redacts_and_truncates() {
        let jwt = fake_jwt(&json!({"a": 1}));
        let body = format!(
            "{{\"message\":\"bad token {jwt}\",\"user\":\"someone@outlook.com\",\"pad\":\"{}\"}}",
            "x ".repeat(400)
        );
        let out = error_excerpt(&body);
        assert!(!out.contains("eyJ"), "{out}");
        assert!(!out.contains("someone@outlook.com"), "{out}");
        assert!(out.chars().count() <= 301);
    }

    #[test]
    fn token_lines_never_hold_the_token() {
        let jwt = fake_jwt(&json!({
            "tid": CONSUMER_TENANT,
            "aud": "https://auth.fl.teams.microsoft.com",
            "oid": "00000000-0000-0000-0000-000000000001",
            "name": "Fabricated Person",
        }));
        let g = Grant {
            access_token: jwt.clone(),
            refresh_present: true,
            scopes: vec!["https://auth.fl.teams.microsoft.com/teams.auth.readwrite".into()],
            expires_in: Some(3600),
            refresh_token_expires_in: None,
            token_type: Some("Bearer".into()),
            field_names: vec!["access_token".into(), "refresh_token".into()],
        };
        let j = jwt_summary(&jwt);
        assert!(j.is_jwt);
        assert_eq!(j.tid.as_deref(), Some(CONSUMER_TENANT));
        assert_eq!(
            j.aud.as_deref(),
            Some("https://auth.fl.teams.microsoft.com")
        );
        let text = token_lines(&g, &j).join("\n");
        assert!(!text.contains(&jwt));
        assert!(!text.contains("eyJ"));
        assert!(
            !text.contains("Fabricated Person"),
            "claim values stay hidden"
        );
        assert!(!text.contains("00000000-0000-0000-0000-000000000001"));
        assert!(text.contains("claim names: "));
        assert!(text.contains("refresh token present: yes"));
        assert!(text.contains("(the personal-account tenant)"));
    }

    #[test]
    fn opaque_token_is_not_a_jwt() {
        let j = jwt_summary("EwBwA8l6BAAUbDba3x2OMJElkF7gJ4z0a2Pxzb");
        assert!(!j.is_jwt);
        assert!(j.tid.is_none());
    }

    #[test]
    fn device_code_request_uses_consumers_v2_and_the_scope() {
        let r = device_code_request(SpikeClient::Office);
        assert_eq!(
            r.url,
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode"
        );
        let p = form_pairs(&r.body);
        assert_eq!(
            pair(&p, "client_id"),
            Some("d3590ed6-52b3-4102-aeff-aad2292ab01c")
        );
        assert_eq!(
            pair(&p, "scope"),
            Some("https://auth.fl.teams.microsoft.com/teams.auth.readwrite offline_access")
        );
        assert_eq!(
            pair(&p, "resource"),
            None,
            "v2 takes a scope, not a resource"
        );
        let az = form_pairs(&device_code_request(SpikeClient::AzCli).body);
        assert_eq!(
            pair(&az, "client_id"),
            Some("04b07795-8ddb-461a-bbee-02f9e1bf7b46")
        );
    }

    #[test]
    fn token_poll_request_is_the_device_code_grant() {
        let r = token_poll_request(SpikeClient::AzCli, "DC-fabricated");
        assert_eq!(
            r.url,
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/token"
        );
        let p = form_pairs(&r.body);
        assert_eq!(
            pair(&p, "grant_type"),
            Some("urn:ietf:params:oauth:grant-type:device_code")
        );
        assert_eq!(pair(&p, "device_code"), Some("DC-fabricated"));
        assert_eq!(pair(&p, "client_id"), Some(SpikeClient::AzCli.id()));
    }

    #[test]
    fn conversations_url_and_host_allowlist() {
        assert_eq!(
            conversations_url(MSGAPI_BASE),
            "https://msgapi.teams.live.com/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=5"
        );
        assert_eq!(
            conversations_url("https://teams.live.com/api/chatsvc/consumer/"),
            "https://teams.live.com/api/chatsvc/consumer/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=5"
        );
        for ok in [
            "https://msgapi.teams.live.com",
            "https://teams.live.com/api/chatsvc/consumer",
            "https://teams.live.com/api/chatsvc/consumer/",
        ] {
            assert!(chat_host_allowed(ok), "{ok}");
        }
        for bad in [
            "http://msgapi.teams.live.com",
            "https://msgapi.teams.live.com:8443",
            "https://teams.live.com",
            "https://teams.live.com/api/mt",
            "https://teams.live.com/api/chatsvc/consumerx",
            "https://teams.live.com.evil.example/api/chatsvc/consumer",
            "https://evilteams.live.com/api/chatsvc/consumer",
            "https://x.msgapi.teams.live.com",
            "https://emea.ng.msg.teams.microsoft.com",
            "https://user@msgapi.teams.live.com",
            "not a url",
        ] {
            assert!(!chat_host_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn options() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // The default is unchanged: device code with the Office client.
        assert_eq!(
            parse_opts(&[]).unwrap(),
            Opts {
                flow: SpikeFlow::DeviceCode,
                client: SpikeClient::Office,
                out: None
            }
        );
        assert_eq!(
            parse_opts(&a(&["--client", "azcli", "--out", "t.json"])).unwrap(),
            Opts {
                flow: SpikeFlow::DeviceCode,
                client: SpikeClient::AzCli,
                out: Some(PathBuf::from("t.json"))
            }
        );
        // --flow pkce defaults the client to teamsweb.
        assert_eq!(
            parse_opts(&a(&["--flow", "pkce"])).unwrap(),
            Opts {
                flow: SpikeFlow::Pkce,
                client: SpikeClient::TeamsWeb,
                out: None
            }
        );
        // An explicit --client overrides the per-flow default.
        assert_eq!(
            parse_opts(&a(&["--flow", "pkce", "--client", "office"])).unwrap(),
            Opts {
                flow: SpikeFlow::Pkce,
                client: SpikeClient::Office,
                out: None
            }
        );
        assert!(parse_opts(&a(&["--client", "teams"])).is_err());
        assert!(parse_opts(&a(&["--flow", "implicit"])).is_err());
        assert!(parse_opts(&a(&["--out"])).is_err());
        assert!(parse_opts(&a(&["--retry"])).is_err());
    }

    #[test]
    fn poll_answers() {
        assert!(matches!(
            parse_poll(r#"{"error":"authorization_pending"}"#),
            Poll::Pending
        ));
        assert!(matches!(
            parse_poll(r#"{"error":"slow_down"}"#),
            Poll::SlowDown
        ));
        match parse_poll(
            r#"{"error":"authorization_declined","error_description":"AADSTS70000: declined.\r\nTrace ID: x"}"#,
        ) {
            Poll::Failed { error, aadsts } => {
                assert_eq!(error, "authorization_declined");
                assert_eq!(aadsts, Some(70000));
            }
            _ => panic!("declined must fail"),
        }
        assert!(matches!(
            parse_poll(r#"{"error":"expired_token","error_codes":[70020]}"#),
            Poll::Failed {
                aadsts: Some(70020),
                ..
            }
        ));
        // Fabricated shape: the values are placeholders, not tokens.
        match parse_poll(
            r#"{"token_type":"Bearer","scope":"https://auth.fl.teams.microsoft.com/teams.auth.readwrite","expires_in":"3599",
                "access_token":"AT","refresh_token":"RT","refresh_token_expires_in":86399}"#,
        ) {
            Poll::Done(g) => {
                assert_eq!(g.access_token, "AT");
                assert!(g.refresh_present);
                assert_eq!(g.scopes.len(), 1);
                assert_eq!(g.expires_in, Some(3599));
                assert_eq!(g.refresh_token_expires_in, Some(86399));
                assert!(g.field_names.contains(&"refresh_token".to_string()));
            }
            _ => panic!("a grant"),
        }
    }

    /// FABRICATED SHAPE, NOT A CAPTURE. The key names follow the work `authz` answer and the
    /// "consumer answer" note in teams-proto `parse_authz` (`regionGtms` values may be objects
    /// with `endpointUrl`); every value is a placeholder. Replace it with the scrubbed live shape
    /// once the spike has run.
    const FABRICATED_AUTHZ_CONSUMER: &str = r#"{
        "skypeToken": {"skypetoken": "FABRICATED-SKYPETOKEN", "expiresIn": 86399},
        "region": "fabricated-region",
        "regionGtms": {
            "chatService": "https://teams.live.com/api/chatsvc/consumer/",
            "chatServiceAfd": {"endpointUrl": "https://msgapi.teams.live.com"},
            "chatSvcAggAfd": {"endpointUrl": "https://chat.fabricated.example"},
            "middleTier": "https://teams.live.com/api/mt",
            "presence": {"endpointUrl": "https://presence.teams.live.com"},
            "unifiedPresence": null
        }
    }"#;

    #[test]
    fn parses_the_fabricated_authz_consumer_shape() {
        let a = parse_authz_consumer(FABRICATED_AUTHZ_CONSUMER).unwrap();
        assert_eq!(a.skype_token.as_deref(), Some("FABRICATED-SKYPETOKEN"));
        assert_eq!(a.token_path, Some("skypeToken.skypetoken"));
        assert_eq!(a.expires_in, Some(86399));
        assert!(a.region_present);
        assert_eq!(a.top_keys, ["region", "regionGtms", "skypeToken"]);
        let entry = |k: &str| a.gtms.iter().find(|e| e.key == k).unwrap();
        assert_eq!(entry("chatService").shape, "string");
        assert_eq!(
            entry("chatService").chat_url.as_deref(),
            Some("https://teams.live.com/api/chatsvc/consumer")
        );
        assert_eq!(entry("chatServiceAfd").shape, "object");
        assert_eq!(entry("unifiedPresence").shape, "null");
        assert!(
            entry("middleTier").chat_url.is_none(),
            "only chat hosts are shown"
        );
        assert!(entry("presence").chat_url.is_none());
        // msgapi first, then chatService, the duplicate msgapi dropped; the unknown host is a
        // candidate but the allowlist keeps the token from it.
        let c = chat_candidates(&a);
        assert_eq!(
            c,
            [
                "https://msgapi.teams.live.com",
                "https://teams.live.com/api/chatsvc/consumer",
                "https://chat.fabricated.example",
            ]
        );
        assert!(!chat_host_allowed(&c[2]));
    }

    #[test]
    fn parses_the_work_authz_shape_and_a_refusal() {
        let a = parse_authz_consumer(
            r#"{"tokens":{"skypeToken":"FABRICATED","expiresIn":"86400"},"regionGtms":{}}"#,
        )
        .unwrap();
        assert_eq!(a.token_path, Some("tokens.skypeToken"));
        assert_eq!(a.expires_in, Some(86400));
        assert!(!a.region_present);
        assert_eq!(chat_candidates(&a), [MSGAPI_BASE]);
        let refused =
            parse_authz_consumer(r#"{"status":{"code":403,"text":"fabricated refusal"}}"#).unwrap();
        assert!(refused.skype_token.is_none());
        assert!(parse_authz_consumer("<html>").is_err());
    }

    #[test]
    fn id_shapes_hide_the_unique_part() {
        assert_eq!(id_shape("19:abc123def@thread.v2"), "19:…@thread.v2");
        assert_eq!(
            id_shape("19:aaaa_bbbb@unq.gbl.spaces"),
            "19:…@unq.gbl.spaces"
        );
        assert_eq!(id_shape("48:notes"), "48:notes");
        assert_eq!(id_shape("48:calllogs"), "48:calllogs");
        assert_eq!(id_shape("8:live:someone.fabricated"), "8:live:…");
        assert_eq!(id_shape("8:orgid:00000000-0000"), "8:orgid:…");
        assert_eq!(id_shape("plainid"), "…");
        assert_eq!(id_shape(""), "…");
    }

    /// FABRICATED SHAPE, NOT A CAPTURE: an msnp24Equivalent conversation page with placeholder
    /// ids and a topic that must never be printed.
    #[test]
    fn conversation_rows_show_shape_only() {
        let body = r#"{"conversations":[
            {"id":"19:fabricatedA@thread.v2","threadProperties":{"threadType":"chat","membercount":"2","topic":"Secret topic"}},
            {"id":"48:notes","threadProperties":{"threadType":"streamofnotes"}},
            {"id":"19:fabricatedB@thread.v2","members":[{},{},{}],"lastMessage":{"content":"hello"}},
            {"id":"19:c@thread.v2"},{"id":"19:d@thread.v2"},{"id":"19:e@thread.v2"}
        ],"_metadata":{"syncState":"fabricated"}}"#;
        let (top, rows) = parse_conversations(body).unwrap();
        assert_eq!(top, ["_metadata", "conversations"]);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].id_shape, "19:…@thread.v2");
        assert_eq!(rows[0].thread_type.as_deref(), Some("chat"));
        assert_eq!(rows[0].members, Some(2));
        assert_eq!(rows[1].id_shape, "48:notes");
        assert_eq!(rows[2].members, Some(3));
        assert!(rows[3].thread_type.is_none());
        assert!(parse_conversations(r#"{"errorCode":911}"#).is_err());
    }

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        // The RFC 7636 Appendix B vector.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_verifier_is_a_valid_pkce_verifier() {
        let v = new_verifier();
        assert_eq!(v.len(), 43);
        assert!((43..=128).contains(&v.len()));
        // Every character is PKCE-unreserved (base64url uses a subset of it).
        assert!(
            v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b)),
            "{v}"
        );
        // Two draws differ (not a constant).
        assert_ne!(new_verifier(), new_verifier());
    }

    #[test]
    fn authorize_url_has_the_right_endpoint_and_params() {
        let url = authorize_url(
            SpikeClient::TeamsWeb,
            "http://localhost:49152/",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "STATE-fabricated",
        );
        assert!(
            url.starts_with("https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?"),
            "{url}"
        );
        let u = url::Url::parse(&url).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(
            q.get("client_id").map(String::as_str),
            Some("4b3e8f46-56d3-427f-b1e2-d239b2ea6bca")
        );
        assert_eq!(q.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(q.get("scope").map(String::as_str), Some(PKCE_SCOPE));
        assert_eq!(
            q.get("code_challenge").map(String::as_str),
            Some("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        );
        assert_eq!(
            q.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(q.get("state").map(String::as_str), Some("STATE-fabricated"));
        assert_eq!(
            q.get("redirect_uri").map(String::as_str),
            Some("http://localhost:49152/")
        );
        assert_eq!(q.get("response_mode").map(String::as_str), Some("query"));
    }

    #[test]
    fn redirect_handler_parses_code_and_state_and_rejects_a_mismatch() {
        let q = request_target_query("GET /?code=AUTHCODE-fabricated&state=S1 HTTP/1.1\r\n")
            .expect("a query");
        assert_eq!(q, "code=AUTHCODE-fabricated&state=S1");
        let r = parse_redirect_query(&q);
        assert_eq!(r.code.as_deref(), Some("AUTHCODE-fabricated"));
        assert_eq!(r.state.as_deref(), Some("S1"));
        // The state must match.
        assert_eq!(accept_code(&r, "S1").unwrap(), "AUTHCODE-fabricated");
        let mismatch = accept_code(&r, "S2").unwrap_err().to_string();
        assert!(mismatch.contains("state"), "{mismatch}");
        assert!(
            !mismatch.contains("AUTHCODE-fabricated"),
            "the code must not be in the error"
        );
        // An authorize error is surfaced, no code.
        let err = parse_redirect_query("error=access_denied&state=S1");
        assert!(accept_code(&err, "S1").is_err());
        // A request line with no query is not a redirect.
        assert!(request_target_query("GET / HTTP/1.1\r\n").is_none());
    }

    #[test]
    fn code_exchange_request_is_the_auth_code_grant() {
        let r = code_exchange_request(
            SpikeClient::TeamsWeb,
            "AUTHCODE-fabricated",
            "http://localhost:49152/",
            "VERIFIER-fabricated",
        );
        assert_eq!(
            r.url,
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/token"
        );
        let p = form_pairs(&r.body);
        assert_eq!(pair(&p, "grant_type"), Some("authorization_code"));
        assert_eq!(pair(&p, "code"), Some("AUTHCODE-fabricated"));
        assert_eq!(pair(&p, "code_verifier"), Some("VERIFIER-fabricated"));
        assert_eq!(pair(&p, "redirect_uri"), Some("http://localhost:49152/"));
        assert_eq!(
            pair(&p, "client_id"),
            Some("4b3e8f46-56d3-427f-b1e2-d239b2ea6bca")
        );
        assert_eq!(
            pair(&p, "client_secret"),
            None,
            "a public client has no secret"
        );
    }

    #[test]
    fn needs_origin_spots_the_spa_refusal() {
        assert!(needs_origin(
            r#"{"error":"invalid_request","error_description":"AADSTS9002327: Tokens issued for the 'Single-Page Application' client-type may only be redeemed via cross-origin requests."}"#
        ));
        assert!(!needs_origin(
            r#"{"error":"invalid_grant","error_description":"AADSTS70008: the code expired."}"#
        ));
    }

    #[test]
    fn scrub_hides_an_auth_code_and_a_verifier() {
        // An auth code and a PKCE verifier are 32+ mixed-case alphanumeric runs, so the scrubber
        // redacts them; the surrounding words survive. Fixed shapes keep the test deterministic
        // (the verifier is the RFC 7636 vector, a public value, not a real secret).
        let code = "M.C107_BAY.2.U.a1B2c3D4e5F6g7H8i9J0kLmNoPqRsTuVwXyZ01234";
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let out = scrub(&format!("code={code} verifier={verifier} end"));
        assert!(!out.contains(code), "{out}");
        assert!(!out.contains(verifier), "{out}");
        assert!(out.ends_with(" end"));
    }
}
