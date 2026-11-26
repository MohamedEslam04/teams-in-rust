//! Starting without a network: when the stored session cannot be refreshed because Teams did
//! not answer, the saved chats open from the account's database and the sign-in is tried again
//! in the background. A session Microsoft rejects still goes to the sign-in screen.

use std::time::Duration;
use teams_proto::Error;

/// Shown when IronTeams starts from the saved chats because it could not reach Teams.
pub const OFFLINE_NOTICE: &str =
    "Offline — showing saved messages. IronTeams will keep trying to connect.";

/// What to do when the stored session's first refresh fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnRefreshError {
    /// No answer from Microsoft: open the saved chats and keep trying.
    Offline,
    /// The session was rejected (revoked, expired, password changed): sign in again.
    SignIn,
    /// Anything else, or nothing saved to show: the "Sign-in failed" screen, as before.
    Fail,
}

/// True when refreshing the session may work later without the user doing anything: no
/// connection, a timeout, a server that is down or busy, or an answer that is not the token
/// service's (a captive portal or a proxy page). A rejected session is not transient.
pub fn is_transient(e: &Error) -> bool {
    match e {
        Error::Http(h) => !h.is_builder(),
        Error::Ws(_) => true,
        // A refresh writes nothing, so any server error is worth another try.
        Error::Status { status, .. } => *status >= 500 || matches!(status, 408 | 425 | 429),
        // A captive portal or a proxy answers with a web page instead of JSON.
        Error::Json(_) => true,
        // The token service's own "try again later" answers (AADSTS errors are `Auth` too).
        Error::Auth(msg) => {
            msg.starts_with("temporarily_unavailable") || msg.starts_with("server_error")
        }
        Error::NotMember { .. }
        | Error::PolicyBlocked { .. }
        | Error::Protocol(_)
        | Error::DeviceCodeExpired
        | Error::NotSignedIn => false,
    }
}

/// The start-up decision for a failed refresh of the stored session. `has_cache` is true when the
/// config knows who the user is and their database file exists.
pub fn on_refresh_error(e: &Error, has_cache: bool) -> OnRefreshError {
    if matches!(e, Error::Auth(_) | Error::PolicyBlocked { .. }) && !is_transient(e) {
        OnRefreshError::SignIn
    } else if is_transient(e) && has_cache {
        OnRefreshError::Offline
    } else {
        OnRefreshError::Fail
    }
}

/// Whether a start with a stored session shows the saved chats first and refreshes the session
/// afterwards (the cache-first start): the config knows who the user is and the account's
/// database exists. An account that never ran here (no self MRI, no database) keeps the
/// sign-in first order, because there is nothing to show yet.
pub fn cache_first(self_mri: Option<&str>, db_exists: bool) -> bool {
    self_mri.is_some_and(|m| !m.is_empty()) && db_exists
}

/// How long to wait before the `attempt`th (0-based) refresh of the session in the background.
/// A cache-first start has not failed yet, so its first refresh (`immediate_first`) is not
/// delayed and the retries after it back off from [`retry_delay`]'s start; any other start has
/// already failed once, so it waits [`retry_delay`] every time.
pub fn refresh_delay(attempt: u32, immediate_first: bool) -> Duration {
    match (immediate_first, attempt) {
        (true, 0) => Duration::ZERO,
        (true, n) => retry_delay(n - 1),
        (false, n) => retry_delay(n),
    }
}

