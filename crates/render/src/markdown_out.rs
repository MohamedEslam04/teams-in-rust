//! Markdown-style composer source -> Teams HTML.
//!
//! Block syntax (line based): fenced code, `> ` quotes, `- `/`* ` and `1. `
//! lists (2-space indentation nests), `#`..`###` headings, `---` rules and
//! GFM pipe tables; everything else is a paragraph, one `<p>` per line like
//! [`crate::text_to_teams_html`]. Inline: `**bold**`, `*italic*`/`_italic_`,
//! `++underline++`, `~~strike~~`, `` `code` ``, `[text](http|https|mailto url)`
//! and `{color:#rrggbb}`/`{bg:#rrggbb}`/`{size:large|small}` ... `{/}` spans.
//! All text is escaped; raw HTML in the source is never passed through.

use crate::compose::escape;

/// Nesting limit for inline constructs; deeper ones are emitted as text.
const MAX_INLINE_DEPTH: usize = 16;

/// Convert Markdown-style composer source into the HTML Teams expects.
pub fn markdown_to_teams_html(src: &str) -> String {
    markdown_html(src, false)
}

/// [`markdown_to_teams_html`]; `code_language` also keeps the language after
/// an opening fence as `<pre class="language-xyz">`.
pub(crate) fn markdown_html(src: &str, code_language: bool) -> String {
    let lines: Vec<&str> = src
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut out = String::with_capacity(src.len() + 32);
    // Blank lines only become `<p>&nbsp;</p>` between paragraphs.
    let mut after_paragraph = true;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if after_paragraph {
                out.push_str("<p>&nbsp;</p>");
            }
            i += 1;
            continue;
        }
        after_paragraph = false;
        if trimmed.starts_with("```") {
            i = fenced_code(&lines, i, code_language, &mut out);
        } else if is_rule(trimmed) {
            out.push_str("<hr>");
            i += 1;
        } else if let Some((level, text)) = heading(trimmed) {
            out.push_str(&format!("<h{level}>"));
            inline(text, &mut out);
            out.push_str(&format!("</h{level}>"));
            i += 1;
        } else if line.trim_start().starts_with('>') {
            i = quote(&lines, i, &mut out);
        } else if list_item(line).is_some() {
            i = list(&lines, i, &mut out);
        } else if is_table_start(&lines, i) {
            i = table(&lines, i, &mut out);
        } else {
            out.push_str("<p>");
            inline(line, &mut out);
            out.push_str("</p>");
            after_paragraph = true;
            i += 1;
        }
    }
    out
}

/// Fence at `start` up to the closing fence (or the end); returns the next line.
fn fenced_code(lines: &[&str], start: usize, code_language: bool, out: &mut String) -> usize {
    let mut i = start + 1;
    let mut code = String::new();
    while i < lines.len() && lines[i].trim() != "```" {
        if i > start + 1 {
            code.push('\n');
        }
        code.push_str(lines[i]);
        i += 1;
    }
    // A newline right after `<pre>` is dropped by HTML parsers; keep ours.
    let lead = if code.starts_with('\n') { "\n" } else { "" };
    let language = lines[start].trim().trim_start_matches('`').trim();
    let language_ok = !language.is_empty()
        && language
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '-' | '_'));
    if code_language && language_ok {
        out.push_str(&format!(
            "<pre class=\"language-{}\"><code>",
            language.to_ascii_lowercase()
        ));
    } else {
        out.push_str("<pre><code>");
    }
    out.push_str(lead);
    escape(&code, out);
    out.push_str("</code></pre>");
    i + 1
}

fn is_rule(trimmed: &str) -> bool {
    trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-')
}

fn heading(trimmed: &str) -> Option<(usize, &str)> {
    let level = trimmed.chars().take_while(|&c| c == '#').count();
    let rest = trimmed[level..].strip_prefix(' ')?;
    ((1..=3).contains(&level) && !rest.trim().is_empty()).then_some((level, rest.trim()))
}

