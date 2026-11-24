# render rich-text model (`rich` module)

## Overview
Private submodule of the `render` crate (`mod rich;` in `../lib.rs`) holding the composer-oriented document model `RichDoc`, which keeps headings, nested lists, code languages, tables, rules and images, unlike the simpler `RichText` in `../lib.rs`. `decode.rs` parses Teams HTML into `RichDoc` with a scraper tree walker; `encode.rs` serialises `RichDoc` back to the HTML the Teams web composer sends, re-validating every value. `mod.rs` holds the data types, the Markdown entry points and shared validators. The module is pure: no I/O, network or environment access.

## Interface
Re-exported from `[../lib.rs]` at the crate root: `RichDoc`, `RichBlock`, `RichList`, `ListItem`, `RichTable`, `TableCell`, `markdown_to_composer_html`, `markdown_to_rich`, `parse_rich_html`.
- `[mod.rs:RichDoc]` `{ blocks: Vec<RichBlock> }`; `RichDoc::to_html(&self) -> String` (`[encode.rs]`, reached through the re-exported type).
- `[mod.rs:RichBlock]` `Paragraph(Vec<Span>)`, `Heading { level, spans }`, `Quote(Vec<Span>)`, `List(RichList)`, `Code { language, text }`, `Table(RichTable)`, `Rule`, `Image { url, width, height }`.
- `[mod.rs:RichList]` `{ ordered, start, items }`; `[mod.rs:ListItem]` `{ spans, children }`; `[mod.rs:RichTable]` `{ rows }`; `[mod.rs:TableCell]` `{ header, colspan, rowspan, spans }` with `TableCell::new(spans)`.
- `[mod.rs:markdown_to_rich]` `fn(&str) -> RichDoc` (via `crate::markdown_out::markdown_html` then `parse_rich_html`); `[mod.rs:markdown_to_composer_html]` `fn(&str) -> String` (`markdown_to_rich(src).to_html()`).
- `[decode.rs:parse_rich_html]` `fn(&str) -> RichDoc`.
- Everything else (`parse_css_color`, `is_safe_link`, `is_http_url`, `clean_language`, `HIGHLIGHT_PALETTE`, the decode `Walker` and encode helpers) is private to `rich`.

## Invariants
- `[encode.rs]` every value is re-validated and escaped (`compose::escape`) so a hostile model cannot inject markup; colours go through `parse_css_color`, languages through `clean_language`, heading level is clamped to 1..=3.
- `[mod.rs:is_safe_link]` only `http://`, `https://`, `mailto:` and `tel:` links with a non-empty remainder are kept (decode drops others, encode omits the `<a>`); block images are accepted and emitted only for http(s) URLs.
- `[mod.rs:clean_language]` a code language must be lowercase, 1..=32 chars of `[a-z0-9+#._-]` and not `plaintext`/`text`; encode falls back to `plaintext`.
- `[mod.rs:parse_css_color]` accepts `#rgb`, `#rrggbb`, `rgb()` and `rgba()` (alpha must satisfy 0 < a <= 1, then dropped); everything else is rejected; output is lowercase `#rrggbb`.
- `[encode.rs:span_html]` fixed nesting order: link, bold, italic, underline, strike, colour, highlight, font size, code, mention; the 8 `HIGHLIGHT_PALETTE` colours round-trip as `HighlightColor` spans.
- `[encode.rs:block_html]`/`[decode.rs:element p]` a paragraph with no text encodes as `<p>&nbsp;</p>` and decodes back to an empty paragraph so blank lines round-trip.
- `[encode.rs:text_html]` collapsible spaces become `&nbsp;` and `\n` becomes `<br>`; empty lists and tables emit nothing; `<ol start=N>` is written only when `start` is `Some(n)` with n != 1; table header-only leading rows go into `<thead>`; `colspan`/`rowspan` are written only when > 1 and decoded only within 1..=1000.
- `[decode.rs:parse_rich_html]` markup-free input is parsed as plain lines (paragraphs only); recursion stops past depth 256 (`parse::MAX_DEPTH`); `script`, `style`, `head`, `title`, `template`, `iframe`, `object`, `embed`, `noscript`, `svg` and `math` are dropped; only a fixed set of CSS properties is honoured; it is documented to never panic on malformed input.
- `[decode.rs:quote_spans]` a `/reply` blockquote reduces to its `itemprop="preview"` content.

## Side Effects
- None.

## Verification
- Run: `cargo test -p render`

Parent crate contract: [render](../../EGM.md)
