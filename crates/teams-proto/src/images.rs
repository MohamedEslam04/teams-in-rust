//! Data layer of the image viewer and the image grid (plan item 9.5), without any UI.
//!
//! * [`image_refs`] lists every image of a message body in order, which is the viewer's
//!   next/previous list ([`message_images`] adds image files attached to the message).
//! * [`ImageRef::candidates`] and [`fetch`] resolve the full-size URL and download it with the
//!   right credential: the skypetoken for AMS objects, a Graph (or SharePoint) bearer for
//!   OneDrive/SharePoint files, and no credential for anything else.
//! * [`grid_layout`] places N images in the 2-column grid of a message bubble.
//! * [`save_image`] writes the bytes to disk with a safe name, a matching extension and
//!   without ever overwriting a file.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Method;
use url::Url;

use crate::ams::is_ams_url;
use crate::broker::Audience;
use crate::client::{Client, RawBody};
use crate::error::{Error, Result};
use crate::http::headers_for;
use crate::model::FileRef;

/// Largest full-size image we download.
pub const MAX_FULL_IMAGE_BYTES: usize = 50 * 1024 * 1024;

/// AMS view with the biggest picture (what the Teams web viewer asks for).
pub const VIEW_FULLSIZE: &str = "imgpsh_fullsize";
/// AMS view that inline `<img>` tags point at (the original, shown scaled by `width`/`height`).
pub const VIEW_ORIGINAL: &str = "imgo";

// ---------------------------------------------------------------------------------------------
// Image references

/// Where an image is served from, which decides the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageHost {
    /// An AMS object (`*-api.asm.skype.com/v1/objects/<id>/views/<view>`): skypetoken.
    Ams,
    /// A OneDrive or SharePoint file on this host: Graph bearer (shares API), SharePoint
    /// bearer as the fallback.
    SharePoint(String),
    /// Any other https URL: fetched without credentials.
    External,
}

/// One image of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    /// The URL the message names (an AMS view URL, a file URL, or a public URL).
    pub url: String,
    pub host: ImageHost,
    /// The AMS object id when this is an AMS image.
    pub object_id: Option<String>,
    /// Size attributes of the `<img>` tag (the size it was posted at, not the pixel size).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// A human name: the file name for file images, the `title` or a useful `alt` for inline
    /// ones. `None` when the message does not name the image.
    pub name: Option<String>,
}

impl ImageRef {
    /// Width and height when both are known and non-zero.
    pub fn size(&self) -> Option<(u32, u32)> {
        match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
            _ => None,
        }
    }

    /// A name to build a file name from: the given name, else the AMS object id, else the last
    /// path segment of the URL, else `image`.
    pub fn suggested_name(&self) -> String {
        if let Some(n) = self.name.as_deref().filter(|n| !n.trim().is_empty()) {
            return n.to_string();
        }
        if let Some(id) = &self.object_id {
            return id.clone();
        }
        Url::parse(&self.url)
            .ok()
            .and_then(|u| {
                u.path_segments()?
                    .rfind(|s| !s.is_empty())
                    .map(percent_decode)
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "image".to_string())
    }

    /// The URLs to try for the full-size picture, best first. An AMS object gives
    /// `views/imgpsh_fullsize` and then `views/imgo` (a thumbnail view is never returned). Other
    /// hosts give the URL itself; [`fetch`] decides how to authenticate it.
    pub fn candidates(&self) -> Vec<String> {
        match (&self.host, ams_base(&self.url)) {
            (ImageHost::Ams, Some(base)) => vec![
                format!("{base}/views/{VIEW_FULLSIZE}"),
                format!("{base}/views/{VIEW_ORIGINAL}"),
            ],
            _ => vec![self.url.clone()],
        }
    }
}

/// `https://host/v1/objects/<id>` of an AMS view URL, without the `/views/...` part.
fn ams_base(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    let path = u.path();
    let idx = path.find("/v1/objects/")?;
    let rest = &path[idx + "/v1/objects/".len()..];
    let id = rest.split('/').next().filter(|s| !s.is_empty())?;
    Some(format!(
        "{}://{}{}/v1/objects/{id}",
        u.scheme(),
        u.host_str()?,
        &path[..idx]
    ))
}

