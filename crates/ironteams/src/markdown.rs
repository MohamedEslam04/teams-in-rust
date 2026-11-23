//! Converts the `render` rich-text model into the Markdown dialect Slint's `StyledText` parses,
//! and pulls out the parts the bubble draws separately (reply quote, images).

use render::{Block, Dir, RichBlock, RichDoc, RichList, RichTable, RichText, Span};
use std::sync::atomic::{AtomicU32, Ordering};

/// A run of a message body: text drawn as `StyledText`, or a code block drawn as a box of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Segment {
    /// Markdown for `StyledText`.
    Text(String),
    /// The code as written, without the trailing line break.
    Code(String),
}

/// What a message bubble needs to draw its content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendered {
    pub quote_author: String,
    pub quote_text: String,
    /// The id of the message the reply quote points to (`""`: none, or not a reply).
    pub quote_id: String,
    /// The whole body as Markdown; a code block is flattened to inline code here.
    pub markdown: String,
    /// The body as text and code blocks, in order. Empty when it has no code block (then
    /// `markdown` is the whole body).
    pub segments: Vec<Segment>,
    pub plain: String,
    pub rtl: bool,
    pub images: Vec<(String, Option<u32>, Option<u32>)>,
}

/// Escapes Markdown control characters in plain text.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '`' | '[' | ']' | '(' | ')' | '#' | '<' | '>' | '~' | '|' | '!'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn link_target(url: &str) -> String {
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

/// `0xrrggbb` of the mention colour (`Theme.mention`), or [`NO_COLOR`] until the look is applied.
static MENTION_COLOR: AtomicU32 = AtomicU32::new(NO_COLOR);
const NO_COLOR: u32 = u32::MAX;

/// Sets the colour mentions are drawn in (the theme's `mention`). `StyledText` has no
/// background, so a mention is bold text in this colour.
pub fn set_mention_color(rgb: u32) {
    MENTION_COLOR.store(rgb & 0xff_ffff, Ordering::Relaxed);
}

/// The colour mentions are drawn in, `None` before [`set_mention_color`].
pub fn mention_color() -> Option<u32> {
    let c = MENTION_COLOR.load(Ordering::Relaxed);
    (c != NO_COLOR).then_some(c)
}

fn spans_md(spans: &[Span]) -> String {
    spans_md_in(spans, mention_color())
}

/// `spans` as Markdown, mentions in the `mention` colour (`0xrrggbb`) when there is one.
fn spans_md_in(spans: &[Span], mention: Option<u32>) -> String {
    let mut out = String::new();
    for s in spans {
        if s.text.is_empty() {
            continue;
        }
        // Keep line breaks as hard breaks; style each line separately so markers never span lines.
        let lines: Vec<&str> = s.text.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                out.push_str("\\\n");
            }
            if line.trim().is_empty() {
                out.push_str(line);
                continue;
            }
            let lead = line.len() - line.trim_start().len();
            let trail = line.len() - line.trim_end().len();
            let core = line.trim();
            let mut t = if s.style.code {
                format!("`{}`", core.replace('`', "'"))
            } else {
                escape(core)
            };
            if s.mention.is_some() || s.style.bold {
                t = format!("**{t}**");
            }
            if s.style.italic {
                t = format!("*{t}*");
            }
            if s.style.strike {
                t = format!("~~{t}~~");
            }
            if s.style.underline {
                t = format!("<u>{t}</u>");
            }
            // A highlight has no background in `StyledText`: it shows as a text color instead.
            // A mention without a color of its own takes the theme's mention color.
            let tint = s
                .style
                .color
                .clone()
                .or_else(|| s.style.highlight.clone())
                .or_else(|| {
                    mention
                        .filter(|_| s.mention.is_some())
                        .map(|c| format!("#{c:06x}"))
                });
            if let Some(c) = tint {
                t = format!("<font color=\"{c}\">{t}</font>");
            }
            if let Some(url) = &s.link {
                t = format!("[{t}]({})", link_target(url));
            }
            out.push_str(&line[..lead]);
            out.push_str(&t);
            out.push_str(&line[line.len() - trail..]);
        }
    }
    out
}