fn quote(lines: &[&str], start: usize, out: &mut String) -> usize {
    out.push_str("<blockquote>");
    let mut i = start;
    while i < lines.len() && lines[i].trim_start().starts_with('>') {
        let body = &lines[i].trim_start()[1..];
        let body = body.strip_prefix(' ').unwrap_or(body);
        if body.trim().is_empty() {
            out.push_str("<p>&nbsp;</p>");
        } else {
            out.push_str("<p>");
            inline(body, out);
            out.push_str("</p>");
        }
        i += 1;
    }
    out.push_str("</blockquote>");
    i
}

/// `(indent columns, ordered, text)` for a list item line.
fn list_item(line: &str) -> Option<(usize, bool, &str)> {
    let body = line.trim_start_matches([' ', '\t']);
    let indent: usize = line[..line.len() - body.len()]
        .chars()
        .map(|c| if c == '\t' { 2 } else { 1 })
        .sum();
    if let Some(rest) = body.strip_prefix("- ").or_else(|| body.strip_prefix("* ")) {
        return Some((indent, false, rest.trim()));
    }
    let digits = body.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(rest) = body[digits..].strip_prefix(". ")
    {
        return Some((indent, true, rest.trim()));
    }
    None
}

fn list_tag(ordered: bool) -> &'static str {
    if ordered { "ol" } else { "ul" }
}

fn list(lines: &[&str], start: usize, out: &mut String) -> usize {
    // `open[d]` = ordered flag of the open list at depth d; an `<li>` is open
    // at every level.
    let mut open: Vec<bool> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let Some((indent, ordered, text)) = list_item(lines[i]) else {
            break;
        };
        let depth = (indent / 2).min(open.len());
        while open.len() > depth + 1 {
            let t = list_tag(open.pop().unwrap_or(false));
            out.push_str(&format!("</li></{t}>"));
        }
        if open.len() == depth + 1 {
            if open[depth] == ordered {
                out.push_str("</li>");
            } else {
                out.push_str(&format!("</li></{}>", list_tag(open[depth])));
                out.push_str(&format!("<{}>", list_tag(ordered)));
                open[depth] = ordered;
            }
        } else {
            out.push_str(&format!("<{}>", list_tag(ordered)));
            open.push(ordered);
        }
        out.push_str("<li>");
        inline(text, out);
        i += 1;
    }
    while let Some(ordered) = open.pop() {
        out.push_str(&format!("</li></{}>", list_tag(ordered)));
    }
    i
}

/// Split a table row into trimmed cell sources (an unescaped `|` separates).
fn split_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let mut cells = vec![String::new()];
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        let cell = cells.last_mut().expect("cells is never empty");
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push_str("\\|");
                chars.next();
            }
            '|' => cells.push(String::new()),
            c => cell.push(c),
        }
    }
    if line.starts_with('|') {
        cells.remove(0);
    }
    if line.ends_with('|') && !line.ends_with("\\|") {
        cells.pop();
    }
    cells.iter().map(|c| c.trim().to_owned()).collect()
}

fn is_separator_row(line: &str) -> bool {
    let cells = split_row(line);
    line.contains('|')
        && !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.strip_prefix(':').unwrap_or(c);
            let c = c.strip_suffix(':').unwrap_or(c);
            !c.is_empty() && c.chars().all(|ch| ch == '-')
        })
}

fn is_table_start(lines: &[&str], i: usize) -> bool {
    lines[i].contains('|') && lines.get(i + 1).is_some_and(|l| is_separator_row(l))
}

fn table(lines: &[&str], start: usize, out: &mut String) -> usize {
    fn row(line: &str, tag: &str, out: &mut String) {
        out.push_str("<tr>");
        for cell in split_row(line) {
            out.push_str(&format!("<{tag}>"));
            inline(&cell, out);
            out.push_str(&format!("</{tag}>"));
        }
        out.push_str("</tr>");
    }
    out.push_str("<table><tbody>");
    row(lines[start], "th", out);
    let mut i = start + 2;
    while i < lines.len() && !lines[i].trim().is_empty() && lines[i].contains('|') {
        row(lines[i], "td", out);
        i += 1;
    }
    out.push_str("</tbody></table>");
    i
}

// ---- inline ----

fn inline(src: &str, out: &mut String) {
    let chars: Vec<char> = src.chars().collect();
    render(&chars, 0, chars.len(), 0, true, out);
}

