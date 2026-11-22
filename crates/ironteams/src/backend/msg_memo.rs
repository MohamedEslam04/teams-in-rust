//! A bounded memo of the costly, pure part of building a message row (`Engine::build_msgs`): the
//! JSON of its properties, the HTML parse, linkify, the Markdown body and the voice and video
//! clips split out of it. A chat view is rebuilt on every dirty mark (a new message, a reaction,
//! an avatar or an image that arrived): the rows that did not change come from here.
//!
//! Only what depends on the row's own text is kept. Pictures (avatars, inline images, link
//! thumbnails), names, presence, translations' state, pins and the marks of the view are read
//! again on every build, so their arrival shows without parsing anything.

use crate::data::LoopData;
use crate::markdown::{Rendered, render_blocks};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use store::MessageRow;
use teams_proto::cards::VoiceClip;
use teams_proto::video_clip::VideoClip;

/// Rows kept: several chats' worth of pages.
pub(super) const CAPACITY: usize = 2000;

/// What a row's text parses into.
pub(super) struct Parsed {
    /// The stored properties (`reactions_json`): reactions, files, cards, mentions.
    pub props: Value,
    /// The voice messages split out of the HTML.
    pub voices: Vec<VoiceClip>,
    /// The video clips split out of the HTML.
    pub videos: Vec<VideoClip>,
    /// Every link in the text, typed ones included.
    pub urls: Vec<String>,
    pub joins: Vec<String>,
    pub loops: Vec<LoopData>,
    /// The body as the bubble draws it.
    pub body: Rendered,
}

/// The version of a row as parsed: a hash of everything `parse` reads (the HTML shown, which is
/// the translation while one is shown, and the properties) and of the edit and delete marks.
pub(super) fn version(r: &MessageRow, html: &str) -> u64 {
    let mut h = DefaultHasher::new();
    html.hash(&mut h);
    r.reactions_json.hash(&mut h);
    r.edited_ms.hash(&mut h);
    r.deleted.hash(&mut h);
    h.finish()
}

/// Parses a row's text: `html` (the shown HTML) and `props_json` (its properties).
pub(super) fn parse(html: &str, props_json: &str) -> Parsed {
    let props: Value = serde_json::from_str(props_json).unwrap_or_else(|_| json!({}));
    // Voice messages are drawn as players: their "▶ Play" link leaves the text.
    let split = teams_proto::cards::split_voice(html);
    let (html, voices) = match split {
        Some((rest, clips)) => (rest, clips),
        None => (html.to_string(), Vec::new()),
    };
    // Video clips are drawn as clip cards: their tags and "▶ Play" links leave the text.
    let (html, videos) = match teams_proto::video_clip::split_video(&html) {
        Some((rest, clips)) => (rest, clips),
        None => (html, Vec::new()),
    };
    let mut rich = render::parse_teams_html(&html);
    // A URL typed or pasted as plain text is a link too.
    render::linkify(&mut rich);
    let urls = render::links(&rich);
    let joins: Vec<String> = urls
        .iter()
        .filter(|u| render::is_meeting_link(u))
        .cloned()
        .collect();
    let loops: Vec<LoopData> = urls
        .iter()
        .filter(|u| render::is_loop_link(u))
        .map(|u| LoopData {
            url: u.clone(),
            title: render::loop_title(u),
        })
        .collect();
    let mut body = render_blocks(&rich);
    // Headings, tables and colors (the plan's 5.4): the richer model draws the body.
    crate::markdown::upgrade_rich(&mut body, &html);
    Parsed {
        props,
        voices,
        videos,
        urls,
        joins,
        loops,
        body,
    }
}

/// The parsed rows by message id, each with the version it was parsed at. When full, the row
/// parsed first is dropped.
pub(super) struct MsgMemo {
    rows: HashMap<String, (u64, Arc<Parsed>)>,
    order: VecDeque<String>,
    capacity: usize,
}

impl Default for MsgMemo {
    fn default() -> Self {
        Self::with_capacity(CAPACITY)
    }
}

impl MsgMemo {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            rows: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    /// The row `id` as parsed at `version`; `None` when it was not, or at another version.
    pub(super) fn get(&self, id: &str, version: u64) -> Option<Arc<Parsed>> {
        self.rows
            .get(id)
            .filter(|(v, _)| *v == version)
            .map(|(_, p)| p.clone())
    }

    /// Keeps `parsed` as the row `id` at `version`, in place of an older version.
    pub(super) fn insert(&mut self, id: &str, version: u64, parsed: Arc<Parsed>) {
        if let Some(slot) = self.rows.get_mut(id) {
            *slot = (version, parsed);
            return;
        }
        self.rows.insert(id.to_string(), (version, parsed));
        self.order.push_back(id.to_string());
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.rows.remove(&old);
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, html: &str) -> MessageRow {
        MessageRow {
            id: id.into(),
            conversation_id: "19:a@thread.v2".into(),
            parent_id: None,
            sender_mri: "8:orgid:x".into(),
            sender_name: "X".into(),
            compose_ms: 1,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            content_html: html.into(),
            client_message_id: None,
            reactions_json: "{}".into(),
        }
    }

