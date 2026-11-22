//! The command palette (Ctrl+Shift+P): the ranking behind its result list and the recent-actions
//! list. The actions are `PaletteCatalog` in `ui/pages/palette.slint` (labels are `@tr` strings, so they
//! are matched in the language shown); running one reuses the callback or store command the same
//! thing already has (`run-palette` in `ui/app.slint`), so nothing here sends a command.

use crate::{AppWindow, ChatRow, PaletteAction, PaletteApi, PaletteItem, SectionRow};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::collections::HashSet;

/// How many used actions the empty palette lists first.
const RECENT_MAX: usize = 5;
/// How many chats and channels follow the actions.
const CHATS_MAX: usize = 8;
/// The list's geometry in logical pixels, which `ui/pages/palette.slint` draws the same way: the space
/// above the first row, a row, and the section heading above the first row of a section.
const LIST_PAD: i32 = 4;
const ROW_H: i32 = 36;
const HEAD_H: i32 = 26;

/// How well `label` matches `query`, or `None` when it does not. Case-insensitive. From best to
/// worst: the label starts with the query, every word of the query starts a word of the label,
/// the query is somewhere inside the label, its letters appear in order (a subsequence, tighter
/// runs and word starts count more). Shorter labels win inside a tier.
pub fn score(query: &str, label: &str) -> Option<i32> {
    let q = query.to_lowercase();
    let tokens: Vec<&str> = q.split_whitespace().collect();
    if tokens.is_empty() {
        return Some(0);
    }
    let label = label.to_lowercase();
    let len = label.chars().count().min(200) as i32;
    let phrase = tokens.join(" ");
    if label.starts_with(&phrase) {
        return Some(1000 - len);
    }
    let words: Vec<&str> = label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if tokens
        .iter()
        .all(|t| words.iter().any(|w| w.starts_with(t)))
    {
        // Words typed in the order the label has them rank above the same words swapped.
        let mut at = 0;
        let mut in_order = true;
        for t in &tokens {
            match words[at..].iter().position(|w| w.starts_with(t)) {
                Some(p) => at += p + 1,
                None => {
                    in_order = false;
                    break;
                }
            }
        }
        return Some(700 - len + if in_order { 50 } else { 0 });
    }
    if let Some(byte) = label.find(&phrase) {
        let pos = label[..byte].chars().count() as i32;
        return Some(450 - pos.min(40));
    }
    let chars: Vec<char> = label.chars().collect();
    let mut from = 0;
    let mut last: Option<usize> = None;
    let mut sum = 100;
    for qc in q.chars().filter(|c| !c.is_whitespace()) {
        let skipped = chars[from..].iter().position(|&c| c == qc)?;
        let i = from + skipped;
        if last.is_some_and(|l| l + 1 == i) {
            sum += 8;
        }
        if i == 0 || !chars[i - 1].is_alphanumeric() {
            sum += 10;
        }
        sum -= skipped as i32;
        last = Some(i);
        from = i + 1;
    }
    Some(sum.clamp(1, 390))
}