/// True when `s` looks like an AMS object id, for example `0-weu-d1-3f9a…`.
pub fn is_ams_object_id(s: &str) -> bool {
    let mut parts = s.split('-');
    let ok = parts.next() == Some("0")
        && parts.next().is_some_and(|r| {
            (2..=5).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        && parts.next().is_some_and(|d| {
            d.len() >= 2 && d.starts_with('d') && d[1..].bytes().all(|b| b.is_ascii_digit())
        })
        && parts
            .next()
            .is_some_and(|h| h.len() >= 8 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    ok && parts.next().is_none()
}

/// The AMS view URL for an object id (from an `itemid` attribute) when the tag has no usable
/// `src`. The region in the id picks the host: `us`, `eu` or `as`; an id of a region we do not
/// know goes to the global host `api.asm.skype.com`.
pub fn ams_url_for_object(id: &str) -> Option<String> {
    if !is_ams_object_id(id) {
        return None;
    }
    let region = id.split('-').nth(1)?;
    let geo = match region {
        r if r.ends_with("us") => "us-api",
        "weu" | "neu" | "frc" | "uks" | "ukw" | "gwc" | "sec" => "eu-api",
        "sea" | "eau" | "jpe" | "jpw" | "inc" | "aue" => "as-api",
        _ => "api",
    };
    Some(format!(
        "https://{geo}.asm.skype.com/v1/objects/{id}/views/{VIEW_ORIGINAL}"
    ))
}

fn classify(url: &str) -> Option<ImageHost> {
    let u = Url::parse(url).ok()?;
    if u.scheme() != "https" {
        return None;
    }
    if is_ams_url(url) {
        return Some(ImageHost::Ams);
    }
    if let Some(Audience::SharePoint(h)) = Audience::sharepoint(u.host_str()?) {
        return Some(ImageHost::SharePoint(h));
    }
    Some(ImageHost::External)
}

/// Alt texts that carry no information (Teams writes `image` for pasted pictures).
fn useful_alt(alt: &str) -> bool {
    let a = alt.trim().to_ascii_lowercase();
    !a.is_empty() && !matches!(a.as_str(), "image" | "img" | "picture" | "photo")
}

/// A number from an HTML size attribute: `250`, `250px` and the fractions the Teams web client
/// writes (`383.0313014827018`, rounded) count; `auto` and `50%` do not.
fn px(v: &str) -> Option<u32> {
    let v = v.trim();
    let digits = v.strip_suffix("px").unwrap_or(v).trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let n: f64 = digits.parse().ok()?;
    (n.is_finite() && n < f64::from(u32::MAX)).then(|| n.round() as u32)
}

/// `width:300px` or `height: 200px` inside a `style` attribute.
fn style_px(style: &str, prop: &str) -> Option<u32> {
    style.split(';').find_map(|decl| {
        let (k, v) = decl.split_once(':')?;
        (k.trim().eq_ignore_ascii_case(prop)).then(|| px(v))?
    })
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn is_emoji(attrs: &[(String, String)]) -> bool {
    let itemtype = attr(attrs, "itemtype").unwrap_or_default();
    let class = attr(attrs, "class").unwrap_or_default();
    let src = attr(attrs, "src").unwrap_or_default();
    itemtype.ends_with("/Emoji")
        || class.contains("emoticon")
        || class.contains("emoji")
        || src.contains("/evergreen-assets/skype/")
}

/// Every image of a message body in document order, with the `width` and `height` of its tag.
/// Emoji, `data:` pictures and tags without a usable `https` source are left out. An `<img>`
/// without `src` but with an AMS object id in `itemid` resolves to that object.
pub fn image_refs(html: &str) -> Vec<ImageRef> {
    img_tags(html)
        .iter()
        .filter(|attrs| !is_emoji(attrs))
        .filter_map(|attrs| {
            let src = attr(attrs, "src").map(str::trim).unwrap_or_default();
            let (url, host) = match classify(src) {
                Some(h) => (src.to_string(), h),
                None => {
                    let url = ams_url_for_object(attr(attrs, "itemid")?.trim())?;
                    (url, ImageHost::Ams)
                }
            };
            let object_id = (host == ImageHost::Ams)
                .then(|| ams_base(&url))
                .flatten()
                .and_then(|b| b.rsplit('/').next().map(str::to_string));
            let style = attr(attrs, "style").unwrap_or_default();
            let width = attr(attrs, "width")
                .and_then(px)
                .or_else(|| style_px(style, "width"));
            let height = attr(attrs, "height")
                .and_then(px)
                .or_else(|| style_px(style, "height"));
            let name = attr(attrs, "title")
                .filter(|t| !t.trim().is_empty())
                .or_else(|| attr(attrs, "alt").filter(|a| useful_alt(a)))
                .map(|s| s.trim().to_string());
            Some(ImageRef {
                url,
                host,
                object_id,
                width,
                height,
                name,
            })
        })
        .collect()
}

/// The viewer's list for one message: its inline images in order, then its attached image
/// files (OneDrive/SharePoint), skipping a file whose URL is already listed.
pub fn message_images(html: &str, files: &[FileRef]) -> Vec<ImageRef> {
    let mut out = image_refs(html);
    for f in files {
        if !is_image_extension(&f.file_type) && !is_image_extension(extension_of(&f.name)) {
            continue;
        }
        let Some(host) = classify(&f.url) else {
            continue;
        };
        if out.iter().any(|i| i.url == f.url) {
            continue;
        }
        out.push(ImageRef {
            url: f.url.clone(),
            host,
            object_id: None,
            width: None,
            height: None,
            name: Some(f.name.clone()).filter(|n| !n.is_empty()),
        });
    }
    out
}

fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, e)| e)
}

fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tif" | "tiff" | "heic" | "avif"
    )
}

