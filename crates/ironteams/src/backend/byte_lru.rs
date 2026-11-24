//! A least-recently-used map with a byte budget, for the decoded pictures kept in memory
//! (avatars, inline images, link thumbnails). Each entry says what it costs; inserting over the
//! budget drops the least recently used entries until the total fits again. The newest entry
//! always stays, even when it alone is over the budget, so one big picture does not make its
//! lookup start the same download again on every render.
//!
//! Empty placeholders (a lookup in progress, a picture that failed) cost next to nothing but are
//! evicted like any other entry: the next lookup then starts the work again.
//!
//! A view that draws its pictures from the cache on every render marks each render's start
//! (`begin_pass`). What the last two renders read is then not evicted, even over the budget: the
//! view holds those pictures anyway, and dropping one makes the next render decode it again,
//! whose arrival drops another one on screen, and so on forever.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use crate::data::Pixels;

/// The budgets of the picture caches in `Engine` and `media::State`.
pub(super) const AVATAR_BUDGET: usize = 16 * 1024 * 1024;
pub(super) const IMAGE_BUDGET: usize = 64 * 1024 * 1024;
pub(super) const THUMB_BUDGET: usize = 16 * 1024 * 1024;

/// What an entry costs besides its pixels: the key, the map slots and the allocation.
const ENTRY_OVERHEAD: usize = 64;

/// The bytes a decoded picture (or an empty placeholder) takes in memory.
pub(super) fn pixels_cost(px: &Option<Pixels>) -> usize {
    ENTRY_OVERHEAD + px.as_ref().map_or(0, pixel_bytes)
}

/// The bytes of a decoded picture's pixels (four per pixel).
pub(super) fn pixel_bytes(p: &Pixels) -> usize {
    p.width() as usize * p.height() as usize * 4
}

struct Entry<V> {
    value: V,
    bytes: usize,
    /// Position in `ByteLru::order`.
    tick: u64,
}

pub(super) struct ByteLru<K, V> {
    budget: usize,
    total: usize,
    next_tick: u64,
    map: HashMap<K, Entry<V>>,
    /// Keys from the least to the most recently used.
    order: BTreeMap<u64, K>,
    /// The first tick of the current render (`begin_pass`), and of the one before it: entries
    /// read or stored since `keep_from` are not evicted.
    pass_start: Option<u64>,
    keep_from: Option<u64>,
    /// Decoded pictures stored so far (`insert_pixels` with a picture).
    #[cfg(test)]
    filled: u64,
}

impl<K: Hash + Eq + Clone, V> ByteLru<K, V> {
    pub(super) fn new(budget: usize) -> Self {
        ByteLru {
            budget,
            total: 0,
            next_tick: 0,
            map: HashMap::new(),
            order: BTreeMap::new(),
            pass_start: None,
            keep_from: None,
            #[cfg(test)]
            filled: 0,
        }
    }

    /// The value for `key`, which becomes the most recently used entry.
    pub(super) fn get<Q>(&mut self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let entry = self.map.get_mut(key)?;
        let k = self.order.remove(&entry.tick)?;
        entry.tick = self.next_tick;
        self.next_tick += 1;
        self.order.insert(entry.tick, k);
        Some(&entry.value)
    }

    /// A render of the view starts: what it and the render before it read stays in the cache.
    pub(super) fn begin_pass(&mut self) {
        self.keep_from = Some(self.pass_start.unwrap_or(self.next_tick));
        self.pass_start = Some(self.next_tick);
    }

    /// Stores `value`, which takes `bytes`, as the most recently used entry (replacing the
    /// entry of the same key), then drops the oldest entries while the total is over budget,
    /// except those the last two renders read (`begin_pass`).
    pub(super) fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.remove(&key);
        let tick = self.next_tick;
        self.next_tick += 1;
        self.order.insert(tick, key.clone());
        self.map.insert(key, Entry { value, bytes, tick });
        self.total += bytes;
        while self.total > self.budget && self.map.len() > 1 {
            let Some((&tick, _)) = self.order.first_key_value() else {
                break;
            };
            if self.keep_from.is_some_and(|keep| tick >= keep) {
                break;
            }
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(gone) = self.map.remove(&oldest) {
                self.total -= gone.bytes;
            }
        }
    }

    pub(super) fn remove<Q>(&mut self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if let Some(gone) = self.map.remove(key) {
            self.order.remove(&gone.tick);
            self.total -= gone.bytes;
        }
    }

    pub(super) fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.total = 0;
    }

    /// The bytes the entries take.
    #[cfg(test)]
    pub(super) fn bytes(&self) -> usize {
        self.total
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.map.len()
    }

    /// How many decoded pictures were stored, replaced ones included.
    #[cfg(test)]
    pub(super) fn filled(&self) -> u64 {
        self.filled
    }
}