/// How long to wait before the `attempt`th (0-based) retry: 5 s, doubling, at most a minute.
pub fn retry_delay(attempt: u32) -> Duration {
    let s = 5u64.saturating_mul(1u64 << attempt.min(16));
    Duration::from_secs(s.min(60))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn refused() -> Error {
        // Nothing listens on port 1.
        teams_proto::client::install_crypto_provider();
        let err = reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        Error::from(err)
    }

    #[tokio::test]
    async fn no_connection_is_transient() {
        assert!(is_transient(&refused().await));
    }

    #[test]
    fn server_trouble_is_transient() {
        for s in [408, 425, 429, 500, 502, 503, 504] {
            assert!(is_transient(&Error::status("authz", s, "")), "{s}");
        }
        let page = serde_json::from_str::<serde_json::Value>("<html>Sign in to the Wi-Fi</html>");
        assert!(is_transient(&Error::Json(page.unwrap_err())));
        assert!(is_transient(&Error::Auth(
            "temporarily_unavailable: AADSTS50196: try again".into()
        )));
        assert!(is_transient(&Error::Auth(
            "server_error: AADSTS90033".into()
        )));
    }

    #[test]
    fn a_rejected_session_is_not_transient() {
        for msg in [
            "invalid_grant: AADSTS700082: The refresh token has expired",
            "invalid_grant: AADSTS50173: The provided grant has expired",
            "interaction_required: AADSTS50076",
            "authz returned no skype token: ",
            "no access_token",
        ] {
            assert!(!is_transient(&Error::Auth(msg.into())), "{msg}");
        }
        for s in [400, 401, 403, 404] {
            assert!(!is_transient(&Error::status("authz", s, "")), "{s}");
        }
        let blocked = teams_proto::auth::parse_token_response(
            r#"{"error":"invalid_grant","error_description":"AADSTS53003: blocked"}"#,
        )
        .unwrap_err();
        assert!(!is_transient(&blocked));
        // A policy block on refresh still means "sign in again", as before.
        assert_eq!(on_refresh_error(&blocked, true), OnRefreshError::SignIn);
        assert!(!is_transient(&Error::Protocol("x".into())));
        assert!(!is_transient(&Error::NotSignedIn));
        assert!(!is_transient(&Error::DeviceCodeExpired));
    }

    #[tokio::test]
    async fn no_network_with_saved_chats_starts_offline() {
        assert_eq!(
            on_refresh_error(&refused().await, true),
            OnRefreshError::Offline
        );
        assert_eq!(
            on_refresh_error(&Error::status("authz", 503, ""), true),
            OnRefreshError::Offline
        );
    }

    #[tokio::test]
    async fn no_network_and_nothing_saved_fails_as_before() {
        assert_eq!(
            on_refresh_error(&refused().await, false),
            OnRefreshError::Fail
        );
    }

    #[test]
    fn a_rejected_session_signs_in_again_even_with_saved_chats() {
        let revoked = Error::Auth("invalid_grant: AADSTS50173: revoked".into());
        assert_eq!(on_refresh_error(&revoked, true), OnRefreshError::SignIn);
        assert_eq!(on_refresh_error(&revoked, false), OnRefreshError::SignIn);
    }

    #[test]
    fn other_errors_fail_as_before() {
        assert_eq!(
            on_refresh_error(&Error::status("authz", 403, ""), true),
            OnRefreshError::Fail
        );
        assert_eq!(
            on_refresh_error(&Error::Protocol("x".into()), true),
            OnRefreshError::Fail
        );
    }

    #[test]
    fn only_an_account_that_ran_before_starts_from_its_saved_chats() {
        assert!(cache_first(Some("8:orgid:me"), true));
        assert!(!cache_first(Some("8:orgid:me"), false), "no database yet");
        assert!(!cache_first(None, true), "who the user is is not known");
        assert!(!cache_first(Some(""), true));
        assert!(!cache_first(None, false), "the first run of an account");
    }

    #[test]
    fn a_cache_first_start_refreshes_at_once_and_backs_off_after() {
        assert_eq!(refresh_delay(0, true), Duration::ZERO);
        assert_eq!(refresh_delay(1, true), Duration::from_secs(5));
        assert_eq!(refresh_delay(2, true), Duration::from_secs(10));
        // A start that already failed once waits before it tries again.
        assert_eq!(refresh_delay(0, false), Duration::from_secs(5));
    }

    #[test]
    fn retries_back_off_to_a_minute() {
        let s: Vec<u64> = (0..7).map(|a| retry_delay(a).as_secs()).collect();
        assert_eq!(s, [5, 10, 20, 40, 60, 60, 60]);
        assert_eq!(retry_delay(u32::MAX).as_secs(), 60);
    }
}
