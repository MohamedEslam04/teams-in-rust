# render

## Overview
Pure, I/O-free library crate (package `render`, `publish = false`; dependencies `scraper` 0.29 without default features and `unicode-bidi` 0.3) that converts Teams message HTML into toolkit-agnostic rich text and back, and provides the composer's Markdown-style editing helpers. Two document models coexist: the simple `RichText` / `Block` / `Span` model defined in `[src/lib.rs]` (message display), and the composer-oriented `RichDoc` model in the private [src/rich](./src/rich/EGM.md) submodule. `src/lib.rs` declares `pub mod edit` and the private modules `compose`, `links`, `markdown_out`, `parse`, `rich`, and re-exports the public API at the crate root.

## Interface
- Core types (`[src/lib.rs]`): `Dir { Ltr, Rtl }`, `FontSize { Small, Large }`, `Style`, `Span { text, style, link, mention }`, `Block { Paragraph, Quote, List, Code, Image }`, `RichText { blocks }`; `base_direction(&str) -> Dir` (first strong character, `Ltr` if none); `plain_text(&RichText) -> String`.
- Parsing (`[src/parse.rs]`): `parse_teams_html(&str) -> RichText`.
- Composing (`[src/compose.rs]`): `text_to_teams_html(&str)`, `reply_html(...)`, `reply_quote_html(...)`.
- Links (`[src/links.rs]`): `links`, `is_loop_link`, `is_meeting_link`, `loop_title`, `linkify`, `linkify_doc`, `linkify_spans`.
- Markdown (`[src/markdown_out.rs]`): `markdown_to_teams_html(&str) -> String` (the `markdown_html(src, code_language)` variant is `pub(crate)`).
- Composer model (re-exported from `[src/rich]`): `RichDoc`, `RichBlock`, `RichList`, `ListItem`, `RichTable`, `TableCell`, `markdown_to_rich`, `markdown_to_composer_html`, `parse_rich_html`; see [rich](./src/rich/EGM.md).
- Editing (`[src/edit.rs]`, public module `render::edit`, not re-exported at the root): `apply(action, text, anchor, cursor) -> Option<Edit>`, `Edit { text, anchor, cursor }`, `shortcut(key, ctrl, alt, shift) -> Option<&'static str>`, `has_formatting(&str) -> bool`, `History` (`record`, `undo`, `redo`), `Snapshot`, `CODE_LANGUAGES`.
- Crate-internal helpers shared by modules: `compose::escape`, `parse::{MAX_DEPTH, has_markup, attr, collapse_ws, raw_text, ...}`, `crate::spans_text`.

## Invariants
- `[src/compose.rs:escape]` escapes exactly `&`, `<`, `>`, `"`, `'`; every module that emits HTML routes text through it (also `markdown_out` and `rich/encode`), so raw HTML in input is never passed through.
- `[src/compose.rs:text_to_teams_html]` one `<p>` per line; blank lines become `<p>&nbsp;</p>`; one trailing `\r` is stripped per line.
- `[src/parse.rs:parse_teams_html]` never panics on malformed input; markup-free input is parsed as plain lines; recursion stops past `MAX_DEPTH` (256); `script`, `style`, `head`, `title` and `template` are skipped; non-http(s) image sources are dropped; nested lists are flattened; only `/reply` blockquotes yield `message_id`, `author_mri`, `author`.
- `[src/links.rs]` only `http://` / `https://` links are collected; `links()` scans paragraphs and lists only; URL end detection strips trailing punctuation and unbalanced closers; `www.` URLs need at least three host labels and are given an `https://` href; spans that already have a link, mention or code style are not linkified.
- `[src/lib.rs:base_direction]` first strong character decides (L -> `Ltr`, R/AL -> `Rtl`), `Ltr` when none.
- `[src/markdown_out.rs]` block precedence is fenced code, rule, heading (levels 1..=3 only), quote, list, table, paragraph; inline nesting is capped at 16 (`MAX_INLINE_DEPTH`); links must be `http`, `https` or `mailto`; span syntax `{color:#rrggbb}`, `{bg:#rrggbb}`, `{size:large|small}` ... `{/}` is validated before any style is emitted; `__` is always literal.
- `[src/edit.rs:apply]` offsets are byte offsets clamped to the text and snapped to char boundaries; inline markers never cross lines; toggling removes markers when every segment is already wrapped; unknown actions and invalid arguments return `None` (colors must be 6 hex digits, link addresses reject `javascript:`, `data:` and other schemes, tables are limited to 1..=20 rows by 1..=10 columns, code languages use ASCII alphanumerics and `+ # - _`).
- `[src/edit.rs:History]` undo and redo work only while the current text equals the text the history last produced; the undo stack holds at most 100 entries (`HISTORY_LIMIT`).
- `[src/edit.rs]` and `[src/markdown_out.rs]` share one syntax (`{kind:..}...{/}` spans, `[label](url)` links, triple-backtick fences, `- ` and `1. ` lists), so edits round-trip into the HTML renderer.

## Side Effects
- None. All modules are pure functions over strings and in-memory structures; there is no disk, network, environment or global-state access.

## Verification
- Run: `cargo test -p render`

Docs synced to commit 480e5184 ("fix(media): avatar lookups outside the runtime read the cache only; test helper ignores pending entries", 2026-11-22)
