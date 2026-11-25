//! Converts the `render` rich-text model into the Markdown dialect Slint's `StyledText` parses,
//! and pulls out the parts the bubble draws separately (reply quote, images).

use render::{Block, Dir, RichBlock, RichDoc, RichList, RichTable, RichText, Span};
use std::sync::atomic::{AtomicU32, Ordering};

/// A run of a message body: text drawn as `StyledText`, or a code block drawn as a box of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Segment {
    /// Markdown for `StyledText`, and whether its lines run right to left.
    Text { md: String, rtl: bool },
    /// The code as written, without the trailing line break.
    Code(String),
}

/// One block of a body as a channel post lays it out (`Rendered::post`): a heading that stands
/// out, a list item that hangs beside its marker. `StyledText` can do neither.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PostBlock {
    pub kind: PostKind,
    /// Markdown for `StyledText` (empty for code).
    pub md: String,
    pub rtl: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PostKind {
    Text,
    /// A heading, level 1 to 3.
    Heading(u8),
    /// A list item: its marker ("•", "◦", "▪" or "3.") and how deep its list is nested (0: top).
    Item {
        marker: String,
        depth: usize,
    },
    /// A code block, the code as written.
    Code(String),
}

/// One message a reply quotes, drawn as a card above the text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuoteCard {
    pub author: String,
    /// Who wrote the quoted message (MRI, `""`: not given), for a quote without a real name.
    pub author_mri: String,
    /// The quoted text; a line break of the original is a `'\n'`.
    pub text: String,
    /// The quoted message (`""`: none), for the jump to it.
    pub id: String,
    /// When the quoted message was sent, epoch ms.
    pub time_ms: Option<i64>,
    pub rtl: bool,
}

/// What a message bubble needs to draw its content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendered {
    /// The messages this one quotes, in order (a message can quote several).
    pub quotes: Vec<QuoteCard>,
    /// The whole body as Markdown; a code block is flattened to inline code here.
    pub markdown: String,
    /// The body as text and code blocks, in order. Empty when it has no code block (then
    /// `markdown` is the whole body).
    pub segments: Vec<Segment>,
    pub plain: String,
    pub rtl: bool,
    pub images: Vec<(String, Option<u32>, Option<u32>)>,
    /// The body as a channel post lays it out, block by block. Empty when it has no heading and
    /// no list (then the post draws `markdown` or `segments` as a chat bubble does).
    pub post: Vec<PostBlock>,
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
    /// The text runs right to left (its first strong character is Arabic or Hebrew).
    rtl: bool,
    /// How a post lays the piece out when that is not as one block of text (or code): a heading,
    /// or the items of a list.
    post: Vec<PostBlock>,
}

impl Piece {
    fn text(md: String) -> Self {
        Piece {
            md,
            code: None,
            rtl: false,
            post: Vec::new(),
        }
    }

    fn rtl(mut self, dir: Dir) -> Self {
        self.rtl = dir == Dir::Rtl;
        self
    }

    fn post(mut self, blocks: Vec<PostBlock>) -> Self {
        self.post = blocks;
        self
    }

    /// The piece as post blocks: its own `post`, or one block of text or code.
    fn post_blocks(&self) -> Vec<PostBlock> {
        if !self.post.is_empty() {
            return self.post.clone();
        }
        match &self.code {
            Some(code) => vec![PostBlock {
                kind: PostKind::Code(code.clone()),
                md: String::new(),
                rtl: false,
            }],
            None if self.md.trim().is_empty() => vec![],
            None => vec![PostBlock {
                kind: PostKind::Text,
                md: self.md.clone(),
                rtl: self.rtl,
            }],
        }
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
            rtl: false,
            post: Vec::new(),
        }
    }
}

/// Splits a paragraph into runs of lines that share a direction (each line has its own, from its
/// first strong character; a line without one follows the line before it, `*dir` at the start).
/// `*dir` is left at the direction of the last line. A paragraph whose lines agree stays whole.
fn direction_runs(spans: &[Span], dir: &mut Dir) -> Vec<(Vec<Span>, Dir)> {
    let dirs = render::line_directions(&spans_plain(spans), *dir);
    *dir = dirs.last().copied().unwrap_or(*dir);
    if dirs.windows(2).all(|w| w[0] == w[1]) {
        return vec![(spans.to_vec(), dirs[0])];
    }
    let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
    for span in spans {
        for (i, part) in span.text.split('\n').enumerate() {
            if i > 0 {
                lines.push(Vec::new());
            }
            if let Some(line) = lines.last_mut() {
                line.push(Span {
                    text: part.to_string(),
                    ..span.clone()
                });
            }
        }
    }
    let mut runs: Vec<(Vec<Span>, Dir)> = Vec::new();
    for (line, line_dir) in lines.into_iter().zip(dirs) {
        match runs.last_mut() {
            Some((run, run_dir)) if *run_dir == line_dir => {
                run.push(Span {
                    text: "\n".into(),
                    style: Default::default(),
                    link: None,
                    mention: None,
                });
                run.extend(line);
            }
            _ => runs.push((line, line_dir)),
        }
    }
    runs
}

