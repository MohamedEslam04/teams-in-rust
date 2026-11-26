//! Error type for the protocol crate.

/// Errors returned by the protocol layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A request that got no answer. Built through `From`, which drops the URL: it can name a
    /// conversation and is no use in a banner (the log line of the request has it).
    #[error("http: {}", http_chain(.0))]
    Http(reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("websocket: {0}")]
    Ws(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    #[error("{context}: HTTP {status}: {body}")]
    Status {
        context: &'static str,
        status: u16,
        body: String,
    },
    /// The chat service refused a read because the signed-in user is not in the thread: a
    /// meeting chat they were invited to but have not joined, or a chat they left. Built by
    /// [`Error::into_not_member`] from the 403 answer, which is raw JSON no banner should show.
    #[error("{context}: not a member of the conversation")]
    NotMember { context: &'static str },
    #[error("auth: {0}")]
    Auth(String),
    /// Sign-in refused by the organization's policy (Conditional Access, an authentication-flow
    /// restriction, security defaults): see [`crate::auth::policy_block_name`]. `detail` is the
    /// text an [`Error::Auth`] would carry (error name and the first line of the description), so
    /// anything that reads the message sees no difference. No tenant or user id is kept.
    #[error("auth: {detail}")]
    PolicyBlocked {
        code: u32,
        name: &'static str,
        summary: &'static str,
        detail: String,
    },
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("device code expired before sign-in finished")]
    DeviceCodeExpired,
    #[error("not signed in")]
    NotSignedIn,
    /// A request was built before the first token refresh answered, so its URL has no service
    /// host yet (a cache-first start opens the saved chats while the session refreshes). Nothing
    /// was sent. It passes once the refresh is done, so callers treat it as a missing connection,
    /// not as a failure to show.
    #[error("still signing in to Teams, try again in a moment")]
    NotReady,
}

/// The error and the causes under it, as "error sending request: client error (Connect): tcp
/// connect error: Connection refused". reqwest's own text stops at its first line, which hides
/// why a request failed (name lookup, certificate, refused or reset connection). The URL is
/// dropped before this runs, and the causes name no path or query.
fn http_chain(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(c) = cause {
        let line = c.to_string();
        if !text.ends_with(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        cause = c.source();
    }
    text
}

impl Error {
    /// The AADSTS code and Microsoft's name for it when a policy of the organization refused the
    /// sign-in, as opposed to a mistyped code, an expired session or an unreachable service.
    pub fn policy_block(&self) -> Option<(u32, &'static str)> {
        match self {
            Error::PolicyBlocked { code, name, .. } => Some((*code, name)),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e.without_url())
    }
}

/// Result alias for the protocol crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Builds a status error, truncating the body so logs stay small.
    pub fn status(context: &'static str, status: u16, body: &str) -> Self {
        let body: String = body.chars().take(400).collect();
        Error::Status {
            context,
            status,
            body,
        }
    }
}

/// Whether a 403 answer's body is the chat service's "you are not a member of this thread":
/// error code 209 with the sub code `AclCheckFailed` ("The initiator ... is not a member of ...").
fn is_not_member_body(body: &str) -> bool {
    body.contains("AclCheckFailed")
        || body.contains("\"errorCode\":209")
        || body.contains("is not a member of")
}

impl Error {
    /// Turns a 403 that says the user is not in the thread into [`Error::NotMember`]; any other
    /// error comes back unchanged.
    pub fn into_not_member(self) -> Self {
        match self {
            Error::Status {
                context,
                status: 403,
                body,
            } if is_not_member_body(&body) => Error::NotMember { context },
            e => e,
        }
    }