fn spans_plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// One block of a body: its Markdown, and for a code block also its code.
struct Piece {
    md: String,
    code: Option<String>,
}

impl Piece {
    fn text(md: String) -> Self {
        Piece { md, code: None }
    }

    /// A code block. `StyledText` has no blocks, so its Markdown is one inline-code run per line
    /// (what [`Rendered::markdown`] keeps); the bubble draws `code` in a box instead.
    fn code(text: &str) -> Self {
        let md = text
            .lines()
            .map(|l| format!("`{}`", l.replace('`', "'")))
            .collect::<Vec<_>>()
            .join("\\\n");
        let code = text.trim_end_matches(['\n', '\r']);
        Piece {
            md,
            code: (!code.trim().is_empty()).then(|| code.to_string()),
        }
    }
}

/// Sets `r.markdown` and `r.segments` from the blocks of a body.
fn set_body(r: &mut Rendered, pieces: Vec<Piece>) {
    r.markdown = pieces
        .iter()
        .map(|p| p.md.as_str())
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    r.segments = Vec::new();
    if pieces.iter().all(|p| p.code.is_none()) {
        return;
    }
    let mut text: Vec<&str> = Vec::new();
    for p in &pieces {
        match &p.code {
            Some(code) => {
                flush_text(&mut text, &mut r.segments);
                r.segments.push(Segment::Code(code.clone()));
            }
            None if !p.md.trim().is_empty() => text.push(&p.md),
            None => {}
        }
    }
    flush_text(&mut text, &mut r.segments);
}

/// Closes the run of text blocks collected so far as one segment.
fn flush_text(text: &mut Vec<&str>, out: &mut Vec<Segment>) {
    if !text.is_empty() {
        out.push(Segment::Text(text.join("\n\n")));
        text.clear();
    }
}

/// Converts a parsed message into bubble parts.
pub fn render_blocks(rt: &RichText) -> Rendered {
    let mut r = Rendered::default();
    let mut md: Vec<Piece> = vec![];
    let mut plain: Vec<String> = vec![];
    let mut first_dir: Option<Dir> = None;
    for b in &rt.blocks {
        match b {
            Block::Quote {
                message_id,
                author,
                preview,
                ..
            } if r.quote_text.is_empty() && md.is_empty() => {
                r.quote_author = author.clone().unwrap_or_default();
                r.quote_text = preview.clone();
                r.quote_id = message_id.clone().unwrap_or_default();
            }
            Block::Quote { preview, .. } => {
                md.push(Piece::text(format!("> {}", escape(preview))));
                plain.push(preview.clone());
            }
            Block::Paragraph { dir, spans } => {
                first_dir.get_or_insert(*dir);
                md.push(Piece::text(spans_md(spans)));
                plain.push(spans_plain(spans));
            }
            Block::List { ordered, items } => {
                let mut lines = vec![];
                for (i, it) in items.iter().enumerate() {
                    let bullet = if *ordered {
                        format!("{}.", i + 1)
                    } else {
                        "-".to_string()
                    };
                    lines.push(format!("{bullet} {}", spans_md(it)));
                    plain.push(spans_plain(it));
                }
                if let Some(firstitem) = items.first() {
                    first_dir.get_or_insert(render::base_direction(&spans_plain(firstitem)));
                }
                md.push(Piece::text(lines.join("\n")));
            }
            Block::Code { text } => {
                md.push(Piece::code(text));
                plain.push(text.clone());
            }
            Block::Image { url, width, height } => r.images.push((url.clone(), *width, *height)),
        }
    }
    set_body(&mut r, md);
    r.plain = plain.join("\n").trim().to_string();
    r.rtl = first_dir == Some(Dir::Rtl);
    r
}

