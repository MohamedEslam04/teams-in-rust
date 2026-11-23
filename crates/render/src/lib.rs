//! Teams HTML <-> toolkit-agnostic rich text.
//!
//! [`parse_teams_html`] turns the HTML Teams sends as message content into a
//! [`RichText`] a UI can render; [`text_to_teams_html`] and [`reply_html`] go
//! the other way for the composer.

mod compose;
pub mod edit;
mod links;
mod markdown_out;
mod parse;
mod rich;

pub use compose::{reply_html, reply_quote_html, text_to_teams_html};
pub use links::{
    is_loop_link, is_meeting_link, linkify, linkify_doc, linkify_spans, links, loop_title,
};
pub use markdown_out::markdown_to_teams_html;
pub use parse::parse_teams_html;
pub use rich::{
    ListItem, RichBlock, RichDoc, RichList, RichTable, TableCell, markdown_to_composer_html,
    markdown_to_rich, parse_rich_html,
};

use unicode_bidi::BidiClass;

/// Base text direction of a paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Ltr,
    Rtl,
}

/// Teams composer font sizes (`xx-small` / `x-large`; normal is `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSize {
    Small,
    Large,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    /// Text color as lowercase `#rrggbb`. Only set by [`parse_rich_html`].
    pub color: Option<String>,
    /// Highlight (background) color as lowercase `#rrggbb`. Only set by
    /// [`parse_rich_html`].
    pub highlight: Option<String>,
    /// Only set by [`parse_rich_html`].
    pub size: Option<FontSize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// May contain `'\n'` for `<br>` line breaks.
    pub text: String,
    pub style: Style,
    pub link: Option<String>,
    /// `Some(itemid)` when this span is a mention (UI highlights it).
    pub mention: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph {
        dir: Dir,
        spans: Vec<Span>,
    },
    /// Teams reply quote. A message can carry several, one per quoted message.
    Quote {
        message_id: Option<String>,
        author_mri: Option<String>,
        author: Option<String>,
        /// When the quoted message was sent (epoch ms), from the `time` element.
        time_ms: Option<i64>,
        /// The quoted text; a line break of the original is a `'\n'`.
        preview: String,
        dir: Dir,
    },
    List {
        ordered: bool,
        items: Vec<Vec<Span>>,
    },
    Code {
        text: String,
    },
    /// AMS (or any other http/https) image.
    Image {
        url: String,
        width: Option<u32>,
        height: Option<u32>,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RichText {
    pub blocks: Vec<Block>,
}

/// Direction of the first strong character (UAX#9 rule P2); `Ltr` when there
/// is none.
pub fn base_direction(text: &str) -> Dir {
    for c in text.chars() {
        match unicode_bidi::bidi_class(c) {
            BidiClass::L => return Dir::Ltr,
            BidiClass::R | BidiClass::AL => return Dir::Rtl,
            _ => {}
        }
    }
    Dir::Ltr
}

fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// Single string for chat-list previews and notifications: blocks joined by
/// `"\n"`, quotes as `"> preview"`, images as `"[image]"`, list items as
/// `"- item"` / `"1. item"`.
pub fn plain_text(rt: &RichText) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(rt.blocks.len());
    for block in &rt.blocks {
        match block {
            Block::Paragraph { spans, .. } => lines.push(spans_text(spans)),
            Block::Quote { preview, .. } => {
                lines.push(format!("> {}", preview.replace('\n', " ")));
            }
            Block::List { ordered, items } => {
                for (i, item) in items.iter().enumerate() {
                    let text = spans_text(item);
                    lines.push(if *ordered {
                        format!("{}. {text}", i + 1)
                    } else {
                        format!("- {text}")
                    });
                }
            }
            Block::Code { text } => lines.push(text.clone()),
            Block::Image { .. } => lines.push("[image]".to_owned()),
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str) -> Span {
        Span {
            text: text.into(),
            style: Style::default(),
            link: None,
            mention: None,
        }
    }

    #[test]
    fn direction_arabic_first() {
        assert_eq!(base_direction("تمام 👍 هراجع الـ PR"), Dir::Rtl);
    }

    #[test]
    fn direction_english_first() {
        assert_eq!(base_direction("Quick question: هل الـ endpoint"), Dir::Ltr);
    }

    #[test]
    fn direction_skips_neutrals_and_digits() {
        assert_eq!(base_direction("123 !? 👍 مرحبا"), Dir::Rtl);
        assert_eq!(base_direction("123 !? hello"), Dir::Ltr);
    }

    #[test]
    fn direction_no_strong_is_ltr() {
        assert_eq!(base_direction(""), Dir::Ltr);
        assert_eq!(base_direction("123 !? 👍"), Dir::Ltr);
    }

    #[test]
    fn direction_other_rtl_scripts() {
        assert_eq!(base_direction("שלום"), Dir::Rtl); // Hebrew
        assert_eq!(base_direction("ܫܠܡܐ"), Dir::Rtl); // Syriac
        assert_eq!(base_direction("ހަށަ"), Dir::Rtl); // Thaana
        assert_eq!(base_direction("\u{FE8D}\u{FEE3}"), Dir::Rtl); // Arabic presentation forms-B
        assert_eq!(base_direction("\u{FB50}"), Dir::Rtl); // Arabic presentation forms-A
    }

    #[test]
    fn plain_text_all_blocks() {
        let rt = RichText {
            blocks: vec![
                Block::Quote {
                    message_id: None,
                    author_mri: None,
                    author: None,
                    time_ms: None,
                    preview: "orig".into(),
                    dir: Dir::Ltr,
                },
                Block::Paragraph {
                    dir: Dir::Ltr,
                    spans: vec![span("hello "), span("world")],
                },
                Block::Image {
                    url: "https://x/y".into(),
                    width: None,
                    height: None,
                },
                Block::Code {
                    text: "a\nb".into(),
                },
                Block::List {
                    ordered: true,
                    items: vec![vec![span("one")], vec![span("two")]],
                },
                Block::List {
                    ordered: false,
                    items: vec![vec![span("x")]],
                },
            ],
        };
        assert_eq!(
            plain_text(&rt),
            "> orig\nhello world\n[image]\na\nb\n1. one\n2. two\n- x"
        );
    }

    #[test]
    fn plain_text_empty() {
        assert_eq!(plain_text(&RichText::default()), "");
    }
}
