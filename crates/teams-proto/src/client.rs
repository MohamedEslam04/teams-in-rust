//! Authenticated client: owns tokens, refreshes them before they expire, and sends requests
//! with the right headers for each backend.

use crate::auth::{self, DeviceCode, SkypeToken};
use crate::broker::{AppToken, Audience, TokenCache};
use crate::client_info::{
    AUTHORITY, CLIENT_ID, RESOURCE_CSA, RESOURCE_GRAPH, RESOURCE_PRESENCE, RESOURCE_SPACES,
    USER_AGENT, scope_for,
};
use crate::error::{Error, Result};
use crate::http::{Tokens, headers_for};
use reqwest::Method;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex;
use url::Url;

/// What an app persists to resume a session: store `refresh_token` in the system keyring and
/// `epid` anywhere (it is not secret).
#[derive(Clone)]
pub struct Session {
    pub tenant: String,
    pub refresh_token: String,
    /// Endpoint id, a UUID reused across runs so Teams does not hit its endpoint limit.
    pub epid: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("tenant", &self.tenant)
            .field("epid", &self.epid)
            .finish_non_exhaustive()
    }
}

/// Generates a new endpoint id.
pub fn new_epid() -> String {
    uuid::Uuid::new_v4().to_string()
}

struct State {
    tokens: Tokens,
    refresh_token: String,
    /// Earliest expiry across the tokens we hold.
    /// Wall-clock expiry. `Instant` would stop during system suspend and miss expiry after sleep.
    expires_at: SystemTime,
    chat_service: String,
    middle_tier: String,
    presence: String,
    /// `regionGtms.mcpService`, when the skypetoken answer has it.
    mcp_service: Option<String>,
    /// The geography the `regionGtms` hosts name (`SkypeToken::geo`).
    geo: Option<String>,
    region: Option<String>,
    /// Tokens for every other audience, minted on first use.
    cache: TokenCache,
}

/// Request body for [`Client::request_raw_as`].
pub enum RawBody<'a> {
    None,
    Json(&'a Value),
    /// A content type and the bytes to send.
    Bytes(&'a str, Vec<u8>),
}

/// Response of [`Client::request_raw_as`].
pub struct RawResponse {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}

/// Authenticated Teams client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    tenant: String,
    epid: String,
    state: Arc<Mutex<State>>,
}

/// Installs the rustls crypto provider once (both reqwest and the websocket use rustls). It is
/// ring, the provider the WebRTC stack needs anyway; reqwest has no built-in one
/// (`rustls-no-provider`), so a client built before this call panics.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn build_http() -> Result<reqwest::Client> {
    install_crypto_provider();
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        // Close idle connections before the servers or a suspend do, so fewer requests start on
        // a dead one.
        .pool_idle_timeout(Duration::from_secs(50))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?)
}

/// Wait before a read that got no answer is sent again.
const RESEND_DELAY: Duration = Duration::from_millis(300);

/// True when a request that failed with `err` may be sent again as it is: a read (GET or HEAD)
/// whose connection failed or broke before an answer. A timeout is not resent: the server may be
/// slow, and a second wait doubles the delay.
fn resend_safe(method: &Method, err: &reqwest::Error) -> bool {
    (*method == Method::GET || *method == Method::HEAD)
        && (err.is_connect() || err.is_request())
        && !err.is_timeout()
}

/// Seconds before expiry at which tokens are refreshed.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);

/// True when tokens expiring at `expires_at` must be refreshed at wall-clock time `now`.
fn refresh_due(expires_at: SystemTime, now: SystemTime) -> bool {
    expires_at <= now + REFRESH_MARGIN
}

/// True when `url` is an https URL on the host of `aud`. Loki also runs per region, as
/// `nam.loki.delve.office.com`, and takes the same token. App tokens are for the app, not for
/// IronTeams's own requests, so an app audience owns nothing.
fn audience_owns(aud: &Audience, url: &Url) -> bool {
    if matches!(aud, Audience::App(_)) {
        return false;
    }
    // The provisioning token is minted for `templates.teams.microsoft.com` but sent to the
    // `/fabric/` path of the main Teams host.
    if *aud == Audience::Templates {
        return url.scheme() == "https"
            && url.host_str().map(str::to_ascii_lowercase).as_deref()
                == Some("teams.microsoft.com")
            && url.path().starts_with("/fabric/");
    }
    // The meeting content service is on the Teams web hosts (`regionGtms.mcpService`); its
    // resource is an app id, not a host.
    if *aud == Audience::MeetingContent {
        return url.scheme() == "https"
            && matches!(
                url.host_str().map(str::to_ascii_lowercase).as_deref(),
                Some("teams.microsoft.com" | "teams.cloud.microsoft")
            )
            && url.path().starts_with("/api/mcps/");
    }
    // Teams web sends the presence token to the regional unified presence service
    // (`regionGtms.presenceUPS`, under `/ups/` on the Teams web host), not to the resource host.
    if *aud == Audience::Presence
        && url.scheme() == "https"
        && matches!(
            url.host_str().map(str::to_ascii_lowercase).as_deref(),
            Some("teams.microsoft.com" | "teams.cloud.microsoft")
        )
        && url.path().starts_with("/ups/")
    {
        return true;
    }
    let Ok(res) = Url::parse(&aud.resource()) else {
        return false;
    };
    let (Some(host), Some(want)) = (url.host_str(), res.host_str()) else {
        return false;
    };
    let (host, want) = (host.to_ascii_lowercase(), want.to_ascii_lowercase());
    url.scheme() == "https"
        && (host == want
            || (*aud == Audience::Loki && host.ends_with(&format!(".{want}")))
            || api_host(aud) == Some(host.as_str()))
}

