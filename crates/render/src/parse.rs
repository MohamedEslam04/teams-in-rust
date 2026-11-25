//! Teams message HTML -> [`RichText`].

use scraper::{ElementRef, Html, Node};

use crate::{Block, RichText, Span, Style, base_direction, plain_text, spans_text};

/// Guards against pathologically nested markup.
pub(crate) const MAX_DEPTH: usize = 256;

/// Parse the HTML of a Teams message (`RichText/Html`) into [`RichText`].
///
/// Input without any tag is treated as plain text (`Text` messages): entities
/// are unescaped and each non-empty line becomes a paragraph. Never panics on
/// malformed input.
pub fn parse_teams_html(html: &str) -> RichText {
    if !has_markup(html) {
        return parse_plain(html);
    }
    let doc = Html::parse_fragment(html);
    let mut w = Walker::default();
    w.walk_children(doc.root_element(), &Ctx::default(), 0);
    w.finish()
}

/// True if `s` contains something that looks like a tag, comment or doctype.
pub(crate) fn has_markup(s: &str) -> bool {
    s.as_bytes()
        .windows(2)
        .any(|w| w[0] == b'<' && (w[1].is_ascii_alphabetic() || matches!(w[1], b'/' | b'!' | b'?')))
}

fn parse_plain(text: &str) -> RichText {
    let doc = Html::parse_fragment(text);
    let decoded: String = doc.root_element().text().collect();
    let blocks = decoded
        .split('\n')
        .map(|l| l.replace('\u{a0}', " "))
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .map(|l| Block::Paragraph {
            dir: base_direction(&l),
            spans: vec![plain_span(l)],
        })
        .collect();
    RichText { blocks }
}

pub(crate) fn plain_span(text: String) -> Span {
    Span {
        text,
        style: Style::default(),
        link: None,
        mention: None,
    }
}

/// Inherited inline formatting.
#[derive(Clone, Default)]
struct Ctx {
    style: Style,
    link: Option<String>,
    mention: Option<String>,
}

#[derive(Default)]
struct ListFrame {
    ordered: bool,
    items: Vec<Vec<Span>>,
}

#[derive(Default)]
struct Walker {
    blocks: Vec<Block>,
    /// Inline run being built (paragraph, or list item while in a list).
    cur: Vec<Span>,
    list: Option<ListFrame>,
    /// Skip `itemprop="mri"` / `"time"` elements (reply-quote metadata).
    skip_quote_meta: bool,
}

pub(crate) fn attr<'a>(el: &ElementRef<'a>, name: &str) -> Option<&'a str> {
    el.value().attr(name)
}

pub(crate) fn attr_lower(el: &ElementRef, name: &str) -> String {
    attr(el, name).unwrap_or("").to_ascii_lowercase()
}

/// `text`, the first text of `el`, without the right-to-left mark that older builds of this app
/// put in front of a block that says `dir="ltr"`: messages already sent still carry it, and here
/// it would turn an English first line into a right-to-left paragraph.
pub(crate) fn own_mark_free<'a>(el: &ElementRef, text: &'a str) -> &'a str {
    if attr_lower(el, "dir") == "ltr" {
        text.trim_start_matches('\u{200F}')
    } else {
        text
    }
}

pub(crate) fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub(crate) fn parse_dim(s: Option<&str>) -> Option<u32> {
    let s = s?.trim();
    s.strip_suffix("px").unwrap_or(s).trim().parse().ok()
}

pub(crate) fn is_emoji_img(el: &ElementRef) -> bool {
    attr_lower(el, "itemtype").ends_with("/emoji")
}

/// Collapse HTML whitespace runs to single spaces (keeps a leading/trailing
/// space if present).
pub(crate) fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c') {
            pending = true;
        } else {
            if pending {
                out.push(' ');
                pending = false;
            }
            out.push(c);
        }
    }
    if pending {
        out.push(' ');
    }
    out
}

pub(crate) fn trim_spans(spans: &mut Vec<Span>) {
    let is_pad = |c: char| c == ' ' || c == '\n';
    while let Some(first) = spans.first_mut() {
        let trimmed = first.text.trim_start_matches(is_pad);
        if trimmed.len() != first.text.len() {
            first.text = trimmed.to_owned();
        }
        if first.text.is_empty() {
            spans.remove(0);
        } else {
            break;
        }
    }
    while let Some(last) = spans.last_mut() {
        let len = last.text.trim_end_matches(is_pad).len();
        last.text.truncate(len);
        if last.text.is_empty() {
            spans.pop();
        } else {
            break;
        }
    }
}