    /// True when the request may succeed later without changes: no connection, a timeout, a
    /// dropped socket or a temporary server answer. False for rejections such as 400 or 403.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Http(e) => e.is_connect() || e.is_timeout() || e.is_request(),
            Error::Ws(_) | Error::NotReady => true,
            Error::Status { status, .. } => matches!(status, 408 | 425 | 429 | 502 | 503 | 504),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_status_codes_are_transient_and_rejections_are_not() {
        for s in [408, 429, 502, 503, 504] {
            assert!(Error::status("send", s, "").is_transient(), "{s}");
        }
        for s in [400, 401, 403, 404, 409, 500] {
            assert!(!Error::status("send", s, "").is_transient(), "{s}");
        }
        assert!(!Error::Protocol("x".into()).is_transient());
        assert!(!Error::Auth("x".into()).is_transient());
        assert!(!Error::NotSignedIn.is_transient());
    }

    #[test]
    fn a_request_before_the_first_refresh_is_transient_and_says_so_plainly() {
        assert!(Error::NotReady.is_transient());
        let shown = Error::NotReady.to_string();
        assert!(!shown.contains("url") && !shown.contains("protocol"), "{shown}");
    }

    // The answer for a meeting chat the user was invited to and has not joined (made-up ids).
    const NOT_A_MEMBER: &str = r#"{"errorCode":209,"message":"{\"subCode\":\"AclCheckFailed\",\"details\":\"The initiator 8:orgid:00000000-0000-0000-0000-000000000000 is not a member of the thread 19:meeting_abc@thread.v2\"}"}"#;

    #[test]
    fn a_403_for_a_thread_the_user_is_not_in_is_a_not_member_error() {
        let e = Error::status("messages", 403, NOT_A_MEMBER).into_not_member();
        assert!(
            matches!(
                e,
                Error::NotMember {
                    context: "messages"
                }
            ),
            "{e:?}"
        );
        assert!(!e.is_transient());
        // Nothing of the raw answer is left in the text.
        let shown = e.to_string();
        assert!(!shown.contains("HTTP") && !shown.contains('{'), "{shown}");
        // The cut body (400 characters) still reads as one.
        let cut = Error::status(
            "messages",
            403,
            &format!("{NOT_A_MEMBER}{}", "x".repeat(500)),
        );
        assert!(matches!(cut.into_not_member(), Error::NotMember { .. }));
        let by_code = Error::status("messages", 403, r#"{"errorCode":209}"#);
        assert!(matches!(by_code.into_not_member(), Error::NotMember { .. }));
    }

    #[test]
    fn other_answers_are_not_turned_into_not_member() {
        for (status, body) in [
            (403, r#"{"errorCode":403,"message":"Forbidden"}"#),
            (404, NOT_A_MEMBER),
            (500, ""),
        ] {
            let e = Error::status("messages", status, body).into_not_member();
            assert!(matches!(e, Error::Status { .. }), "{status}: {e:?}");
        }
        assert!(matches!(
            Error::NotSignedIn.into_not_member(),
            Error::NotSignedIn
        ));
    }

    #[test]
    fn socket_errors_are_transient() {
        let e = tokio_tungstenite::tungstenite::Error::ConnectionClosed;
        assert!(Error::Ws(Box::new(e)).is_transient());
    }

    #[tokio::test]
    async fn refused_connection_is_transient() {
        // Nothing listens on port 1.
        crate::client::install_crypto_provider();
        let err = reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        assert!(Error::Http(err).is_transient());
    }

    #[tokio::test]
    async fn a_failed_request_says_why_it_failed() {
        crate::client::install_crypto_provider();
        let err = reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        let shown = Error::from(err).to_string();
        assert!(
            shown.starts_with("http: error sending request: "),
            "{shown}"
        );
        assert!(
            shown.to_lowercase().contains("refused") || shown.contains("connect"),
            "the cause is shown: {shown}"
        );
    }

    #[tokio::test]
    async fn a_failed_request_does_not_show_its_url() {
        crate::client::install_crypto_provider();
        let err = reqwest::Client::new()
            .get("http://127.0.0.1:1/v1/users/ME/conversations/19:secret@thread.v2/messages")
            .send()
            .await
            .unwrap_err();
        let shown = Error::from(err).to_string();
        assert!(shown.starts_with("http: "), "{shown}");
        assert!(
            !shown.contains("127.0.0.1") && !shown.contains("19:secret"),
            "{shown}"
        );
    }
}