/// The attribute lists of every `<img>` tag, names lower-cased and values entity-decoded.
fn img_tags(html: &str) -> Vec<Vec<(String, String)>> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"<!--") {
            i = html[i + 4..].find("-->").map_or(b.len(), |e| i + 4 + e + 3);
            continue;
        }
        let is_img = b.len() >= i + 5
            && b[i + 1..i + 4].eq_ignore_ascii_case(b"img")
            && matches!(b[i + 4], b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>');
        if !is_img {
            i += 1;
            continue;
        }
        i += 4;
        let mut attrs = Vec::new();
        loop {
            while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
                i += 1;
            }
            if i >= b.len() || b[i] == b'>' {
                i += 1;
                break;
            }
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/')
            {
                i += 1;
            }
            let name = html[start..i].to_ascii_lowercase();
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let mut value = String::new();
            if i < b.len() && b[i] == b'=' {
                i += 1;
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                    let q = b[i];
                    i += 1;
                    let s = i;
                    while i < b.len() && b[i] != q {
                        i += 1;
                    }
                    value = decode_entities(&html[s..i]);
                    i += 1;
                } else {
                    let s = i;
                    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                        i += 1;
                    }
                    value = decode_entities(&html[s..i]);
                }
            }
            if !name.is_empty() {
                attrs.push((name, value));
            }
        }
        out.push(attrs);
    }
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let ch = match &rest[1..e] {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                n if n.starts_with("#x") || n.starts_with("#X") => {
                    char::from_u32(u32::from_str_radix(&n[2..], 16).ok()?)?
                }
                n if n.starts_with('#') => char::from_u32(n[1..].parse().ok()?)?,
                _ => return None,
            };
            Some((ch, e + 1))
        });
        match decoded {
            Some((ch, used)) => {
                out.push(ch);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------------------------
// Fetching

/// A downloaded image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedImage {
    pub bytes: Vec<u8>,
    /// `image/png`, `image/jpeg`, ... From the bytes when the server's type is missing or
    /// generic, otherwise the server's.
    pub content_type: String,
    /// The URL that answered (for AMS the view that worked).
    pub url: String,
}

/// The image content type of `bytes` from their magic number, if it is a known format.
pub fn sniff_content_type(bytes: &[u8]) -> Option<&'static str> {
    let b = bytes;
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"BM") && b.len() > 14 {
        Some("image/bmp")
    } else if b.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || b.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        Some("image/tiff")
    } else if b.len() >= 12 && &b[4..8] == b"ftyp" && matches!(&b[8..12], b"avif" | b"avis") {
        Some("image/avif")
    } else if b.len() >= 12
        && &b[4..8] == b"ftyp"
        && matches!(&b[8..12], b"heic" | b"heix" | b"mif1")
    {
        Some("image/heic")
    } else if b.starts_with(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else {
        let head = String::from_utf8_lossy(&b[..b.len().min(512)]).to_ascii_lowercase();
        (head.contains("<svg")).then_some("image/svg+xml")
    }
}

/// The content type to report: the server's when it is a specific image type, else sniffed,
/// else the server's value, else `application/octet-stream`.
pub fn resolve_content_type(header: Option<&str>, bytes: &[u8]) -> String {
    let header = header
        .map(|h| {
            h.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|h| !h.is_empty());
    match (&header, sniff_content_type(bytes)) {
        (Some(h), _) if h.starts_with("image/") => h.clone(),
        (_, Some(s)) => s.to_string(),
        (Some(h), None) => h.clone(),
        (None, None) => "application/octet-stream".into(),
    }
}

/// The Graph shares id of a SharePoint/OneDrive URL: `u!` and the base64url of the URL without
/// padding.
pub fn graph_share_id(file_url: &str) -> String {
    format!("u!{}", URL_SAFE_NO_PAD.encode(file_url))
}

/// The Graph shares URL of a SharePoint/OneDrive file URL (`/shares/u!<base64url>/driveItem/content`).
pub fn graph_share_content_url(file_url: &str) -> String {
    format!("{}/content", graph_share_item_url(file_url))
}

/// The Graph URL of the drive item behind a SharePoint/OneDrive file URL
/// (`/shares/u!<base64url>/driveItem`).
pub fn graph_share_item_url(file_url: &str) -> String {
    format!(
        "https://graph.microsoft.com/v1.0/shares/{}/driveItem",
        graph_share_id(file_url)
    )
}

/// True for an https URL on a SharePoint or OneDrive host, the files `shares` can resolve.
pub fn is_sharepoint_url(url: &str) -> bool {
    matches!(classify(url), Some(ImageHost::SharePoint(_)))
}

fn too_big(len: usize) -> Error {
    Error::Protocol(format!(
        "image larger than {MAX_FULL_IMAGE_BYTES} bytes ({len})"
    ))
}

/// Downloads one AMS view with the skypetoken (cookie and `X-Skypetoken`, like the web client).
async fn fetch_ams(client: &Client, url: &str) -> Result<FetchedImage> {
    if !is_ams_url(url) {
        return Err(Error::Protocol(
            "refusing to send credentials to a non-AMS image host".into(),
        ));
    }
    client.ensure_fresh().await?;
    let parsed = Url::parse(url).map_err(|e| Error::Protocol(format!("bad url: {e}")))?;
    let tokens = client.tokens().await;
    let mut req = client.http().get(parsed.clone());
    for (k, v) in headers_for(&parsed, &tokens, client.epid()) {
        req = req.header(k, v);
    }
    req = req
        .header(
            "Cookie",
            format!("skypetoken_asm={}", tokens.skype.unwrap_or_default()),
        )
        .header("Accept", "image/*");
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(Error::status("image_download", status, ""));
    }
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if resp
        .content_length()
        .is_some_and(|l| l as usize > MAX_FULL_IMAGE_BYTES)
    {
        return Err(too_big(resp.content_length().unwrap_or(0) as usize));
    }
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_FULL_IMAGE_BYTES {
        return Err(too_big(bytes.len()));
    }
    Ok(FetchedImage {
        content_type: resolve_content_type(ct.as_deref(), &bytes),
        bytes: bytes.to_vec(),
        url: url.to_string(),
    })
}

/// [`fetch_ams`] for an address and, when the connection to it fails, for its Teams-domain twin
/// (see [`crate::ams::asyncgw_twin`]). An answer from the service, even an error, is final.
async fn fetch_ams_routed(client: &Client, url: &str) -> Result<FetchedImage> {
    let mut last = None;
    for (i, attempt) in crate::ams::ams_attempts(url).iter().enumerate() {
        match fetch_ams(client, attempt).await {
            Ok(f) => {
                if i > 0 {
                    crate::ams::ams_remember(attempt);
                }
                return Ok(f);
            }
            Err(e @ Error::Http(_)) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| Error::Protocol("no image URL".into())))
}

async fn fetch_bearer(
    client: &Client,
    aud: &Audience,
    url: &str,
    context: &'static str,
) -> Result<FetchedImage> {
    let r = client
        .request_raw_as(
            aud,
            Method::GET,
            url,
            RawBody::None,
            &[("Accept", "image/*")],
        )
        .await?;
    if !(200..300).contains(&r.status) {
        return Err(Error::status(context, r.status, ""));
    }
    if r.body.len() > MAX_FULL_IMAGE_BYTES {
        return Err(too_big(r.body.len()));
    }
    let ct = r
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    Ok(FetchedImage {
        content_type: resolve_content_type(ct.as_deref(), &r.body),
        bytes: r.body,
        url: url.to_string(),
    })
}

