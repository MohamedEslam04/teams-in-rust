//! AMS (Azure Media Services for Skype): inline image download and upload.

use crate::client::Client;
use crate::error::{Error, Result};
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use url::Url;

/// Host of the media service URLs messages point at (`…/v1/objects/<id>/views/<view>`).
pub const AMS_VIEW_HOST: &str = "as-api.asm.skype.com";
/// Host objects are created and uploaded on.
const AMS_UPLOAD: &str = "https://as-prod.asyncgw.teams.microsoft.com/v1/objects/";
/// AMS object type of a voice message recording. Confirmed 2026-11-21 from the metadata of a
/// voice message Teams mobile uploaded (`ironteams-cli ams-meta`): `sharing/audio`, bytes in the
/// `audio` slot (Teams adds a `transcript_info` slot itself).
pub const AUDIO_OBJECT_TYPE: &str = "sharing/audio";
/// Content slot an audio object's bytes are written to (its view is `audio`).
const AUDIO_CONTENT: &str = "audio";

/// Set once an `asm.skype.com` address could not be reached while its Teams-domain twin answered,
/// so later downloads go to the twin first instead of waiting for the first address to fail
/// again. Per process: it is a property of the network the app runs on.
static PREFER_ASYNCGW: AtomicBool = AtomicBool::new(false);

/// Largest image we download.
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// True if `url` is an AMS image URL we may send the skypetoken to.
pub fn is_ams_url(url: &str) -> bool {
    let Ok(u) = Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or_default();
    u.scheme() == "https"
        && (host == "api.asm.skype.com"
            || host.ends_with("-api.asm.skype.com")
            || host.ends_with(".asyncgw.teams.microsoft.com"))
}

/// Rewrites a thumbnail view URL to the full-size view.
pub fn full_size_url(url: &str) -> String {
    for view in ["/views/imgt1", "/views/imgo"] {
        if let Some(base) = url.strip_suffix(view) {
            return format!("{base}/views/imgpsh_fullsize");
        }
    }
    url.to_string()
}

/// HTML for an inline image message referencing an uploaded AMS object.
pub fn image_html(object_id: &str, width: u32, height: u32) -> String {
    format!(
        r#"<p><img itemscope="image" style="vertical-align:bottom" src="https://as-api.asm.skype.com/v1/objects/{object_id}/views/imgo" alt="image" itemtype="http://schema.skype.com/AMSImage" width="{width}" height="{height}" id="{object_id}"></p>"#
    )
}

/// The Teams-domain twin of an AMS address: `https://us-api.asm.skype.com/v1/objects/<id>/...` is
/// also served at `https://us-prod.asyncgw.teams.microsoft.com/v1/objects/<id>/...`, the host the
/// uploads already use. Some networks cannot reach the old Skype domain (a blocked name, a
/// filtered address) while the Teams one works. `None` for an address that is not a regional
/// `<region>-api.asm.skype.com` one.
pub fn asyncgw_twin(url: &str) -> Option<String> {
    let mut u = Url::parse(url).ok()?;
    let region = u.host_str()?.strip_suffix("-api.asm.skype.com")?.to_owned();
    if region.is_empty() || region.contains('.') {
        return None;
    }
    u.set_host(Some(&format!("{region}-prod.asyncgw.teams.microsoft.com")))
        .ok()?;
    Some(u.into())
}

/// The addresses to try for one AMS download, in order: the address itself, then its twin; the
/// twin first when `prefer_twin`.
pub fn ams_attempts_with(url: &str, prefer_twin: bool) -> Vec<String> {
    match asyncgw_twin(url) {
        Some(twin) if prefer_twin => vec![twin, url.to_owned()],
        Some(twin) => vec![url.to_owned(), twin],
        None => vec![url.to_owned()],
    }
}

/// [`ams_attempts_with`] with what this process learned about the network.
pub fn ams_attempts(url: &str) -> Vec<String> {
    ams_attempts_with(url, PREFER_ASYNCGW.load(Ordering::Relaxed))
}

/// A download that had to leave its first address succeeded at `used`: remember which domain
/// works from here.
pub fn ams_remember(used: &str) {
    PREFER_ASYNCGW.store(
        used.contains(".asyncgw.teams.microsoft.com"),
        Ordering::Relaxed,
    );
}

impl Client {
    /// Gets the bytes of an AMS address, falling back to its twin when the connection fails (not
    /// when the service answers with an error: the object is the same on both).
    async fn ams_bytes(
        &self,
        url: &str,
        extra: &[(&str, String)],
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let mut last = None;
        for (i, attempt) in ams_attempts(url).iter().enumerate() {
            match self.bytes(attempt, extra, max_bytes).await {
                Ok(bytes) => {
                    if i > 0 {
                        ams_remember(attempt);
                    }
                    return Ok(bytes);
                }
                Err(e @ Error::Http(_)) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| Error::Protocol("no AMS address".into())))
    }

    /// Downloads an AMS image. Refuses non-AMS hosts so the token never leaves Microsoft.
    pub async fn download_image(&self, url: &str) -> Result<Vec<u8>> {
        if !is_ams_url(url) {
            return Err(Error::Protocol(
                "refusing to send credentials to a non-AMS image host".into(),
            ));
        }
        let skype = self.tokens().await.skype.unwrap_or_default();
        self.ams_bytes(
            url,
            &[
                ("Cookie", format!("skypetoken_asm={skype}")),
                ("Accept", "image/*".into()),
            ],
            MAX_IMAGE_BYTES,
        )
        .await
    }