/// [`auth::refresh`] with an optional `claims` request parameter (conditional access).
async fn mint_with_claims(
    http: &reqwest::Client,
    tenant: &str,
    refresh_token: &str,
    resource: &str,
    claims: Option<&str>,
) -> Result<auth::TokenResponse> {
    let scope = scope_for(resource);
    let mut form = vec![
        ("client_id", CLIENT_ID),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", scope.as_str()),
    ];
    if let Some(c) = claims {
        form.push(("claims", c));
    }
    let resp = http
        .post(format!("{AUTHORITY}/{tenant}/oauth2/v2.0/token"))
        .header("User-Agent", USER_AGENT)
        .form(&form)
        .send()
        .await?;
    match auth::parse_token_response(&resp.text().await?)? {
        auth::PollOutcome::Done(t) => Ok(t),
        _ => Err(Error::Auth(
            "unexpected pending response to a refresh".into(),
        )),
    }
}

/// [`Client::mcp_service`] of a state.
fn mcps_of(st: &State) -> String {
    match &st.mcp_service {
        Some(u) => u.clone(),
        None => crate::recaps::mcps_base(st.geo.as_deref(), &st.middle_tier),
    }
}

/// The API host of an audience whose token resource is not the host that serves the API.
fn api_host(aud: &Audience) -> Option<&'static str> {
    match aud {
        Audience::Flow => Some("api.flow.microsoft.com"),
        Audience::OneNote => Some("www.onenote.com"),
        Audience::Engage => Some("engage.cloud.microsoft"),
        _ => None,
    }
}

impl Client {
    /// Starts a device-code sign-in. `on_code` is called once with the code to show to the user;
    /// the future resolves when the user has signed in.
    pub async fn sign_in_device_code(
        tenant: &str,
        epid: &str,
        on_code: impl FnOnce(&DeviceCode),
    ) -> Result<Client> {
        let http = build_http()?;
        let dc = auth::start_device_code(&http, tenant).await?;
        on_code(&dc);
        let tok = auth::poll_device_code(&http, tenant, &dc).await?;
        let refresh_token = tok
            .refresh_token
            .clone()
            .ok_or_else(|| Error::Auth("sign-in returned no refresh token".into()))?;
        Self::connect(
            http,
            Session {
                tenant: tenant.into(),
                refresh_token,
                epid: epid.into(),
            },
        )
        .await
    }

    /// Resumes a session from a stored refresh token.
    pub async fn resume(session: Session) -> Result<Client> {
        Self::connect(build_http()?, session).await
    }

    /// A client for a stored session, without touching the network. Its tokens are due, so the
    /// first request refreshes them; [`Client::sign_in_stored`] does that up front.
    pub fn restore(session: Session) -> Result<Client> {
        Ok(Self::unrefreshed(build_http()?, session))
    }

    /// Refreshes every token of a [`Client::restore`]d client: the network half of
    /// [`Client::resume`].
    pub async fn sign_in_stored(&self) -> Result<()> {
        self.refresh_all().await?;
        // Once per start: where Meeting Recaps will look (a host, never a token).
        let st = self.state.lock().await;
        let from = if st.mcp_service.is_some() {
            "regionGtms.mcpService"
        } else if st.geo.is_some() {
            "regionGtms geography"
        } else {
            "middle tier"
        };
        tracing::debug!(base = %mcps_of(&st), from, "meeting content service");
        Ok(())
    }

    /// Takes over the session of `other` (a new sign-in of the same tenant), so every clone of
    /// this client uses its tokens from now on.
    pub async fn adopt(&self, other: Client) {
        let mut theirs = other.state.lock().await;
        let mut mine = self.state.lock().await;
        std::mem::swap(&mut *mine, &mut *theirs);
    }

