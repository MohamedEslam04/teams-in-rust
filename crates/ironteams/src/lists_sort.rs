//! How the rows of one chat list section are ordered: by recent activity (the list as it comes),
//! A to Z, or unread first. A pure function over the rows, so the order is tested without a window.

/// The order of one section, chosen per section and saved on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    /// The order the list already has: newest activity first (Teams' own order in a section of
    /// the account).
    #[default]
    Recent,
    /// By name, ignoring case.
    Alpha,
    /// Unread rows first, newest first, then the others newest first.
    Unread,
}

impl SortBy {
    /// The name saved in the lists' JSON and sent by the UI.
    pub fn name(self) -> &'static str {
        match self {
            SortBy::Recent => "recent",
            SortBy::Alpha => "alpha",
            SortBy::Unread => "unread",
        }
    }

    pub fn parse(name: &str) -> Option<SortBy> {
        Some(match name {
            "recent" => SortBy::Recent,
            "alpha" => SortBy::Alpha,
            "unread" => SortBy::Unread,
            _ => return None,
        })
    }
}

/// What the order looks at in a row.
pub struct SortKey {
    pub title: String,
    pub unread: bool,
    /// Time of the last message, in ms.
    pub activity_ms: i64,
}

/// Orders `items` in place. Rows that compare equal keep the order they came in, so the result
/// never shuffles between two renders of the same list.
pub fn sort_items<T>(items: &mut [T], by: SortBy, key: impl Fn(&T) -> SortKey) {
    match by {
        SortBy::Recent => {}
        SortBy::Alpha => items.sort_by_cached_key(|t| key(t).title.to_lowercase()),
        SortBy::Unread => items.sort_by_cached_key(|t| {
            let k = key(t);
            (!k.unread, std::cmp::Reverse(k.activity_ms))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(id, title, unread, activity)`.
    type Row = (&'static str, &'static str, bool, i64);

    fn sorted(rows: &[Row], by: SortBy) -> Vec<&'static str> {
        let mut v = rows.to_vec();
        sort_items(&mut v, by, |r| SortKey {
            title: r.1.to_string(),
            unread: r.2,
            activity_ms: r.3,
        });
        v.iter().map(|r| r.0).collect()
    }

    #[test]
    fn recent_keeps_the_order_it_was_given() {
        let rows = [("b", "Zed", false, 1), ("a", "Amy", true, 9)];
        assert_eq!(sorted(&rows, SortBy::Recent), vec!["b", "a"]);
    }

    #[test]
    fn a_to_z_ignores_case() {
        let rows = [
            ("1", "mona", false, 0),
            ("2", "Zaki", false, 0),
            ("3", "Amy", false, 0),
            ("4", "bob", false, 0),
        ];
        assert_eq!(sorted(&rows, SortBy::Alpha), vec!["3", "4", "1", "2"]);
    }

    #[test]
    fn a_to_z_orders_arabic_names_and_puts_them_after_latin_ones() {
        let rows = [
            ("1", "يوسف", false, 0),
            ("2", "Youssef", false, 0),
            ("3", "أحمد", false, 0),
            ("4", "بسمة", false, 0),
        ];
        // Latin first; the Arabic names in alphabet order (alef, beh, yeh).
        assert_eq!(sorted(&rows, SortBy::Alpha), vec!["2", "3", "4", "1"]);
    }

    #[test]
    fn rows_with_the_same_name_keep_their_order() {
        let rows = [
            ("x", "Ahmed", false, 5),
            ("y", "ahmed", false, 9),
            ("z", "AHMED", false, 1),
        ];
        assert_eq!(sorted(&rows, SortBy::Alpha), vec!["x", "y", "z"]);
        // And the same input always gives the same answer.
        assert_eq!(sorted(&rows, SortBy::Alpha), sorted(&rows, SortBy::Alpha));
    }

    #[test]
    fn unread_first_puts_unread_rows_on_top_newest_first_then_the_rest_newest_first() {
        let rows = [
            ("read-new", "A", false, 100),
            ("unread-old", "B", true, 10),
            ("read-old", "C", false, 5),
            ("unread-new", "D", true, 50),
        ];
        assert_eq!(
            sorted(&rows, SortBy::Unread),
            vec!["unread-new", "unread-old", "read-new", "read-old"]
        );
    }

    #[test]
    fn unread_first_keeps_the_given_order_for_equal_times() {
        let rows = [
            ("a", "A", true, 7),
            ("b", "B", false, 7),
            ("c", "C", true, 7),
            ("d", "D", false, 7),
        ];
        assert_eq!(sorted(&rows, SortBy::Unread), vec!["a", "c", "b", "d"]);
    }

    #[test]
    fn nothing_unread_is_newest_first_and_an_empty_list_is_fine() {
        let rows = [("a", "A", false, 1), ("b", "B", false, 3)];
        assert_eq!(sorted(&rows, SortBy::Unread), vec!["b", "a"]);
        assert!(sorted(&[], SortBy::Alpha).is_empty());
        assert!(sorted(&[], SortBy::Unread).is_empty());
    }

    #[test]
    fn names_round_trip_and_unknown_names_are_refused() {
        for by in [SortBy::Recent, SortBy::Alpha, SortBy::Unread] {
            assert_eq!(SortBy::parse(by.name()), Some(by));
        }
        assert_eq!(SortBy::parse("newest"), None);
        assert_eq!(SortBy::default(), SortBy::Recent);
    }
}