    /// Downloads any AMS object (voice messages, videos). Refuses non-AMS hosts.
    pub async fn download_media(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>> {
        if !is_ams_url(url) {
            return Err(Error::Protocol(
                "refusing to send credentials to a non-AMS host".into(),
            ));
        }
        let skype = self.tokens().await.skype.unwrap_or_default();
        self.ams_bytes(
            url,
            &[("Cookie", format!("skypetoken_asm={skype}"))],
            max_bytes,
        )
        .await
    }

    /// Uploads an image visible to `conversation_id` and returns the AMS object id.
    pub async fn upload_image(
        &self,
        conversation_id: &str,
        filename: &str,
        data: Vec<u8>,
    ) -> Result<String> {
        self.upload_object(conversation_id, filename, "pish/image", "imgpsh", data)
            .await
    }

    /// Uploads a voice message recording (AAC in MP4) visible to `conversation_id` and returns
    /// the AMS object id.
    pub async fn upload_voice(
        &self,
        conversation_id: &str,
        filename: &str,
        data: Vec<u8>,
    ) -> Result<String> {
        self.upload_object(
            conversation_id,
            filename,
            AUDIO_OBJECT_TYPE,
            AUDIO_CONTENT,
            data,
        )
        .await
    }

    /// Creates an AMS object of `kind` that `conversation_id` may read, writes `data` to its
    /// `content` slot and returns its id.
    pub(crate) async fn upload_object(
        &self,
        conversation_id: &str,
        filename: &str,
        kind: &str,
        content: &str,
        data: Vec<u8>,
    ) -> Result<String> {
        let mut perms = serde_json::Map::new();
        perms.insert(conversation_id.to_string(), json!(["read"]));
        let body = json!({"type": kind, "permissions": perms, "filename": filename, "sharingMode": "Inline"});
        let v = self
            .json("ams_create", Method::POST, AMS_UPLOAD, Some(&body))
            .await?;
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Protocol("AMS create returned no id".into()))?
            .to_string();
        let url = format!("{AMS_UPLOAD}{id}/content/{content}");
        self.ensure_fresh().await?;
        let skype = self.tokens().await.skype.unwrap_or_default();
        let resp = self
            .http()
            .put(&url)
            .header("Authorization", format!("skype_token {skype}"))
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::status(
                "ams_upload",
                status,
                &resp.text().await.unwrap_or_default(),
            ));
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ams_host_allowlist() {
        assert!(is_ams_url(
            "https://eu-api.asm.skype.com/v1/objects/0-x/views/imgo"
        ));
        assert!(is_ams_url(
            "https://api.asm.skype.com/v1/objects/0-x/views/imgo"
        ));
        assert!(is_ams_url(
            "https://as-prod.asyncgw.teams.microsoft.com/v1/objects/0-x"
        ));
        assert!(!is_ams_url("http://eu-api.asm.skype.com/v1/objects/0-x"));
        assert!(!is_ams_url("https://evil.com/api.asm.skype.com"));
        assert!(!is_ams_url("https://asm.skype.com.evil.com/x"));
    }

    #[test]
    fn an_ams_address_has_a_twin_on_the_teams_domain() {
        assert_eq!(
            asyncgw_twin("https://us-api.asm.skype.com/v1/objects/0-eus-d1-a/views/imgo?x=1"),
            Some(
                "https://us-prod.asyncgw.teams.microsoft.com/v1/objects/0-eus-d1-a/views/imgo?x=1"
                    .into()
            )
        );
        assert_eq!(
            asyncgw_twin("https://as-api.asm.skype.com/v1/objects/0-x/views/imgo"),
            Some("https://as-prod.asyncgw.teams.microsoft.com/v1/objects/0-x/views/imgo".into())
        );
        // Already on the Teams domain, no region, or not AMS at all: no twin.
        assert_eq!(
            asyncgw_twin("https://us-prod.asyncgw.teams.microsoft.com/v1/objects/0-x"),
            None
        );
        assert_eq!(
            asyncgw_twin("https://api.asm.skype.com/v1/objects/0-x"),
            None
        );
        assert_eq!(asyncgw_twin("https://evil.com/v1/objects/0-x"), None);
        assert_eq!(
            asyncgw_twin("https://x.evil-api.asm.skype.com.evil.com/v1/objects/0-x"),
            None
        );
    }

    #[test]
    fn the_attempts_try_the_address_then_its_twin_and_the_twin_first_once_it_is_known_to_work() {
        let url = "https://us-api.asm.skype.com/v1/objects/0-x/views/imgo";
        let twin = "https://us-prod.asyncgw.teams.microsoft.com/v1/objects/0-x/views/imgo";
        assert_eq!(ams_attempts_with(url, false), [url, twin]);
        assert_eq!(ams_attempts_with(url, true), [twin, url]);
        // Without a twin there is one attempt either way.
        let own = "https://api.asm.skype.com/v1/objects/0-x/views/imgo";
        assert_eq!(ams_attempts_with(own, true), [own]);
    }

    #[test]
    fn full_size_rewrite() {
        assert_eq!(
            full_size_url("https://h/v1/objects/1/views/imgo"),
            "https://h/v1/objects/1/views/imgpsh_fullsize"
        );
        assert_eq!(
            full_size_url("https://h/v1/objects/1/views/imgt1"),
            "https://h/v1/objects/1/views/imgpsh_fullsize"
        );
        assert_eq!(full_size_url("https://h/x.png"), "https://h/x.png");
    }

    #[test]
    fn image_html_shape() {
        let h = image_html("0-weu-d1-abc", 250, 141);
        assert!(h.contains("itemtype=\"http://schema.skype.com/AMSImage\""));
        assert!(h.contains("/v1/objects/0-weu-d1-abc/views/imgo"));
    }
}