impl Walker {
    fn finish(mut self) -> RichText {
        self.flush_paragraph();
        RichText {
            blocks: self.blocks,
        }
    }

    fn flush_paragraph(&mut self) {
        let mut spans = std::mem::take(&mut self.cur);
        trim_spans(&mut spans);
        if spans.is_empty() {
            return;
        }
        let dir = base_direction(&spans_text(&spans));
        self.blocks.push(Block::Paragraph { dir, spans });
    }

    fn end_item(&mut self) {
        let mut spans = std::mem::take(&mut self.cur);
        trim_spans(&mut spans);
        if spans.is_empty() {
            return;
        }
        if let Some(list) = &mut self.list {
            list.items.push(spans);
        }
    }

    /// Block boundary: new paragraph, or a soft line break inside a list item.
    fn block_break(&mut self) {
        if self.list.is_some() {
            self.push_newline();
        } else {
            self.flush_paragraph();
        }
    }

    fn at_line_start_or_space(&self) -> bool {
        match self.cur.last().and_then(|s| s.text.chars().last()) {
            None => true,
            Some(c) => c == ' ' || c == '\n',
        }
    }

    fn push_span(&mut self, text: String, ctx: &Ctx) {
        if text.is_empty() {
            return;
        }
        if ctx.mention.is_none()
            && let Some(last) = self.cur.last_mut()
            && last.mention.is_none()
            && last.style == ctx.style
            && last.link == ctx.link
        {
            last.text.push_str(&text);
            return;
        }
        self.cur.push(Span {
            text,
            style: ctx.style.clone(),
            link: ctx.link.clone(),
            mention: ctx.mention.clone(),
        });
    }

    fn push_text(&mut self, raw: &str, ctx: &Ctx) {
        let mut text = collapse_ws(raw);
        if text.starts_with(' ') && self.at_line_start_or_space() {
            text.remove(0);
        }
        self.push_span(text.replace('\u{a0}', " "), ctx);
    }

    fn push_newline(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        while let Some(last) = self.cur.last_mut() {
            let len = last.text.trim_end_matches(' ').len();
            last.text.truncate(len);
            if last.text.is_empty() {
                self.cur.pop();
            } else {
                break;
            }
        }
        if self.cur.is_empty() {
            return;
        }
        self.push_span("\n".to_owned(), &Ctx::default());
    }