fn lit(ch: char, out: &mut String) {
    escape(ch.encode_utf8(&mut [0; 4]), out);
}

/// Index of the next backtick in `c[from..end]`.
fn next_backtick(c: &[char], from: usize, end: usize) -> Option<usize> {
    (from..end).find(|&k| c[k] == '`')
}

/// Closing delimiter (`ch` repeated `len` times) for an opener at `i`.
fn find_close(c: &[char], i: usize, ch: char, len: usize, end: usize) -> Option<usize> {
    let first = i + len;
    if first >= end || c[first].is_whitespace() {
        return None;
    }
    // `snake_case` and `foo_bar_` are not emphasis.
    if ch == '_' && i > 0 && c[i - 1].is_alphanumeric() {
        return None;
    }
    let mut k = first;
    while k < end {
        if c[k] == '\\' {
            k += 2;
        } else if c[k] == '`' {
            k = next_backtick(c, k + 1, end).map_or(k + 1, |j| j + 1);
        } else if c[k] == ch {
            let run = c[k..end].iter().take_while(|&&x| x == ch).count();
            let closes = if len == 2 { run >= 2 } else { run == 1 };
            let after_ok = ch != '_' || c.get(k + 1).is_none_or(|n| !n.is_alphanumeric());
            if closes && k > first && !c[k - 1].is_whitespace() && after_ok {
                // `**a *b***`: the last two of a longer run close the bold.
                return Some(if len == 2 { k + run - 2 } else { k });
            }
            k += run;
        } else {
            k += 1;
        }
    }
    None
}

fn render(c: &[char], mut i: usize, end: usize, depth: usize, links: bool, out: &mut String) {
    while i < end {
        let ch = c[i];
        let doubled = i + 1 < end && c[i + 1] == ch;
        match ch {
            '\\' if i + 1 < end && c[i + 1].is_ascii_punctuation() => {
                lit(c[i + 1], out);
                i += 2;
                continue;
            }
            '`' => {
                if let Some(j) = next_backtick(c, i + 1, end)
                    && j > i + 1
                {
                    out.push_str("<code>");
                    escape(&c[i + 1..j].iter().collect::<String>(), out);
                    out.push_str("</code>");
                    i = j + 1;
                    continue;
                }
            }
            // `__init__` stays literal; underline is `++`.
            '_' if doubled => {
                out.push_str("__");
                i += 2;
                continue;
            }
            '*' | '~' | '+' | '_' => {
                let (tag, len) = match (ch, doubled) {
                    ('*', true) => ("b", 2),
                    ('*', false) | ('_', false) => ("i", 1),
                    ('+', true) => ("u", 2),
                    ('~', true) => ("s", 2),
                    _ => ("", 1),
                };
                if !tag.is_empty()
                    && depth < MAX_INLINE_DEPTH
                    && let Some(j) = find_close(c, i, ch, len, end)
                {
                    out.push_str(&format!("<{tag}>"));
                    render(c, i + len, j, depth + 1, links, out);
                    out.push_str(&format!("</{tag}>"));
                    i = j + len;
                    continue;
                }
                // Unmatched: literal; a doubled `**` is consumed whole.
                for _ in 0..if tag.is_empty() { 1 } else { len } {
                    lit(ch, out);
                }
                i += if tag.is_empty() { 1 } else { len };
                continue;
            }
            '[' if links && depth < MAX_INLINE_DEPTH => {
                if let Some((text_end, url, next)) = parse_link(c, i, end) {
                    out.push_str("<a href=\"");
                    escape(&url, out);
                    out.push_str("\">");
                    render(c, i + 1, text_end, depth + 1, false, out);
                    out.push_str("</a>");
                    i = next;
                    continue;
                }
            }
            '{' if depth < MAX_INLINE_DEPTH => {
                if let Some((style, open_len)) = span_open(c, i, end)
                    && let Some(close) = span_close(c, i + open_len, end)
                {
                    out.push_str(&format!("<span style=\"{style}\">"));
                    render(c, i + open_len, close, depth + 1, links, out);
                    out.push_str("</span>");
                    i = close + 3;
                    continue;
                }
            }
            _ => {}
        }
        lit(ch, out);
        i += 1;
    }
}