/// Sets `r.markdown`, `r.segments` and `r.rtl` from the blocks of a body. A body that has code
/// blocks or lines of both directions is cut into segments, each text segment aligned by its own
/// direction; any other body is one run of text aligned by `r.rtl`.
fn set_body(r: &mut Rendered, pieces: Vec<Piece>) {
    r.post = if pieces.iter().any(|p| !p.post.is_empty()) {
        pieces.iter().flat_map(Piece::post_blocks).collect()
    } else {
        Vec::new()
    };
    r.markdown = pieces
        .iter()
        .map(|p| p.md.as_str())
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    r.segments = Vec::new();
    let mut texts = pieces
        .iter()
        .filter(|p| p.code.is_none() && !p.md.trim().is_empty());
    r.rtl = texts.clone().next().is_some_and(|p| p.rtl);
    let mixed = texts.any(|p| p.rtl != r.rtl);
    if !mixed && pieces.iter().all(|p| p.code.is_none()) {
        return;
    }
    let mut text: Vec<&str> = Vec::new();
    let mut text_rtl = false;
    for p in &pieces {
        match &p.code {
            Some(code) => {
                flush_text(&mut text, text_rtl, &mut r.segments);
                r.segments.push(Segment::Code(code.clone()));
            }
            None if !p.md.trim().is_empty() => {
                if p.rtl != text_rtl {
                    flush_text(&mut text, text_rtl, &mut r.segments);
                }
                text_rtl = p.rtl;
                text.push(&p.md);
            }
            None => {}
        }
    }
    flush_text(&mut text, text_rtl, &mut r.segments);
}

/// Closes the run of text blocks collected so far as one segment.
fn flush_text(text: &mut Vec<&str>, rtl: bool, out: &mut Vec<Segment>) {
    if !text.is_empty() {
        out.push(Segment::Text {
            md: text.join("\n\n"),
            rtl,
        });
        text.clear();
    }
}

