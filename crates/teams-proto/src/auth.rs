//! Sign-in: OAuth device code, refresh tokens and the skypetoken exchange.

use crate::client_info::{
    AUTHORITY, CLIENT_ID, FALLBACK_CHAT_SERVICE, FALLBACK_MIDDLE_TIER, FALLBACK_PRESENCE,
    RESOURCE_SPACES, USER_AGENT, scope_for,
};
use crate::error::{Error, Result};
use serde_json::Value;
use std::time::Duration;

/// A pending device-code sign-in. Show `user_code` and `verification_url` to the user.
#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval_s: u64,
    pub expires_in_s: u64,
    pub message: String,
}

/// An OAuth token response.
#[derive(Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in_s: u64,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("expires_in_s", &self.expires_in_s)
            .finish_non_exhaustive()
    }
}

/// Result of one device-code poll.
#[derive(Debug)]
pub enum PollOutcome {
    /// The user has not finished signing in yet.
    Pending,
    /// Poll less often.
    SlowDown,
    /// Signed in.
    Done(TokenResponse),
}

/// Result of the skypetoken exchange, including where this user's chat data lives.
#[derive(Clone)]
pub struct SkypeToken {
    pub token: String,
    pub expires_in_s: u64,
    /// Chat service base URL, e.g. `https://emea.ng.msg.teams.microsoft.com`.
    pub chat_service: String,
    /// Middle-tier base URL, e.g. `https://teams.microsoft.com/api/mt/emea`.
    pub middle_tier: String,
    /// Presence service base URL with its version, e.g.
    /// `https://teams.cloud.microsoft/ups/noam/v1` (`regionGtms.presenceUPS`).
    pub presence: String,
    /// Meeting content service base URL (`regionGtms.mcpService`), e.g.
    /// `https://teams.microsoft.com/api/mcps/eu`. Absent from some answers.
    pub mcp_service: Option<String>,
    /// The service geography (`emea`, `amer`, `apac`) named by the `regionGtms` hosts that carry
    /// one (see [`geo_of`]), for guessing the meeting content service when it is not named.
    pub geo: Option<String>,
    pub region: Option<String>,
}

impl std::fmt::Debug for SkypeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkypeToken")
            .field("expires_in_s", &self.expires_in_s)
            .field("chat_service", &self.chat_service)
            .field("middle_tier", &self.middle_tier)
            .field("presence", &self.presence)
            .field("mcp_service", &self.mcp_service)
            .field("geo", &self.geo)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

/// Numbers in Microsoft responses are sometimes sent as strings.
fn num(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn s(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}

/// A `regionGtms` URL: a string, or (in the consumer answer) an object with `endpointUrl`.
fn url_of(v: Option<&Value>) -> Option<String> {
    s(v).or_else(|| s(v?.get("endpointUrl")))
}

/// The service geography named by the `regionGtms` hosts that carry one: CSA
/// (`…/api/csa/emea`), presence (`…/ups/emea`), teams and channels (`…/api/mt/emea`), calling
/// (`api-emea.flightproxy…`), then the chat service and AMS when their host names a geography
/// rather than a country. The middle tier is not asked: its partition need not be where the
/// data lives (the reference answer pairs `mt/part/amer-03` with `csa/emea` and `mcps/eu`).
fn geo_of(gtms: Option<&Value>) -> Option<String> {
    const KEYS: [&str; 7] = [
        "chatSvcAggAfd",
        "presenceUPS",
        "teamsAndChannelsService",
        "calling_conversationServiceUrl",
        "chatService",
        "chatServiceAfd",
        "ams",
    ];
    const GEOS: [&str; 4] = ["emea", "amer", "noam", "apac"];
    let g = gtms?;
    KEYS.iter().filter_map(|k| url_of(g.get(*k))).find_map(|u| {
        u.split(|c: char| !c.is_ascii_alphanumeric())
            .find(|part| GEOS.contains(part))
            .map(str::to_owned)
    })
}

/// The AADSTS errors Microsoft documents as a policy of the organization refusing the sign-in
/// (<https://learn.microsoft.com/entra/identity-platform/reference-error-codes>), with the name
/// and a short description for the copy-and-report button. 50199 (confirm the app) and 53004
/// (register for multi-factor) are not blocks, and 50076/50079 ask for multi-factor, not refuse.
const POLICY_BLOCKS: [(u32, &str, &str); 7] = [
    (
        50131,
        "ConditionalAccessFailed",
        "A Conditional Access check failed (device state, suspicious activity or an access policy).",
    ),
    (
        53000,
        "DeviceNotCompliant",
        "Conditional Access requires a compliant device.",
    ),
    (
        53001,
        "DeviceNotDomainJoined",
        "Conditional Access requires a domain-joined device.",
    ),
    (
        53002,
        "ApplicationUsedIsNotAnApprovedApp",
        "Conditional Access allows only approved apps.",
    ),
    (
        53003,
        "BlockedByConditionalAccess",
        "Access has been blocked by Conditional Access policies.",
    ),
    (
        530032,
        "BlockedByConditionalAccessOnSecurityPolicy",
        "A tenant security policy blocks this request.",
    ),
    (
        530035,
        "BlockedBySecurityDefaults",
        "Access has been blocked by security defaults.",
    ),
];

/// Microsoft's name for `code` when it is one of the policy blocks, else `None`.
pub fn policy_block_name(code: u32) -> Option<&'static str> {
    POLICY_BLOCKS.iter().find(|b| b.0 == code).map(|b| b.1)
}