/// `[text](url)` at `i`: `(index of ']', url, index after ')')`. Only
/// http/https/mailto URLs qualify.
fn parse_link(c: &[char], i: usize, end: usize) -> Option<(usize, String, usize)> {
    let mut depth = 0usize;
    let mut k = i;
    let text_end = loop {
        if k >= end {
            return None;
        }
        match c[k] {
            '\\' => k += 1,
            '`' => k = next_backtick(c, k + 1, end).unwrap_or(k),
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break k;
                }
            }
            _ => {}
        }
        k += 1;
    };
    if text_end == i + 1 || text_end + 1 >= end || c[text_end + 1] != '(' {
        return None;
    }
    let mut parens = 1usize;
    let mut k = text_end + 2;
    while k < end {
        match c[k] {
            '(' => parens += 1,
            ')' => {
                parens -= 1;
                if parens == 0 {
                    break;
                }
            }
            ch if ch.is_whitespace() || ch.is_control() => return None,
            _ => {}
        }
        k += 1;
    }
    if k >= end {
        return None;
    }
    let url: String = c[text_end + 2..k].iter().collect();
    let lower = url.to_ascii_lowercase();
    let ok = ["http://", "https://", "mailto:"]
        .iter()
        .any(|p| lower.starts_with(p) && lower.len() > p.len());
    ok.then_some((text_end, url, k + 1))
}

/// A valid `{color:#rrggbb}` / `{bg:#rrggbb}` / `{size:large|small}` opener at
/// `i`: `(CSS declaration, opener length)`.
fn span_open(c: &[char], i: usize, end: usize) -> Option<(String, usize)> {
    let close = (i..end.min(i + 16)).find(|&k| c[k] == '}')?;
    let body: String = c[i + 1..close].iter().collect();
    let hex = |v: &str| {
        let h = v.strip_prefix('#')?;
        (h.len() == 6 && h.chars().all(|x| x.is_ascii_hexdigit()))
            .then(|| format!("#{}", h.to_ascii_lowercase()))
    };
    let style = match body.split_once(':')? {
        ("color", v) => format!("color:{}", hex(v)?),
        ("bg", v) => format!("background-color:{}", hex(v)?),
        ("size", v @ ("large" | "small")) => format!("font-size:{v}"),
        _ => return None,
    };
    Some((style, close + 1 - i))
}