/// A public image: no credential of ours is sent (the generic headers give the user agent only).
async fn fetch_public(client: &Client, url: &str) -> Result<FetchedImage> {
    let parsed = Url::parse(url).map_err(|e| Error::Protocol(format!("bad url: {e}")))?;
    if parsed.scheme() != "https" {
        return Err(Error::Protocol("only https images are fetched".into()));
    }
    let resp = client
        .http()
        .get(parsed)
        .header("User-Agent", crate::client_info::USER_AGENT)
        .header("Accept", "image/*")
        .send()
        .await?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(Error::status("image_download", status, ""));
    }
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_FULL_IMAGE_BYTES {
        return Err(too_big(bytes.len()));
    }
    Ok(FetchedImage {
        content_type: resolve_content_type(ct.as_deref(), &bytes),
        bytes: bytes.to_vec(),
        url: url.to_string(),
    })
}

/// Fetches the full-size picture of `image` with the credential its host needs.
///
/// * AMS: [`ImageRef::candidates`] in order, first success wins.
/// * SharePoint/OneDrive: Graph `shares/u!…/driveItem/content`, then the file URL with a
///   SharePoint bearer for that host.
/// * Anything else: a plain https GET, no credentials.
pub async fn fetch(client: &Client, image: &ImageRef) -> Result<FetchedImage> {
    match &image.host {
        ImageHost::Ams => {
            let mut last = None;
            for url in image.candidates() {
                match fetch_ams_routed(client, &url).await {
                    Ok(f) => return Ok(f),
                    Err(e) => last = Some(e),
                }
            }
            Err(last.unwrap_or_else(|| Error::Protocol("no image URL".into())))
        }
        ImageHost::SharePoint(host) => {
            let via_graph = fetch_bearer(
                client,
                &Audience::Graph,
                &graph_share_content_url(&image.url),
                "image_graph",
            )
            .await;
            match (via_graph, Audience::sharepoint(host)) {
                (Ok(f), _) => Ok(f),
                (Err(e), None) => Err(e),
                (Err(_), Some(aud)) => {
                    fetch_bearer(client, &aud, &image.url, "image_sharepoint").await
                }
            }
        }
        ImageHost::External => fetch_public(client, &image.url).await,
    }
}

// ---------------------------------------------------------------------------------------------
// Grid layout

/// Gap between grid cells, in the unit of the width passed to [`grid_layout`].
pub const GRID_GAP: f32 = 4.0;
/// The grid never gets wider than this.
pub const GRID_MAX_WIDTH: f32 = 420.0;
/// A lone image is at most this tall.
pub const SINGLE_MAX_HEIGHT: f32 = 320.0;
/// Images shown in the grid; further ones fold into a "+K more" on the last cell.
pub const GRID_MAX_CELLS: usize = 4;

/// One cell of the grid. The picture fills the cell (cropped to cover it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridCell {
    /// Index of the image in the message's list.
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// `Some(k)` on the last cell when `k` more images are not shown ("+k more").
    pub overflow: Option<usize>,
}

/// The cells and their bounding box.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub cells: Vec<GridCell>,
    pub width: f32,
    pub height: f32,
}

/// Places `sizes` (pixel or posted size of each image, `(0, 0)` when unknown) in a grid of at
/// most `available_width`. The rules are our choice, as the plan only says Teams uses two
/// columns for several images:
///
/// * 0 images: no cells.
/// * 1 image: keeps its aspect ratio, never scaled above its own size, at most
///   [`GRID_MAX_WIDTH`] by [`SINGLE_MAX_HEIGHT`]; 4:3 when its size is unknown.
/// * 2 images: two square cells side by side.
/// * 3 images: the first spans both columns at 2:1, the other two are squares below it.
/// * 4 images: 2 by 2 squares.
/// * more than 4: the 2 by 2 of four, and the fourth cell has `overflow` = `n - 4` ("+K more").
///   Opening any cell opens the viewer on the whole list.
pub fn grid_layout(sizes: &[(u32, u32)], available_width: f32) -> Grid {
    let n = sizes.len();
    let max_w = if available_width.is_finite() {
        available_width.clamp(0.0, GRID_MAX_WIDTH)
    } else {
        0.0
    };
    let cell = |index, x, y, w, h| GridCell {
        index,
        x,
        y,
        w,
        h,
        overflow: None,
    };
    if n == 0 || max_w <= 0.0 {
        return Grid {
            cells: Vec::new(),
            width: 0.0,
            height: 0.0,
        };
    }
    if n == 1 {
        let (sw, sh) = sizes[0];
        let (nw, nh) = if sw > 0 && sh > 0 {
            (sw as f32, sh as f32)
        } else {
            (4.0, 3.0)
        };
        let known = sw > 0 && sh > 0;
        let mut scale = (max_w / nw).min(SINGLE_MAX_HEIGHT / nh);
        if known {
            scale = scale.min(1.0);
        }
        let (w, h) = (nw * scale, nh * scale);
        return Grid {
            cells: vec![cell(0, 0.0, 0.0, w, h)],
            width: w,
            height: h,
        };
    }
    // Very narrow grids shrink the gap so the cells stay inside.
    let gap = GRID_GAP.min(max_w / 4.0);
    let half = (max_w - gap) / 2.0;
    let mut cells = Vec::new();
    let height = match n {
        2 => {
            cells.push(cell(0, 0.0, 0.0, half, half));
            cells.push(cell(1, half + gap, 0.0, half, half));
            half
        }
        3 => {
            let top = max_w / 2.0;
            cells.push(cell(0, 0.0, 0.0, max_w, top));
            cells.push(cell(1, 0.0, top + gap, half, half));
            cells.push(cell(2, half + gap, top + gap, half, half));
            top + gap + half
        }
        _ => {
            for i in 0..GRID_MAX_CELLS {
                let (col, row) = ((i % 2) as f32, (i / 2) as f32);
                cells.push(cell(i, col * (half + gap), row * (half + gap), half, half));
            }
            if n > GRID_MAX_CELLS {
                cells[GRID_MAX_CELLS - 1].overflow = Some(n - GRID_MAX_CELLS);
            }
            half * 2.0 + gap
        }
    };
    Grid {
        cells,
        width: max_w,
        height,
    }
}