/// The indexes of the labels that match `query`, best first (ties: shorter label, then the
/// original order).
pub fn rank(query: &str, labels: &[&str]) -> Vec<usize> {
    let mut hits: Vec<(i32, usize, usize)> = labels
        .iter()
        .enumerate()
        .filter_map(|(i, l)| score(query, l).map(|s| (s, l.chars().count(), i)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    hits.into_iter().map(|h| h.2).collect()
}

/// The recent-actions list (comma-separated ids, newest first) after `id` was used.
pub fn push_recent(recents: &str, id: &str) -> String {
    if id.is_empty() {
        return recents.to_string();
    }
    std::iter::once(id)
        .chain(recents.split(',').filter(|r| !r.is_empty() && *r != id))
        .take(RECENT_MAX)
        .collect::<Vec<_>>()
        .join(",")
}

fn action_item(a: &PaletteAction, section: &str) -> PaletteItem {
    PaletteItem {
        id: a.id.clone(),
        cmd: a.cmd.clone(),
        arg: a.arg.clone(),
        label: a.label.clone(),
        keys: a.keys.clone(),
        kind: "action".into(),
        section: section.into(),
        icon: a.icon.clone(),
        has_icon: true,
        ..Default::default()
    }
}

fn chat_item(c: &ChatRow) -> PaletteItem {
    PaletteItem {
        id: c.id.clone(),
        label: c.title.clone(),
        kind: "chat".into(),
        section: "chats".into(),
        initials: c.initials.clone(),
        color: c.color,
        ..Default::default()
    }
}

/// The rows of the palette, with where each one sits in the list (the window scrolls the selected
/// row into view from that, so it does not depend on which rows are drawn).
pub fn build(
    query: &str,
    actions: &[PaletteAction],
    chats: &[ChatRow],
    recents: &str,
) -> Vec<PaletteItem> {
    let mut rows = pick(query, actions, chats, recents);
    let mut y = LIST_PAD;
    let mut section = SharedString::new();
    for (i, r) in rows.iter_mut().enumerate() {
        r.head = i == 0 || r.section != section;
        section = r.section.clone();
        r.top = y;
        r.height = ROW_H + if r.head { HEAD_H } else { 0 };
        y += r.height;
    }
    rows
}

/// Which rows show. An empty query lists the recent actions, then the rest. Otherwise the
/// matching actions come first and, from two characters on, the matching chats and channels.
fn pick(
    query: &str,
    actions: &[PaletteAction],
    chats: &[ChatRow],
    recents: &str,
) -> Vec<PaletteItem> {
    let shown: Vec<&PaletteAction> = actions.iter().filter(|a| !a.hidden).collect();
    let q = query.trim();
    let mut out = Vec::new();
    if q.is_empty() {
        let mut recent: Vec<&PaletteAction> = Vec::new();
        for id in recents.split(',') {
            if let Some(a) = shown.iter().copied().find(|a| a.id.as_str() == id)
                && !recent.iter().any(|r| r.id == a.id)
            {
                recent.push(a);
            }
        }
        recent.truncate(RECENT_MAX);
        out.extend(recent.iter().map(|a| action_item(a, "recent")));
        out.extend(
            shown
                .iter()
                .filter(|a| !recent.iter().any(|r| r.id == a.id))
                .map(|a| action_item(a, "actions")),
        );
        return out;
    }
    let labels: Vec<&str> = shown.iter().map(|a| a.label.as_str()).collect();
    out.extend(
        rank(q, &labels)
            .into_iter()
            .map(|i| action_item(shown[i], "actions")),
    );
    if q.chars().count() >= 2 {
        let mut seen = HashSet::new();
        let rows: Vec<&ChatRow> = chats
            .iter()
            .filter(|c| !c.title.is_empty() && seen.insert(c.id.clone()))
            .collect();
        let titles: Vec<&str> = rows.iter().map(|c| c.title.as_str()).collect();
        out.extend(
            rank(q, &titles)
                .into_iter()
                .take(CHATS_MAX)
                .map(|i| chat_item(rows[i])),
        );
    }
    out
}

/// Attaches the palette's ranking to a window.
pub fn wire(window: &AppWindow) {
    let api = window.global::<PaletteApi>();
    api.on_results(
        |query, actions, favorites, chats, channels, sections: ModelRc<SectionRow>, recents| {
            let actions: Vec<PaletteAction> = actions.iter().collect();
            let mut rows: Vec<ChatRow> = Vec::new();
            // Chats only matter from two characters on; skip copying a long list before that.
            if query.trim().chars().count() >= 2 {
                rows.extend(favorites.iter());
                for s in sections.iter() {
                    rows.extend(s.items.iter());
                }
                rows.extend(chats.iter());
                rows.extend(channels.iter());
            }
            ModelRc::new(VecModel::from(build(&query, &actions, &rows, &recents)))
        },
    );
    api.on_push_recent(|recents, id| push_recent(&recents, &id).into());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(id: &str, label: &str) -> PaletteAction {
        PaletteAction {
            id: id.into(),
            label: label.into(),
            ..Default::default()
        }
    }

    fn chat(id: &str, title: &str) -> ChatRow {
        ChatRow {
            id: id.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    fn ids(rows: &[PaletteItem]) -> Vec<String> {
        rows.iter().map(|r| r.id.to_string()).collect()
    }

    #[test]
    fn matching_ignores_case_and_rejects_what_is_not_there() {
        assert!(score("ACTIVITY", "Go to Activity").is_some());
        assert!(score("activity", "Go to CALLS").is_none());
        assert!(score("zz", "Go to Activity").is_none());
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("   ", "anything"), Some(0));
    }

    #[test]
    fn prefix_beats_word_start_beats_inside_beats_subsequence() {
        let prefix = score("go", "Go to Activity").unwrap();
        let word = score("act", "Go to Activity").unwrap();
        let inside = score("tiv", "Go to Activity").unwrap();
        let subsequence = score("gta", "Go to Activity").unwrap();
        assert!(prefix > word, "{prefix} > {word}");
        assert!(word > inside, "{word} > {inside}");
        assert!(inside > subsequence, "{inside} > {subsequence}");
    }

    #[test]
    fn several_words_match_word_starts_in_any_order_but_in_order_wins() {
        let ordered = score("set stat", "Set status: Busy").unwrap();
        let swapped = score("stat set", "Set status: Busy").unwrap();
        assert!(ordered > swapped);
        assert!(swapped > 0);
        assert!(score("mark read", "Mark all as read").is_some());
    }

    #[test]
    fn a_subsequence_prefers_tight_runs_and_word_starts() {
        let tight = score("rpt", "Report a problem").unwrap();
        let loose = score("rpt", "Reset the status of the person, then try").unwrap();
        assert!(tight > loose);
        assert!(score("rpt", "Toggle theme").is_none());
    }

    #[test]
    fn shorter_labels_rank_first_inside_a_tier() {
        let labels = ["Go to a chat or channel", "Go to Chat", "Go to Teams"];
        assert_eq!(rank("go to", &labels), vec![1, 2, 0]);
    }

    #[test]
    fn arabic_labels_match_on_the_translated_text() {
        let labels = ["الانتقال إلى النشاط", "الانتقال إلى الفرق"];
        assert_eq!(rank("النشاط", &labels), vec![0]);
        // Both start with it: the shorter label first.
        assert_eq!(rank("الانتقال", &labels), vec![1, 0]);
        assert!(rank("activity", &labels).is_empty());
    }

    #[test]
    fn recents_keep_the_last_five_newest_first_without_repeats() {
        let mut r = String::new();
        for id in ["a", "b", "c", "d", "e", "f"] {
            r = push_recent(&r, id);
        }
        assert_eq!(r, "f,e,d,c,b");
        assert_eq!(push_recent(&r, "d"), "d,f,e,c,b");
        assert_eq!(push_recent(&r, ""), r);
    }

    #[test]
    fn an_empty_query_lists_recent_actions_first_then_the_others_once() {
        let actions = [
            action("a", "Alpha"),
            action("b", "Beta"),
            action("c", "Gamma"),
        ];
        let rows = build("", &actions, &[], "c,missing,a");
        assert_eq!(ids(&rows), ["c", "a", "b"]);
        let sections: Vec<_> = rows.iter().map(|r| r.section.as_str()).collect();
        assert_eq!(sections, ["recent", "recent", "actions"]);
        let rows = build("  ", &actions, &[], "");
        assert_eq!(ids(&rows), ["a", "b", "c"]);
        assert!(rows.iter().all(|r| r.section == "actions"));
    }

    #[test]
    fn hidden_actions_never_show() {
        let mut calls = action("go-calls", "Go to Calls");
        calls.hidden = true;
        let actions = [calls, action("go-chat", "Go to Chat")];
        assert_eq!(ids(&build("go to", &actions, &[], "")), ["go-chat"]);
        assert_eq!(ids(&build("", &actions, &[], "go-calls")), ["go-chat"]);
    }

    #[test]
    fn chats_follow_the_actions_from_two_characters_on_without_duplicates() {
        let actions = [action("search", "Search")];
        let chats = [
            chat("c1", "Search team"),
            chat("c1", "Search team"),
            chat("c2", "Lunch"),
        ];
        let one = build("s", &actions, &chats, "");
        assert_eq!(ids(&one), ["search"]);
        let two = build("se", &actions, &chats, "");
        assert_eq!(ids(&two), ["search", "c1"]);
        assert_eq!(two[1].kind, "chat");
        assert_eq!(two[1].section, "chats");
        assert_eq!(two[0].kind, "action");
    }

    #[test]
    fn rows_know_their_place_in_the_list() {
        let actions = [action("a", "Alpha"), action("b", "Beta")];
        let chats = [chat("c1", "Alpine club")];
        let rows = build("al", &actions, &chats, "");
        // Alpha (actions heading above it), then Alpine club (chats heading above it).
        assert_eq!(ids(&rows), ["a", "c1"]);
        assert_eq!(
            rows.iter().map(|r| r.head).collect::<Vec<_>>(),
            [true, true]
        );
        assert_eq!(rows[0].top, LIST_PAD);
        assert_eq!(rows[0].height, ROW_H + HEAD_H);
        assert_eq!(rows[1].top, LIST_PAD + ROW_H + HEAD_H);
        // Two rows of one section share a heading.
        let rows = build("", &actions, &[], "");
        assert!(!rows[1].head);
        assert_eq!(rows[1].top, LIST_PAD + ROW_H + HEAD_H);
        assert_eq!(rows[1].height, ROW_H);
    }

    #[test]
    fn the_chat_part_is_capped() {
        let chats: Vec<ChatRow> = (0..30)
            .map(|i| chat(&format!("c{i}"), &format!("Project {i}")))
            .collect();
        assert_eq!(build("project", &[], &chats, "").len(), CHATS_MAX);
    }
}
