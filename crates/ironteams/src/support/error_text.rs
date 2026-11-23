//! The words an error shows in the banner: a failed command's text, and raw status lines cut out.

/// The text a failed command shows. A request Teams never answered says so plainly instead of
/// the client library's wording, and a refusal says what it means instead of the status line and
/// the server's JSON (the log line of the failure has those).
pub(crate) fn shown_error(err: &anyhow::Error) -> String {
    match err.downcast_ref::<teams_proto::Error>() {
        Some(e) if e.is_transient() => {
            "Couldn't reach Teams. Check the connection and try again.".to_owned()
        }
        Some(teams_proto::Error::Status { status, .. }) => status_text(*status).to_owned(),
        Some(teams_proto::Error::NotMember { .. }) => NOT_A_MEMBER_BANNER.to_owned(),
        _ => err.to_string(),
    }
}

const NOT_A_MEMBER_BANNER: &str = "You're not a member of this conversation.";

/// A short sentence for an HTTP status Teams answered with.
fn status_text(status: u16) -> &'static str {
    match status {
        401 => "Teams asked you to sign in again.",
        403 => "Teams didn't allow that.",
        404 => "Teams couldn't find that.",
        408 | 425 | 429 | 502 | 503 | 504 => {
            "Couldn't reach Teams. Check the connection and try again."
        }
        500..=599 => "Teams had a problem. Try again in a moment.",
        _ => "Teams couldn't do that. Try again.",
    }
}

/// Banner text with a raw status line cut out. Error texts built as `"{what}: {err}"` carry the
/// protocol error's `context: HTTP 403: {json}`; that part becomes a short sentence and the
/// original goes to the log. Text without one comes back as it was.
pub(crate) fn plain_banner(text: &str) -> String {
    let Some(at) = text.find(": HTTP ") else {
        return text.to_owned();
    };
    let digits = &text[at + ": HTTP ".len()..];
    let code: String = digits.chars().take_while(char::is_ascii_digit).collect();
    if code.len() != 3 {
        return text.to_owned();
    }
    tracing::warn!(raw = %teams_proto::logsafe::text(text), "an error with a raw HTTP answer would have reached the banner");
    // The context word(s) in front of the status line belong to it ("messages", "delve person").
    let start = text[..at].rfind(": ").map_or(0, |i| i + 2);
    let sentence = status_text(code.parse().unwrap_or(0));
    format!("{}{sentence}", &text[..start])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unanswered_request_says_so_and_other_errors_keep_their_text() {
        let unanswered = anyhow::Error::from(teams_proto::Error::status("messages", 503, ""));
        assert_eq!(
            shown_error(&unanswered),
            "Couldn't reach Teams. Check the connection and try again."
        );
        let refused = anyhow::Error::from(teams_proto::Error::status("messages", 403, "no"));
        assert_eq!(shown_error(&refused), "Teams didn't allow that.");
        let gone = anyhow::Error::from(teams_proto::Error::status("send", 404, r#"{"a":1}"#));
        assert_eq!(shown_error(&gone), "Teams couldn't find that.");
        let broken = anyhow::Error::from(teams_proto::Error::status("send", 500, "boom"));
        assert_eq!(
            shown_error(&broken),
            "Teams had a problem. Try again in a moment."
        );
        let left = anyhow::Error::from(teams_proto::Error::NotMember {
            context: "messages",
        });
        assert_eq!(
            shown_error(&left),
            "You're not a member of this conversation."
        );
        assert_eq!(
            shown_error(&anyhow::anyhow!("This conversation was deleted.")),
            "This conversation was deleted."
        );
    }
    #[test]
    fn a_raw_status_line_never_reaches_the_banner() {
        let raw =
            r#"messages: HTTP 403: {"errorCode":209,"message":"{\"subCode\":\"AclCheckFailed\"}"}"#;
        assert_eq!(plain_banner(raw), "Teams didn't allow that.");
        assert_eq!(
            plain_banner(&format!("Couldn't load the scheduled messages: {raw}")),
            "Couldn't load the scheduled messages: Teams didn't allow that."
        );
        assert_eq!(
            plain_banner("delve person: HTTP 404: not found"),
            "Teams couldn't find that."
        );
        assert_eq!(
            plain_banner("Reminder set for 10:00"),
            "Reminder set for 10:00"
        );
        assert_eq!(plain_banner("HTTP is fine: HTTP/2"), "HTTP is fine: HTTP/2");
    }
}