/// Index of the `{/}` closing a span whose content starts at `from`, skipping
/// nested spans.
fn span_close(c: &[char], from: usize, end: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut k = from;
    while k < end {
        if c[k] == '\\' {
            k += 2;
            continue;
        }
        if c[k] == '{' {
            if c[k..end].starts_with(&['{', '/', '}']) {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
                k += 3;
                continue;
            }
            if let Some((_, len)) = span_open(c, k, end) {
                depth += 1;
                k += len;
                continue;
            }
        }
        k += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, Dir, Span, Style, parse_teams_html, plain_text};

    fn md(s: &str) -> String {
        markdown_to_teams_html(s)
    }

    fn style(f: impl Fn(&mut Style)) -> Style {
        let mut s = Style::default();
        f(&mut s);
        s
    }

    fn span(text: &str, style: Style) -> Span {
        Span {
            text: text.into(),
            style,
            link: None,
            mention: None,
        }
    }

    fn para(spans: Vec<Span>) -> Block {
        Block::Paragraph {
            dir: Dir::Ltr,
            spans,
        }
    }

    fn blocks(s: &str) -> Vec<Block> {
        parse_teams_html(&md(s)).blocks
    }

    // ---- inline HTML ----

    #[test]
    fn plain_matches_text_path() {
        assert_eq!(md("hello"), "<p>hello</p>");
        assert_eq!(md("one\n\ntwo"), "<p>one</p><p>&nbsp;</p><p>two</p>");
        assert_eq!(md(""), "<p>&nbsp;</p>");
        assert_eq!(md("a\r\nb"), "<p>a</p><p>b</p>");
    }

    #[test]
    fn bold_italic_underline_strike_code() {
        assert_eq!(md("**b**"), "<p><b>b</b></p>");
        assert_eq!(md("*i*"), "<p><i>i</i></p>");
        assert_eq!(md("_i_"), "<p><i>i</i></p>");
        assert_eq!(md("++u++"), "<p><u>u</u></p>");
        assert_eq!(md("~~s~~"), "<p><s>s</s></p>");
        assert_eq!(md("`c`"), "<p><code>c</code></p>");
    }

    #[test]
    fn nested_and_adjacent_inline() {
        assert_eq!(
            md("**bold *and italic* text** and ~~gone~~"),
            "<p><b>bold <i>and italic</i> text</b> and <s>gone</s></p>"
        );
    }

    #[test]
    fn code_span_is_verbatim_and_escaped() {
        assert_eq!(md("`a **b** <i>`"), "<p><code>a **b** &lt;i&gt;</code></p>");
    }

    #[test]
    fn unmatched_and_spaced_delimiters_stay_text() {
        assert_eq!(md("**oops"), "<p>**oops</p>");
        assert_eq!(md("2 * 3 * 4"), "<p>2 * 3 * 4</p>");
        assert_eq!(md("snake_case_name"), "<p>snake_case_name</p>");
        assert_eq!(md("__init__"), "<p>__init__</p>");
        assert_eq!(md("a ~ b"), "<p>a ~ b</p>");
        assert_eq!(md("`open"), "<p>`open</p>");
    }

    #[test]
    fn backslash_escapes() {
        assert_eq!(md(r"\*not italic\*"), "<p>*not italic*</p>");
        assert_eq!(md(r"a\b"), r"<p>a\b</p>");
    }

    #[test]
    fn raw_html_is_escaped() {
        assert_eq!(
            md(r#"<script>alert("x")</script> & 'q'"#),
            "<p>&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;q&#39;</p>"
        );
        assert_eq!(
            md("**<img src=x onerror=1>**"),
            "<p><b>&lt;img src=x onerror=1&gt;</b></p>"
        );
    }

    // ---- links ----

    #[test]
    fn links() {
        assert_eq!(
            md("[Rust](https://rust-lang.org/a?b=1&c=2)"),
            r#"<p><a href="https://rust-lang.org/a?b=1&amp;c=2">Rust</a></p>"#
        );
        assert_eq!(
            md("[mail](mailto:a@b.c) and [web](HTTP://x.y)"),
            r#"<p><a href="mailto:a@b.c">mail</a> and <a href="HTTP://x.y">web</a></p>"#
        );
        assert_eq!(
            md("[**b**](https://x.y)"),
            r#"<p><a href="https://x.y"><b>b</b></a></p>"#
        );
    }

    #[test]
    fn unsafe_or_malformed_links_stay_text() {
        assert_eq!(
            md("[x](javascript:alert(1))"),
            "<p>[x](javascript:alert(1))</p>"
        );
        assert_eq!(md("[x](ftp://h)"), "<p>[x](ftp://h)</p>");
        assert_eq!(md("[x](https://a b)"), "<p>[x](https://a b)</p>");
        assert_eq!(md("[x](https://a"), "<p>[x](https://a</p>");
        assert_eq!(md("[](https://a)"), "<p>[](https://a)</p>");
        assert_eq!(md("[x]"), "<p>[x]</p>");
        assert_eq!(
            md(r#"[x](https://a"onmouseover="1)"#),
            r#"<p><a href="https://a&quot;onmouseover=&quot;1">x</a></p>"#
        );
    }

    // ---- span styles ----

    #[test]
    fn span_styles() {
        assert_eq!(
            md("{color:#FF0000}red{/} {bg:#00ff00}hl{/} {size:large}big{/} {size:small}sm{/}"),
            concat!(
                r#"<p><span style="color:#ff0000">red</span> "#,
                r#"<span style="background-color:#00ff00">hl</span> "#,
                r#"<span style="font-size:large">big</span> "#,
                r#"<span style="font-size:small">sm</span></p>"#
            )
        );
        assert_eq!(
            md("{color:#112233}a **b** {bg:#445566}c{/}{/}"),
            concat!(
                r#"<p><span style="color:#112233">a <b>b</b> "#,
                r#"<span style="background-color:#445566">c</span></span></p>"#
            )
        );
    }

    #[test]
    fn invalid_span_styles_stay_text() {
        for bad in [
            "{color:red}x{/}",
            "{color:#12345}x{/}",
            "{color:#12345g}x{/}",
            "{color:#ff0000;position:fixed}x{/}",
            "{size:huge}x{/}",
            "{size:20px}x{/}",
            "{bg:url(x)}x{/}",
            "{color:#ff0000}unclosed",
            "{/}",
        ] {
            let html = md(bad);
            assert!(!html.contains("<span"), "{bad} -> {html}");
        }
        assert_eq!(md("{color:red}x{/}"), "<p>{color:red}x{/}</p>");
    }

    // ---- blocks HTML ----

    #[test]
    fn headings_and_rule() {
        assert_eq!(
            md("# One\n## Two **b**\n### Three\n#### Four\n#nospace"),
            "<h1>One</h1><h2>Two <b>b</b></h2><h3>Three</h3><p>#### Four</p><p>#nospace</p>"
        );
        assert_eq!(md("a\n---\nb"), "<p>a</p><hr><p>b</p>");
    }

    #[test]
    fn quote_block() {
        assert_eq!(
            md("> one\n> two **b**\n>\nafter"),
            "<blockquote><p>one</p><p>two <b>b</b></p><p>&nbsp;</p></blockquote><p>after</p>"
        );
    }

    #[test]
    fn fenced_code_block() {
        assert_eq!(
            md("```rust\nfn a() { <b>&</b> }\n  **x**\n```\nafter"),
            "<pre><code>fn a() { &lt;b&gt;&amp;&lt;/b&gt; }\n  **x**</code></pre><p>after</p>"
        );
        // Unclosed fence runs to the end; blank first line is preserved.
        assert_eq!(md("```\n\nx"), "<pre><code>\n\nx</code></pre>");
    }

    #[test]
    fn lists() {
        assert_eq!(
            md("- a\n* b **c**\n\ntext"),
            "<ul><li>a</li><li>b <b>c</b></li></ul><p>text</p>"
        );
        assert_eq!(md("1. one\n2. two"), "<ol><li>one</li><li>two</li></ol>");
    }

    #[test]
    fn nested_lists() {
        assert_eq!(
            md("- a\n  - b\n    1. c\n  - d\n- e"),
            concat!(
                "<ul><li>a<ul><li>b<ol><li>c</li></ol></li><li>d</li></ul></li>",
                "<li>e</li></ul>"
            )
        );
        // Over-indented first item and a marker switch at the same level.
        assert_eq!(md("    - a"), "<ul><li>a</li></ul>");
        assert_eq!(md("- a\n1. b"), "<ul><li>a</li></ul><ol><li>b</li></ol>");
    }

    #[test]
    fn tables() {
        assert_eq!(
            md("| A | B |\n|---|:-:|\n| 1 | **2** |\n| x \\| y | `z` |\nafter"),
            concat!(
                "<table><tbody><tr><th>A</th><th>B</th></tr>",
                "<tr><td>1</td><td><b>2</b></td></tr>",
                "<tr><td>x | y</td><td><code>z</code></td></tr></tbody></table><p>after</p>"
            )
        );
        // Without the separator row it is plain text.
        assert_eq!(md("| a | b |"), "<p>| a | b |</p>");
    }

    // ---- round trip through parse_teams_html ----

    #[test]
    fn roundtrip_inline_styles() {
        assert_eq!(
            blocks("**b** *i* ++u++ ~~s~~ `c` plain"),
            vec![para(vec![
                span("b", style(|s| s.bold = true)),
                span(" ", Style::default()),
                span("i", style(|s| s.italic = true)),
                span(" ", Style::default()),
                span("u", style(|s| s.underline = true)),
                span(" ", Style::default()),
                span("s", style(|s| s.strike = true)),
                span(" ", Style::default()),
                span("c", style(|s| s.code = true)),
                span(" plain", Style::default()),
            ])]
        );
    }

    #[test]
    fn roundtrip_nested_style() {
        assert_eq!(
            blocks("**a *b***"),
            vec![para(vec![
                span("a ", style(|s| s.bold = true)),
                span(
                    "b",
                    style(|s| {
                        s.bold = true;
                        s.italic = true;
                    })
                ),
            ])]
        );
    }

    #[test]
    fn roundtrip_link() {
        assert_eq!(
            blocks("see [docs](https://x.y/a?b=1&c=2)"),
            vec![para(vec![
                span("see ", Style::default()),
                Span {
                    link: Some("https://x.y/a?b=1&c=2".into()),
                    ..span("docs", Style::default())
                },
            ])]
        );
    }

    #[test]
    fn roundtrip_span_styles_keep_text() {
        assert_eq!(
            blocks("{color:#ff0000}red{/} and {bg:#00ff00}hl{/}"),
            vec![para(vec![span("red and hl", Style::default())])]
        );
    }

    #[test]
    fn roundtrip_headings_are_bold_paragraphs() {
        assert_eq!(
            blocks("# Title\nbody"),
            vec![
                para(vec![span("Title", style(|s| s.bold = true))]),
                para(vec![span("body", Style::default())]),
            ]
        );
    }

    #[test]
    fn roundtrip_code_block_and_hr() {
        assert_eq!(
            blocks("a\n---\n```\nlet x = 1 < 2;\n  y\n```\nb"),
            vec![
                para(vec![span("a", Style::default())]),
                Block::Code {
                    text: "let x = 1 < 2;\n  y".into()
                },
                para(vec![span("b", Style::default())]),
            ]
        );
    }

    #[test]
    fn roundtrip_quote() {
        let b = blocks("> hello **world**\n> line 2\nafter");
        assert_eq!(
            b,
            vec![
                Block::Quote {
                    message_id: None,
                    author_mri: None,
                    author: None,
                    time_ms: None,
                    preview: "hello world line 2".into(),
                    dir: Dir::Ltr,
                },
                para(vec![span("after", Style::default())]),
            ]
        );
    }

    #[test]
    fn roundtrip_lists() {
        assert_eq!(
            blocks("- a\n  - b\n- c"),
            vec![Block::List {
                ordered: false,
                items: vec![
                    vec![span("a", Style::default())],
                    vec![span("b", Style::default())],
                    vec![span("c", Style::default())],
                ],
            }]
        );
        assert_eq!(
            blocks("1. **x**\n2. y"),
            vec![Block::List {
                ordered: true,
                items: vec![
                    vec![span("x", style(|s| s.bold = true))],
                    vec![span("y", Style::default())],
                ],
            }]
        );
    }

    #[test]
    fn roundtrip_table_rows_become_lines() {
        let rt = parse_teams_html(&md("| A | B |\n|---|---|\n| 1 | 2 |\nafter"));
        assert_eq!(plain_text(&rt), "A B\n1 2\nafter");
    }

    #[test]
    fn roundtrip_blank_line_and_rtl() {
        let rt = parse_teams_html(&md("مرحبا **بالعالم**\n\nhello"));
        assert_eq!(rt.blocks.len(), 2);
        assert!(matches!(
            rt.blocks[0],
            Block::Paragraph { dir: Dir::Rtl, .. }
        ));
        assert_eq!(plain_text(&rt), "مرحبا بالعالم\nhello");
    }

    #[test]
    fn roundtrip_hostile_text_survives_verbatim() {
        let src = r#"<b onclick="x">hi</b> & <script>1</script>"#;
        assert_eq!(plain_text(&parse_teams_html(&md(src))), src);
    }

    #[test]
    fn pathological_input_does_not_panic() {
        for s in [
            "[[[[[[[[",
            "]]]]((((",
            "**".repeat(500).as_str(),
            "{color:#ffffff}".repeat(100).as_str(),
            "[a](https://x)".repeat(50).as_str(),
            "- ".repeat(50).as_str(),
            "|\n|-|\n|",
            "\\",
            "{",
            "```",
        ] {
            let _ = md(s);
        }
        let deep = format!("{}x{}", "*a ".repeat(40), " a*".repeat(40));
        let _ = md(&deep);
    }
}