impl<K: Hash + Eq + Clone> ByteLru<K, Option<Pixels>> {
    /// `insert` for a decoded picture, or `None` for a lookup in progress or a failure.
    pub(super) fn insert_pixels(&mut self, key: K, px: Option<Pixels>) {
        let bytes = pixels_cost(&px);
        #[cfg(test)]
        if px.is_some() {
            self.filled += 1;
        }
        self.insert(key, px, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lru(budget: usize) -> ByteLru<String, u32> {
        ByteLru::new(budget)
    }

    #[test]
    fn the_least_recently_used_entry_goes_first_when_over_budget() {
        let mut c = lru(100);
        c.insert("a".into(), 1, 40);
        c.insert("b".into(), 2, 40);
        // Reading `a` makes `b` the oldest.
        assert_eq!(c.get("a"), Some(&1));
        c.insert("c".into(), 3, 40);
        assert_eq!(c.get("b"), None);
        assert_eq!(c.get("a"), Some(&1));
        assert_eq!(c.get("c"), Some(&3));
        assert_eq!(c.bytes(), 80);
    }

    #[test]
    fn the_total_never_stays_over_the_budget_while_entries_fit() {
        let mut c = lru(100);
        for i in 0..50 {
            c.insert(format!("k{i}"), i, 30);
            assert!(c.bytes() <= 100, "{} bytes after {i}", c.bytes());
        }
        assert_eq!(c.len(), 3);
        assert_eq!(c.get("k49"), Some(&49));
        assert_eq!(c.get("k0"), None);
    }

    #[test]
    fn replacing_a_key_counts_its_new_size_only() {
        let mut c = lru(100);
        c.insert("a".into(), 1, 60);
        c.insert("a".into(), 2, 10);
        assert_eq!((c.len(), c.bytes()), (1, 10));
        assert_eq!(c.get("a"), Some(&2));
    }

    #[test]
    fn empty_placeholders_cost_nothing_but_are_evicted_in_their_turn() {
        let mut c = lru(100);
        c.insert("none".into(), 0, 0);
        c.insert("big".into(), 1, 90);
        assert_eq!(c.get("none"), Some(&0), "still there: the total fits");
        assert_eq!(c.get("big"), Some(&1));
        // `none` is the oldest now: it goes first, and `big` after it.
        c.insert("big2".into(), 2, 90);
        assert_eq!(c.get("none"), None, "a zero-cost entry is evictable too");
        assert_eq!(c.get("big"), None);
        assert_eq!(c.get("big2"), Some(&2));
    }

    #[test]
    fn an_entry_over_the_budget_by_itself_stays_alone() {
        let mut c = lru(100);
        c.insert("a".into(), 1, 10);
        c.insert("huge".into(), 2, 500);
        assert_eq!(c.len(), 1);
        assert_eq!(c.get("huge"), Some(&2));
    }

    /// One render of a view showing `keys`: what is not cached is asked for (a placeholder
    /// marks it in progress), as `Engine::inline_image` does. Returns the keys asked for.
    fn render(c: &mut ByteLru<String, u32>, keys: &[&str]) -> Vec<String> {
        c.begin_pass();
        let mut asked = Vec::new();
        for k in keys {
            if c.get(*k).is_none() {
                c.insert((*k).into(), 0, 1);
                asked.push((*k).to_string());
            }
        }
        asked
    }

    #[test]
    fn pictures_a_render_reads_are_not_evicted_by_each_other() {
        // Room for two pictures, four on screen.
        let mut c = lru(100);
        let shown = ["a", "b", "c", "d"];
        assert_eq!(render(&mut c, &shown).len(), 4);
        // The decoded pictures arrive; each arrival draws the view again.
        for k in shown {
            c.insert(k.into(), 1, 40);
            assert!(render(&mut c, &shown).is_empty(), "nothing asked again");
        }
        for _ in 0..3 {
            assert!(render(&mut c, &shown).is_empty());
        }
        assert_eq!(
            (c.len(), c.bytes()),
            (4, 160),
            "over the budget while shown"
        );
    }

    #[test]
    fn pictures_no_longer_shown_go_once_two_renders_left_them_out() {
        let mut c = lru(100);
        render(&mut c, &["a", "b", "c"]);
        for k in ["a", "b", "c"] {
            c.insert(k.into(), 1, 40);
        }
        // Another chat: `a`, `b` and `c` stay through its first render.
        render(&mut c, &["x"]);
        c.insert("x".into(), 2, 40);
        assert_eq!(c.len(), 4);
        render(&mut c, &["x"]);
        c.insert("y".into(), 3, 40);
        assert_eq!(c.bytes(), 80, "back within the budget");
        for k in ["a", "b", "c"] {
            assert_eq!(c.get(k), None, "{k} is no longer shown");
        }
        assert!(c.get("x").is_some() && c.get("y").is_some());
    }

    #[test]
    fn without_renders_the_budget_holds_as_before() {
        let mut c = lru(100);
        for k in ["a", "b", "c", "d"] {
            c.insert(k.into(), 1, 40);
        }
        assert_eq!((c.len(), c.bytes()), (2, 80));
    }

    #[test]
    fn remove_and_clear_free_the_bytes() {
        let mut c = lru(100);
        c.insert("a".into(), 1, 30);
        c.insert("b".into(), 2, 30);
        c.remove("a");
        assert_eq!((c.len(), c.bytes()), (1, 30));
        c.clear();
        assert_eq!((c.len(), c.bytes()), (0, 0));
        assert_eq!(c.get("b"), None);
    }

    #[test]
    fn insert_pixels_charges_the_picture_and_evicts_over_the_budget() {
        let mut c: ByteLru<String, Option<Pixels>> =
            ByteLru::new(2 * pixels_cost(&Some(Pixels::new(10, 10))));
        for i in 0..5 {
            c.insert_pixels(format!("k{i}"), Some(Pixels::new(10, 10)));
        }
        assert_eq!(c.len(), 2);
        c.insert_pixels("none".into(), None);
        assert_eq!(c.len(), 2, "the placeholder pushed the oldest picture out");
        assert!(c.get("k4").is_some());
    }

    #[test]
    fn pixels_cost_follows_the_picture_size() {
        assert_eq!(pixels_cost(&None), ENTRY_OVERHEAD);
        let px = Pixels::new(10, 5);
        assert_eq!(pixels_cost(&Some(px)), ENTRY_OVERHEAD + 200);
    }
}