/// Converts a parsed message into bubble parts.
pub fn render_blocks(rt: &RichText) -> Rendered {
    let mut r = Rendered::default();
    let mut md: Vec<Piece> = vec![];
    let mut plain: Vec<String> = vec![];
    let mut dir = Dir::Ltr;
    for b in &rt.blocks {
        match b {
            // A Teams reply quote and each message of a forward is a card wherever it stands; a
            // plain blockquote only when it opens the message.
            Block::Quote {
                message_id,
                author_mri,
                author,
                time_ms,
                forwarded,
                preview,
                dir,
                ..
            } if message_id.is_some()
                || author.is_some()
                || *forwarded
                || (r.quotes.is_empty() && md.is_empty()) =>
            {
                r.quotes.push(QuoteCard {
                    author: author.clone().unwrap_or_default(),
                    author_mri: author_mri.clone().unwrap_or_default(),
                    text: preview.clone(),
                    id: message_id.clone().unwrap_or_default(),
                    time_ms: *time_ms,
                    rtl: *dir == Dir::Rtl,
                });
            }
            Block::Quote { preview, .. } => {
                dir = render::text_direction(preview).unwrap_or(dir);
                md.push(Piece::text(format!("> {}", escape(&preview.replace('\n', " ")))).rtl(dir));
                plain.push(preview.clone());
            }
            Block::Paragraph { spans, .. } => {
                for (run, run_dir) in direction_runs(spans, &mut dir) {
                    md.push(Piece::text(spans_md(&run)).rtl(run_dir));
                }
                plain.push(spans_plain(spans));
            }
            Block::List { ordered, items } => {
                let mut lines = vec![];
                let mut post = vec![];
                // A list is one run of text: it takes the direction of its first item. In a post
                // each item takes its own.
                let list_dir = items
                    .first()
                    .and_then(|it| render::text_direction(&spans_plain(it)))
                    .unwrap_or(dir);
                for (i, it) in items.iter().enumerate() {
                    let bullet = if *ordered {
                        format!("{}.", i + 1)
                    } else {
                        "-".to_string()
                    };
                    lines.push(format!("{bullet} {}", spans_md(it)));
                    plain.push(spans_plain(it));
                    post.push(item_block(
                        it,
                        if *ordered { bullet } else { "•".into() },
                        0,
                        list_dir,
                    ));
                }
                dir = list_dir;
                md.push(Piece::text(lines.join("\n")).rtl(dir).post(post));
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

fn list_md(
    list: &RichList,
    depth: usize,
    dir: Dir,
    out: &mut Vec<String>,
    post: &mut Vec<PostBlock>,
) {
    for (i, item) in list.items.iter().enumerate() {
        let number = format!("{}.", list.start.unwrap_or(1) as usize + i);
        let bullet = if list.ordered {
            number.clone()
        } else {
            "-".to_string()
        };
        out.push(format!(
            "{}{bullet} {}",
            "  ".repeat(depth),
            spans_md(&item.spans)
        ));
        // Markdown's bullets for each depth, as `StyledText` draws them.
        let marker = if list.ordered {
            number
        } else {
            ["•", "◦", "▪"][depth % 3].to_string()
        };
        post.push(item_block(&item.spans, marker, depth, dir));
        for child in &item.children {
            list_md(child, depth + 1, dir, out, post);
        }
    }
}

/// A list item as a post block: aligned by its own direction (`dir` when it has no strong
/// character), its text never read as the start of another list.
fn item_block(spans: &[Span], marker: String, depth: usize, dir: Dir) -> PostBlock {
    let dir = render::text_direction(&spans_plain(spans)).unwrap_or(dir);
    PostBlock {
        kind: PostKind::Item { marker, depth },
        md: literal_start(spans_md(spans)),
        rtl: dir == Dir::Rtl,
    }
}

/// `md` with a leading list marker ("- ", "+ ", "2. ", "3) ") escaped, so a line drawn on its
/// own keeps it as text.
fn literal_start(md: String) -> String {
    if md.starts_with("- ") || md.starts_with("+ ") {
        return format!("\\{md}");
    }
    let digits = md.len() - md.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0 && matches!(md[digits..].chars().next(), Some('.' | ')')) {
        return format!("{}\\{}", &md[..digits], &md[digits..]);
    }
    md
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
    // The reply quotes are drawn as cards: leave out as many quote blocks as there are cards.
    let mut cards = r.quotes.len();
    let mut dir = Dir::Ltr;
    for b in &doc.blocks {
        match b {
            RichBlock::Quote(_) if cards > 0 => cards -= 1,
            RichBlock::Paragraph(spans) => {
                for (run, run_dir) in direction_runs(spans, &mut dir) {
                    md.push(Piece::text(spans_md(&run)).rtl(run_dir));
                }
            }
            RichBlock::Heading { level, spans } => {
                dir = render::text_direction(&spans_plain(spans)).unwrap_or(dir);
                let bold: Vec<Span> = spans
                    .iter()
                    .map(|s| {
                        let mut s = s.clone();
                        s.style.bold = true;
                        s
                    })
                    .collect();
                let text = spans_md(&bold);
                let heading = PostBlock {
                    kind: PostKind::Heading(*level),
                    md: text.clone(),
                    rtl: dir == Dir::Rtl,
                };
                md.push(Piece::text(text).rtl(dir).post(vec![heading]));
            }
            RichBlock::Quote(spans) => {
                dir = render::text_direction(&spans_plain(spans)).unwrap_or(dir);
                md.push(
                    Piece::text(
                        spans_md(spans)
                            .split('\n')
                            .map(|l| format!("> {l}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                    .rtl(dir),
                );
            }
            RichBlock::List(l) => {
                if let Some(first) = l.items.first() {
                    dir = render::text_direction(&spans_plain(&first.spans)).unwrap_or(dir);
                }
                let (mut lines, mut post) = (Vec::new(), Vec::new());
                list_md(l, 0, dir, &mut lines, &mut post);
                md.push(Piece::text(lines.join("\n")).rtl(dir).post(post));
            }
            RichBlock::Code { text, .. } => md.push(Piece::code(text)),
            RichBlock::Table(t) => md.push(Piece::text(table_md(t)).rtl(dir)),
            RichBlock::Rule => md.push(Piece::text("──────────".into()).rtl(dir)),
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

    fn text_segment(md: &str, rtl: bool) -> Segment {
        Segment::Text { md: md.into(), rtl }
    }

    #[test]
    fn every_paragraph_of_a_mixed_message_has_its_own_direction() {
        let x = r("<p>Hi team</p><p>حاضر</p><p>ok تمام</p>");
        assert_eq!(
            x.segments,
            vec![
                text_segment("Hi team", false),
                text_segment("حاضر", true),
                text_segment("ok تمام", false),
            ]
        );
        assert!(!x.rtl);
    }

    #[test]
    fn every_line_break_of_a_mixed_paragraph_has_its_own_direction() {
        let x = r("<p>Hi team<br>حاضر<br>ok تمام</p>");
        assert_eq!(
            x.segments,
            vec![
                text_segment("Hi team", false),
                text_segment("حاضر", true),
                text_segment("ok تمام", false),
            ]
        );
        // Lines that agree stay one run with their line breaks.
        let x = r("<p>one<br>two</p><p>حاضر<br>تمام</p>");
        assert_eq!(
            x.segments,
            vec![
                text_segment("one\\\ntwo", false),
                text_segment("حاضر\\\nتمام", true)
            ]
        );
    }

    #[test]
    fn a_paragraph_without_a_strong_character_follows_the_one_before() {
        let x = r("<p>حاضر</p><p>👍</p><p>123</p>");
        assert!(x.segments.is_empty());
        assert!(x.rtl);
    }

    #[test]
    fn a_rich_message_keeps_the_direction_of_every_line() {
        let x = rich("<h1>Title</h1><p>حاضر</p><p><u>ok</u></p>");
        assert_eq!(
            x.segments,
            vec![
                text_segment("**Title**", false),
                text_segment("حاضر", true),
                text_segment("<u>ok</u>", false),
            ]
        );
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
        assert_eq!(
            x.quotes,
            vec![QuoteCard {
                author: "Sara".into(),
                author_mri: "8:orgid:a".into(),
                text: "original text".into(),
                id: "1".into(),
                time_ms: Some(1),
                rtl: false,
            }],
            "id is the quoted message, for the jump to it"
        );
        assert_eq!(x.markdown, "my reply");
    }

    const FWD: &str = r#"<blockquote itemtype="http://schema.skype.com/Forward">"#;

    #[test]
    fn a_forwarded_gif_is_an_image_of_the_message_not_card_text() {
        let html = format!(
            "{FWD}<div><img src=\"https://media4.giphy.com/x/giphy.gif\" alt=\"GIF\" \
             itemtype=\"http://schema.skype.com/Giphy\" style=\"width:480px; height:320px\"></div></blockquote>"
        );
        let x = r(&html);
        assert_eq!(x.quotes.len(), 1);
        assert_eq!(x.quotes[0].text, "");
        assert_eq!(x.images.len(), 1);
        assert_eq!(x.images[0].0, "https://media4.giphy.com/x/giphy.gif");
        assert_eq!(x.plain, "");
    }

    #[test]
    fn every_forwarded_message_is_a_quote_card() {
        let html = format!(
            "{FWD}<p>سطر واحد</p><p>سطر اثنان</p></blockquote>\
             {FWD}<p>first</p><p>second</p></blockquote>\
             {FWD}<h1><span style=\"font-size:x-large\"><strong>Something went wrong</strong></span></h1></blockquote>"
        );
        let x = r(&html);
        let cards: Vec<(&str, bool)> = x.quotes.iter().map(|q| (q.text.as_str(), q.rtl)).collect();
        assert_eq!(
            cards,
            vec![
                ("سطر واحد\nسطر اثنان", true),
                ("first\nsecond", false),
                ("Something went wrong", false),
            ]
        );
        assert!(
            x.quotes
                .iter()
                .all(|q| q.author.is_empty() && q.id.is_empty())
        );
        assert_eq!(x.markdown, "", "no quote is left as '> ' text");
        assert!(x.segments.is_empty());
    }

    #[test]
    fn a_forward_after_text_is_still_a_card() {
        let x = r(&format!("<p>look</p>{FWD}<p>fwd</p></blockquote>"));
        assert_eq!(x.quotes.len(), 1);
        assert_eq!(x.quotes[0].text, "fwd");
        assert_eq!(x.markdown, "look");
    }

    #[test]
    fn a_plain_blockquote_after_the_first_stays_a_text_line() {
        let x = r(
            "<blockquote>one</blockquote><blockquote>two</blockquote><blockquote>three</blockquote>",
        );
        assert_eq!(x.quotes.len(), 1, "only the one that opens the message");
        assert_eq!(x.quotes[0].text, "one");
        assert_eq!(x.markdown, "> two\n\n> three");
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
        assert_eq!(x.quotes.len(), 1);
        assert_eq!(x.quotes[0].text, "original");
        assert_eq!(x.markdown, "**Title**");
    }

    #[test]
    fn every_reply_quote_is_a_card_and_none_leaks_into_the_text() {
        let quote = |id: &str, name: &str, text: &str| {
            render::reply_quote_html(id, &format!("8:orgid:{id}"), name, 1731234000000, text)
        };
        let html = format!(
            "{}{}{}<p>my answer</p>",
            quote("11", "Sara", "one\ntwo"),
            quote("22", "Omar", "بص انا غلطت"),
            quote("33", "Sara", "three"),
        );
        let x = r(&html);
        let cards: Vec<_> = x
            .quotes
            .iter()
            .map(|q| (q.author.as_str(), q.text.as_str(), q.id.as_str(), q.rtl))
            .collect();
        assert_eq!(
            cards,
            vec![
                ("Sara", "one two", "11", false),
                ("Omar", "بص انا غلطت", "22", true),
                ("Sara", "three", "33", false),
            ]
        );
        assert_eq!(x.markdown, "my answer");
        assert!(!x.markdown.contains('>') && !x.markdown.contains('<'));
        // A message with a heading takes the rich path: still three cards and no quote text.
        let y = rich(&format!("{html}<h2>Title</h2>"));
        assert_eq!(y.quotes.len(), 3);
        assert_eq!(y.markdown, "my answer\n\n**Title**");
    }

    #[test]
    fn nested_lists_and_rules_parse() {
        let x = rich("<h2>x</h2><ul><li>a<ul><li>b</li></ul></li></ul><hr>");
        parses(&x.markdown);
        assert!(x.markdown.contains("- a\n  - b"), "{:?}", x.markdown);
    }

    #[test]
    fn a_post_lays_out_headings_and_list_items_block_by_block() {
        let x = rich(
            "<h2>What is new</h2><p>intro</p><ul><li>one</li><li>two<ul><li>deep</li></ul></li></ul><ol><li>first</li><li>2. second</li></ol><pre>code</pre>",
        );
        let item = |marker: &str, depth| PostKind::Item {
            marker: marker.into(),
            depth,
        };
        let blocks: Vec<(PostKind, &str)> = x
            .post
            .iter()
            .map(|b| (b.kind.clone(), b.md.as_str()))
            .collect();
        assert_eq!(
            blocks,
            vec![
                (PostKind::Heading(2), "**What is new**"),
                (PostKind::Text, "intro"),
                (item("•", 0), "one"),
                (item("•", 0), "two"),
                (item("◦", 1), "deep"),
                (item("1.", 0), "first"),
                // Drawn on its own, the item's text must not become a list of its own.
                (item("2.", 0), "2\\. second"),
                (PostKind::Code("code".into()), ""),
            ]
        );
        for b in &x.post {
            parses(&b.md);
        }
        // The chat bubble's markdown is what it was.
        assert!(x.markdown.contains("- two\n  - deep"), "{:?}", x.markdown);
    }

    #[test]
    fn each_list_item_of_a_post_is_aligned_by_its_own_direction() {
        let x = r("<p>intro</p><ul><li>نقطة أولى</li><li>second</li></ul>");
        let blocks: Vec<(&str, bool)> = x.post.iter().map(|b| (b.md.as_str(), b.rtl)).collect();
        assert_eq!(
            blocks,
            vec![("intro", false), ("نقطة أولى", true), ("second", false)]
        );
        // A body with no heading and no list is drawn as a bubble draws it.
        assert!(r("<p>hello</p><p>there</p>").post.is_empty());
    }

    #[test]
    fn a_code_block_is_its_own_segment_between_runs_of_text() {
        let x = r(
            "<p>before</p><p>more</p><pre><code>let a = 1;\n    let b = 2;\n</code></pre><p>after</p>",
        );
        assert_eq!(
            x.segments,
            vec![
                Segment::Text {
                    md: "before\n\nmore".into(),
                    rtl: false
                },
                Segment::Code("let a = 1;\n    let b = 2;".into()),
                Segment::Text {
                    md: "after".into(),
                    rtl: false
                },
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
                Segment::Text {
                    md: "**Title**".into(),
                    rtl: false
                },
                Segment::Code("x = 1".into()),
                Segment::Text {
                    md: "end".into(),
                    rtl: false
                },
            ]
        );
    }

    #[test]
    fn each_segment_of_text_is_markdown_slint_accepts() {
        let x = r("<p>a <b>b</b></p><pre>1\n2</pre><ul><li>x</li></ul>");
        for s in &x.segments {
            if let Segment::Text { md, .. } = s {
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