    fn walk_children(&mut self, el: ElementRef, ctx: &Ctx, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for (i, child) in el.children().enumerate() {
            match child.value() {
                Node::Text(t) => {
                    self.push_text(if i == 0 { own_mark_free(&el, t) } else { t }, ctx)
                }
                Node::Element(_) => {
                    if let Some(e) = ElementRef::wrap(child) {
                        self.element(e, ctx, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    fn element(&mut self, el: ElementRef, ctx: &Ctx, depth: usize) {
        if self.skip_quote_meta && matches!(attr(&el, "itemprop"), Some("mri" | "time")) {
            return;
        }
        let name = el.value().name();
        let styled = |f: fn(&mut Style)| {
            let mut c = ctx.clone();
            f(&mut c.style);
            c
        };
        match name {
            "script" | "style" | "head" | "title" | "template" => {}
            "br" => self.push_newline(),
            "hr" => self.block_break(),
            "p" | "div" | "section" | "article" | "header" | "footer" | "center" | "figure"
            | "figcaption" | "table" | "thead" | "tbody" | "tfoot" | "tr" | "dl" | "dt" | "dd" => {
                self.block_break();
                self.walk_children(el, ctx, depth);
                self.block_break();
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block_break();
                self.walk_children(el, &styled(|s| s.bold = true), depth);
                self.block_break();
            }
            "td" | "th" => {
                self.walk_children(el, ctx, depth);
                self.push_text(" ", ctx);
            }
            "blockquote" => {
                if self.list.is_some() {
                    self.walk_children(el, ctx, depth);
                } else {
                    self.quote(el);
                }
            }
            "pre" => {
                if self.list.is_some() {
                    self.walk_children(el, &styled(|s| s.code = true), depth);
                } else {
                    self.flush_paragraph();
                    let mut text = String::new();
                    raw_text(el, &mut text, 0);
                    if !text.trim().is_empty() {
                        self.blocks.push(Block::Code { text });
                    }
                }
            }
            "ul" | "ol" => {
                if self.list.is_some() {
                    // Nested list: flatten its items into the outer list.
                    self.end_item();
                    self.walk_children(el, ctx, depth);
                    self.end_item();
                } else {
                    self.flush_paragraph();
                    self.list = Some(ListFrame {
                        ordered: name == "ol",
                        items: Vec::new(),
                    });
                    self.walk_children(el, ctx, depth);
                    self.end_item();
                    if let Some(list) = self.list.take()
                        && !list.items.is_empty()
                    {
                        self.blocks.push(Block::List {
                            ordered: list.ordered,
                            items: list.items,
                        });
                    }
                }
            }
            "li" => {
                if self.list.is_some() {
                    self.end_item();
                    self.walk_children(el, ctx, depth);
                    self.end_item();
                } else {
                    self.block_break();
                    self.walk_children(el, ctx, depth);
                    self.block_break();
                }
            }
            "img" => self.image(el, ctx),
            "b" | "strong" => self.walk_children(el, &styled(|s| s.bold = true), depth),
            "i" | "em" => self.walk_children(el, &styled(|s| s.italic = true), depth),
            "u" => self.walk_children(el, &styled(|s| s.underline = true), depth),
            "s" | "strike" | "del" => self.walk_children(el, &styled(|s| s.strike = true), depth),
            "code" | "tt" => self.walk_children(el, &styled(|s| s.code = true), depth),
            "a" => {
                let mut c = ctx.clone();
                if let Some(href) = non_empty(attr(&el, "href")) {
                    c.link = Some(href);
                }
                self.walk_children(el, &c, depth);
            }
            "at" => {
                let mut c = ctx.clone();
                c.mention = Some(attr(&el, "id").unwrap_or("").to_owned());
                self.walk_children(el, &c, depth);
            }
            _ => {
                let mut c = ctx.clone();
                if name == "span" && attr_lower(&el, "itemtype").ends_with("/mention") {
                    c.mention = Some(attr(&el, "itemid").unwrap_or("").to_owned());
                }
                self.walk_children(el, &c, depth);
            }
        }
    }

    fn image(&mut self, el: ElementRef, ctx: &Ctx) {
        if is_emoji_img(&el) {
            if let Some(alt) = attr(&el, "alt") {
                self.push_span(alt.to_owned(), ctx);
            }
            return;
        }
        let Some(src) = non_empty(attr(&el, "src")) else {
            return;
        };
        let lower = src.to_ascii_lowercase();
        if self.list.is_some() || !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return;
        }
        self.flush_paragraph();
        self.blocks.push(Block::Image {
            url: src,
            width: parse_dim(attr(&el, "width")),
            height: parse_dim(attr(&el, "height")),
        });
    }

    fn quote(&mut self, el: ElementRef) {
        self.flush_paragraph();
        let is_reply = attr_lower(&el, "itemtype").ends_with("/reply");
        let forwarded = attr_lower(&el, "itemtype").ends_with("/forward");
        let mut message_id = None;
        let mut author_mri = None;
        let mut author = None;
        let mut time_ms = None;
        let preview = if is_reply {
            message_id = non_empty(attr(&el, "itemid"));
            let find = |prop: &str| {
                el.descendants()
                    .filter_map(ElementRef::wrap)
                    .find(|e| attr(e, "itemprop") == Some(prop))
            };
            if let Some(mri) = find("mri") {
                author_mri = non_empty(attr(&mri, "itemid"));
                author = Some(inline_text(mri, false)).filter(|s| !s.is_empty());
            }
            time_ms = find("time").and_then(|t| attr(&t, "itemid")?.trim().parse().ok());
            match find("preview") {
                Some(p) => quote_lines(p, false),
                None => quote_lines(el, true),
            }
        } else {
            quote_lines(el, false)
        };
        let dir = base_direction(&preview);
        self.blocks.push(Block::Quote {
            message_id,
            author_mri,
            author,
            time_ms,
            forwarded,
            preview,
            dir,
        });
    }
}

/// Descendant text with emoji/mention handling, flattened to one
/// whitespace-collapsed line.
fn inline_text(el: ElementRef, skip_quote_meta: bool) -> String {
    let mut w = Walker {
        skip_quote_meta,
        ..Walker::default()
    };
    w.walk_children(el, &Ctx::default(), 0);
    let text = plain_text(&w.finish());
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Like [`inline_text`] but keeps the line breaks (`<br>`, one `<p>` per line) of a quoted
/// message as `'\n'`; empty lines are dropped.
fn quote_lines(el: ElementRef, skip_quote_meta: bool) -> String {
    let mut w = Walker {
        skip_quote_meta,
        ..Walker::default()
    };
    w.walk_children(el, &Ctx::default(), 0);
    plain_text(&w.finish())
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Verbatim descendant text (for `<pre>`), `<br>` as newline.
pub(crate) fn raw_text(el: ElementRef, out: &mut String, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for child in el.children() {
        match child.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => match e.name() {
                "br" => out.push('\n'),
                "script" | "style" => {}
                _ => {
                    if let Some(e) = ElementRef::wrap(child) {
                        raw_text(e, out, depth + 1);
                    }
                }
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dir;

    type StyleFn = fn(&mut Style);

    fn st() -> Style {
        Style::default()
    }

    fn sp(text: &str) -> Span {
        Span {
            text: text.into(),
            style: st(),
            link: None,
            mention: None,
        }
    }

    fn para(dir: Dir, spans: Vec<Span>) -> Block {
        Block::Paragraph { dir, spans }
    }

    fn ltr(text: &str) -> Block {
        para(Dir::Ltr, vec![sp(text)])
    }

    fn parse(html: &str) -> Vec<Block> {
        parse_teams_html(html).blocks
    }

    fn styled(text: &str, f: impl Fn(&mut Style)) -> Span {
        let mut s = sp(text);
        f(&mut s.style);
        s
    }

    // ---- plain text ----

    #[test]
    fn plain_text_input() {
        assert_eq!(parse("hello world"), vec![ltr("hello world")]);
    }

    #[test]
    fn plain_text_unescapes_entities() {
        assert_eq!(
            parse("a &amp; b &lt;c&gt; &#x1F44D;"),
            vec![ltr("a & b <c> \u{1F44D}")]
        );
    }

    #[test]
    fn plain_text_lines_and_less_than() {
        assert_eq!(
            parse("1 < 2\n\n  next  \r\nlast"),
            vec![ltr("1 < 2"), ltr("next"), ltr("last")]
        );
    }

    #[test]
    fn empty_input() {
        assert!(parse("").is_empty());
        assert!(parse("   \n ").is_empty());
    }

    // ---- paragraphs / whitespace ----

    #[test]
    fn paragraphs_and_divs() {
        assert_eq!(
            parse("<p>one</p><div>two</div><p>three</p>"),
            vec![ltr("one"), ltr("two"), ltr("three")]
        );
    }

    #[test]
    fn nested_div_with_paragraphs() {
        assert_eq!(
            parse("<div>a<p>b</p>c</div>"),
            vec![ltr("a"), ltr("b"), ltr("c")]
        );
    }

    #[test]
    fn empty_paragraphs_dropped() {
        assert_eq!(
            parse("<p></p><p>&nbsp;</p><p> </p><p>x</p><p><br></p>"),
            vec![ltr("x")]
        );
    }

    #[test]
    fn whitespace_collapsing() {
        assert_eq!(
            parse("<p>  hello \n\t  world  </p>"),
            vec![ltr("hello world")]
        );
    }

    #[test]
    fn collapse_across_inline_boundaries() {
        assert_eq!(
            parse("<p>a <b> b</b> c</p>"),
            vec![para(
                Dir::Ltr,
                vec![sp("a "), styled("b", |s| s.bold = true), sp(" c")]
            )]
        );
    }

    #[test]
    fn nbsp_becomes_space() {
        assert_eq!(parse("<p>a&nbsp;b</p>"), vec![ltr("a b")]);
        assert_eq!(parse("<p>a\u{a0}b</p>"), vec![ltr("a b")]);
    }

    #[test]
    fn br_is_newline_in_paragraph() {
        assert_eq!(parse("<p>a<br>b <br/> c</p>"), vec![ltr("a\nb\nc")]);
    }

    #[test]
    fn leading_and_trailing_br_trimmed() {
        assert_eq!(parse("<p><br>a<br></p>"), vec![ltr("a")]);
    }

    // ---- direction ----

    #[test]
    fn arabic_paragraph_is_rtl() {
        assert_eq!(
            parse("<p>تمام 👍 هراجع الـ PR</p>"),
            vec![para(Dir::Rtl, vec![sp("تمام 👍 هراجع الـ PR")])]
        );
    }

    #[test]
    fn english_first_mixed_is_ltr() {
        assert_eq!(
            parse("<p>Quick question: هل الـ endpoint</p>"),
            vec![ltr("Quick question: هل الـ endpoint")]
        );
    }

    #[test]
    fn per_paragraph_direction() {
        let blocks = parse("<p>hello</p><p>مرحبا</p>");
        assert!(matches!(blocks[0], Block::Paragraph { dir: Dir::Ltr, .. }));
        assert!(matches!(blocks[1], Block::Paragraph { dir: Dir::Rtl, .. }));
    }

    // ---- inline styles ----

    #[test]
    fn simple_styles() {
        let cases: [(&str, StyleFn); 9] = [
            ("b", |s| s.bold = true),
            ("strong", |s| s.bold = true),
            ("i", |s| s.italic = true),
            ("em", |s| s.italic = true),
            ("u", |s| s.underline = true),
            ("s", |s| s.strike = true),
            ("strike", |s| s.strike = true),
            ("del", |s| s.strike = true),
            ("code", |s| s.code = true),
        ];
        for (tag, f) in cases {
            assert_eq!(
                parse(&format!("<p><{tag}>x</{tag}></p>")),
                vec![para(Dir::Ltr, vec![styled("x", f)])],
                "tag {tag}"
            );
        }
    }

    #[test]
    fn nested_styles_combine() {
        let mut both = sp("bi");
        both.style.bold = true;
        both.style.italic = true;
        assert_eq!(
            parse("<p><b>b<i>bi</i></b><i>i</i></p>"),
            vec![para(
                Dir::Ltr,
                vec![
                    styled("b", |s| s.bold = true),
                    both,
                    styled("i", |s| s.italic = true)
                ]
            )]
        );
    }

    #[test]
    fn adjacent_same_style_spans_merge() {
        assert_eq!(
            parse("<p><b>a</b><b>b</b>c</p>"),
            vec![para(
                Dir::Ltr,
                vec![styled("ab", |s| s.bold = true), sp("c")]
            )]
        );
    }

    #[test]
    fn link() {
        let mut l = sp("site");
        l.link = Some("https://example.com/a?b=1&c=2".into());
        assert_eq!(
            parse(r#"<p>see <a href="https://example.com/a?b=1&amp;c=2">site</a></p>"#),
            vec![para(Dir::Ltr, vec![sp("see "), l])]
        );
    }

    #[test]
    fn link_with_bold_inside() {
        let mut l = styled("x", |s| s.bold = true);
        l.link = Some("https://e.com".into());
        assert_eq!(
            parse(r#"<a href="https://e.com"><b>x</b></a>"#),
            vec![para(Dir::Ltr, vec![l])]
        );
    }

    #[test]
    fn anchor_without_href_is_plain() {
        assert_eq!(parse("<a>x</a>"), vec![ltr("x")]);
    }

    // ---- code ----

    #[test]
    fn pre_block_preserves_whitespace() {
        assert_eq!(
            parse("<pre>fn main() {\n    println!(\"hi\");\n}</pre>"),
            vec![Block::Code {
                text: "fn main() {\n    println!(\"hi\");\n}".into()
            }]
        );
    }

    #[test]
    fn pre_with_inner_code_and_entities() {
        assert_eq!(
            parse("<p>before</p><pre><code>a &lt; b\n  c</code></pre><p>after</p>"),
            vec![
                ltr("before"),
                Block::Code {
                    text: "a < b\n  c".into()
                },
                ltr("after")
            ]
        );
    }

    #[test]
    fn pre_with_br_lines() {
        assert_eq!(
            parse("<pre>a<br>b</pre>"),
            vec![Block::Code {
                text: "a\nb".into()
            }]
        );
    }

    // ---- lists ----

    #[test]
    fn unordered_list() {
        assert_eq!(
            parse("<ul><li>one</li><li>two <b>x</b></li></ul>"),
            vec![Block::List {
                ordered: false,
                items: vec![
                    vec![sp("one")],
                    vec![sp("two "), styled("x", |s| s.bold = true)]
                ]
            }]
        );
    }

    #[test]
    fn ordered_list_between_paragraphs() {
        assert_eq!(
            parse("<p>intro</p>\n<ol>\n <li>a</li>\n <li>b</li>\n</ol>\n<p>outro</p>"),
            vec![
                ltr("intro"),
                Block::List {
                    ordered: true,
                    items: vec![vec![sp("a")], vec![sp("b")]]
                },
                ltr("outro")
            ]
        );
    }

    #[test]
    fn list_item_with_paragraph_and_nested_list() {
        assert_eq!(
            parse("<ul><li><p>a</p></li><li>b<ul><li>c</li></ul></li></ul>"),
            vec![Block::List {
                ordered: false,
                items: vec![vec![sp("a")], vec![sp("b")], vec![sp("c")]]
            }]
        );
    }

    #[test]
    fn empty_list_dropped() {
        assert!(parse("<ul><li></li></ul>").is_empty());
    }

    // ---- mentions ----

    #[test]
    fn mention_span() {
        let mut m = sp("Ahmed Ali");
        m.mention = Some("0".into());
        assert_eq!(
            parse(
                r#"<p><span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">Ahmed Ali</span> please check</p>"#
            ),
            vec![para(Dir::Ltr, vec![m, sp(" please check")])]
        );
    }

    #[test]
    fn mention_at_tag() {
        let mut m = sp("Name");
        m.mention = Some("8:orgid:abc".into());
        assert_eq!(
            parse(r#"<p>hi <at id="8:orgid:abc">Name</at>!</p>"#),
            vec![para(Dir::Ltr, vec![sp("hi "), m, sp("!")])]
        );
    }

    #[test]
    fn two_adjacent_mentions_stay_separate() {
        let html = r#"<span itemtype="http://schema.skype.com/Mention" itemid="0">A</span><span itemtype="http://schema.skype.com/Mention" itemid="1">B</span>"#;
        let Block::Paragraph { spans, .. } = &parse(html)[0] else {
            panic!()
        };
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].mention.as_deref(), Some("1"));
    }

    // ---- emoji ----

    const EMOJI: &str = r#"<span class="animated-emoticon-50-cool" title="Cool" type="(cool)"><img itemscope itemtype="http://schema.skype.com/Emoji" itemid="cool" src="https://statics.teams.cdn.office.net/evergreen-assets/skype/v2/cool/50.png" title="Cool" alt="😎" style="width:20px;height:20px;"></span>"#;

    #[test]
    fn emoji_span_uses_alt_only() {
        assert_eq!(
            parse(&format!("<p>nice {EMOJI} work</p>")),
            vec![ltr("nice 😎 work")]
        );
    }

    #[test]
    fn bare_emoji_img() {
        assert_eq!(
            parse(
                r#"<p>x<img itemtype="http://schema.skype.com/Emoji" alt="👍" src="https://e/1.png">y</p>"#
            ),
            vec![ltr("x👍y")]
        );
    }

    #[test]
    fn emoji_only_message_is_not_an_image_block() {
        assert_eq!(parse(EMOJI), vec![ltr("😎")]);
    }

    #[test]
    fn emoji_inherits_style() {
        assert_eq!(
            parse(r#"<b><img itemtype="http://schema.skype.com/Emoji" alt="👍"></b>"#),
            vec![para(Dir::Ltr, vec![styled("👍", |s| s.bold = true)])]
        );
    }

    #[test]
    fn numeric_entity_emoji() {
        assert_eq!(parse("<p>&#x1F44D;</p>"), vec![ltr("\u{1F44D}")]);
    }

    // ---- images ----

    #[test]
    fn ams_image() {
        assert_eq!(
            parse(
                r#"<p><img itemscope="png" itemtype="http://schema.skype.com/AMSImage" src="https://eu-api.asm.skype.com/v1/objects/0-weu-d1-abc/views/imgo" width="250" height="141" id="x-1"></p>"#
            ),
            vec![Block::Image {
                url: "https://eu-api.asm.skype.com/v1/objects/0-weu-d1-abc/views/imgo".into(),
                width: Some(250),
                height: Some(141)
            }]
        );
    }

    #[test]
    fn image_without_numeric_dims() {
        assert_eq!(
            parse(r#"<img src="http://x/y.png" width="auto" height="12px">"#),
            vec![Block::Image {
                url: "http://x/y.png".into(),
                width: None,
                height: Some(12)
            }]
        );
    }

    #[test]
    fn image_splits_paragraph() {
        assert_eq!(
            parse(r#"<p>before<img src="https://x/y.png">after</p>"#),
            vec![
                ltr("before"),
                Block::Image {
                    url: "https://x/y.png".into(),
                    width: None,
                    height: None
                },
                ltr("after")
            ]
        );
    }

    #[test]
    fn non_http_image_ignored() {
        assert!(parse(r#"<img src="data:image/png;base64,AAAA">"#).is_empty());
        assert!(parse(r#"<img>"#).is_empty());
    }

    // ---- quotes ----

    const REPLY: &str = r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="1731234567890"><strong itemprop="mri" itemid="8:orgid:11111111-2222-3333-4444-555555555555">Sara Hassan</strong><span itemprop="time" itemid="1731234567890"></span><p itemprop="preview">the original message text</p></blockquote><p>my reply</p>"#;

    #[test]
    fn reply_quote() {
        assert_eq!(
            parse(REPLY),
            vec![
                Block::Quote {
                    message_id: Some("1731234567890".into()),
                    author_mri: Some("8:orgid:11111111-2222-3333-4444-555555555555".into()),
                    author: Some("Sara Hassan".into()),
                    time_ms: Some(1731234567890),
                    forwarded: false,
                    preview: "the original message text".into(),
                    dir: Dir::Ltr
                },
                ltr("my reply")
            ]
        );
    }

    #[test]
    fn reply_quote_arabic_preview_is_rtl() {
        let html = REPLY.replace("the original message text", "الرسالة الأصلية");
        let Block::Quote { dir, preview, .. } = &parse(&html)[0] else {
            panic!()
        };
        assert_eq!(*dir, Dir::Rtl);
        assert_eq!(preview, "الرسالة الأصلية");
    }

    #[test]
    fn reply_quote_preview_with_emoji_and_entities() {
        let html = REPLY.replace("the original message text", &format!("a &amp; b {EMOJI}"));
        let Block::Quote { preview, .. } = &parse(&html)[0] else {
            panic!()
        };
        assert_eq!(preview, "a & b 😎");
    }

    #[test]
    fn reply_quote_without_preview_element_uses_text() {
        let html = r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="5"><strong itemprop="mri" itemid="8:x">Bob</strong><span itemprop="time" itemid="5"></span>hello there</blockquote>"#;
        assert_eq!(
            parse(html),
            vec![Block::Quote {
                message_id: Some("5".into()),
                author_mri: Some("8:x".into()),
                author: Some("Bob".into()),
                time_ms: Some(5),
                forwarded: false,
                preview: "hello there".into(),
                dir: Dir::Ltr
            }]
        );
    }

    fn reply_html(id: &str, name: &str, preview: &str) -> String {
        format!(
            r#"<blockquote itemscope="" itemtype="http://schema.skype.com/Reply" itemid="{id}"><strong itemprop="mri" itemid="8:orgid:{id}">{name}</strong><span itemprop="time" itemid="{id}"></span><p itemprop="preview">{preview}</p></blockquote>"#
        )
    }

    #[test]
    fn several_reply_quotes_and_the_text_after_them() {
        let html = format!(
            "{}<p>&nbsp;</p>{}{}<p>my answer</p>",
            reply_html("1731234100000", "Sara Hassan", "first line<br>second line"),
            reply_html("1731234200000", "Omar Ali", "بص انا غلطت<br>خلاص"),
            reply_html("1731234300000", "Sara Hassan", "one<br><br>two"),
        );
        let quote = |id: &str, name: &str, preview: &str, dir| Block::Quote {
            message_id: Some(id.into()),
            author_mri: Some(format!("8:orgid:{id}")),
            author: Some(name.into()),
            time_ms: id.parse().ok(),
            forwarded: false,
            preview: preview.into(),
            dir,
        };
        assert_eq!(
            parse(&html),
            vec![
                quote(
                    "1731234100000",
                    "Sara Hassan",
                    "first line\nsecond line",
                    Dir::Ltr
                ),
                quote("1731234200000", "Omar Ali", "بص انا غلطت\nخلاص", Dir::Rtl),
                quote("1731234300000", "Sara Hassan", "one\ntwo", Dir::Ltr),
                ltr("my answer"),
            ]
        );
    }

    #[test]
    fn plain_blockquote() {
        assert_eq!(
            parse("<blockquote>quoted  text</blockquote><p>x</p>"),
            vec![
                Block::Quote {
                    message_id: None,
                    author_mri: None,
                    author: None,
                    time_ms: None,
                    forwarded: false,
                    preview: "quoted text".into(),
                    dir: Dir::Ltr
                },
                ltr("x")
            ]
        );
    }

    #[test]
    fn forwarded_blockquotes_are_marked_and_keep_their_paragraphs() {
        let html = r#"<blockquote itemtype="http://schema.skype.com/Forward"><p>one</p><p>two</p></blockquote><blockquote itemtype="http://schema.skype.com/Forward"><p>مرحبا</p></blockquote>"#;
        let blocks = parse(html);
        let quote = |preview: &str, dir| Block::Quote {
            message_id: None,
            author_mri: None,
            author: None,
            time_ms: None,
            forwarded: true,
            preview: preview.into(),
            dir,
        };
        assert_eq!(
            blocks,
            vec![quote("one\ntwo", Dir::Ltr), quote("مرحبا", Dir::Rtl)]
        );
    }

    // ---- unknown / hostile / malformed ----

    #[test]
    fn unknown_tags_keep_text() {
        assert_eq!(
            parse("<p>a <foo>b</foo> <font color=red>c</font></p>"),
            vec![ltr("a b c")]
        );
    }

    #[test]
    fn script_and_style_stripped() {
        assert_eq!(
            parse("<style>p{color:red}</style><p>a<script>alert(1)</script>b</p>"),
            vec![ltr("ab")]
        );
    }

    #[test]
    fn unclosed_tags() {
        assert_eq!(
            parse("<p>one<p>two <b>bold"),
            vec![
                ltr("one"),
                para(
                    Dir::Ltr,
                    vec![sp("two "), styled("bold", |s| s.bold = true)]
                )
            ]
        );
    }

    #[test]
    fn stray_closing_tags_and_garbage() {
        // A stray </p> makes the HTML parser open an empty paragraph.
        assert_eq!(parse("</div></b>text</p><"), vec![ltr("text"), ltr("<")]);
    }

    #[test]
    fn malformed_attributes_do_not_panic() {
        let _ = parse(r#"<a href="unterminated>text</a><img src='x"#);
        let _ = parse("<<<>>><p <b>x");
        let _ = parse("<!-- comment --><!DOCTYPE html><?php ?>x");
    }

    #[test]
    fn comment_ignored() {
        assert_eq!(parse("<!-- hi --><p>x</p>"), vec![ltr("x")]);
    }

    #[test]
    fn deeply_nested_does_not_overflow() {
        let html = format!("{}x{}", "<b>".repeat(5000), "</b>".repeat(5000));
        let _ = parse(&html);
        let html = format!("{}x", "<div>".repeat(5000));
        let _ = parse(&html);
    }

    #[test]
    fn headings_are_bold_paragraphs() {
        assert_eq!(
            parse("<h1>Title</h1><p>x</p>"),
            vec![
                para(Dir::Ltr, vec![styled("Title", |s| s.bold = true)]),
                ltr("x")
            ]
        );
    }

    #[test]
    fn mixed_message() {
        let rt = parse_teams_html(&format!(
            "{REPLY}<p><b>مرحبا</b> team</p><ul><li>x</li></ul>"
        ));
        assert_eq!(
            plain_text(&rt),
            "> the original message text\nmy reply\nمرحبا team\n- x"
        );
    }
    #[test]
    fn adjacent_mentions_keep_the_space_between_them_in_plain_text() {
        for gap in ["&nbsp;", " ", "\n", "\u{a0}", " \n "] {
            let html = format!(
                r#"<p><span itemtype="http://schema.skype.com/Mention" itemid="0">Ana Test</span>{gap}<span itemtype="http://schema.skype.com/Mention" itemid="1">Bo Test</span> hi</p>"#
            );
            assert_eq!(
                plain_text(&parse_teams_html(&html)),
                "Ana Test Bo Test hi",
                "gap {gap:?}"
            );
        }
    }
}