// ---------------------------------------------------------------------------------------------
// Saving

/// The file extension (without dot) for an image content type, `bin` when unknown.
pub fn extension_for(content_type: &str) -> &'static str {
    match content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" | "image/pjpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" | "image/x-ms-bmp" => "bmp",
        "image/svg+xml" => "svg",
        "image/tiff" => "tiff",
        "image/avif" => "avif",
        "image/heic" | "image/heif" => "heic",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        _ => "bin",
    }
}

/// A file name that is safe on Linux, macOS and Windows: no path separators or control
/// characters, no leading dots, no reserved Windows names, at most 100 characters; `image`
/// when nothing is left.
pub fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);
    let mut out: String = cleaned.chars().take(100).collect();
    out = out.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() || out.chars().all(|c| c == '_') {
        return "image".into();
    }
    let stem = out
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    out
}

/// `name` with the extension of `content_type`: kept when it already has it (`jpg` and `jpeg`
/// count as the same), replaced when it is another image extension, appended otherwise.
pub fn with_extension(name: &str, content_type: &str) -> String {
    let want = extension_for(content_type);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            let e = ext.to_ascii_lowercase();
            let same =
                e == want || (want == "jpg" && e == "jpeg") || (want == "tiff" && e == "tif");
            if same {
                name.to_string()
            } else if is_image_extension(&e) {
                format!("{stem}.{want}")
            } else {
                format!("{name}.{want}")
            }
        }
        _ => format!("{name}.{want}"),
    }
}

