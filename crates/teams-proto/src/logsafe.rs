//! Text that is safe to put in the log file. The log lives on disk for weeks and goes into the
//! problem report, so a URL is logged without its query string (pre-signed links carry their
//! secret there) and a server's answer is logged short, with token-like runs masked.

/// The longest server text that is logged, in characters.
const MAX_TEXT: usize = 200;

/// `url` without the query string and the fragment. Scheme, host and path stay.
pub fn url(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    url[..end].to_owned()
}

/// A server's text for the log: query strings dropped, anything that looks like a token masked
/// (`Bearer x`, a run of 24 or more letters, digits, `+-_` that has a digit, any run of 40 or
/// more) and at most 200 characters, ending in `...` when it was cut.
pub fn text(text: &str) -> String {
    let clean = mask_secrets(&strip_queries(text));
    match clean.char_indices().nth(MAX_TEXT) {
        Some((cut, _)) => format!("{}...", &clean[..cut]),
        None => clean,
    }
}

/// `?a=b&c=d` inside a text becomes `?...`. A `?` that is only punctuation stays.
fn strip_queries(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('?') {
        let (before, after) = rest.split_at(at + 1);
        out.push_str(before);
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>' | '<'))
            .unwrap_or(after.len());
        let (query, tail) = after.split_at(end);
        if query.contains('=') {
            out.push_str("...");
        } else {
            out.push_str(query);
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

fn is_secret_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_')
}

/// Replaces token-like runs with `<secret>`. Dots and slashes end a run, so a host name or a path
/// is kept and each part of a JWT is masked on its own.
fn mask_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut after_bearer = false;
    while let Some(first) = rest.chars().next() {
        let end = rest.find(|c| !is_secret_char(c)).unwrap_or(rest.len());
        if end == 0 {
            out.push(first);
            rest = &rest[first.len_utf8()..];
            continue;
        }
        let (run, tail) = rest.split_at(end);
        let digit = run.bytes().any(|b| b.is_ascii_digit());
        if after_bearer || run.len() >= 40 || (run.len() >= 24 && digit) {
            out.push_str("<secret>");
        } else {
            out.push_str(run);
        }
        after_bearer = run.eq_ignore_ascii_case("bearer");
        rest = tail;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_loses_its_query_and_fragment() {
        assert_eq!(
            url("https://a.example/img/p.png?sig=abc&se=1#frag"),
            "https://a.example/img/p.png"
        );
        assert_eq!(
            url("https://a.example/p#only-fragment"),
            "https://a.example/p"
        );
    }

    #[test]
    fn url_without_a_query_is_unchanged() {
        let u = "https://eu-api.asm.skype.com/v1/objects/0-weu-d1-4c5f1b2a3d/views/imgo";
        assert_eq!(url(u), u);
    }

    #[test]
    fn short_plain_text_is_unchanged() {
        let t = "HTTP 403: not allowed to change this event. Why? Ask the organizer.";
        assert_eq!(text(t), t);
        assert_eq!(text(""), "");
    }

    #[test]
    fn long_text_is_cut_with_a_marker() {
        let long = "word ".repeat(100);
        let out = text(&long);
        assert_eq!(out.chars().count(), 203);
        assert!(out.ends_with("..."));
        assert!(out.starts_with("word word"));
    }

    #[test]
    fn the_cut_never_splits_a_character() {
        let long = "é".repeat(500);
        let out = text(&long);
        assert_eq!(out.chars().count(), 203);
    }

    #[test]
    fn a_text_of_exactly_the_limit_is_not_marked() {
        let exact = "a ".repeat(100);
        assert_eq!(text(&exact), exact);
    }

    #[test]
    fn token_like_runs_are_masked() {
        let hex = "0123456789abcdef0123456789abcdef";
        let out = text(&format!("failed with key {hex} for user"));
        assert_eq!(out, "failed with key <secret> for user");
        let b64 = "dGhpcyBpcyBhIHNlY3JldCB0b2tlbiB2YWx1ZSE9";
        assert_eq!(text(&format!("sig={b64}")), "sig=<secret>");
    }

    #[test]
    fn a_jwt_is_masked_part_by_part() {
        let jwt = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4ifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        assert_eq!(text(jwt), "<secret>.<secret>.<secret>");
    }

    #[test]
    fn a_bearer_token_is_masked_even_when_short() {
        assert_eq!(
            text("Authorization: Bearer abc123"),
            "Authorization: Bearer <secret>"
        );
        assert_eq!(text("bearer xyz"), "bearer <secret>");
    }

    #[test]
    fn a_query_inside_a_text_is_dropped() {
        assert_eq!(
            text("GET https://h.example/p?x=1&y=2 failed"),
            "GET https://h.example/p?... failed"
        );
    }

    #[test]
    fn long_words_and_hosts_without_digits_are_kept() {
        let t = "https://emea.ng.msg.teams.microsoft.com/v1/users InternalServerErrorOccurred";
        assert_eq!(text(t), t);
    }
}
