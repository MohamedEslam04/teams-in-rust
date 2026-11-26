//! Teams message HTML -> [`RichDoc`].

use scraper::{ElementRef, Html, Node};

use super::{
    HIGHLIGHT_ITEMTYPE, HIGHLIGHT_PALETTE, ListItem, RichBlock, RichDoc, RichList, RichTable,
    TableCell, clean_language, is_http_url, is_safe_link, parse_css_color,
};
use crate::parse::{
    MAX_DEPTH, attr, attr_lower, collapse_ws, emoticon_text, has_markup, is_emoji_img, non_empty,
    own_mark_free, parse_dim, raw_text, trim_spans,
};
use crate::{FontSize, Span, Style, parse_teams_html};

/// Parse the HTML of a Teams message (or composer content) into a [`RichDoc`].
///
/// Input without any tag is plain text: one paragraph per non-empty line.
/// Scripts, event attributes, unknown styles and unsafe URLs are dropped; only
/// what the model can represent survives. Never panics on malformed input.
pub fn parse_rich_html(html: &str) -> RichDoc {
    if !has_markup(html) {
        let blocks = parse_teams_html(html)
            .blocks
            .into_iter()
            .filter_map(|b| match b {
                crate::Block::Paragraph { spans, .. } => Some(RichBlock::Paragraph(spans)),
                _ => None,
            })
            .collect();
        return RichDoc { blocks };
    }
    let doc = Html::parse_fragment(html);
    let mut w = Walker::default();
    w.walk_children(doc.root_element(), &Ctx::default(), 0);
    w.finish()
}

/// Inherited inline formatting.
#[derive(Clone, Default)]
struct Ctx {
    style: Style,
    link: Option<String>,
    mention: Option<String>,
}

#[derive(Default)]
struct Walker {
    blocks: Vec<RichBlock>,
    /// Inline run being built.
    cur: Vec<Span>,
    /// Collect inline spans only (headings, list items, cells, quotes): block
    /// elements become line breaks instead of new blocks.
    inline: bool,
    /// Inline mode inside `<li>`: nested lists are collected, not flattened.
    collect_lists: bool,
    nested: Vec<RichList>,
}

impl Walker {
    fn inline_walker(collect_lists: bool) -> Self {
        Walker {
            inline: true,
            collect_lists,
            ..Walker::default()
        }
    }

    fn finish(mut self) -> RichDoc {
        self.flush_paragraph();
        RichDoc {
            blocks: self.blocks,
        }
    }

    fn take_spans(&mut self) -> Vec<Span> {
        let mut spans = std::mem::take(&mut self.cur);
        trim_spans(&mut spans);
        spans
    }

    fn flush_paragraph(&mut self) {
        let spans = self.take_spans();
        if !spans.is_empty() {
            self.blocks.push(RichBlock::Paragraph(spans));
        }
    }

