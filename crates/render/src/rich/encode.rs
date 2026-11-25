//! [`RichDoc`] -> the HTML the Teams web composer sends.
//!
//! Tags follow CKEditor 5 as configured by Teams (`strong`, `i`, `u`, `s`,
//! `code`, `h1`..`h3`, `figure.table > table.copy-paste-table`, code blocks as
//! `pre.language-xyz.skipProofing`); lists, quotes and the color/highlight
//! spans follow rusty-teams (MIT, Daniel Daeschle) which verified them against
//! Teams. Every model value is re-validated, so a hostile model cannot inject
//! markup.

use super::{
    HIGHLIGHT_ITEMTYPE, HIGHLIGHT_PALETTE, ListItem, RichBlock, RichDoc, RichList, RichTable,
    TableCell, clean_language, is_http_url, is_safe_link, parse_css_color,
};
use crate::compose::{dir_attr, escape, with_rtl_mark};
use crate::{Dir, FontSize, Span, spans_text, text_direction};

impl RichDoc {
    /// Teams message HTML for this document. When a block is right-to-left,
    /// every paragraph, heading, quote and list item with a strong character
    /// says its own direction (`dir="rtl"` or `dir="ltr"`); a mostly right-to-left message that
    /// does not start right-to-left also gets a leading U+200F for Teams mobile
    /// (see [`with_rtl_mark`]).
    pub fn to_html(&self) -> String {
        let mixed = self.blocks.iter().any(block_is_rtl);
        let mut out = String::new();
        for block in &self.blocks {
            block_html(block, mixed, &mut out);
        }
        with_rtl_mark(out)
    }
}

fn spans_dir(spans: &[Span]) -> Option<Dir> {
    text_direction(&spans_text(spans))
}

fn block_is_rtl(block: &RichBlock) -> bool {
    fn list_is_rtl(list: &RichList) -> bool {
        list.items.iter().any(|item| {
            spans_dir(&item.spans) == Some(Dir::Rtl) || item.children.iter().any(list_is_rtl)
        })
    }
    match block {
        RichBlock::Paragraph(spans)
        | RichBlock::Heading { spans, .. }
        | RichBlock::Quote(spans) => spans_dir(spans) == Some(Dir::Rtl),
        RichBlock::List(list) => list_is_rtl(list),
        _ => false,
    }
}

fn block_html(block: &RichBlock, mixed: bool, out: &mut String) {
    match block {
        RichBlock::Paragraph(spans) => {
            out.push_str("<p");
            out.push_str(dir_attr(spans_dir(spans), mixed));
            out.push('>');
            if !spans_html(spans, out) {
                out.push_str("&nbsp;");
            }
            out.push_str("</p>");
        }
        RichBlock::Heading { level, spans } => {
            let level = (*level).clamp(1, 3);
            out.push_str(&format!("<h{level}{}>", dir_attr(spans_dir(spans), mixed)));
            spans_html(spans, out);
            out.push_str(&format!("</h{level}>"));
        }
        RichBlock::Quote(spans) => {
            out.push_str(&format!(
                "<blockquote{}>",
                dir_attr(spans_dir(spans), mixed)
            ));
            spans_html(spans, out);
            out.push_str("</blockquote>");
        }
        RichBlock::List(list) => list_html(list, mixed, out),
        RichBlock::Code { language, text } => {
            let language = language
                .as_deref()
                .and_then(clean_language)
                .unwrap_or_else(|| "plaintext".to_owned());
            out.push_str(&format!(
                "<pre class=\"language-{language} skipProofing\" spellcheck=\"false\"><code>"
            ));
            let text = text.replace('\r', "");
            let mut first = true;
            for line in text.split('\n') {
                if !first {
                    out.push_str("<br>");
                }
                first = false;
                escape(line, out);
            }
            out.push_str("</code></pre>");
        }
        RichBlock::Table(table) => table_html(table, out),
        RichBlock::Rule => out.push_str("<hr>"),
        RichBlock::Image { url, width, height } => {
            if !is_http_url(url) {
                return;
            }
            out.push_str("<p><img src=\"");
            escape(url.trim(), out);
            out.push('"');
            if let Some(w) = width {
                out.push_str(&format!(" width=\"{w}\""));
            }
            if let Some(h) = height {
                out.push_str(&format!(" height=\"{h}\""));
            }
            out.push_str("></p>");
        }
    }
}

fn list_html(list: &RichList, mixed: bool, out: &mut String) {
    if list.items.is_empty() {
        return;
    }
    match (list.ordered, list.start) {
        (false, _) => out.push_str("<ul>"),
        (true, Some(n)) if n != 1 => out.push_str(&format!("<ol start=\"{n}\">")),
        (true, _) => out.push_str("<ol>"),
    }
    for ListItem { spans, children } in &list.items {
        out.push_str("<li");
        out.push_str(dir_attr(spans_dir(spans), mixed));
        out.push('>');
        spans_html(spans, out);
        for child in children {
            list_html(child, mixed, out);
        }
        out.push_str("</li>");
    }
    out.push_str(if list.ordered { "</ol>" } else { "</ul>" });
}

