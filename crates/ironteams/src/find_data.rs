//! Plain `Send` data for the Search page, the quick views, Find in chat and the Shared tab, plus
//! the text helpers they share (query terms and snippet highlighting).

use crate::data::Pixels;

/// One row of the Search page or a quick view.
#[derive(Clone)]
pub struct HitData {
    pub id: String,
    /// `message`, `file`, `person`, `channel`, `chat`, `meeting`, `thread` or `unread-group`.
    pub kind: &'static str,
    /// Non-empty on the first row of a section of the All tab ("Messages", "Files", …).
    pub section: String,
    pub title: String,
    pub subtitle: String,
    /// Markdown: the matching words are bold.
    pub snippet: String,
    pub time: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    /// The person the photo shows (a message's sender, a People hit), for the hover card.
    pub person: String,
    pub rtl: bool,
    /// On an `unread-group` header: the number of unread messages under it.
    pub count: i32,
}

/// A suggestion in the "In" and "Person" filter editors.
#[derive(Clone)]
pub struct PickData {
    pub id: String,
    pub title: String,
    pub subtitle: String,
}

/// One row of the Shared tab.
#[derive(Clone)]
pub struct SharedData {
    /// `file` or `link`.
    pub kind: &'static str,
    pub name: String,
    pub ext: String,
    pub shared_by: String,
    pub date: String,
    pub url: String,
    /// A SharePoint drive item: Download, Share, Rename and Delete apply.
    pub editable: bool,
    /// The address is on SharePoint: "Open in SharePoint" applies.
    pub sharepoint: bool,
    /// `folder`, `link` or a document kind (`word`, `excel`, `pdf`, `image`, `archive`, ...).
    pub icon: &'static str,
    /// What is being done to the row while the server answers ("Deleting…"), or "".
    pub busy: String,
    /// The row was just created: highlighted for a moment.
    pub highlight: bool,
}

/// The words of a query, lowercased. Quotes are dropped.
pub fn terms_of(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|w| w.trim_matches('"').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// True when `text` contains every term (case-insensitive).
pub fn matches_all(text: &str, terms: &[String]) -> bool {
    let lower = text.to_lowercase();
    !terms.is_empty() && terms.iter().all(|t| lower.contains(t.as_str()))
}

fn lower_chars(s: &str) -> Vec<char> {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Escapes what Markdown would read as formatting or a link.
fn md_escape(c: char, out: &mut String) {
    if matches!(
        c,
        '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '#' | '~' | '|'
    ) {
        out.push('\\');
    }
    out.push(c);
}

/// A snippet of `text` of at most `max` characters around the first match, as Markdown with the
/// matching words in bold. Newlines become spaces.
pub fn highlight_md(text: &str, terms: &[String], max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let chars: Vec<char> = flat.trim().chars().collect();
    let lower = lower_chars(&chars.iter().collect::<String>());
    let mut marked = vec![false; chars.len()];
    let mut first: Option<usize> = None;
    for term in terms {
        let t = lower_chars(term);
        if t.is_empty() || t.len() > lower.len() {
            continue;
        }
        let mut i = 0;
        while i + t.len() <= lower.len() {
            if lower[i..i + t.len()] == t[..] {
                marked[i..i + t.len()].iter_mut().for_each(|m| *m = true);
                first = Some(first.map_or(i, |f| f.min(i)));
                i += t.len();
            } else {
                i += 1;
            }
        }
    }
    // Window: start a little before the first match so it is visible.
    let start = first.map_or(0, |f| f.saturating_sub(max / 4));
    let end = (start + max).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let mut bold = false;
    for i in start..end {
        if marked[i] != bold {
            out.push_str("**");
            bold = marked[i];
        }
        md_escape(chars[i], &mut out);
    }
    if bold {
        out.push_str("**");
    }
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// A snippet the cache's search index already cut and marked (matches between
/// [`store::MARK_START`] and [`store::MARK_END`]) as Markdown with the matches in bold, like
/// [`highlight_md`]. Newlines become spaces.
pub fn marked_md(snippet: &str) -> String {
    let mut out = String::with_capacity(snippet.len() + 8);
    for c in snippet.trim().chars() {
        match c {
            store::MARK_START | store::MARK_END => out.push_str("**"),
            c if c.is_whitespace() => out.push(' '),
            c => md_escape(c, &mut out),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marked_snippet_becomes_bold_markdown() {
        let s = format!(
            "…the {}quarterly{} *report*\n{}ready{}",
            store::MARK_START,
            store::MARK_END,
            store::MARK_START,
            store::MARK_END
        );
        assert_eq!(marked_md(&s), "…the **quarterly** \\*report\\* **ready**");
    }

    fn t(q: &str) -> Vec<String> {
        terms_of(q)
    }

    #[test]
    fn terms_are_lowercase_words_without_quotes() {
        assert_eq!(t("  Quarterly \"Report\"  "), ["quarterly", "report"]);
        assert!(t("   ").is_empty());
    }

    #[test]
    fn a_message_must_contain_every_term() {
        assert!(matches_all("The Quarterly report", &t("report quarterly")));
        assert!(!matches_all("The Quarterly report", &t("report budget")));
        assert!(
            !matches_all("anything", &[]),
            "an empty query matches nothing"
        );
    }

    #[test]
    fn matching_words_are_bold_whatever_their_case() {
        assert_eq!(
            highlight_md("See the Quarterly REPORT now", &t("quarterly report"), 80),
            "See the **Quarterly** **REPORT** now"
        );
    }

    #[test]
    fn markdown_in_the_text_is_escaped() {
        assert_eq!(
            highlight_md("a_b *x* [c]", &t("x"), 80),
            "a\\_b \\***x**\\* \\[c\\]"
        );
    }

    #[test]
    fn long_text_is_cut_around_the_first_match() {
        let text = format!("{} needle {}", "a".repeat(100), "b".repeat(100));
        let s = highlight_md(&text, &t("needle"), 40);
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.contains("**needle**"));
        assert!(s.chars().count() < 60);
    }

    #[test]
    fn newlines_become_spaces_and_arabic_matches() {
        assert_eq!(highlight_md("one\ntwo", &t("two"), 80), "one **two**");
        assert_eq!(
            highlight_md("مرحبا بالعالم", &t("بالعالم"), 80),
            "مرحبا **بالعالم**"
        );
    }
}