/// True when `doc` has something the plain model of [`render_blocks`] cannot show: headings,
/// tables, rules, colors, highlights, underline or a code language.
fn needs_rich(doc: &RichDoc) -> bool {
    let styled = |spans: &[Span]| {
        spans
            .iter()
            .any(|s| s.style.color.is_some() || s.style.highlight.is_some() || s.style.underline)
    };
    let list_styled = |l: &RichList| -> bool {
        fn walk(l: &RichList, styled: &dyn Fn(&[Span]) -> bool) -> bool {
            l.items
                .iter()
                .any(|i| styled(&i.spans) || i.children.iter().any(|c| walk(c, styled)))
        }
        walk(l, &styled)
    };
    doc.blocks.iter().any(|b| match b {
        RichBlock::Heading { .. } | RichBlock::Table(_) | RichBlock::Rule => true,
        RichBlock::Paragraph(spans) | RichBlock::Quote(spans) => styled(spans),
        RichBlock::List(l) => list_styled(l),
        RichBlock::Code { .. } | RichBlock::Image { .. } => false,
    })
}

fn list_md(list: &RichList, depth: usize, out: &mut Vec<String>) {
    for (i, item) in list.items.iter().enumerate() {
        let bullet = if list.ordered {
            format!("{}.", list.start.unwrap_or(1) as usize + i)
        } else {
            "-".to_string()
        };
        out.push(format!(
            "{}{bullet} {}",
            "  ".repeat(depth),
            spans_md(&item.spans)
        ));
        for child in &item.children {
            list_md(child, depth + 1, out);
        }
    }
}

/// A table as aligned monospace rows (`StyledText` has no tables): cells padded with no-break
/// spaces, `│` between columns and a rule under the header.
fn table_md(table: &RichTable) -> String {
    let rows: Vec<Vec<String>> = table
        .rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| {
                    spans_plain(&c.spans)
                        .replace('\n', " ")
                        .replace('`', "'")
                        .trim()
                        .to_string()
                })
                .collect()
        })
        .collect();
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|t| t.chars().count())
                .max()
                .unwrap_or(1)
                .max(1)
        })
        .collect();
    let line = |cells: &[String]| {
        let parts: Vec<String> = (0..cols)
            .map(|c| {
                let text = cells.get(c).map_or("", String::as_str);
                let pad = widths[c] - text.chars().count();
                format!("{text}{}", "\u{a0}".repeat(pad))
            })
            .collect();
        format!("`{}`", parts.join("\u{a0}│\u{a0}"))
    };
    let mut lines: Vec<String> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        lines.push(line(row));
        if i == 0
            && table
                .rows
                .first()
                .is_some_and(|r| r.iter().any(|c| c.header))
        {
            let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            lines.push(format!("`{}`", rule.join("─┼─")));
        }
    }
    lines.join("\\\n")
}