    /// Block boundary: new paragraph, or one line break when collecting inline
    /// content.
    fn block_break(&mut self) {
        if self.inline {
            let ends_line = self.cur.last().is_none_or(|s| s.text.ends_with('\n'));
            if !ends_line {
                self.push_newline();
            }
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
        if !self.cur.is_empty() {
            self.push_span("\n".to_owned(), &Ctx::default());
        }
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

    fn element(&mut self, el: ElementRef, parent: &Ctx, depth: usize) {
        let name = el.value().name();
        let mut ctx = parent.clone();
        if let Some(style) = attr(&el, "style") {
            apply_css(&mut ctx, style);
        }
        let styled = |f: fn(&mut Style)| {
            let mut c = ctx.clone();
            f(&mut c.style);
            c
        };
        match name {
            "script" | "style" | "head" | "title" | "template" | "iframe" | "object" | "embed"
            | "noscript" | "svg" | "math" => {}
            "br" => self.push_newline(),
            "hr" => {
                self.block_break();
                if !self.inline {
                    self.blocks.push(RichBlock::Rule);
                }
            }
            "p" => {
                self.block_break();
                let before = self.blocks.len();
                self.walk_children(el, &ctx, depth);
                if !self.inline && self.blocks.len() == before && is_blank_line(&el) {
                    self.cur.clear();
                    self.blocks.push(RichBlock::Paragraph(Vec::new()));
                } else {
                    self.block_break();
                }
            }
            "div" | "section" | "article" | "header" | "footer" | "center" | "figure"
            | "figcaption" | "tbody" | "thead" | "tfoot" | "tr" | "dl" | "dt" | "dd" => {
                self.block_break();
                self.walk_children(el, &ctx, depth);
                self.block_break();
            }
            "table" => {
                if self.inline {
                    self.block_break();
                    self.walk_children(el, &ctx, depth);
                    self.block_break();
                } else {
                    self.flush_paragraph();
                    let rows = table_rows(el, &ctx, depth);
                    if !rows.is_empty() {
                        self.blocks.push(RichBlock::Table(RichTable { rows }));
                    }
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block_break();
                if self.inline {
                    self.walk_children(el, &ctx, depth);
                    self.block_break();
                } else {
                    let spans = inline_spans(el, &ctx, depth);
                    if !spans.is_empty() {
                        let level = (name.as_bytes()[1] - b'0').min(3);
                        self.blocks.push(RichBlock::Heading { level, spans });
                    }
                }
            }
            "blockquote" => {
                if self.inline {
                    self.block_break();
                    self.walk_children(el, &ctx, depth);
                    self.block_break();
                } else {
                    self.flush_paragraph();
                    let spans = quote_spans(el, &ctx, depth);
                    if !spans.is_empty() {
                        self.blocks.push(RichBlock::Quote(spans));
                    }
                }
            }
            "pre" => {
                if self.inline {
                    self.walk_children(el, &styled(|s| s.code = true), depth);
                } else {
                    self.flush_paragraph();
                    let mut text = String::new();
                    raw_text(el, &mut text, 0);
                    if !text.trim().is_empty() {
                        self.blocks.push(RichBlock::Code {
                            language: code_language(el),
                            text,
                        });
                    }
                }
            }
            "ul" | "ol" => {
                if self.collect_lists {
                    let list = parse_list(el, &ctx, depth);
                    if !list.items.is_empty() {
                        self.nested.push(list);
                    }
                } else if self.inline {
                    self.block_break();
                    self.walk_children(el, &ctx, depth);
                    self.block_break();
                } else {
                    self.flush_paragraph();
                    let list = parse_list(el, &ctx, depth);
                    if !list.items.is_empty() {
                        self.blocks.push(RichBlock::List(list));
                    }
                }
            }
            "li" => {
                self.block_break();
                self.walk_children(el, &ctx, depth);
                self.block_break();
            }
            "td" | "th" => {
                self.walk_children(el, &ctx, depth);
                self.push_text(" ", &ctx);
            }
            "img" => self.image(el, &ctx),
            "emoji" => {
                if let Some(text) = emoticon_text(attr(&el, "alt"), attr(&el, "id")) {
                    self.push_span(text, &ctx);
                }
                self.walk_children(el, &ctx, depth);
            }
            "b" | "strong" => self.walk_children(el, &styled(|s| s.bold = true), depth),
            "i" | "em" => self.walk_children(el, &styled(|s| s.italic = true), depth),
            "u" => self.walk_children(el, &styled(|s| s.underline = true), depth),
            "s" | "strike" | "del" => self.walk_children(el, &styled(|s| s.strike = true), depth),
            "code" | "tt" => self.walk_children(el, &styled(|s| s.code = true), depth),
            "a" => {
                let mut c = ctx.clone();
                if let Some(href) = non_empty(attr(&el, "href"))
                    && is_safe_link(&href)
                {
                    c.link = Some(href);
                }
                self.walk_children(el, &c, depth);
            }
            "at" => {
                ctx.mention = Some(attr(&el, "id").unwrap_or("").to_owned());
                self.walk_children(el, &ctx, depth);
            }
            _ => {
                if name == "font"
                    && let Some(color) = attr(&el, "color").and_then(parse_css_color)
                {
                    ctx.style.color = Some(color);
                }
                let itemtype = attr_lower(&el, "itemtype");
                if name == "span" && itemtype.ends_with("/mention") {
                    ctx.mention = Some(attr(&el, "itemid").unwrap_or("").to_owned());
                }
                if name == "span" && itemtype == HIGHLIGHT_ITEMTYPE.to_ascii_lowercase() {
                    let styled_bg = attr(&el, "style").is_some_and(has_valid_background);
                    if !styled_bg
                        && let Some(slug) = attr(&el, "itemid")
                        && let Some((_, hex)) = HIGHLIGHT_PALETTE.iter().find(|(s, _)| *s == slug)
                    {
                        ctx.style.highlight = Some(hex.to_ascii_lowercase());
                    }
                }
                self.walk_children(el, &ctx, depth);
            }
        }
    }

    fn image(&mut self, el: ElementRef, ctx: &Ctx) {
        if is_emoji_img(&el) {
            if let Some(text) = emoticon_text(attr(&el, "alt"), attr(&el, "itemid")) {
                self.push_span(text, ctx);
            }
            return;
        }
        let Some(src) = non_empty(attr(&el, "src")) else {
            return;
        };
        if self.inline || !is_http_url(&src) {
            return;
        }
        self.flush_paragraph();
        self.blocks.push(RichBlock::Image {
            url: src,
            width: parse_dim(attr(&el, "width")),
            height: parse_dim(attr(&el, "height")),
        });
    }
}

/// `<p>` that only holds `&nbsp;` (the composer's blank line).
fn is_blank_line(el: &ElementRef) -> bool {
    let text: String = el.text().collect();
    text.contains('\u{a0}') && text.chars().all(|c| c.is_whitespace())
}

/// Trimmed inline spans of `el`'s content; block children become `'\n'`.
fn inline_spans(el: ElementRef, ctx: &Ctx, depth: usize) -> Vec<Span> {
    let mut w = Walker::inline_walker(false);
    w.walk_children(el, ctx, depth);
    w.take_spans()
}

/// Spans of a `<blockquote>`. A Teams reply quote reduces to its preview.
fn quote_spans(el: ElementRef, ctx: &Ctx, depth: usize) -> Vec<Span> {
    if attr_lower(&el, "itemtype").ends_with("/reply")
        && let Some(preview) = el
            .descendants()
            .filter_map(ElementRef::wrap)
            .find(|e| attr(e, "itemprop") == Some("preview"))
    {
        return inline_spans(preview, ctx, depth);
    }
    inline_spans(el, ctx, depth)
}

/// `language-xyz` from the `<pre>` or its `<code>` class.
fn code_language(pre: ElementRef) -> Option<String> {
    let from = |e: &ElementRef| {
        attr(e, "class")?
            .split_whitespace()
            .find_map(|c| c.strip_prefix("language-"))
            .map(str::to_owned)
    };
    let lang = from(&pre).or_else(|| {
        pre.descendants()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().name() == "code")
            .and_then(|c| from(&c))
    })?;
    clean_language(&lang)
}

fn parse_list(el: ElementRef, ctx: &Ctx, depth: usize) -> RichList {
    let ordered = el.value().name() == "ol";
    let start = ordered
        .then(|| attr(&el, "start")?.trim().parse::<u32>().ok())
        .flatten()
        .filter(|&n| n != 1);
    let mut items: Vec<ListItem> = Vec::new();
    if depth <= MAX_DEPTH {
        for child in el.children().filter_map(ElementRef::wrap) {
            match child.value().name() {
                "li" => {
                    let mut c = ctx.clone();
                    if let Some(style) = attr(&child, "style") {
                        apply_css(&mut c, style);
                    }
                    let mut w = Walker::inline_walker(true);
                    w.walk_children(child, &c, depth + 1);
                    let nested = std::mem::take(&mut w.nested);
                    items.push(ListItem {
                        spans: w.take_spans(),
                        children: nested,
                    });
                }
                // A list placed directly in a list belongs to the previous item.
                "ul" | "ol" => {
                    let nested = parse_list(child, ctx, depth + 1);
                    if !nested.items.is_empty() {
                        match items.last_mut() {
                            Some(last) => last.children.push(nested),
                            None => items.push(ListItem {
                                spans: Vec::new(),
                                children: vec![nested],
                            }),
                        }
                    }
                }
                _ => {}
            }
        }
    }
    RichList {
        ordered,
        start,
        items,
    }
}

fn table_rows(el: ElementRef, ctx: &Ctx, depth: usize) -> Vec<Vec<TableCell>> {
    let mut rows = Vec::new();
    collect_rows(el, ctx, depth, &mut rows);
    rows
}

fn collect_rows(el: ElementRef, ctx: &Ctx, depth: usize, rows: &mut Vec<Vec<TableCell>>) {
    if depth > MAX_DEPTH {
        return;
    }
    for child in el.children().filter_map(ElementRef::wrap) {
        match child.value().name() {
            "thead" | "tbody" | "tfoot" => collect_rows(child, ctx, depth + 1, rows),
            "tr" => {
                let cells: Vec<TableCell> = child
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|c| matches!(c.value().name(), "td" | "th"))
                    .map(|c| {
                        let mut cc = ctx.clone();
                        if let Some(style) = attr(&c, "style") {
                            apply_css(&mut cc, style);
                        }
                        TableCell {
                            header: c.value().name() == "th",
                            colspan: span_attr(&c, "colspan"),
                            rowspan: span_attr(&c, "rowspan"),
                            spans: inline_spans(c, &cc, depth + 1),
                        }
                    })
                    .collect();
                if !cells.is_empty() {
                    rows.push(cells);
                }
            }
            _ => {}
        }
    }
}

fn span_attr(el: &ElementRef, name: &str) -> u32 {
    attr(el, name)
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| (1..=1000).contains(n))
        .unwrap_or(1)
}