/// Writes `bytes` to `target` and returns the path written.
///
/// * `target` is a directory (it exists as one, or the text ends with a separator): the file is
///   `<dir>/<safe_filename(name)>`. Otherwise `target` is the file path and `name` is unused.
/// * The extension comes from `content_type` ([`with_extension`]).
/// * An existing file is never overwritten: `photo.png` becomes `photo-1.png`, `photo-2.png`, ...
///   The file is created exclusively, so two writers cannot pick the same name.
/// * Missing parent directories are created.
pub fn save_image(
    target: &Path,
    name: &str,
    content_type: &str,
    bytes: &[u8],
) -> io::Result<PathBuf> {
    let ends_with_sep = target
        .as_os_str()
        .to_string_lossy()
        .ends_with(std::path::is_separator);
    let (dir, file) = if target.is_dir() || ends_with_sep {
        (target.to_path_buf(), safe_filename(name))
    } else {
        let file = target
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .filter(|f| !f.is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name in target"))?;
        let dir = target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        (dir, file)
    };
    let file = with_extension(&file, content_type);
    std::fs::create_dir_all(&dir)?;
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (file.clone(), String::new()),
    };
    for n in 0u32..10_000 {
        let candidate = if n == 0 {
            dir.join(&file)
        } else {
            dir.join(format!("{stem}-{n}{ext}"))
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut f) => {
                f.write_all(bytes)?;
                f.flush()?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free file name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMS: &str = "https://eu-api.asm.skype.com/v1/objects/0-weu-d1-3f9a1b2c4d5e6f708192a3b4c5d6e7f8/views/imgo";

    // ---- parser ----

    #[test]
    fn inline_images_in_document_order_with_sizes() {
        let html = format!(
            r#"<p>one <img itemscope="image" src="{AMS}" alt="image" itemtype="http://schema.skype.com/AMSImage" width="250" height="141" id="x-1"></p><p>two<img src="https://eu-api.asm.skype.com/v1/objects/0-weu-d1-aaaaaaaa11111111/views/imgo" width=300 height='200'></p>"#
        );
        let r = image_refs(&html);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].url, AMS);
        assert_eq!(r[0].host, ImageHost::Ams);
        assert_eq!(
            r[0].object_id.as_deref(),
            Some("0-weu-d1-3f9a1b2c4d5e6f708192a3b4c5d6e7f8")
        );
        assert_eq!(r[0].size(), Some((250, 141)));
        assert_eq!(r[0].name, None, "alt=image says nothing");
        assert_eq!(r[1].size(), Some((300, 200)));
    }

    #[test]
    fn emoji_data_uris_and_relative_sources_are_skipped() {
        let html = r#"<img itemtype="http://schema.skype.com/Emoji" src="https://statics.teams.cdn.office.net/evergreen-assets/skype/v2/cool/50.png"><img src="data:image/png;base64,AAAA"><img src="/rel.png"><img><img src="http://plain.example/a.png"><IMG SRC="https://ok.example/a.png">"#;
        let r = image_refs(html);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://ok.example/a.png");
        assert_eq!(r[0].host, ImageHost::External);
    }

    #[test]
    fn itemid_resolves_when_src_is_missing() {
        let r = image_refs(
            r#"<img itemtype="http://schema.skype.com/AMSImage" itemid="0-weu-d1-3f9a1b2c4d5e6f70" width="10" height="20">"#,
        );
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].url,
            "https://eu-api.asm.skype.com/v1/objects/0-weu-d1-3f9a1b2c4d5e6f70/views/imgo"
        );
        assert_eq!(r[0].host, ImageHost::Ams);
        assert_eq!(r[0].size(), Some((10, 20)));
        // Not an AMS id: ignored.
        assert!(image_refs(r#"<img itemid="cool">"#).is_empty());
    }

    #[test]
    fn object_id_regions() {
        assert!(
            ams_url_for_object("0-wus-d3-0123456789abcdef")
                .unwrap()
                .starts_with("https://us-api.")
        );
        assert!(
            ams_url_for_object("0-neu-d1-0123456789abcdef")
                .unwrap()
                .starts_with("https://eu-api.")
        );
        assert!(
            ams_url_for_object("0-sea-d1-0123456789abcdef")
                .unwrap()
                .starts_with("https://as-api.")
        );
        assert!(
            ams_url_for_object("0-xyz-d1-0123456789abcdef")
                .unwrap()
                .starts_with("https://api.")
        );
        assert!(ams_url_for_object("x-1").is_none());
        assert!(ams_url_for_object("0-weu-d1-zz").is_none());
    }

    #[test]
    fn fractional_sizes_from_the_web_client_are_rounded() {
        let r = image_refs(
            r#"<img src="https://eu-api.asm.skype.com/v1/objects/0-eus-d11-f527822fa5d43a1d/views/imgo" width="383.0313014827018" height="250">"#,
        );
        assert_eq!(r[0].size(), Some((383, 250)));
        assert_eq!(px("1e9"), None);
        assert_eq!(px("."), None);
    }

    #[test]
    fn size_from_style_and_junk_attributes() {
        let r = image_refs(
            r#"<img src="https://a.example/p.png" style="width: 320px; height:240px" width="auto">"#,
        );
        assert_eq!(r[0].size(), Some((320, 240)));
        let r = image_refs(r#"<img src="https://a.example/p.png" width="50%" height="0">"#);
        assert_eq!(r[0].size(), None);
    }

    #[test]
    fn entities_comments_and_names() {
        let html = r#"<!-- <img src="https://hidden.example/x.png"> --><img src="https://a.example/p.png?a=1&amp;b=2" title="Team &quot;photo&quot;" alt="ignored">"#;
        let r = image_refs(html);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://a.example/p.png?a=1&b=2");
        assert_eq!(r[0].name.as_deref(), Some("Team \"photo\""));
        let r = image_refs(r#"<img src="https://a.example/dir/My%20Pic.png" alt="Sunset">"#);
        assert_eq!(r[0].suggested_name(), "Sunset");
        let r = image_refs(r#"<img src="https://a.example/dir/My%20Pic.png">"#);
        assert_eq!(r[0].suggested_name(), "My Pic.png");
    }

    #[test]
    fn broken_html_does_not_panic() {
        let _ = image_refs(r#"<img src="https://a.example/x.png"#);
        let _ = image_refs("<img");
        let _ = image_refs("<img ");
        let _ = image_refs("<!-- <img");
        let _ = image_refs("<p>é<img src='é'></p>é");
    }

    #[test]
    fn attached_image_files_follow_inline_ones() {
        let files = vec![
            FileRef {
                name: "diagram.PNG".into(),
                url: "https://contoso-my.sharepoint.com/:i:/g/personal/x/abc".into(),
                file_type: "png".into(),
            },
            FileRef {
                name: "report.pdf".into(),
                url: "https://contoso-my.sharepoint.com/:b:/g/personal/x/def".into(),
                file_type: "pdf".into(),
            },
        ];
        let r = message_images(&format!(r#"<img src="{AMS}">"#), &files);
        assert_eq!(r.len(), 2);
        assert_eq!(
            r[1].host,
            ImageHost::SharePoint("contoso-my.sharepoint.com".into())
        );
        assert_eq!(r[1].name.as_deref(), Some("diagram.PNG"));
        assert_eq!(r[1].candidates(), vec![r[1].url.clone()]);
    }

    // ---- URL resolution ----

    #[test]
    fn ams_candidates_prefer_fullsize_then_original() {
        let r = &image_refs(&format!(r#"<img src="{AMS}">"#))[0];
        let base =
            "https://eu-api.asm.skype.com/v1/objects/0-weu-d1-3f9a1b2c4d5e6f708192a3b4c5d6e7f8";
        assert_eq!(
            r.candidates(),
            vec![
                format!("{base}/views/imgpsh_fullsize"),
                format!("{base}/views/imgo")
            ]
        );
        // A thumbnail source still resolves to the full-size views.
        let t = &image_refs(&format!(r#"<img src="{base}/views/imgt1">"#))[0];
        assert_eq!(t.candidates(), r.candidates());
        // A global-host URL keeps its host.
        let g = &image_refs(
            r#"<img src="https://api.asm.skype.com/v1/objects/0-x-d1-aaaaaaaa/views/imgo">"#,
        )[0];
        assert!(g.candidates()[0].starts_with(
            "https://api.asm.skype.com/v1/objects/0-x-d1-aaaaaaaa/views/imgpsh_fullsize"
        ));
    }

    #[test]
    fn non_ams_candidates_and_hosts() {
        let r = image_refs(
            r#"<img src="https://contoso.sharepoint.com/sites/a/p.png"><img src="https://cdn.example/p.png">"#,
        );
        assert_eq!(
            r[0].host,
            ImageHost::SharePoint("contoso.sharepoint.com".into())
        );
        assert_eq!(r[1].host, ImageHost::External);
        assert_eq!(
            r[1].candidates(),
            vec!["https://cdn.example/p.png".to_string()]
        );
        // Look-alike hosts never get the AMS credential.
        let r = image_refs(
            r#"<img src="https://evil.com/v1/objects/0-a-d1-aaaaaaaa/views/imgo"><img src="https://asm.skype.com.evil.com/x.png">"#,
        );
        assert!(r.iter().all(|i| i.host == ImageHost::External));
    }

    #[test]
    fn graph_share_url_encoding() {
        // "u!" + base64url without padding, per the Graph shares API.
        assert_eq!(
            graph_share_content_url("https://contoso.sharepoint.com/a b"),
            "https://graph.microsoft.com/v1.0/shares/u!aHR0cHM6Ly9jb250b3NvLnNoYXJlcG9pbnQuY29tL2EgYg/driveItem/content"
        );
    }

    #[test]
    fn graph_share_item_lookup_and_host_check() {
        assert_eq!(
            graph_share_id("https://contoso.sharepoint.com/a b"),
            "u!aHR0cHM6Ly9jb250b3NvLnNoYXJlcG9pbnQuY29tL2EgYg"
        );
        assert_eq!(
            graph_share_item_url("https://contoso.sharepoint.com/a b"),
            "https://graph.microsoft.com/v1.0/shares/u!aHR0cHM6Ly9jb250b3NvLnNoYXJlcG9pbnQuY29tL2EgYg/driveItem"
        );
        assert!(is_sharepoint_url(
            "https://contoso-my.sharepoint.com/:w:/p/x/doc"
        ));
        assert!(!is_sharepoint_url("http://contoso.sharepoint.com/x"));
        assert!(!is_sharepoint_url(
            "https://evil.com/contoso.sharepoint.com"
        ));
        assert!(!is_sharepoint_url("https://example.com/a.docx"));
        assert!(!is_sharepoint_url("not a url"));
    }

    #[test]
    fn content_type_resolution() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert_eq!(sniff_content_type(&png), Some("image/png"));
        assert_eq!(
            sniff_content_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_content_type(b"GIF89a.."), Some("image/gif"));
        assert_eq!(
            sniff_content_type(b"RIFF\0\0\0\0WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(
            sniff_content_type(b"<?xml?><svg xmlns=''/>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff_content_type(b"hello"), None);
        // A specific server type wins; a generic one is replaced by the sniffed one.
        assert_eq!(
            resolve_content_type(Some("image/jpeg; charset=x"), &png),
            "image/jpeg"
        );
        assert_eq!(
            resolve_content_type(Some("application/octet-stream"), &png),
            "image/png"
        );
        assert_eq!(resolve_content_type(None, &png), "image/png");
        assert_eq!(
            resolve_content_type(None, b"??"),
            "application/octet-stream"
        );
        assert_eq!(resolve_content_type(Some("text/html"), b"??"), "text/html");
    }

    // ---- grid ----

    fn assert_inside(g: &Grid) {
        for c in &g.cells {
            assert!(c.x >= 0.0 && c.y >= 0.0 && c.w >= 0.0 && c.h >= 0.0);
            assert!(c.x + c.w <= g.width + 0.001, "{c:?} in {g:?}");
            assert!(c.y + c.h <= g.height + 0.001, "{c:?} in {g:?}");
        }
        for (i, a) in g.cells.iter().enumerate() {
            for b in &g.cells[i + 1..] {
                let apart = a.x + a.w <= b.x + 0.001
                    || b.x + b.w <= a.x + 0.001
                    || a.y + a.h <= b.y + 0.001
                    || b.y + b.h <= a.y + 0.001;
                assert!(apart, "overlap {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn grid_empty_and_degenerate() {
        assert!(grid_layout(&[], 300.0).cells.is_empty());
        assert!(grid_layout(&[(10, 10)], 0.0).cells.is_empty());
        assert!(grid_layout(&[(10, 10)], f32::NAN).cells.is_empty());
        assert!(grid_layout(&[(10, 10)], -5.0).cells.is_empty());
    }

    #[test]
    fn grid_one_image_keeps_aspect_and_is_not_upscaled() {
        // Wide, large: limited by width.
        let g = grid_layout(&[(2000, 1000)], 300.0);
        assert_eq!(g.cells.len(), 1);
        assert!((g.cells[0].w - 300.0).abs() < 0.01 && (g.cells[0].h - 150.0).abs() < 0.01);
        // Tall, large: limited by height.
        let g = grid_layout(&[(1000, 4000)], 400.0);
        assert!((g.cells[0].h - SINGLE_MAX_HEIGHT).abs() < 0.01);
        assert!((g.cells[0].w - 80.0).abs() < 0.01);
        // Small: never scaled above its size.
        let g = grid_layout(&[(100, 50)], 400.0);
        assert!((g.cells[0].w - 100.0).abs() < 0.01 && (g.cells[0].h - 50.0).abs() < 0.01);
        // Unknown size: 4:3 filling the width.
        let g = grid_layout(&[(0, 0)], 300.0);
        assert!((g.cells[0].w - 300.0).abs() < 0.01 && (g.cells[0].h - 225.0).abs() < 0.01);
        // The grid is capped at GRID_MAX_WIDTH.
        let g = grid_layout(&[(5000, 3000)], 2000.0);
        assert!(g.width <= GRID_MAX_WIDTH + 0.01);
        assert_inside(&g);
    }

    #[test]
    fn grid_two_images_side_by_side() {
        let g = grid_layout(&[(1, 1), (9, 3)], 304.0);
        assert_eq!(g.cells.len(), 2);
        assert_eq!((g.cells[0].w, g.cells[0].h), (150.0, 150.0));
        assert_eq!(g.cells[1].x, 154.0);
        assert_eq!(g.cells[1].y, 0.0);
        assert_eq!(g.height, 150.0);
        assert_inside(&g);
    }

    #[test]
    fn grid_three_images_one_wide_two_square() {
        let g = grid_layout(&[(1, 1); 3], 304.0);
        assert_eq!(g.cells.len(), 3);
        assert_eq!((g.cells[0].w, g.cells[0].h), (304.0, 152.0));
        assert_eq!((g.cells[1].x, g.cells[1].y), (0.0, 156.0));
        assert_eq!((g.cells[2].x, g.cells[2].y), (154.0, 156.0));
        assert_eq!(g.height, 156.0 + 150.0);
        assert_inside(&g);
    }

    #[test]
    fn grid_four_images_two_by_two() {
        let g = grid_layout(&[(1, 1); 4], 304.0);
        assert_eq!(g.cells.len(), 4);
        let pos: Vec<_> = g.cells.iter().map(|c| (c.x, c.y)).collect();
        assert_eq!(
            pos,
            vec![(0.0, 0.0), (154.0, 0.0), (0.0, 154.0), (154.0, 154.0)]
        );
        assert!(g.cells.iter().all(|c| c.overflow.is_none()));
        assert_eq!(g.height, 304.0);
        assert_inside(&g);
    }

    #[test]
    fn grid_more_than_four_folds_the_rest_into_plus_k() {
        let g = grid_layout(&[(1, 1); 7], 304.0);
        assert_eq!(g.cells.len(), 4);
        assert_eq!(g.cells[3].index, 3);
        assert_eq!(g.cells[3].overflow, Some(3));
        assert!(g.cells[..3].iter().all(|c| c.overflow.is_none()));
        let g5 = grid_layout(&[(1, 1); 5], 304.0);
        assert_eq!(g5.cells[3].overflow, Some(1));
        assert_inside(&g);
        // Indices stay in order for the viewer.
        assert_eq!(
            g.cells.iter().map(|c| c.index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn grid_narrow_widths_stay_inside() {
        for n in 1..=9 {
            for w in [1.0, 5.0, 40.0, 200.0, 420.0, 900.0] {
                assert_inside(&grid_layout(&vec![(640, 480); n], w));
            }
        }
    }

    // ---- save ----

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ironteams-images-{name}-{}-{}",
            std::process::id(),
            crate::time::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn extensions_from_content_type() {
        assert_eq!(extension_for("image/png"), "png");
        assert_eq!(extension_for("IMAGE/JPEG; q=1"), "jpg");
        assert_eq!(extension_for("image/svg+xml"), "svg");
        assert_eq!(extension_for("text/plain"), "bin");
        assert_eq!(with_extension("photo", "image/png"), "photo.png");
        assert_eq!(with_extension("photo.PNG", "image/png"), "photo.PNG");
        assert_eq!(with_extension("photo.jpeg", "image/jpeg"), "photo.jpeg");
        assert_eq!(with_extension("photo.png", "image/jpeg"), "photo.jpg");
        assert_eq!(with_extension("v1.2 final", "image/gif"), "v1.2 final.gif");
        assert_eq!(with_extension(".hidden", "image/gif"), ".hidden.gif");
    }

    #[test]
    fn filenames_are_safe() {
        assert_eq!(safe_filename("a/b\\c:d*e?.png"), "a_b_c_d_e_.png");
        assert_eq!(safe_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_filename("  ..hidden  "), "hidden");
        assert_eq!(safe_filename(""), "image");
        assert_eq!(safe_filename("///"), "image");
        assert_eq!(safe_filename("CON.png"), "_CON.png");
        assert_eq!(safe_filename("lpt1"), "_lpt1");
        assert_eq!(safe_filename("tab\tname\n"), "tab_name");
        let long = "é".repeat(300);
        assert_eq!(safe_filename(&long).chars().count(), 100);
        assert_eq!(
            safe_filename("Sunset photo (1).jpg"),
            "Sunset photo (1).jpg"
        );
    }

    #[test]
    fn save_into_a_directory_never_overwrites() {
        let dir = tmp("dir");
        std::fs::create_dir_all(&dir).unwrap();
        let p1 = save_image(&dir, "shot", "image/png", b"one").unwrap();
        let p2 = save_image(&dir, "shot", "image/png", b"two").unwrap();
        let p3 = save_image(&dir, "shot.png", "image/png", b"three").unwrap();
        assert_eq!(p1, dir.join("shot.png"));
        assert_eq!(p2, dir.join("shot-1.png"));
        assert_eq!(p3, dir.join("shot-2.png"));
        assert_eq!(std::fs::read(&p1).unwrap(), b"one");
        assert_eq!(std::fs::read(&p2).unwrap(), b"two");
        assert_eq!(std::fs::read(&p3).unwrap(), b"three");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_to_a_file_path_and_a_trailing_separator() {
        let dir = tmp("file");
        // File path without extension: gets one; missing parents are created.
        let p = save_image(&dir.join("sub/out"), "ignored", "image/jpeg", b"j").unwrap();
        assert_eq!(p, dir.join("sub/out.jpg"));
        // Existing file at the same target: suffix.
        let p2 = save_image(&dir.join("sub/out"), "ignored", "image/jpeg", b"k").unwrap();
        assert_eq!(p2, dir.join("sub/out-1.jpg"));
        // A directory that does not exist yet, spelled with a trailing separator; the name is
        // made safe so it cannot climb out of it.
        let target = format!("{}/new/", dir.display());
        let p3 = save_image(Path::new(&target), "../evil/name", "image/gif", b"g").unwrap();
        assert_eq!(p3, dir.join("new/_evil_name.gif"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_reports_io_errors() {
        // A file where a directory is needed.
        let dir = tmp("err");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), b"x").unwrap();
        assert!(save_image(&dir.join("f/child"), "x", "image/png", b"x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