/// Replaces the markdown of a message that has headings, tables, colors or underline with one
/// built from the richer [`RichDoc`] (a heading becomes a bold line, a table aligned monospace
/// rows, a color a `<font>` span). `r.markdown` is left alone for a plain message.
pub fn upgrade_rich(r: &mut Rendered, html: &str) {
    let mut doc = render::parse_rich_html(html);
    if !needs_rich(&doc) {
        return;
    }
    render::linkify_doc(&mut doc);
    let mut md: Vec<Piece> = Vec::new();
    for (i, b) in doc.blocks.iter().enumerate() {
        match b {
            // The reply quote is drawn above the bubble.
            RichBlock::Quote(_) if i == 0 && !r.quote_text.is_empty() => {}
            RichBlock::Paragraph(spans) => md.push(Piece::text(spans_md(spans))),
            RichBlock::Heading { spans, .. } => {
                let bold: Vec<Span> = spans
                    .iter()
                    .map(|s| {
                        let mut s = s.clone();
                        s.style.bold = true;
                        s
                    })
                    .collect();
                md.push(Piece::text(spans_md(&bold)));
            }
            RichBlock::Quote(spans) => md.push(Piece::text(
                spans_md(spans)
                    .split('\n')
                    .map(|l| format!("> {l}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )),
            RichBlock::List(l) => {
                let mut lines = Vec::new();
                list_md(l, 0, &mut lines);
                md.push(Piece::text(lines.join("\n")));
            }
            RichBlock::Code { text, .. } => md.push(Piece::code(text)),
            RichBlock::Table(t) => md.push(Piece::text(table_md(t))),
            RichBlock::Rule => md.push(Piece::text("──────────".into())),
            RichBlock::Image { .. } => {}
        }
    }
    set_body(r, md);
}

/// Emoji for a Teams reaction key (`like`, `heart`, or a codepoint key like `1f525_fire`).
pub fn reaction_emoji(key: &str) -> String {
    match key {
        "like" => "👍".into(),
        "heart" => "❤️".into(),
        "laugh" => "😆".into(),
        "surprised" => "😮".into(),
        "sad" => "😢".into(),
        "angry" => "😡".into(),
        _ => {
            let code = key.split('_').next().unwrap_or_default();
            let parsed: Option<String> = code
                .split('-')
                .map(|h| u32::from_str_radix(h, 16).ok().and_then(char::from_u32))
                .collect();
            parsed
                .filter(|s| !s.is_empty() && code.len() >= 4)
                .or_else(|| named_reaction(key))
                .unwrap_or_else(|| key.to_string())
        }
    }
}

/// Teams' named reactions from the Skype emoticon set (`loudlycrying`, `cwl`, `yes`, …).
const NAMED: &[(&str, &str)] = &[
    ("smile", "🙂"),
    ("happy", "😊"),
    ("bigsmile", "😃"),
    ("grinning", "😁"),
    ("cwl", "😂"),
    ("rofl", "🤣"),
    ("wink", "😉"),
    ("cool", "😎"),
    ("tongueout", "😛"),
    ("blush", "😊"),
    ("inlove", "😍"),
    ("kiss", "😘"),
    ("cry", "😢"),
    ("loudlycrying", "😭"),
    ("sweat", "😓"),
    ("speechless", "😐"),
    ("wonder", "🤔"),
    ("think", "🤔"),
    ("sleepy", "😪"),
    ("yawn", "🥱"),
    ("dull", "😑"),
    ("worry", "😟"),
    ("puke", "🤮"),
    ("devil", "😈"),
    ("angel", "😇"),
    ("nerd", "🤓"),
    ("lipssealed", "🤐"),
    ("party", "🥳"),
    ("facepalm", "🤦"),
    ("shock", "😱"),
    ("rage", "😡"),
    ("smirk", "😏"),
    ("eg", "😈"),
    ("wasntme", "🙄"),
    ("hug", "🤗"),
    ("yes", "👍"),
    ("no", "👎"),
    ("ok", "👌"),
    ("clap", "👏"),
    ("fire", "🔥"),
    ("star", "⭐"),
    ("brokenheart", "💔"),
    ("muscle", "💪"),
    ("punch", "👊"),
    ("handshake", "🤝"),
    ("praying", "🙏"),
    ("victory", "✌️"),
    ("hi", "👋"),
    ("wave", "👋"),
    ("rock", "🤘"),
    ("cake", "🎂"),
    ("coffee", "☕"),
    ("beer", "🍺"),
    ("100", "💯"),
    ("tada", "🎉"),
    ("eyes", "👀"),
    ("rocket", "🚀"),
];

/// A named reaction: the Teams table, then the emoji with that short name ("thumbs_up"), then
/// the emoji whose name starts with it once spaces are dropped (`loudlycrying` is "loudly crying
/// face").
fn named_reaction(key: &str) -> Option<String> {
    let key = key.to_lowercase();
    if let Some((_, e)) = NAMED.iter().find(|(k, _)| *k == key) {
        return Some((*e).to_string());
    }
    if let Some(e) = emoji::by_shortcode(&key) {
        return Some(e.ch.to_string());
    }
    if key.len() < 4 {
        return None;
    }
    let squash = |n: &str| -> String { n.chars().filter(|c| c.is_alphanumeric()).collect() };
    let all = emoji::all();
    all.iter()
        .find(|e| squash(e.name) == key || squash(e.name) == format!("{key}face"))
        .or_else(|| all.iter().find(|e| squash(e.name).starts_with(&key)))
        .map(|e| e.ch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(html: &str) -> Rendered {
        render_blocks(&render::parse_teams_html(html))
    }

    #[test]
    fn a_plain_text_url_becomes_a_markdown_link() {
        let mut rich = render::parse_teams_html("<p>see https://a.example/x, ok</p>");
        render::linkify(&mut rich);
        let x = render_blocks(&rich);
        assert_eq!(
            x.markdown,
            "see [https://a.example/x](https://a.example/x), ok"
        );
        assert_eq!(x.plain, "see https://a.example/x, ok");
    }

    #[test]
    fn a_plain_text_url_in_a_rich_message_becomes_a_markdown_link() {
        let html = "<h1>Notes</h1><p>https://a.example/x</p>";
        let mut x = render_blocks(&render::parse_teams_html(html));
        upgrade_rich(&mut x, html);
        assert!(
            x.markdown
                .contains("[https://a.example/x](https://a.example/x)"),
            "{}",
            x.markdown
        );
    }

    #[test]
    fn named_teams_reactions_become_emoji() {
        assert_eq!(reaction_emoji("loudlycrying"), "😭");
        assert_eq!(reaction_emoji("cwl"), "😂");
        assert_eq!(reaction_emoji("yes"), "👍");
        // Not in the table: found by the emoji's name.
        assert_eq!(reaction_emoji("pensive"), "😔");
        assert_eq!(reaction_emoji("1f525_fire"), "🔥");
        // Unknown names stay as they are.
        assert_eq!(reaction_emoji("zzqq"), "zzqq");
    }

    #[test]
    fn styles_become_markdown() {
        let x = r("<p>Hello <b>bold</b> and <i>it</i> <a href=\"https://x.com/a b\">link</a></p>");
        assert_eq!(
            x.markdown,
            "Hello **bold** and *it* [link](https://x.com/a%20b)"
        );
        assert_eq!(x.plain, "Hello bold and it link");
        assert!(!x.rtl);
    }

    #[test]
    fn escapes_markdown_chars() {
        let x = r("<p>a*b_c [x](y) # not a heading</p>");
        assert_eq!(x.markdown, "a\\*b\\_c \\[x\\]\\(y\\) \\# not a heading");
    }

    #[test]
    fn arabic_is_rtl() {
        let x = r("<p>تمام 👍 هراجع الـ PR</p>");
        assert!(x.rtl);
        assert_eq!(x.markdown, "تمام 👍 هراجع الـ PR");
    }

    #[test]
    fn reply_quote_is_extracted() {
        let html = render::reply_html("1", "8:orgid:a", "Sara", 1, "original text", "my reply");
        let x = r(&html);
        assert_eq!(x.quote_author, "Sara");
        assert_eq!(x.quote_text, "original text");
        assert_eq!(x.quote_id, "1", "the quoted message, for the jump to it");
        assert_eq!(x.markdown, "my reply");
    }

    #[test]
    fn mentions_are_bold_and_lists_kept() {
        let x = r(
            r#"<p>hi <span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">Ahmed</span></p><ul><li>one</li><li>two</li></ul>"#,
        );
        // The mention color is process-wide (the look sets it, other tests may too): only the
        // bold is checked here, the color is covered by `spans_md_in` below.
        let md = &x.markdown;
        assert!(md.starts_with("hi "), "{md:?}");
        assert!(md.contains("**Ahmed**"), "{md:?}");
        assert!(md.ends_with("\n\n- one\n- two"), "{md:?}");
    }

    #[test]
    fn images_are_separate() {
        let x = r(
            r#"<p><img itemtype="http://schema.skype.com/AMSImage" src="https://eu-api.asm.skype.com/v1/objects/0-a/views/imgo" width="250" height="141"></p>"#,
        );
        assert_eq!(
            x.images,
            vec![(
                "https://eu-api.asm.skype.com/v1/objects/0-a/views/imgo".into(),
                Some(250),
                Some(141)
            )]
        );
        assert_eq!(x.markdown, "");
    }

    #[test]
    fn line_breaks_are_hard_breaks() {
        assert_eq!(r("<p>a<br>b</p>").markdown, "a\\\nb");
    }

    fn rich(html: &str) -> Rendered {
        let mut x = r(html);
        upgrade_rich(&mut x, html);
        x
    }

    /// Slint must accept what we hand it, or the bubble shows the raw markers.
    fn parses(md: &str) {
        slint::StyledText::from_markdown(md).unwrap_or_else(|e| panic!("{md:?}: {e}"));
    }

    #[test]
    fn plain_messages_keep_the_plain_markdown() {
        let html = "<p>Hello <b>bold</b></p><ul><li>a</li></ul>";
        assert_eq!(rich(html).markdown, r(html).markdown);
    }

    #[test]
    fn headings_show_as_bold_lines() {
        let x = rich("<h1>Release notes</h1><p>text</p><h3>Fixes</h3>");
        assert_eq!(x.markdown, "**Release notes**\n\ntext\n\n**Fixes**");
        parses(&x.markdown);
    }

    #[test]
    fn colors_and_underline_become_font_and_u_tags() {
        let x = rich(
            r##"<p>a <span style="color:#ff0000">red</span> <u>under</u> <span style="background-color:#fdd472">hl</span></p>"##,
        );
        assert_eq!(
            x.markdown,
            "a <font color=\"#ff0000\">red</font> <u>under</u> <font color=\"#fdd472\">hl</font>"
        );
        parses(&x.markdown);
    }

    #[test]
    fn a_color_nests_inside_bold_and_links() {
        let x = rich(
            r##"<p><a href="https://x.io"><b><span style="color:#00ff00">go</span></b></a></p>"##,
        );
        parses(&x.markdown);
        assert!(
            x.markdown.contains("<font color=\"#00ff00\">**go**</font>"),
            "{:?}",
            x.markdown
        );
    }

    #[test]
    fn a_table_becomes_aligned_rows_under_a_header_rule() {
        let x = rich(
            "<table><tbody><tr><th>Name</th><th>Qty</th></tr><tr><td>Tea</td><td>12</td></tr></tbody></table>",
        );
        parses(&x.markdown);
        let lines: Vec<&str> = x.markdown.split("\\\n").collect();
        assert_eq!(lines.len(), 3, "{:?}", x.markdown);
        assert!(lines[0].contains("Name") && lines[0].contains('│'));
        assert!(lines[1].contains('─'));
        assert!(lines[2].contains("Tea"));
        // Same width: the columns line up.
        assert_eq!(lines[0].chars().count(), lines[2].chars().count());
    }

    #[test]
    fn a_reply_quote_is_not_drawn_twice_and_images_stay_separate() {
        let html = format!(
            "{}<h2>Title</h2>",
            render::reply_quote_html("1", "8:orgid:a", "Sara", 1, "original")
        );
        let x = rich(&html);
        assert_eq!(x.quote_text, "original");
        assert_eq!(x.markdown, "**Title**");
    }

    #[test]
    fn nested_lists_and_rules_parse() {
        let x = rich("<h2>x</h2><ul><li>a<ul><li>b</li></ul></li></ul><hr>");
        parses(&x.markdown);
        assert!(x.markdown.contains("- a\n  - b"), "{:?}", x.markdown);
    }

    #[test]
    fn a_code_block_is_its_own_segment_between_runs_of_text() {
        let x = r(
            "<p>before</p><p>more</p><pre><code>let a = 1;\n    let b = 2;\n</code></pre><p>after</p>",
        );
        assert_eq!(
            x.segments,
            vec![
                Segment::Text("before\n\nmore".into()),
                Segment::Code("let a = 1;\n    let b = 2;".into()),
                Segment::Text("after".into()),
            ]
        );
        // The flat markdown stays: one inline-code run per line.
        assert!(x.markdown.contains("`let a = 1;`"), "{:?}", x.markdown);
        assert!(x.plain.contains("let b = 2;"));
    }

    #[test]
    fn a_message_without_code_has_no_segments() {
        assert!(
            r("<p>hello <b>there</b></p><ul><li>a</li></ul>")
                .segments
                .is_empty()
        );
    }

    #[test]
    fn a_message_that_is_only_code_is_one_code_segment() {
        let x = r("<pre>fn main() {}</pre>");
        assert_eq!(x.segments, vec![Segment::Code("fn main() {}".into())]);
    }

    #[test]
    fn an_empty_code_block_adds_no_segment() {
        let x = r("<p>text</p><pre>\n</pre>");
        assert!(x.segments.is_empty(), "{:?}", x.segments);
    }

    #[test]
    fn a_code_block_in_a_rich_message_is_a_segment_too() {
        let x = rich("<h1>Title</h1><pre><code>x = 1</code></pre><p>end</p>");
        assert_eq!(
            x.segments,
            vec![
                Segment::Text("**Title**".into()),
                Segment::Code("x = 1".into()),
                Segment::Text("end".into()),
            ]
        );
    }

    #[test]
    fn each_segment_of_text_is_markdown_slint_accepts() {
        let x = r("<p>a <b>b</b></p><pre>1\n2</pre><ul><li>x</li></ul>");
        for s in &x.segments {
            if let Segment::Text(md) = s {
                parses(md);
            }
        }
        assert_eq!(x.segments.len(), 3);
    }

    fn mention_span(text: &str) -> Span {
        let mut s = render::parse_teams_html(
            r#"<p><span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">x</span></p>"#,
        );
        let Some(Block::Paragraph { spans, .. }) = s.blocks.pop() else {
            panic!("a paragraph")
        };
        let mut span = spans.into_iter().next().expect("the mention span");
        span.text = text.into();
        span
    }

    #[test]
    fn a_mention_takes_the_mention_color_and_stays_bold() {
        let md = spans_md_in(&[mention_span("Ahmed")], Some(0xb4b8ff));
        assert_eq!(md, "<font color=\"#b4b8ff\">**Ahmed**</font>");
        parses(&md);
    }

    #[test]
    fn a_mention_is_only_bold_without_a_mention_color() {
        assert_eq!(spans_md_in(&[mention_span("Ahmed")], None), "**Ahmed**");
    }

    #[test]
    fn a_color_set_in_the_message_wins_over_the_mention_color() {
        let mut s = mention_span("Ahmed");
        s.style.color = Some("#ff0000".into());
        let md = spans_md_in(&[s], Some(0xb4b8ff));
        assert_eq!(md, "<font color=\"#ff0000\">**Ahmed**</font>");
    }

    #[test]
    fn plain_text_never_takes_the_mention_color() {
        let plain = render::parse_teams_html("<p>hello</p>");
        let Some(Block::Paragraph { spans, .. }) = plain.blocks.first() else {
            panic!("a paragraph")
        };
        assert_eq!(spans_md_in(spans, Some(0xb4b8ff)), "hello");
    }

    #[test]
    fn reaction_keys() {
        assert_eq!(reaction_emoji("like"), "👍");
        assert_eq!(reaction_emoji("1f525_fire"), "🔥");
        assert_eq!(reaction_emoji("weird"), "weird");
    }
}