fn has_valid_background(style: &str) -> bool {
    css_decls(style).any(|(k, v)| k == "background-color" && parse_css_color(v).is_some())
}

fn css_decls(style: &str) -> impl Iterator<Item = (String, &str)> {
    style.split(';').filter_map(|d| {
        let (k, v) = d.split_once(':')?;
        Some((k.trim().to_ascii_lowercase(), v.trim()))
    })
}

/// Fold the formatting declarations of an inline `style` into `ctx`; anything
/// else (and invalid values) is ignored.
fn apply_css(ctx: &mut Ctx, style: &str) {
    for (name, value) in css_decls(style) {
        let lower = value.to_ascii_lowercase();
        match name.as_str() {
            "color" => {
                if let Some(c) = parse_css_color(value) {
                    ctx.style.color = Some(c);
                }
            }
            "background-color" => {
                if let Some(c) = parse_css_color(value) {
                    ctx.style.highlight = Some(c);
                }
            }
            "font-size" => match lower.as_str() {
                "xx-small" | "x-small" | "small" | "smaller" => {
                    ctx.style.size = Some(FontSize::Small);
                }
                "large" | "x-large" | "xx-large" | "larger" => {
                    ctx.style.size = Some(FontSize::Large);
                }
                _ => {}
            },
            "text-decoration" | "text-decoration-line" => {
                if lower.contains("underline") {
                    ctx.style.underline = true;
                }
                if lower.contains("line-through") {
                    ctx.style.strike = true;
                }
            }
            "font-weight"
                if lower == "bold"
                    || lower == "bolder"
                    || lower.parse::<u32>().is_ok_and(|w| w >= 600) =>
            {
                ctx.style.bold = true;
            }
            "font-style" if lower == "italic" || lower == "oblique" => {
                ctx.style.italic = true;
            }
            _ => {}
        }
    }
}