    /// The memo's view of a build: the kept parse when the version matches, else a new one.
    fn lookup(memo: &mut MsgMemo, r: &MessageRow, parses: &mut usize) -> Arc<Parsed> {
        let v = version(r, &r.content_html);
        if let Some(p) = memo.get(&r.id, v) {
            return p;
        }
        *parses += 1;
        let p = Arc::new(parse(&r.content_html, &r.reactions_json));
        memo.insert(&r.id, v, p.clone());
        p
    }

    #[test]
    fn an_unchanged_row_is_parsed_once() {
        let mut memo = MsgMemo::default();
        let mut parses = 0;
        let r = row("1", "<p>hello <b>there</b></p>");
        let a = lookup(&mut memo, &r, &mut parses);
        let b = lookup(&mut memo, &r.clone(), &mut parses);
        assert_eq!(parses, 1);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.body.plain, "hello there");
    }

    #[test]
    fn an_edit_in_place_is_parsed_again() {
        let mut memo = MsgMemo::default();
        let mut parses = 0;
        let mut r = row("1", "<p>before</p>");
        lookup(&mut memo, &r, &mut parses);
        r.content_html = "<p>after https://example.com</p>".into();
        r.edited_ms = Some(2);
        let p = lookup(&mut memo, &r, &mut parses);
        assert_eq!(parses, 2);
        assert_eq!(p.body.plain, "after https://example.com");
        assert_eq!(p.urls, ["https://example.com"]);
        // The new version replaced the old one.
        assert_eq!(memo.len(), 1);
    }

    #[test]
    fn a_reaction_is_parsed_again() {
        let mut memo = MsgMemo::default();
        let mut parses = 0;
        let mut r = row("1", "<p>hi</p>");
        assert!(lookup(&mut memo, &r, &mut parses).props.get("r").is_none());
        r.reactions_json = r#"{"r":[{"key":"like","users":["8:orgid:y"]}]}"#.into();
        let p = lookup(&mut memo, &r, &mut parses);
        assert_eq!(parses, 2);
        assert_eq!(p.props["r"][0]["key"], "like");
    }

    #[test]
    fn a_delete_is_parsed_again() {
        let mut memo = MsgMemo::default();
        let mut parses = 0;
        let mut r = row("1", "<p>gone soon</p>");
        lookup(&mut memo, &r, &mut parses);
        // A delete that kept the text still counts as a new version.
        r.deleted = true;
        lookup(&mut memo, &r, &mut parses);
        assert_eq!(parses, 2);
        r.content_html.clear();
        let p = lookup(&mut memo, &r, &mut parses);
        assert_eq!(parses, 3);
        assert_eq!(p.body.plain, "");
    }

    #[test]
    fn a_shown_translation_is_a_version_of_its_own() {
        let r = row("1", "<p>hola</p>");
        assert_ne!(version(&r, &r.content_html), version(&r, "<p>hello</p>"));
        assert_eq!(
            version(&r, "<p>hello</p>"),
            version(&r.clone(), "<p>hello</p>")
        );
    }

    #[test]
    fn the_row_parsed_first_leaves_when_full() {
        let mut memo = MsgMemo::with_capacity(2);
        let mut parses = 0;
        let (a, b, c) = (row("a", "a"), row("b", "b"), row("c", "c"));
        lookup(&mut memo, &a, &mut parses);
        lookup(&mut memo, &b, &mut parses);
        // A new version of `a` keeps its place: it is still the oldest.
        let mut a2 = a.clone();
        a2.content_html = "a2".into();
        lookup(&mut memo, &a2, &mut parses);
        lookup(&mut memo, &c, &mut parses);
        assert_eq!(memo.len(), 2);
        assert!(memo.get("a", version(&a2, "a2")).is_none());
        assert!(memo.get("b", version(&b, "b")).is_some());
        assert!(memo.get("c", version(&c, "c")).is_some());
    }

    #[test]
    fn voice_and_video_parts_leave_the_text() {
        let html = r#"<p>clip</p><video src="https://x.example/v.mp4"></video>"#;
        let direct = parse(html, "{}");
        // Whatever the splitters take out, the memo keeps the same as a fresh parse would give.
        let mut memo = MsgMemo::default();
        let mut parses = 0;
        let r = row("1", html);
        let kept = lookup(&mut memo, &r, &mut parses);
        assert_eq!(kept.body, direct.body);
        assert_eq!(kept.videos, direct.videos);
        assert_eq!(kept.voices, direct.voices);
    }
}