/// The first AADSTS number in `text` (`AADSTS53003: ...`).
pub fn aadsts_code(text: &str) -> Option<u32> {
    let i = text.find("AADSTS")? + "AADSTS".len();
    let digits: String = text[i..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The error for a token service answer that carries `error`. The message keeps the error name and
/// the first line of the description (the rest is trace and correlation ids). A policy block gets
/// its own variant; the AADSTS code is logged, nothing else of the answer is.
fn auth_failure(err: &str, v: &Value) -> Error {
    let desc = s(v.get("error_description")).unwrap_or_default();
    let first_line = desc.lines().next().unwrap_or_default().to_string();
    let detail = format!("{err}: {first_line}");
    let code = aadsts_code(&first_line).or_else(|| {
        v.get("error_codes")?
            .as_array()?
            .first()?
            .as_u64()
            .and_then(|c| u32::try_from(c).ok())
    });
    let block = code.and_then(|c| POLICY_BLOCKS.iter().find(|b| b.0 == c));
    if let Some(c) = code {
        tracing::info!(
            aadsts = c,
            error = err,
            policy_block = block.is_some(),
            "sign-in refused"
        );
    }
    match block {
        Some(&(code, name, summary)) => Error::PolicyBlocked {
            code,
            name,
            summary,
            detail,
        },
        None => Error::Auth(detail),
    }
}

/// Parses the device-code start response.
pub fn parse_device_code(body: &str) -> Result<DeviceCode> {
    let v: Value = serde_json::from_str(body)?;
    if let Some(err) = s(v.get("error")) {
        return Err(auth_failure(&err, &v));
    }
    Ok(DeviceCode {
        device_code: s(v.get("device_code")).ok_or_else(|| Error::Auth("no device_code".into()))?,
        user_code: s(v.get("user_code")).ok_or_else(|| Error::Auth("no user_code".into()))?,
        verification_url: s(v.get("verification_url"))
            .or_else(|| s(v.get("verification_uri")))
            .unwrap_or_else(|| "https://microsoft.com/devicelogin".into()),
        interval_s: num(v.get("interval")).unwrap_or(5).max(1),
        expires_in_s: num(v.get("expires_in")).unwrap_or(900),
        message: s(v.get("message")).unwrap_or_default(),
    })
}

/// Parses a token endpoint response (device-code poll or refresh).
pub fn parse_token_response(body: &str) -> Result<PollOutcome> {
    let v: Value = serde_json::from_str(body)?;
    if let Some(err) = s(v.get("error")) {
        return match err.as_str() {
            "authorization_pending" => Ok(PollOutcome::Pending),
            "slow_down" => Ok(PollOutcome::SlowDown),
            "expired_token" | "code_expired" => Err(Error::DeviceCodeExpired),
            _ => Err(auth_failure(&err, &v)),
        };
    }
    Ok(PollOutcome::Done(TokenResponse {
        access_token: s(v.get("access_token"))
            .ok_or_else(|| Error::Auth("no access_token".into()))?,
        refresh_token: s(v.get("refresh_token")),
        expires_in_s: num(v.get("expires_in")).unwrap_or(3600),
    }))
}

/// Parses the `authsvc/v1.0/authz` response.
pub fn parse_authz(body: &str) -> Result<SkypeToken> {
    let v: Value = serde_json::from_str(body)?;
    let tokens = v.get("tokens");
    let token = tokens
        .and_then(|t| t.get("skypeToken"))
        .or_else(|| v.get("skypeToken").and_then(|t| t.get("skypetoken")))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            let msg = s(v.get("message")).or_else(|| {
                v.pointer("/status/text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            Error::Auth(format!(
                "authz returned no skype token: {}",
                msg.unwrap_or_default()
            ))
        })?
        .to_string();
    let expires_in_s = num(tokens.and_then(|t| t.get("expiresIn"))).unwrap_or(3600);
    let region = s(v.get("region"));
    let gtms = v.get("regionGtms");
    let trim = |u: String| u.trim_end_matches('/').to_string();
    let chat_service = s(gtms.and_then(|g| g.get("chatService")))
        .map(trim)
        .or_else(|| {
            region
                .as_ref()
                .map(|r| format!("https://{r}.ng.msg.teams.microsoft.com"))
        })
        .unwrap_or_else(|| FALLBACK_CHAT_SERVICE.into());
    let middle_tier = s(gtms.and_then(|g| g.get("middleTier")))
        .map(trim)
        .or_else(|| {
            region
                .as_ref()
                .map(|r| format!("https://teams.microsoft.com/api/mt/{r}"))
        })
        .unwrap_or_else(|| FALLBACK_MIDDLE_TIER.into());
    // Teams web sends presence to the regional unified presence service; the global host
    // refuses this client's presence token.
    let presence = s(gtms.and_then(|g| g.get("presenceUPS")))
        .map(|u| format!("{}/v1", trim(u)))
        .unwrap_or_else(|| FALLBACK_PRESENCE.into());
    // The middle tier's partition is not where the meetings live (an EMEA tenant can have an
    // `amer` middle tier), so the meeting content service comes from its own key.
    let mcp_service = url_of(gtms.and_then(|g| g.get("mcpService"))).map(trim);
    if let Some(Value::Object(g)) = gtms {
        // Key names only: the values are this account's hosts.
        let keys: Vec<&str> = g.keys().map(String::as_str).collect();
        tracing::debug!(?keys, "authz regionGtms");
    }
    Ok(SkypeToken {
        token,
        expires_in_s,
        chat_service,
        middle_tier,
        presence,
        mcp_service,
        geo: geo_of(gtms),
        region,
    })
}

fn token_url(tenant: &str, v2: bool) -> String {
    if v2 {
        format!("{AUTHORITY}/{tenant}/oauth2/v2.0/token")
    } else {
        format!("{AUTHORITY}/{tenant}/oauth2/token")
    }
}

/// Starts a device-code sign-in.
pub async fn start_device_code(http: &reqwest::Client, tenant: &str) -> Result<DeviceCode> {
    let resp = http
        .post(format!("{AUTHORITY}/{tenant}/oauth2/devicecode"))
        .header("User-Agent", USER_AGENT)
        .form(&[("client_id", CLIENT_ID), ("resource", RESOURCE_SPACES)])
        .send()
        .await?;
    let body = resp.text().await?;
    parse_device_code(&body)
}

/// Polls until the user finishes signing in, the code expires, or sign-in fails.
pub async fn poll_device_code(
    http: &reqwest::Client,
    tenant: &str,
    dc: &DeviceCode,
) -> Result<TokenResponse> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(dc.expires_in_s);
    let mut interval = dc.interval_s;
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() > deadline {
            return Err(Error::DeviceCodeExpired);
        }
        let resp = http
            .post(token_url(tenant, false))
            .header("User-Agent", USER_AGENT)
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("code", dc.device_code.as_str()),
            ])
            .send()
            .await?;
        match parse_token_response(&resp.text().await?)? {
            PollOutcome::Pending => {}
            PollOutcome::SlowDown => interval += 5,
            PollOutcome::Done(t) => return Ok(t),
        }
    }
}