    async fn connect(http: reqwest::Client, session: Session) -> Result<Client> {
        let client = Self::unrefreshed(http, session);
        client.sign_in_stored().await?;
        Ok(client)
    }

    fn unrefreshed(http: reqwest::Client, session: Session) -> Client {
        let state = State {
            tokens: Tokens::default(),
            refresh_token: session.refresh_token,
            expires_at: SystemTime::UNIX_EPOCH,
            chat_service: String::new(),
            middle_tier: String::new(),
            presence: String::new(),
            mcp_service: None,
            geo: None,
            region: None,
            cache: TokenCache::default(),
        };
        Client {
            http,
            tenant: session.tenant,
            epid: session.epid,
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// A client with no credentials, for demo mode and tests. Every network call fails.
    pub fn offline(epid: &str) -> Result<Client> {
        let state = State {
            tokens: Tokens::default(),
            refresh_token: String::new(),
            expires_at: SystemTime::now() + Duration::from_secs(365 * 24 * 3600),
            chat_service: "https://offline.invalid".into(),
            middle_tier: "https://offline.invalid".into(),
            presence: "https://offline.invalid".into(),
            mcp_service: None,
            geo: None,
            region: None,
            cache: TokenCache::default(),
        };
        Ok(Client {
            http: build_http()?,
            tenant: String::new(),
            epid: epid.into(),
            state: Arc::new(Mutex::new(state)),
        })
    }

    /// Refreshes every access token and the skypetoken. The four token requests (Spaces, CSA,
    /// Presence, Graph) are sent at the same time, each from the stored refresh token; the
    /// skypetoken exchange follows the Spaces token and overlaps the other three.
    pub async fn refresh_all(&self) -> Result<()> {
        let rt = self.state.lock().await.refresh_token.clone();
        let http = &self.http;
        let Fetched {
            spaces,
            csa,
            presence,
            graph,
            sky,
        } = fetch_tokens(
            |resource| auth::refresh(http, &self.tenant, &rt, resource),
            |spaces_token| async move { auth::skypetoken(http, &spaces_token).await },
        )
        .await?;
        // The newest refresh token: CSA's, else Spaces', else the one used.
        let rt = csa
            .refresh_token
            .clone()
            .or_else(|| spaces.refresh_token.clone())
            .unwrap_or(rt);
        let min_s = [
            Some(spaces.expires_in_s),
            Some(csa.expires_in_s),
            presence.as_ref().map(|p| p.expires_in_s),
            graph.as_ref().map(|g| g.expires_in_s),
            Some(sky.expires_in_s),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(3600);
        let now = SystemTime::now();
        let mut st = self.state.lock().await;
        st.refresh_token = rt;
        st.cache.clear();
        st.cache.put(
            &Audience::Spaces,
            spaces.access_token.clone(),
            now,
            spaces.expires_in_s,
        );
        st.cache.put(
            &Audience::Csa,
            csa.access_token.clone(),
            now,
            csa.expires_in_s,
        );
        if let Some(p) = &presence {
            st.cache.put(
                &Audience::Presence,
                p.access_token.clone(),
                now,
                p.expires_in_s,
            );
        }
        if let Some(g) = &graph {
            st.cache.put(
                &Audience::Graph,
                g.access_token.clone(),
                now,
                g.expires_in_s,
            );
        }
        st.tokens = Tokens {
            skype: Some(sky.token),
            spaces: Some(spaces.access_token),
            csa: Some(csa.access_token),
            presence: presence.map(|p| p.access_token),
            graph: graph.map(|g| g.access_token),
        };
        st.expires_at = SystemTime::now() + Duration::from_secs(min_s);
        st.chat_service = sky.chat_service;
        st.middle_tier = sky.middle_tier;
        st.presence = sky.presence;
        st.mcp_service = sky.mcp_service;
        st.geo = sky.geo;
        st.region = sky.region;
        tracing::info!(chat_service = %st.chat_service, region = ?st.region, ttl_s = min_s, "tokens refreshed");
        Ok(())
    }

    /// Refreshes tokens if any of them expires soon.
    pub async fn ensure_fresh(&self) -> Result<()> {
        let due = refresh_due(self.state.lock().await.expires_at, SystemTime::now());
        if due {
            self.refresh_all().await
        } else {
            Ok(())
        }
    }

    /// An access token for `aud`, minted with the refresh token on first use and again shortly
    /// before it expires (wall clock). The rotated refresh token replaces the stored one.
    pub async fn token_for(&self, aud: &Audience) -> Result<String> {
        let rt = {
            let st = self.state.lock().await;
            if let Some(t) = st.cache.get(aud, SystemTime::now()) {
                return Ok(t);
            }
            st.refresh_token.clone()
        };
        let tok = auth::refresh(&self.http, &self.tenant, &rt, &aud.resource()).await?;
        let mut st = self.state.lock().await;
        if let Some(new_rt) = tok.refresh_token {
            st.refresh_token = new_rt;
        }
        st.cache.put(
            aud,
            tok.access_token.clone(),
            SystemTime::now(),
            tok.expires_in_s,
        );
        tracing::debug!(audience = %aud.resource(), ttl_s = tok.expires_in_s, "token minted");
        Ok(tok.access_token)
    }

    /// A token for a Teams app's own resource (app single sign-on), minted from the session's
    /// refresh token with the Teams client id, as Teams does: the app's Entra registration
    /// pre-authorizes the Teams client ids for its `api://<domain>/<app id>`. Cached per
    /// resource like the other audiences. `claims` (a conditional access challenge, JSON) is
    /// passed to the token request and skips the cache. Checking that the asking app owns
    /// `resource` is the caller's job (`webhost::shim::check_token_request`).
    pub async fn app_token(&self, resource: &str, claims: Option<&str>) -> Result<AppToken> {
        let aud = Audience::app(resource)
            .ok_or_else(|| Error::Protocol(format!("not an app resource: {resource:?}")))?;
        let rt = {
            let st = self.state.lock().await;
            if claims.is_none()
                && let Some((access_token, expires_at)) = st.cache.entry(&aud, SystemTime::now())
            {
                return Ok(AppToken {
                    access_token,
                    expires_at,
                });
            }
            st.refresh_token.clone()
        };
        let tok = mint_with_claims(&self.http, &self.tenant, &rt, resource, claims).await?;
        let now = SystemTime::now();
        let mut st = self.state.lock().await;
        if let Some(new_rt) = tok.refresh_token {
            st.refresh_token = new_rt;
        }
        st.cache
            .put(&aud, tok.access_token.clone(), now, tok.expires_in_s);
        tracing::debug!(audience = %resource, ttl_s = tok.expires_in_s, "app token minted");
        Ok(AppToken {
            access_token: tok.access_token,
            expires_at: now + Duration::from_secs(tok.expires_in_s),
        })
    }

    /// Sends a request with a bearer token for `aud`. The token is only attached when `url` is
    /// an https URL on the audience's own host, so it can never leak elsewhere. Retries once
    /// with a fresh token after a 401.
    pub async fn request_as(
        &self,
        aud: &Audience,
        method: Method,
        url: &str,
        body: Option<&Value>,
        extra: &[(&str, &str)],
    ) -> Result<(u16, String)> {
        let body = body.map_or(RawBody::None, RawBody::Json);
        let r = self.request_raw_as(aud, method, url, body, extra).await?;
        Ok((r.status, String::from_utf8_lossy(&r.body).into_owned()))
    }

    /// [`Client::request_as`] with a raw body and a raw response (status, headers, bytes), for
    /// file content and for calls whose answer is a header (`Location` of an async monitor).
    pub async fn request_raw_as(
        &self,
        aud: &Audience,
        method: Method,
        url: &str,
        body: RawBody<'_>,
        extra: &[(&str, &str)],
    ) -> Result<RawResponse> {
        let parsed = Url::parse(url).map_err(|e| Error::Protocol(format!("bad url {url}: {e}")))?;
        if !audience_owns(aud, &parsed) {
            return Err(Error::Protocol(format!(
                "{url} is not a host of {}",
                aud.resource()
            )));
        }
        for attempt in 0..2 {
            let token = self.token_for(aud).await?;
            let mut req = self
                .http
                .request(method.clone(), parsed.clone())
                .header("User-Agent", crate::client_info::USER_AGENT)
                .header("Authorization", format!("Bearer {token}"));
            for (k, v) in extra {
                req = req.header(*k, *v);
            }
            req = match &body {
                RawBody::Json(b) => req.json(*b),
                RawBody::Bytes(content_type, data) => {
                    req.header("Content-Type", *content_type).body(data.clone())
                }
                RawBody::None if method == Method::POST || method == Method::PUT => {
                    req.header("Content-Length", "0")
                }
                RawBody::None => req,
            };
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            if status == 401 && attempt == 0 {
                tracing::warn!(audience = %aud.resource(), "401, minting a new token");
                self.state.lock().await.cache.invalidate(aud);
                continue;
            }
            let headers = resp.headers().clone();
            return Ok(RawResponse {
                status,
                headers,
                body: resp.bytes().await?.to_vec(),
            });
        }
        unreachable!("loop returns on second attempt")
    }

    /// [`Client::request_as`] that parses a JSON response; non-2xx statuses become errors.
    pub async fn json_as(
        &self,
        context: &'static str,
        aud: &Audience,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let (status, text) = self.request_as(aud, method, url, body, &[]).await?;
        if !(200..300).contains(&status) {
            return Err(Error::status(context, status, &text));
        }
        if text.trim().is_empty() {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_str(&text)?)
        }
    }

    /// Current session data to persist (the refresh token rotates).
    pub async fn session(&self) -> Session {
        Session {
            tenant: self.tenant.clone(),
            refresh_token: self.state.lock().await.refresh_token.clone(),
            epid: self.epid.clone(),
        }
    }

    /// Endpoint id of this client.
    pub fn epid(&self) -> &str {
        &self.epid
    }

    /// Chat service base URL for this user's region.
    pub async fn chat_service(&self) -> String {
        self.state.lock().await.chat_service.clone()
    }

    /// Middle-tier base URL for this user's region.
    pub async fn middle_tier(&self) -> String {
        self.state.lock().await.middle_tier.clone()
    }

    /// The meeting content service (`regionGtms.mcpService`), or one guessed from the geography
    /// of the other hosts when the skypetoken answer did not name it (see
    /// [`crate::recaps::mcps_base`]).
    pub async fn mcp_service(&self) -> String {
        mcps_of(&*self.state.lock().await)
    }

    /// Presence service base URL (with `/v1`) for this user's region.
    pub async fn presence_service(&self) -> String {
        self.state.lock().await.presence.clone()
    }

    /// A snapshot of the current tokens (for the trouter connection).
    pub async fn tokens(&self) -> Tokens {
        self.state.lock().await.tokens.clone()
    }

    /// The underlying HTTP client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Sends a request with credentials for its backend. Retries once after a 401 by refreshing.
    pub async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        extra: &[(&str, &str)],
    ) -> Result<(u16, String)> {
        let (status, text, _) = self.request_location(method, url, body, extra).await?;
        Ok((status, text))
    }

    /// [`Client::request`] that also returns the `Location` response header, which is where the
    /// chat service reports the id of a thread it just created.
    pub async fn request_location(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        extra: &[(&str, &str)],
    ) -> Result<(u16, String, Option<String>)> {
        self.ensure_fresh().await?;
        let mut refreshed = false;
        let mut resent = false;
        loop {
            let parsed =
                Url::parse(url).map_err(|e| Error::Protocol(format!("bad url {url}: {e}")))?;
            let tokens = self.tokens().await;
            let mut req = self.http.request(method.clone(), parsed.clone());
            for (k, v) in headers_for(&parsed, &tokens, &self.epid) {
                req = req.header(k, v);
            }
            for (k, v) in extra {
                req = req.header(*k, *v);
            }
            req = match body {
                Some(b) => req.json(b),
                None if method == Method::POST || method == Method::PUT => {
                    req.header("Content-Length", "0")
                }
                None => req,
            };
            let resp = match req.send().await {
                Ok(resp) => resp,
                // A pooled connection the server or a suspend dropped fails at once with "error
                // sending request". A read is safe to send again on a fresh connection; a write
                // is not (it may have arrived), so it reports the error.
                Err(e) if !resent && resend_safe(&method, &e) => {
                    tracing::warn!(%url, err = %e, "request failed, sending it once more");
                    resent = true;
                    tokio::time::sleep(RESEND_DELAY).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = resp.status().as_u16();
            if status == 401 && !refreshed {
                tracing::warn!(%url, "401, refreshing tokens");
                refreshed = true;
                self.refresh_all().await?;
                continue;
            }
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            return Ok((status, resp.text().await?, location));
        }
    }

    /// Sends a request and parses a JSON response; non-2xx statuses become errors.
    pub async fn json(
        &self,
        context: &'static str,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let (status, text) = self.request(method, url, body, &[]).await?;
        if !(200..300).contains(&status) {
            return Err(Error::status(context, status, &text));
        }
        if text.trim().is_empty() {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_str(&text)?)
        }
    }

    /// Downloads bytes (images, avatars) with credentials for the URL's backend plus `extra` headers.
    pub async fn bytes(
        &self,
        url: &str,
        extra: &[(&str, String)],
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        self.ensure_fresh().await?;
        let parsed = Url::parse(url).map_err(|e| Error::Protocol(format!("bad url: {e}")))?;
        let tokens = self.tokens().await;
        let mut req = self.http.get(parsed.clone());
        for (k, v) in headers_for(&parsed, &tokens, &self.epid) {
            req = req.header(k, v);
        }
        for (k, v) in extra {
            req = req.header(*k, v);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::status("download", status, ""));
        }
        let data = resp.bytes().await?;
        if data.len() > max_bytes {
            return Err(Error::Protocol(format!(
                "{DOWNLOAD_TOO_LARGE} {max_bytes} bytes"
            )));
        }
        Ok(data.to_vec())
    }
}

/// The start of the [`Error::Protocol`] text `Client::bytes` gives for a download over its cap.
/// A larger answer stays larger: callers use this to tell it from a failure worth a retry.
pub const DOWNLOAD_TOO_LARGE: &str = "download larger than";

/// What one token refresh got from the network.
struct Fetched {
    spaces: auth::TokenResponse,
    csa: auth::TokenResponse,
    /// Optional: some tenants do not grant it (the skypetoken stands in).
    presence: Option<auth::TokenResponse>,
    /// Optional: only file upload needs it.
    graph: Option<auth::TokenResponse>,
    sky: SkypeToken,
}

/// Requests the Spaces, CSA, Presence and Graph tokens concurrently (`refresh` sends one request
/// for a resource), and the skypetoken (`authz`, given the Spaces access token) as soon as the
/// Spaces token is in. Errors are reported in the order the requests used to run one after the
/// other: Spaces, CSA, then the skypetoken; Presence and Graph failures are dropped.
async fn fetch_tokens<R, RF, A, AF>(refresh: R, authz: A) -> Result<Fetched>
where
    R: Fn(&'static str) -> RF,
    RF: std::future::Future<Output = Result<auth::TokenResponse>>,
    A: FnOnce(String) -> AF,
    AF: std::future::Future<Output = Result<SkypeToken>>,
{
    let spaces_then_authz = async {
        let spaces = refresh(RESOURCE_SPACES).await?;
        let sky = authz(spaces.access_token.clone()).await;
        Ok::<_, Error>((spaces, sky))
    };
    let (spaces, csa, presence, graph) = tokio::join!(
        spaces_then_authz,
        refresh(RESOURCE_CSA),
        refresh(RESOURCE_PRESENCE),
        refresh(RESOURCE_GRAPH),
    );
    let (spaces, sky) = spaces?;
    let csa = csa?;
    let sky = sky?;
    Ok(Fetched {
        spaces,
        csa,
        presence: presence.ok(),
        graph: graph.ok(),
        sky,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_a_read_whose_connection_failed_is_sent_again() {
        // Nothing listens on port 1: the connection fails before any answer.
        install_crypto_provider();
        let err = reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        assert!(resend_safe(&Method::GET, &err));
        assert!(resend_safe(&Method::HEAD, &err));
        assert!(!resend_safe(&Method::POST, &err));
        assert!(!resend_safe(&Method::PUT, &err));
        assert!(!resend_safe(&Method::DELETE, &err));
    }

    #[test]
    fn bearer_only_goes_to_the_audience_host() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(audience_owns(
            &Audience::Graph,
            &u("https://graph.microsoft.com/v1.0/me")
        ));
        assert!(audience_owns(
            &Audience::Sydney,
            &u("https://substrate.office.com/m365Copilot/x")
        ));
        assert!(!audience_owns(
            &Audience::Graph,
            &u("http://graph.microsoft.com/v1.0/me")
        ));
        assert!(!audience_owns(
            &Audience::Graph,
            &u("https://graph.microsoft.com.evil.com/")
        ));
        assert!(!audience_owns(
            &Audience::Outlook,
            &u("https://graph.microsoft.com/")
        ));
        // Flow and OneNote mint for one host and are served from another.
        assert!(audience_owns(
            &Audience::Flow,
            &u("https://api.flow.microsoft.com/providers/Microsoft.ProcessSimple/environments")
        ));
        assert!(audience_owns(
            &Audience::OneNote,
            &u("https://www.onenote.com/api/v1.0/me/notes/notebooks")
        ));
        assert!(!audience_owns(
            &Audience::Graph,
            &u("https://api.flow.microsoft.com/")
        ));
        assert!(!audience_owns(
            &Audience::Flow,
            &u("https://www.onenote.com/")
        ));
        // Loki is regional, and only its own domain counts.
        assert!(audience_owns(
            &Audience::Loki,
            &u("https://nam.loki.delve.office.com/api/v2/person")
        ));
        assert!(!audience_owns(
            &Audience::Loki,
            &u("https://nam.loki.delve.office.com.evil.com/")
        ));
        assert!(!audience_owns(
            &Audience::Loki,
            &u("https://evilloki.delve.office.com/")
        ));
        assert!(!audience_owns(
            &Audience::Graph,
            &u("https://nam.graph.microsoft.com/")
        ));
        // The provisioning token only goes to the /fabric/ path of the Teams host.
        assert!(audience_owns(
            &Audience::Templates,
            &u("https://teams.microsoft.com/fabric/emea/templates/api/teams/g/channels")
        ));
        assert!(!audience_owns(
            &Audience::Templates,
            &u("https://teams.microsoft.com/api/mt/emea/beta/teams")
        ));
        assert!(!audience_owns(
            &Audience::Templates,
            &u("https://teams.microsoft.com.evil.com/fabric/x")
        ));
        assert!(!audience_owns(
            &Audience::Templates,
            &u("http://teams.microsoft.com/fabric/x")
        ));
        assert!(!audience_owns(
            &Audience::Teams,
            &u("https://templates.teams.microsoft.com/fabric/x")
        ));
        // The Engage token goes to the Engage GraphQL host, never to the REST API host.
        assert!(audience_owns(
            &Audience::Engage,
            &u("https://engage.cloud.microsoft/graphql")
        ));
        assert!(!audience_owns(
            &Audience::Engage,
            &u("https://engage.cloud.microsoft.evil.com/graphql")
        ));
        assert!(!audience_owns(
            &Audience::Engage,
            &u("https://api.yammer.com/api/v1/messages.json")
        ));
        assert!(!audience_owns(
            &Audience::Yammer,
            &u("https://engage.cloud.microsoft/graphql")
        ));
        // The meeting content token goes to MCPS on the Teams web hosts, nowhere else.
        assert!(audience_owns(
            &Audience::MeetingContent,
            &u("https://teams.microsoft.com/api/mcps/eu/user/meetings/contents?top=1")
        ));
        assert!(audience_owns(
            &Audience::MeetingContent,
            &u("https://teams.cloud.microsoft/api/mcps/eu/x")
        ));
        assert!(!audience_owns(
            &Audience::MeetingContent,
            &u("https://teams.microsoft.com/api/mt/emea/beta/me")
        ));
        assert!(!audience_owns(
            &Audience::MeetingContent,
            &u("https://teams.microsoft.com.evil.com/api/mcps/eu/x")
        ));
        assert!(!audience_owns(
            &Audience::MeetingContent,
            &u("http://teams.microsoft.com/api/mcps/eu/x")
        ));
        // The presence token also goes to the regional unified presence service under `/ups/`.
        assert!(audience_owns(
            &Audience::Presence,
            &u("https://teams.cloud.microsoft/ups/noam/v1/presence/getpresence/")
        ));
        assert!(audience_owns(
            &Audience::Presence,
            &u("https://presence.teams.microsoft.com/v1/me/forceavailability/")
        ));
        assert!(audience_owns(
            &Audience::Presence,
            &u("https://teams.microsoft.com/ups/noam/v1/me/workLocation/")
        ));
        assert!(!audience_owns(
            &Audience::Presence,
            &u("https://teams.microsoft.com/api/mt/emea/beta/me")
        ));
        assert!(!audience_owns(
            &Audience::Presence,
            &u("https://teams.cloud.microsoft/api/csa/x")
        ));
        assert!(!audience_owns(
            &Audience::Presence,
            &u("https://teams.cloud.microsoft.evil.com/ups/noam/v1/x")
        ));
        assert!(!audience_owns(
            &Audience::Presence,
            &u("http://teams.cloud.microsoft/ups/noam/v1/x")
        ));
        assert!(!audience_owns(
            &Audience::Teams,
            &u("https://teams.cloud.microsoft/ups/noam/v1/x")
        ));
        let sp = Audience::sharepoint("contoso-my.sharepoint.com").unwrap();
        assert!(audience_owns(
            &sp,
            &u("https://contoso-my.sharepoint.com/_api/web")
        ));
        assert!(!audience_owns(
            &sp,
            &u("https://contoso.sharepoint.com/_api/web")
        ));
    }

    #[test]
    fn app_audiences_own_no_host() {
        let u = |s: &str| Url::parse(s).unwrap();
        let app = Audience::app("api://app.contoso.com/0d5e1a2b").unwrap();
        assert!(!audience_owns(&app, &u("https://app.contoso.com/api")));
        let graph = Audience::app("https://graph.microsoft.com").unwrap();
        assert!(!audience_owns(
            &graph,
            &u("https://graph.microsoft.com/v1.0/me")
        ));
    }

    #[tokio::test]
    async fn app_token_serves_a_cached_token_and_refuses_bad_resources() {
        let client = Client::offline("epid").unwrap();
        let r = "api://app.contoso.com/0d5e1a2b";
        let exp = {
            let mut st = client.state.lock().await;
            let now = SystemTime::now();
            st.cache
                .put(&Audience::app(r).unwrap(), "CACHED".into(), now, 3600);
            now + Duration::from_secs(3600)
        };
        let t = client.app_token(r, None).await.unwrap();
        assert_eq!(t.access_token, "CACHED");
        assert_eq!(t.expires_at, exp);
        assert!(matches!(
            client.app_token("api://x.com/a openid", None).await,
            Err(Error::Protocol(_))
        ));
    }

    fn stored(rt: &str) -> Session {
        Session {
            tenant: "organizations".into(),
            refresh_token: rt.into(),
            epid: "epid".into(),
        }
    }

    #[tokio::test]
    async fn a_restored_client_has_the_stored_session_and_due_tokens() {
        // No runtime network: this returns at once, offline or not.
        let c = Client::restore(stored("RT")).unwrap();
        assert_eq!(c.session().await.refresh_token, "RT");
        assert!(refresh_due(
            c.state.lock().await.expires_at,
            SystemTime::now()
        ));
    }

    #[tokio::test]
    async fn adopting_a_new_sign_in_reaches_every_clone() {
        let c = Client::restore(stored("OLD")).unwrap();
        let clone = c.clone();
        c.adopt(Client::restore(stored("NEW")).unwrap()).await;
        assert_eq!(clone.session().await.refresh_token, "NEW");
    }

    #[test]
    fn refresh_due_uses_wall_clock_margin() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let exp = t0 + Duration::from_secs(3600);
        assert!(!refresh_due(exp, t0));
        assert!(!refresh_due(exp, t0 + Duration::from_secs(3600 - 301)));
        assert!(refresh_due(exp, t0 + Duration::from_secs(3600 - 300)));
        // Eight hours of suspend: wall clock moved on, so the tokens are due.
        assert!(refresh_due(exp, t0 + Duration::from_secs(8 * 3600)));
    }

    fn token(access: &str, rotated: Option<&str>) -> auth::TokenResponse {
        auth::TokenResponse {
            access_token: access.into(),
            refresh_token: rotated.map(str::to_owned),
            expires_in_s: 3600,
        }
    }

    fn skype() -> SkypeToken {
        SkypeToken {
            token: "sky".into(),
            expires_in_s: 3600,
            chat_service: String::new(),
            middle_tier: String::new(),
            presence: String::new(),
            mcp_service: None,
            geo: None,
            region: None,
        }
    }

    #[tokio::test]
    async fn the_four_token_requests_are_in_flight_together() {
        // Every request waits until all four are pending, as a server that answers only then
        // would: one after the other, none would ever be answered.
        let gate = Arc::new(tokio::sync::Barrier::new(4));
        let refresh = |resource: &'static str| {
            let gate = gate.clone();
            async move {
                gate.wait().await;
                Ok(token(resource, None))
            }
        };
        let authz = |spaces: String| async move {
            assert_eq!(
                spaces, RESOURCE_SPACES,
                "the skypetoken gets the Spaces token"
            );
            Ok(skype())
        };
        let got = tokio::time::timeout(Duration::from_secs(5), fetch_tokens(refresh, authz))
            .await
            .expect("the requests were sent one after the other")
            .unwrap();
        assert_eq!(got.spaces.access_token, RESOURCE_SPACES);
        assert_eq!(got.csa.access_token, RESOURCE_CSA);
        assert_eq!(got.presence.unwrap().access_token, RESOURCE_PRESENCE);
        assert_eq!(got.graph.unwrap().access_token, RESOURCE_GRAPH);
        assert_eq!(got.sky.token, "sky");
    }

    #[tokio::test]
    async fn presence_and_graph_are_optional_and_the_others_are_not() {
        let only = |bad: &'static str| {
            move |resource: &'static str| async move {
                if resource == bad {
                    Err(Error::Auth("no access_token".into()))
                } else {
                    Ok(token(resource, None))
                }
            }
        };
        let authz = |_: String| async { Ok(skype()) };
        let got = fetch_tokens(only(RESOURCE_PRESENCE), authz).await.unwrap();
        assert!(got.presence.is_none() && got.graph.is_some());
        let got = fetch_tokens(only(RESOURCE_GRAPH), authz).await.unwrap();
        assert!(got.presence.is_some() && got.graph.is_none());
        assert!(fetch_tokens(only(RESOURCE_SPACES), authz).await.is_err());
        assert!(fetch_tokens(only(RESOURCE_CSA), authz).await.is_err());
        let sky_fails = |_: String| async { Err(Error::NotSignedIn) };
        assert!(
            matches!(
                fetch_tokens(only(RESOURCE_GRAPH), sky_fails).await,
                Err(Error::NotSignedIn)
            ),
            "a failed skypetoken exchange fails the refresh"
        );
    }

    #[tokio::test]
    async fn failures_are_reported_in_the_order_the_requests_used_to_run() {
        // Spaces before CSA, as when the requests ran one after the other.
        let refresh = |resource: &'static str| async move {
            Err::<auth::TokenResponse, _>(Error::Auth(format!("bad {resource}")))
        };
        let authz = |_: String| async { Ok(skype()) };
        let err = fetch_tokens(refresh, authz).await.err().unwrap();
        assert!(
            matches!(&err, Error::Auth(m) if m.contains(RESOURCE_SPACES)),
            "{err}"
        );
    }
}
