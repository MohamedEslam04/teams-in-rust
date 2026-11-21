//! Opt-in "new version" check (plan X3.5).
//!
//! Off unless the user turns on Settings > About > "Check for updates". Then IronTeams makes one
//! `GET` to the GitHub releases API per start (and one more for each "Check now"). The request has
//! no cookies, no token and nothing about the user: only an `Accept` header and the generic
//! `User-Agent: ironteams`. The answer is compared with this build's version; IronTeams never downloads or
//! installs anything (the package managers update the Flatpak, AUR and RPM installs). Copies that
//! can update themselves (Velopack installs) use `updater` instead.
//!
//! The version comparison and the JSON parsing are pure. The network call is skipped in demo
//! mode (`IRONTEAMS_DEMO`) and in tests.

use std::cmp::Ordering;
use std::time::Duration;

pub const LATEST_URL: &str =
    "https://api.github.com/repos/MohamedEslam04/teams-in-rust/releases/latest";
/// Where "Release notes" goes when the answer has no usable `html_url`.
pub const RELEASES_URL: &str = "https://github.com/MohamedEslam04/teams-in-rust/releases";
const USER_AGENT: &str = "ironteams";
const TIMEOUT: Duration = Duration::from_secs(10);

/// A parsed version: `major.minor.patch`, and whether it has a pre-release suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: bool,
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // A pre-release is older than the same release.
        self.core
            .cmp(&other.core)
            .then_with(|| other.pre.cmp(&self.pre))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Reads `1.2.3`, `v1.2.3`, `1.2` or `1.2.3-rc.1+build5`. `None` for anything else.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s
        .strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s);
    // Build metadata does not take part in the order.
    let s = s.split('+').next().unwrap_or("");
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) if !p.is_empty() => (c, true),
        Some(_) => return None,
        None => (s, false),
    };
    let mut nums = [0u64; 3];
    for (n, part) in core.split('.').enumerate() {
        if n == 3 || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        nums[n] = part.parse().ok()?;
    }
    Some(Version { core: nums, pre })
}

/// `true` when `latest` is a newer version than `current`. Unreadable versions are never newer.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// The newest release as far as IronTeams cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The tag without a leading `v`.
    pub version: String,
    /// The release page to open.
    pub url: String,
}

impl Release {
    /// Reads the GitHub "latest release" JSON (`tag_name`, `html_url`). The URL is only used
    /// when it is a `https://github.com/` address.
    pub fn from_json(body: &str) -> Result<Release, String> {
        let v: serde_json::Value =
            serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
        let tag = v
            .get("tag_name")
            .and_then(|t| t.as_str())
            .ok_or("no tag_name")?
            .trim();
        parse_version(tag).ok_or("unreadable tag_name")?;
        let version = tag
            .strip_prefix('v')
            .or_else(|| tag.strip_prefix('V'))
            .unwrap_or(tag)
            .to_string();
        let url = v
            .get("html_url")
            .and_then(|u| u.as_str())
            .filter(|u| u.starts_with("https://github.com/"))
            .unwrap_or(RELEASES_URL)
            .to_string();
        Ok(Release { version, url })
    }
}

/// `false` in demo mode and in tests: nothing may reach the network from there.
pub fn network_allowed() -> bool {
    !(cfg!(test) || compat::var_os("IRONTEAMS_DEMO").is_some())
}

/// Asks GitHub for the latest release. Blocks (up to the 10 s timeout), so call it from a
/// thread. Fails without a request in demo mode and in tests.
pub fn fetch_latest() -> Result<Release, String> {
    if !network_allowed() {
        return Err("update checks are off in demo mode and in tests".into());
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(get_latest())
}

async fn get_latest() -> Result<Release, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(LATEST_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;
    Release::from_json(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags_and_versions() {
        assert!(parse_version("1.2.3").is_some());
        assert_eq!(parse_version("v1.2.3"), parse_version("1.2.3"));
        assert_eq!(parse_version("V1.2.3"), parse_version("1.2.3"));
        assert_eq!(parse_version(" 1.2 "), parse_version("1.2.0"));
        assert_eq!(parse_version("1"), parse_version("1.0.0"));
        assert_eq!(parse_version("1.2.3+build5"), parse_version("1.2.3"));
        assert!(parse_version("1.2.3-rc.1").unwrap().pre);
        for bad in [
            "", "v", "x", "1.2.3.4", "1..2", "1.a.3", "1.2.3-", "-1.2.3", "+1.2.3", "1.-2",
        ] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn newer_means_a_higher_number() {
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.9", "0.2.0"));
        assert!(is_newer("0.9.9", "1.0.0"));
        assert!(is_newer("1.2.3", "v1.10.0"), "numbers, not text");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.0.0", "v1.0.0"), "a leading v changes nothing");
    }

    #[test]
    fn a_pre_release_is_older_than_the_same_release() {
        assert!(
            is_newer("1.0.0-rc.1", "1.0.0"),
            "the release replaces its rc"
        );
        assert!(!is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0-rc.1"));
        assert!(is_newer("0.9.0", "1.0.0-rc.1"), "a later core still wins");
        assert!(!is_newer("1.0.1", "1.0.0-rc.1"));
    }

    #[test]
    fn unreadable_versions_are_never_newer() {
        assert!(!is_newer("0.1.0", "nightly"));
        assert!(!is_newer("dev", "1.0.0"));
        assert!(!is_newer("", ""));
    }

    #[test]
    fn reads_the_release_json() {
        let r = Release::from_json(
            r#"{"tag_name":"v0.3.0","html_url":"https://github.com/MohamedEslam04/teams-in-rust/releases/tag/v0.3.0","assets":[]}"#,
        )
        .unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(
            r.url,
            "https://github.com/MohamedEslam04/teams-in-rust/releases/tag/v0.3.0"
        );
    }

    #[test]
    fn a_foreign_or_missing_release_url_falls_back_to_the_releases_page() {
        let r = Release::from_json(r#"{"tag_name":"0.3.0","html_url":"https://evil.invalid/x"}"#)
            .unwrap();
        assert_eq!(r.url, RELEASES_URL);
        let r = Release::from_json(r#"{"tag_name":"0.3.0"}"#).unwrap();
        assert_eq!(r.url, RELEASES_URL);
    }

    #[test]
    fn bad_answers_are_errors() {
        for body in [
            "",
            "{",
            "[]",
            r#"{"message":"Not Found"}"#,
            r#"{"tag_name":"soon"}"#,
            r#"{"tag_name":7}"#,
        ] {
            assert!(Release::from_json(body).is_err(), "{body:?}");
        }
    }

    #[test]
    fn both_urls_name_the_repository_of_the_update_feeds() {
        let repo = crate::updater::REPO_URL.trim_start_matches("https://github.com/");
        assert!(LATEST_URL.contains(&format!("/repos/{repo}/")));
        assert!(RELEASES_URL.starts_with(crate::updater::REPO_URL));
    }

    #[test]
    fn tests_never_reach_the_network() {
        assert!(!network_allowed());
        assert!(fetch_latest().is_err());
    }
}