/// Exchanges a refresh token for an access token for `resource`.
pub async fn refresh(
    http: &reqwest::Client,
    tenant: &str,
    refresh_token: &str,
    resource: &str,
) -> Result<TokenResponse> {
    let scope = scope_for(resource);
    let resp = http
        .post(token_url(tenant, true))
        .header("User-Agent", USER_AGENT)
        .form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", scope.as_str()),
        ])
        .send()
        .await?;
    match parse_token_response(&resp.text().await?)? {
        PollOutcome::Done(t) => Ok(t),
        _ => Err(Error::Auth(
            "unexpected pending response to a refresh".into(),
        )),
    }
}

/// Exchanges the `api.spaces.skype.com` access token for a skypetoken.
pub async fn skypetoken(http: &reqwest::Client, spaces_access_token: &str) -> Result<SkypeToken> {
    let resp = http
        .post("https://teams.microsoft.com/api/authsvc/v1.0/authz")
        .header("User-Agent", USER_AGENT)
        .header("Authorization", format!("Bearer {spaces_access_token}"))
        .header("Accept", "application/json; ver=1.0")
        .header("Content-Length", "0")
        .send()
        .await?;
    let status = resp.status().as_u16();
    let body = resp.text().await?;
    if status >= 400 {
        return Err(Error::status("authz", status, &body));
    }
    parse_authz(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_code_accepts_string_numbers() {
        let dc = parse_device_code(
            r#"{"user_code":"GRUE55CDY","device_code":"DC","verification_url":"https://microsoft.com/devicelogin",
                "expires_in":"900","interval":"5","message":"To sign in..."}"#,
        )
        .unwrap();
        assert_eq!(dc.user_code, "GRUE55CDY");
        assert_eq!(dc.interval_s, 5);
        assert_eq!(dc.expires_in_s, 900);
    }

    #[test]
    fn device_code_v2_uri_field() {
        let dc = parse_device_code(
            r#"{"user_code":"A","device_code":"B","verification_uri":"https://login.microsoft.com/device","expires_in":600,"interval":1}"#,
        )
        .unwrap();
        assert_eq!(dc.verification_url, "https://login.microsoft.com/device");
    }

    #[test]
    fn token_poll_states() {
        assert!(matches!(
            parse_token_response(r#"{"error":"authorization_pending"}"#).unwrap(),
            PollOutcome::Pending
        ));
        assert!(matches!(
            parse_token_response(r#"{"error":"slow_down"}"#).unwrap(),
            PollOutcome::SlowDown
        ));
        assert!(matches!(
            parse_token_response(r#"{"error":"expired_token"}"#),
            Err(Error::DeviceCodeExpired)
        ));
        assert!(matches!(
            parse_token_response(r#"{"error":"invalid_grant","error_description":"AADSTS70000: bad\r\nTrace ID: x"}"#),
            Err(Error::Auth(m)) if m == "invalid_grant: AADSTS70000: bad"
        ));
        match parse_token_response(
            r#"{"access_token":"AT","refresh_token":"RT","expires_in":"3599"}"#,
        )
        .unwrap()
        {
            PollOutcome::Done(t) => {
                assert_eq!(t.access_token, "AT");
                assert_eq!(t.refresh_token.as_deref(), Some("RT"));
                assert_eq!(t.expires_in_s, 3599);
                assert!(!format!("{t:?}").contains("AT"));
            }
            other => panic!("{other:?}"),
        }
    }

    // Error bodies in the shape the token service documents (ids are made up). The device-code
    // start answers 400 with this body when a policy blocks the flow; the poll answers the same
    // way once the user finishes signing in on a blocked account.
    const START_BLOCKED_BY_CA: &str = r#"{"error":"invalid_request","error_description":"AADSTS53003: Access has been blocked by Conditional Access policies. The access policy does not allow token issuance.\r\nTrace ID: 00000000-0000-0000-0000-000000000001\r\nCorrelation ID: 00000000-0000-0000-0000-000000000002\r\nTimestamp: 2026-11-24 10:00:00Z","error_codes":[53003],"timestamp":"2026-11-24 10:00:00Z","trace_id":"00000000-0000-0000-0000-000000000001","correlation_id":"00000000-0000-0000-0000-000000000002","error_uri":"https://login.microsoftonline.com/error?code=53003"}"#;
    const POLL_BLOCKED_BY_SECURITY_POLICY: &str = r#"{"error":"access_denied","error_description":"AADSTS530032: The tenant admin has configured a security policy that blocks this request.\r\nTrace ID: x","error_codes":[530032]}"#;
    // 50199 only asks the user to confirm the app: not a block. 70018 is a mistyped user code.
    const POLL_CONFIRMATION_PROMPT: &str = r#"{"error":"interaction_required","error_description":"AADSTS50199: For security reasons, user confirmation is required for this request.\r\nTrace ID: x","error_codes":[50199]}"#;
    const START_BAD_USER_CODE: &str = r#"{"error":"invalid_grant","error_description":"AADSTS70018: Invalid verification code due to User typing in wrong user code for device code flow.","error_codes":[70018]}"#;

    #[test]
    fn device_code_start_blocked_by_conditional_access_is_a_policy_block() {
        let e = parse_device_code(START_BLOCKED_BY_CA).unwrap_err();
        let (code, name) = e.policy_block().expect("a policy block");
        assert_eq!(code, 53003);
        assert_eq!(name, "BlockedByConditionalAccess");
        // The text other code reads stays what it was: error name and the first line.
        assert_eq!(
            e.to_string(),
            "auth: invalid_request: AADSTS53003: Access has been blocked by Conditional Access policies. The access policy does not allow token issuance."
        );
    }

    #[test]
    fn token_poll_blocked_by_a_security_policy_is_a_policy_block() {
        let e = parse_token_response(POLL_BLOCKED_BY_SECURITY_POLICY).unwrap_err();
        assert_eq!(e.policy_block().map(|(c, _)| c), Some(530032));
    }

    #[test]
    fn other_sign_in_failures_are_not_policy_blocks() {
        let e = parse_token_response(POLL_CONFIRMATION_PROMPT).unwrap_err();
        assert!(e.policy_block().is_none());
        assert!(matches!(e, Error::Auth(m) if m.starts_with("interaction_required: AADSTS50199")));
        let e = parse_device_code(START_BAD_USER_CODE).unwrap_err();
        assert!(e.policy_block().is_none());
        assert!(matches!(e, Error::Auth(_)));
        // No AADSTS number at all.
        let e = parse_token_response(r#"{"error":"invalid_grant","error_description":"bad"}"#)
            .unwrap_err();
        assert!(e.policy_block().is_none());
    }

    #[test]
    fn the_code_comes_from_the_description_or_else_error_codes() {
        assert_eq!(aadsts_code("x: AADSTS53003: y"), Some(53003));
        assert_eq!(aadsts_code("AADSTS: none"), None);
        assert_eq!(aadsts_code("no code"), None);
        // A description without the number still has it in `error_codes`.
        let e = parse_token_response(
            r#"{"error":"access_denied","error_description":"blocked","error_codes":[53003]}"#,
        )
        .unwrap_err();
        assert_eq!(e.policy_block().map(|(c, _)| c), Some(53003));
    }

    /// The codes Microsoft documents as policy blocks (reference-error-codes). 50199 and 53004
    /// (register for multi-factor) are not among them.
    #[test]
    fn the_policy_block_list() {
        for c in [53000, 53001, 53002, 53003, 530032, 530035, 50131] {
            assert!(policy_block_name(c).is_some(), "{c}");
        }
        for c in [50199, 53004, 70016, 70018, 50076, 700082] {
            assert!(policy_block_name(c).is_none(), "{c}");
        }
    }

    #[test]
    fn authz_with_region_gtms() {
        let t = parse_authz(
            r#"{"tokens":{"skypeToken":"SKY","expiresIn":86399,"tokenType":"SkypeToken"},"region":"emea","partition":"emea02",
                "regionGtms":{"chatService":"https://emea.ng.msg.teams.microsoft.com/","middleTier":"https://teams.microsoft.com/api/mt/emea"}}"#,
        )
        .unwrap();
        assert_eq!(t.token, "SKY");
        assert_eq!(t.expires_in_s, 86399);
        assert_eq!(t.chat_service, "https://emea.ng.msg.teams.microsoft.com");
        assert_eq!(t.middle_tier, "https://teams.microsoft.com/api/mt/emea");
        assert!(!format!("{t:?}").contains("SKY"));
        assert_eq!(t.presence, FALLBACK_PRESENCE);
    }

    #[test]
    fn authz_reads_the_regional_presence_service() {
        let t = parse_authz(
            r#"{"tokens":{"skypeToken":"S","expiresIn":100},"region":"amer",
                "regionGtms":{"presenceUPS":"https://teams.cloud.microsoft/ups/noam/"}}"#,
        )
        .unwrap();
        assert_eq!(t.presence, "https://teams.cloud.microsoft/ups/noam/v1");
    }

    /// The meeting content service has its own key: an EMEA tenant may have an `amer` middle
    /// tier (shape of the reference capture).
    #[test]
    fn authz_reads_the_meeting_content_service() {
        let t = parse_authz(
            r#"{"tokens":{"skypeToken":"S","expiresIn":100},"region":"emea",
                "regionGtms":{"middleTier":"https://teams.microsoft.com/api/mt/part/amer-03",
                "mcpService":"https://teams.microsoft.com/api/mcps/eu/"}}"#,
        )
        .unwrap();
        assert_eq!(
            t.mcp_service.as_deref(),
            Some("https://teams.microsoft.com/api/mcps/eu")
        );
        let t = parse_authz(r#"{"tokens":{"skypeToken":"S","expiresIn":100}}"#).unwrap();
        assert_eq!(t.mcp_service, None);
        assert_eq!(t.geo, None);
    }

    /// Without `mcpService`, the geography comes from the hosts that name one, not from the
    /// middle tier's partition (the reference answer: `amer-03` middle tier, `emea` CSA and
    /// presence, a country code in the chat service).
    #[test]
    fn authz_reads_the_geography_from_the_hosts_that_carry_it() {
        let t = parse_authz(
            r#"{"tokens":{"skypeToken":"S","expiresIn":100},"region":"amer",
                "regionGtms":{"chatService":"https://at.ng.msg.teams.microsoft.com",
                "middleTier":"https://teams.microsoft.com/api/mt/part/amer-03",
                "chatSvcAggAfd":"https://teams.microsoft.com/api/csa/emea",
                "presenceUPS":"https://teams.microsoft.com/ups/emea"}}"#,
        )
        .unwrap();
        assert_eq!(t.mcp_service, None);
        assert_eq!(t.geo.as_deref(), Some("emea"));
        // Calling's flight proxy host, and the consumer answer's object form.
        let t = parse_authz(
            r#"{"tokens":{"skypeToken":"S","expiresIn":100},
                "regionGtms":{"calling_conversationServiceUrl":"https://api-apac.flightproxy.teams.microsoft.com/api/v2/epconv",
                "mcpService":{"endpointUrl":"https://teams.microsoft.com/api/mcps/ap"}}}"#,
        )
        .unwrap();
        assert_eq!(t.geo.as_deref(), Some("apac"));
        assert_eq!(
            t.mcp_service.as_deref(),
            Some("https://teams.microsoft.com/api/mcps/ap")
        );
        // A country code is not a geography.
        let t = parse_authz(
            r#"{"tokens":{"skypeToken":"S","expiresIn":100},
                "regionGtms":{"chatService":"https://at.ng.msg.teams.microsoft.com"}}"#,
        )
        .unwrap();
        assert_eq!(t.geo, None);
    }

    #[test]
    fn authz_falls_back_to_region_then_default() {
        let t = parse_authz(r#"{"tokens":{"skypeToken":"S","expiresIn":"100"},"region":"amer"}"#)
            .unwrap();
        assert_eq!(t.chat_service, "https://amer.ng.msg.teams.microsoft.com");
        let t = parse_authz(r#"{"skypeToken":{"skypetoken":"S"}}"#).unwrap();
        assert_eq!(t.chat_service, FALLBACK_CHAT_SERVICE);
        assert!(parse_authz(r#"{"status":{"code":403,"text":"denied"}}"#).is_err());
    }
}