fn table_html(table: &RichTable, out: &mut String) {
    if table.rows.is_empty() {
        return;
    }
    let is_header = |row: &Vec<TableCell>| !row.is_empty() && row.iter().all(|c| c.header);
    let head = table.rows.iter().take_while(|r| is_header(r)).count();
    out.push_str("<figure class=\"table\"><table class=\"copy-paste-table\">");
    if head > 0 {
        out.push_str("<thead>");
        table.rows[..head].iter().for_each(|r| row_html(r, out));
        out.push_str("</thead>");
    }
    if head < table.rows.len() {
        out.push_str("<tbody>");
        table.rows[head..].iter().for_each(|r| row_html(r, out));
        out.push_str("</tbody>");
    }
    out.push_str("</table></figure>");
}

fn row_html(row: &[TableCell], out: &mut String) {
    out.push_str("<tr>");
    for cell in row {
        let tag = if cell.header { "th" } else { "td" };
        out.push_str(&format!("<{tag}"));
        if cell.colspan > 1 {
            out.push_str(&format!(" colspan=\"{}\"", cell.colspan));
        }
        if cell.rowspan > 1 {
            out.push_str(&format!(" rowspan=\"{}\"", cell.rowspan));
        }
        out.push('>');
        let lines = split_lines(&cell.spans);
        if lines.len() <= 1 {
            // One line: no `<p>`; empty: `&nbsp;`.
            let line = lines.first().map_or(&[][..], Vec::as_slice);
            if !spans_html(line, out) {
                out.push_str("&nbsp;");
            }
        } else {
            for line in &lines {
                out.push_str("<p>");
                if !spans_html(line, out) {
                    out.push_str("&nbsp;");
                }
                out.push_str("</p>");
            }
        }
        out.push_str(&format!("</{tag}>"));
    }
    out.push_str("</tr>");
}

/// Split spans at `'\n'` into lines (cell content is one `<p>` per line).
fn split_lines(spans: &[Span]) -> Vec<Vec<Span>> {
    let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
    for span in spans {
        let mut parts = span.text.split('\n');
        if let Some(first) = parts.next() {
            push_part(&mut lines, span, first);
        }
        for part in parts {
            lines.push(Vec::new());
            push_part(&mut lines, span, part);
        }
    }
    if lines.iter().all(Vec::is_empty) {
        lines.truncate(1);
    }
    lines
}

fn push_part(lines: &mut [Vec<Span>], span: &Span, text: &str) {
    if !text.is_empty()
        && let Some(line) = lines.last_mut()
    {
        line.push(Span {
            text: text.to_owned(),
            ..span.clone()
        });
    }
}

/// Inline run; `'\n'` is `<br>`. Returns false when nothing was written.
fn spans_html(spans: &[Span], out: &mut String) -> bool {
    let start = out.len();
    // Spaces that HTML would collapse are written as `&nbsp;`.
    let mut prev_space = true;
    for span in spans.iter().filter(|s| !s.text.is_empty()) {
        span_html(span, &mut prev_space, out);
    }
    out.len() > start
}

fn span_html(span: &Span, prev_space: &mut bool, out: &mut String) {
    let mut closers: Vec<&'static str> = Vec::new();
    if let Some(url) = span.link.as_deref().filter(|u| is_safe_link(u)) {
        out.push_str("<a href=\"");
        escape(url.trim(), out);
        out.push_str("\">");
        closers.push("</a>");
    }
    let st = &span.style;
    for (on, open, close) in [
        (st.bold, "<strong>", "</strong>"),
        (st.italic, "<i>", "</i>"),
        (st.underline, "<u>", "</u>"),
        (st.strike, "<s>", "</s>"),
    ] {
        if on {
            out.push_str(open);
            closers.push(close);
        }
    }
    if let Some(color) = st.color.as_deref().and_then(parse_css_color) {
        out.push_str(&format!("<span style=\"color:{color};\">"));
        closers.push("</span>");
    }
    if let Some(color) = st.highlight.as_deref().and_then(parse_css_color) {
        match HIGHLIGHT_PALETTE
            .iter()
            .find(|(_, hex)| hex.eq_ignore_ascii_case(&color))
        {
            Some((slug, hex)) => out.push_str(&format!(
                "<span itemtype=\"{HIGHLIGHT_ITEMTYPE}\" itemid=\"{slug}\" style=\"background-color:{hex}\">"
            )),
            None => out.push_str(&format!("<span style=\"background-color:{color};\">")),
        }
        closers.push("</span>");
    }
    if let Some(size) = st.size {
        let css = match size {
            FontSize::Small => "xx-small",
            FontSize::Large => "x-large",
        };
        out.push_str(&format!("<span style=\"font-size:{css};\">"));
        closers.push("</span>");
    }
    if st.code {
        out.push_str("<code>");
        closers.push("</code>");
    }
    if let Some(id) = &span.mention {
        out.push_str("<span itemscope itemtype=\"http://schema.skype.com/Mention\" itemid=\"");
        escape(id, out);
        out.push_str("\">");
        closers.push("</span>");
    }
    text_html(&span.text, prev_space, out);
    for close in closers.into_iter().rev() {
        out.push_str(close);
    }
}

fn text_html(text: &str, prev_space: &mut bool, out: &mut String) {
    for c in text.chars() {
        match c {
            '\n' => {
                out.push_str("<br>");
                *prev_space = true;
            }
            ' ' if *prev_space => out.push_str("&nbsp;"),
            ' ' => {
                out.push(' ');
                *prev_space = true;
            }
            c => {
                escape(c.encode_utf8(&mut [0; 4]), out);
                *prev_space = false;
            }
        }
    }
}
