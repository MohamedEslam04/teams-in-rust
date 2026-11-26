//! The window-level shortcuts a user can rebind (Settings > Keyboard shortcuts).
//!
//! [`ACTIONS`] is the table of (action id, default chord). `ShortcutApi.action` (see
//! `shortcuts.rs`) turns a key press into an action id by looking the chord up in the current
//! [`Bindings`], so `app_keys.slint` has no key of its own for these actions. The bindings are the
//! defaults plus what the user changed (`appearance::ShortcutPrefs`, `shortcuts.json`).
//!
//! Not here, on purpose: the text boxes' own keys (copy, paste, undo, Enter to send), the
//! composer's formatting chords, the in-call keys, Ctrl+Shift+1 to 9 and Esc. They stay where they
//! are; the chords that belong to them are [`RESERVED`], so a rebind cannot take them.

use crate::{AppWindow, ShortcutItem, ShortcutStore};
use appearance::ShortcutPrefs;
use slint::platform::Key;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Every customizable action and its default chord, in the order Settings lists them. The ids
/// are the ones `app_keys.slint` runs; `shortcuts`, `search`, `goto`, `new-chat`, `settings`,
/// `next-unread-wrap` and `previous-unread-wrap` are also command palette ids, so the palette
/// shows the current chord.
pub const ACTIONS: &[(&str, &str)] = &[
    ("shortcuts", "Ctrl+."),
    ("palette", "Ctrl+Shift+P"),
    ("search", "Ctrl+Alt+E"),
    ("goto", "Ctrl+Alt+G"),
    ("new-chat", "Alt+Shift+N"),
    ("settings", "Ctrl+Shift+,"),
    ("back", "Alt+Left"),
    ("forward", "Alt+Right"),
    ("search-box", "Ctrl+E"),
    ("commands", "Ctrl+/"),
    ("filter-box", "Ctrl+Shift+F"),
    ("filter-unread", "Ctrl+Alt+U"),
    ("filter-chats", "Ctrl+Alt+C"),
    ("filter-channels", "Ctrl+Alt+A"),
    ("filter-clear", "Ctrl+Alt+Z"),
    ("collapse-all", "Alt+Q"),
    ("next-unread", "Alt+Down"),
    ("previous-unread", "Alt+Up"),
    ("next-unread-wrap", "Alt+Shift+Down"),
    ("previous-unread-wrap", "Alt+Shift+Up"),
    ("focus-composer", "Alt+Shift+R"),
    ("focus-messages", "Alt+Shift+M"),
    ("unreads", "Alt+Shift+U"),
    ("find", "Ctrl+F"),
    ("jump-first", "Ctrl+Alt+Home"),
    ("expand-composer", "Ctrl+Shift+X"),
    ("attach-file", "Alt+Shift+O"),
    ("audio-call", "Alt+Shift+A"),
    ("video-call", "Alt+Shift+V"),
    ("share-screen", "Ctrl+Shift+E"),
];

/// Chords that other parts of the app keep for themselves (the text boxes, the composer's
/// formatting, the voice recorder, the call keys, Find in Settings). A rebind cannot take them.
/// Ctrl+Shift+1 to 9 (the app bar) are refused in [`check`].
const RESERVED: &[&str] = &[
    "Ctrl+A",
    "Ctrl+C",
    "Ctrl+V",
    "Ctrl+X",
    "Ctrl+Z",
    "Ctrl+Y",
    "Ctrl+B",
    "Ctrl+I",
    "Ctrl+U",
    "Ctrl+K",
    "Ctrl+Alt+X",
    "Ctrl+Alt+P",
    "Ctrl+Alt+F",
    "Ctrl+Alt+1",
    "Ctrl+Alt+2",
    "Ctrl+Alt+3",
    "Ctrl+Alt+4",
    "Ctrl+Alt+Shift+C",
    "Ctrl+Alt+Shift+B",
    "Ctrl+Shift+R",
    "Ctrl+Shift+I",
    "Ctrl+Shift+M",
    "Ctrl+Shift+O",
    "Ctrl+Shift+K",
    "Ctrl+Shift+H",
    "Ctrl+Shift+B",
];

/// Keys that are not one character, by the name they have in a chord's text.
const NAMED: &[(&str, Key)] = &[
    ("Up", Key::UpArrow),
    ("Down", Key::DownArrow),
    ("Left", Key::LeftArrow),
    ("Right", Key::RightArrow),
    ("Home", Key::Home),
    ("End", Key::End),
    ("PageUp", Key::PageUp),
    ("PageDown", Key::PageDown),
    ("Insert", Key::Insert),
    ("Delete", Key::Delete),
    ("Tab", Key::Tab),
    ("Space", Key::Space),
    ("F1", Key::F1),
    ("F2", Key::F2),
    ("F3", Key::F3),
    ("F4", Key::F4),
    ("F5", Key::F5),
    ("F6", Key::F6),
    ("F7", Key::F7),
    ("F8", Key::F8),
    ("F9", Key::F9),
    ("F10", Key::F10),
    ("F11", Key::F11),
    ("F12", Key::F12),
];

/// The keys that are only a modifier: pressing one is not yet a chord.
const MODIFIER_KEYS: &[Key] = &[
    Key::Shift,
    Key::ShiftR,
    Key::Control,
    Key::ControlR,
    Key::Alt,
    Key::AltGr,
    Key::Meta,
    Key::MetaR,
    Key::CapsLock,
];

/// What a key types with Shift held, and the key it is on without it (US layout). A chord keeps
/// the unshifted key, so Ctrl+Shift+, matches whether the layout reports "," or "<".
const SHIFTED: &[(char, char)] = &[
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    (':', ';'),
    ('"', '\''),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    ('_', '-'),
    ('+', '='),
    ('~', '`'),
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
];

fn unshifted(c: char) -> char {
    SHIFTED
        .iter()
        .find(|(shifted, _)| *shifted == c)
        .map_or(c, |(_, base)| *base)
}

/// A key press is only a modifier going down (Ctrl, Shift, Alt, Meta, Caps Lock).
pub fn is_modifier(key: &str) -> bool {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => MODIFIER_KEYS.iter().any(|k| char::from(*k) == c),
        _ => false,
    }
}

/// A key with the modifiers held: what a shortcut is bound to. `key` is a lower case character
/// or a name from [`NAMED`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    ctrl: bool,
    alt: bool,
    shift: bool,
    key: String,
}

impl Chord {
    /// The chord written as "Ctrl+Alt+E" (case and the order of the modifiers do not matter).
    pub fn parse(text: &str) -> Option<Chord> {
        let mut rest = text.trim();
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        while let Some((head, tail)) = rest.split_once('+') {
            match head.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                _ => break,
            }
            rest = tail;
        }
        let key = if let Some((name, _)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(rest))
        {
            (*name).to_owned()
        } else {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Self::plain(c, shift),
                _ => return None,
            }
        };
        Some(Chord {
            ctrl,
            alt,
            shift,
            key,
        })
    }

    /// The chord of a key press (`key` is the key's text as Slint delivers it), or `None` for a
    /// key that cannot be part of a chord (a modifier, Esc, Enter, Backspace...).
    pub fn from_event(key: &str, ctrl: bool, alt: bool, shift: bool) -> Option<Chord> {
        let mut chars = key.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let key = if let Some((name, _)) = NAMED.iter().find(|(_, k)| char::from(*k) == c) {
            (*name).to_owned()
        } else if c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c) {
            return None;
        } else {
            Self::plain(c, shift)
        };
        Some(Chord {
            ctrl,
            alt,
            shift,
            key,
        })
    }

    /// The stored form of a one-character key.
    fn plain(c: char, shift: bool) -> String {
        let base = if shift { unshifted(c) } else { c };
        base.to_lowercase().collect()
    }

    /// A function key (F1 to F12), which needs no other modifier.
    fn is_function_key(&self) -> bool {
        self.key.len() >= 2
            && self.key.starts_with('F')
            && self.key[1..].chars().all(|d| d.is_ascii_digit())
    }
}

/// "Ctrl+Alt+E": modifiers in a fixed order, then the key; this is also what is saved.
impl std::fmt::Display for Chord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.key.chars().count() == 1 {
            let upper = self.key.to_uppercase();
            if upper.chars().count() == 1 {
                return f.write_str(&upper);
            }
        }
        f.write_str(&self.key)
    }
}

/// Why a chord was not accepted for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Only a modifier is down so far: keep waiting for the rest of the chord.
    Wait,
    /// A key that cannot be part of a chord (Esc, Enter, a Meta chord...).
    Unsupported,
    /// A plain key would take over typing: it needs Ctrl or Alt (or be a function key).
    NeedsModifier,
    /// A chord the text boxes, the composer, the calls or the app bar keep.
    Reserved,
    /// Another action already has it (that action's id).
    Taken(&'static str),
}

impl Reject {
    /// The word `ShortcutStore.rebind` answers with.
    pub fn code(self) -> &'static str {
        match self {
            Reject::Wait => "wait",
            Reject::Unsupported => "unsupported",
            Reject::NeedsModifier => "needs-modifier",
            Reject::Reserved => "reserved",
            Reject::Taken(_) => "taken",
        }
    }
}

/// Whether any action may be bound to `c`, apart from the other actions' chords.
fn check(c: &Chord) -> Result<(), Reject> {
    if !(c.ctrl || c.alt || c.is_function_key()) {
        return Err(Reject::NeedsModifier);
    }
    let app_bar = c.ctrl && c.shift && !c.alt && matches!(c.key.as_bytes(), [b'1'..=b'9']);
    if app_bar
        || RESERVED
            .iter()
            .filter_map(|r| Chord::parse(r))
            .any(|r| r == *c)
    {
        return Err(Reject::Reserved);
    }
    Ok(())
}

fn default_chord(index: usize) -> Chord {
    Chord::parse(ACTIONS[index].1).expect("a default chord that parses")
}

/// The chord of every action in [`ACTIONS`]: the defaults, with the user's changes on top. No two
/// actions share a chord.
#[derive(Debug, Clone)]
pub struct Bindings {
    chords: Vec<Chord>,
}

impl Bindings {
    pub fn defaults() -> Bindings {
        Bindings {
            chords: (0..ACTIONS.len()).map(default_chord).collect(),
        }
    }

    /// The defaults with the saved changes applied. A saved chord that is not valid, or that
    /// another action has, is ignored (a file edited by hand, or a default a release moved).
    pub fn from_prefs(prefs: &ShortcutPrefs) -> Bindings {
        let mut b = Bindings::defaults();
        let mut pending: Vec<(usize, Chord)> = ACTIONS
            .iter()
            .enumerate()
            .filter_map(|(i, (id, _))| {
                let chord = Chord::parse(prefs.bindings.get(*id)?)?;
                (check(&chord).is_ok() && chord != b.chords[i]).then_some((i, chord))
            })
            .collect();
        // A change may need a chord another change frees (A took B's old key, B moved on):
        // apply what fits, and go again while something still does.
        loop {
            let before = pending.len();
            pending.retain(|(i, chord)| match b.owner(chord) {
                Some(owner) if owner != *i => true,
                _ => {
                    b.chords[*i] = chord.clone();
                    false
                }
            });
            if pending.len() == before {
                return b;
            }
        }
    }

    /// The index of the action that has `chord`.
    fn owner(&self, chord: &Chord) -> Option<usize> {
        self.chords.iter().position(|c| c == chord)
    }

    fn index(id: &str) -> Option<usize> {
        ACTIONS.iter().position(|(a, _)| *a == id)
    }

    /// The action a chord runs, or `""`.
    pub fn action(&self, chord: &Chord) -> &'static str {
        self.owner(chord).map_or("", |i| ACTIONS[i].0)
    }

    /// The chord of `id` as text, or `None` for an id that is not an action.
    pub fn display(&self, id: &str) -> Option<String> {
        Some(self.chords[Self::index(id)?].to_string())
    }

    /// Binds `id` to a key press from the Settings page.
    pub fn capture(
        &mut self,
        id: &str,
        key: &str,
        (ctrl, alt, shift, meta): (bool, bool, bool, bool),
    ) -> Result<(), Reject> {
        if is_modifier(key) {
            return Err(Reject::Wait);
        }
        if meta {
            return Err(Reject::Unsupported);
        }
        let chord = Chord::from_event(key, ctrl, alt, shift).ok_or(Reject::Unsupported)?;
        self.rebind(id, chord)
    }

    /// Binds `id` to `chord` unless that is not allowed (see [`Reject`]). The same chord it
    /// already has is accepted and changes nothing.
    pub fn rebind(&mut self, id: &str, chord: Chord) -> Result<(), Reject> {
        let i = Self::index(id).ok_or(Reject::Unsupported)?;
        check(&chord)?;
        match self.owner(&chord) {
            Some(owner) if owner != i => Err(Reject::Taken(ACTIONS[owner].0)),
            _ => {
                self.chords[i] = chord;
                Ok(())
            }
        }
    }

    pub fn reset(&mut self, id: &str) {
        if let Some(i) = Self::index(id) {
            self.chords[i] = default_chord(i);
        }
    }

    pub fn reset_all(&mut self) {
        *self = Bindings::defaults();
    }

    /// The chords that differ from the default, by action id: what is saved.
    pub fn overrides(&self) -> BTreeMap<String, String> {
        ACTIONS
            .iter()
            .enumerate()
            .filter(|(i, _)| self.chords[*i] != default_chord(*i))
            .map(|(i, (id, _))| ((*id).to_owned(), self.chords[i].to_string()))
            .collect()
    }
}

thread_local! {
    /// What the window's keys mean now (the UI lives on one thread).
    static BINDINGS: RefCell<Bindings> = RefCell::new(Bindings::defaults());
    /// Where changes are saved; `None` until [`install`].
    static FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The action of a key press, or `""`. `key` is the key's text (case is ignored).
pub fn action(key: &str, ctrl: bool, alt: bool, shift: bool) -> &'static str {
    match Chord::from_event(key, ctrl, alt, shift) {
        Some(chord) => BINDINGS.with(|b| b.borrow().action(&chord)),
        None => "",
    }
}

/// The chord now bound to action `id`, as text; `None` when `id` is not a customizable action.
pub fn display(id: &str) -> Option<String> {
    BINDINGS.with(|b| b.borrow().display(id))
}

/// Shows the bindings to the Settings page and the shortcut list.
fn push(window: &AppWindow) {
    let (rows, customized) = BINDINGS.with(|b| {
        let b = b.borrow();
        let rows: Vec<ShortcutItem> = ACTIONS
            .iter()
            .enumerate()
            .map(|(i, (id, _))| ShortcutItem {
                id: (*id).into(),
                keys: b.chords[i].to_string().into(),
                default_keys: default_chord(i).to_string().into(),
                custom: b.chords[i] != default_chord(i),
            })
            .collect();
        let customized = rows.iter().any(|r| r.custom);
        (rows, customized)
    });
    let store = window.global::<ShortcutStore>();
    store.set_items(ModelRc::new(VecModel::from(rows)));
    store.set_customized(customized);
    // The chord texts that are looked up by id (`keys`) are drawn again.
    store.set_version(store.get_version() + 1);
}

/// Saves what differs from the defaults.
fn save() {
    let Some(path) = FILE.with(|f| f.borrow().clone()) else {
        return;
    };
    let prefs = ShortcutPrefs {
        bindings: BINDINGS.with(|b| b.borrow().overrides()),
    };
    if let Err(e) = prefs.save_to(&path) {
        tracing::warn!(%e, "shortcuts: could not save");
    }
}

/// Attaches the shortcut table to a window with the default chords (what a test gets);
/// [`install`] then loads the saved ones.
pub fn wire(window: &AppWindow) {
    BINDINGS.with(|b| *b.borrow_mut() = Bindings::defaults());
    FILE.with(|f| *f.borrow_mut() = None);
    let store = window.global::<ShortcutStore>();
    store.on_keys(|id: SharedString, _version: i32| display(&id).unwrap_or_default().into());
    let weak = window.as_weak();
    store.on_rebind(
        move |id: SharedString,
              key: SharedString,
              ctrl: bool,
              alt: bool,
              shift: bool,
              meta: bool| {
            let result =
                BINDINGS.with(|b| b.borrow_mut().capture(&id, &key, (ctrl, alt, shift, meta)));
            let Some(window) = weak.upgrade() else {
                return SharedString::new();
            };
            match result {
                Ok(()) => {
                    save();
                    push(&window);
                    SharedString::new()
                }
                Err(reject) => {
                    if let Reject::Taken(other) = reject {
                        window.global::<ShortcutStore>().set_conflict(other.into());
                    }
                    reject.code().into()
                }
            }
        },
    );
    let weak = window.as_weak();
    store.on_reset(move |id: SharedString| {
        BINDINGS.with(|b| b.borrow_mut().reset(&id));
        save();
        if let Some(window) = weak.upgrade() {
            push(&window);
        }
    });
    let weak = window.as_weak();
    store.on_reset_all(move || {
        BINDINGS.with(|b| b.borrow_mut().reset_all());
        save();
        if let Some(window) = weak.upgrade() {
            push(&window);
        }
    });
    push(window);
}

/// Loads the saved chords from `dir` (the config directory) and saves changes there.
pub fn install(window: &AppWindow, dir: PathBuf) {
    let path = dir.join(appearance::shortcuts::FILE_NAME);
    let prefs = ShortcutPrefs::load_from(&path);
    BINDINGS.with(|b| *b.borrow_mut() = Bindings::from_prefs(&prefs));
    FILE.with(|f| *f.borrow_mut() = Some(path));
    push(window);
}

/// [`install`] with the real config directory.
pub fn install_system(window: &AppWindow) {
    match appearance::config_dir() {
        Ok(dir) => install(window, dir),
        Err(e) => tracing::warn!(%e, "shortcuts: no config directory, the defaults stay"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        Chord::parse(text).unwrap_or_else(|| panic!("{text} parses"))
    }

    #[test]
    fn every_default_parses_is_allowed_and_is_unique() {
        let b = Bindings::defaults();
        for (i, (id, text)) in ACTIONS.iter().enumerate() {
            let c = Chord::parse(text).unwrap_or_else(|| panic!("{id}: {text} parses"));
            assert_eq!(&c.to_string(), text, "{id} is written the way it is shown");
            assert_eq!(check(&c), Ok(()), "{id}: {text} is allowed");
            assert_eq!(b.owner(&c), Some(i), "{id}: {text} is not shared");
        }
        let ids: std::collections::BTreeSet<_> = ACTIONS.iter().map(|(id, _)| id).collect();
        assert_eq!(ids.len(), ACTIONS.len(), "an action is listed once");
        for r in RESERVED {
            assert!(Chord::parse(r).is_some(), "{r} parses");
            assert!(
                !ACTIONS.iter().any(|(_, d)| chord(d) == chord(r)),
                "{r} is reserved, so it cannot also be a default"
            );
        }
    }

    #[test]
    fn chords_read_and_write_the_same_text() {
        for text in [
            "Ctrl+Alt+E",
            "Alt+Shift+N",
            "Ctrl+Shift+,",
            "Ctrl+.",
            "Ctrl++",
            "Alt+Down",
            "Ctrl+Alt+Home",
            "Ctrl+Alt+Shift+F5",
            "Ctrl+Space",
        ] {
            assert_eq!(chord(text).to_string(), text);
        }
        // Case, spacing and the order of the modifiers are forgiven; the result is canonical.
        assert_eq!(chord("shift+alt+n").to_string(), "Alt+Shift+N");
        assert_eq!(chord(" control+E ").to_string(), "Ctrl+E");
        assert_eq!(chord("ctrl+pagedown").to_string(), "Ctrl+PageDown");
        assert_eq!(chord("Ctrl+Shift+<").to_string(), "Ctrl+Shift+,");
        for bad in ["", "Ctrl+", "Ctrl+Alt", "Ctrl+AB", "Banana+E", "Ctrl+Esc"] {
            assert_eq!(Chord::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_key_press_is_the_same_chord_however_the_layout_spells_it() {
        let ev = |k: &str, c, a, s| Chord::from_event(k, c, a, s).unwrap();
        assert_eq!(ev("E", true, true, false), chord("Ctrl+Alt+E"));
        assert_eq!(ev("e", true, true, false), chord("Ctrl+Alt+E"));
        // Shift+, arrives as "<" on some layouts and as "," on others.
        assert_eq!(ev("<", true, false, true), chord("Ctrl+Shift+,"));
        assert_eq!(ev(",", true, false, true), chord("Ctrl+Shift+,"));
        // Without Shift a "<" is its own key.
        assert_ne!(ev("<", true, false, false), chord("Ctrl+,"));
        assert_eq!(
            ev(&char::from(Key::DownArrow).to_string(), false, true, false),
            chord("Alt+Down")
        );
        assert_eq!(
            ev(&char::from(Key::Home).to_string(), true, true, false),
            chord("Ctrl+Alt+Home")
        );
        assert_eq!(
            ev(&char::from(Key::F5).to_string(), false, false, false),
            chord("F5")
        );
        assert_eq!(ev(" ", true, false, false), chord("Ctrl+Space"));
        // Not part of a chord.
        for k in [
            Key::Escape,
            Key::Return,
            Key::Backspace,
            Key::Control,
            Key::Shift,
        ] {
            assert_eq!(
                Chord::from_event(&char::from(k).to_string(), true, false, false),
                None
            );
        }
        assert_eq!(Chord::from_event("", true, false, false), None);
        assert_eq!(Chord::from_event("ab", true, false, false), None);
        assert!(is_modifier(&char::from(Key::Control).to_string()));
        assert!(is_modifier(&char::from(Key::Shift).to_string()));
        assert!(!is_modifier("e"));
    }

    #[test]
    fn the_defaults_run_the_actions_they_did_before_the_table() {
        let b = Bindings::defaults();
        let run = |k: &str, c, a, s| b.action(&Chord::from_event(k, c, a, s).unwrap());
        assert_eq!(run(".", true, false, false), "shortcuts");
        assert_eq!(run("P", true, false, true), "palette");
        assert_eq!(run("E", true, true, false), "search");
        assert_eq!(run("N", false, true, true), "new-chat");
        assert_eq!(run(",", true, false, true), "settings");
        assert_eq!(run("<", true, false, true), "settings");
        assert_eq!(run("q", false, true, false), "collapse-all");
        assert_eq!(run("Q", false, true, false), "collapse-all");
        assert_eq!(
            run(&char::from(Key::LeftArrow).to_string(), false, true, false),
            "back"
        );
        assert_eq!(
            run(&char::from(Key::RightArrow).to_string(), false, true, false),
            "forward"
        );
        assert_eq!(
            run(&char::from(Key::DownArrow).to_string(), false, true, false),
            "next-unread"
        );
        assert_eq!(
            run(&char::from(Key::UpArrow).to_string(), false, true, false),
            "previous-unread"
        );
        assert_eq!(
            run(&char::from(Key::DownArrow).to_string(), false, true, true),
            "next-unread-wrap"
        );
        assert_eq!(
            run(&char::from(Key::UpArrow).to_string(), false, true, true),
            "previous-unread-wrap"
        );
        assert_eq!(run("U", false, true, true), "unreads");
        // Chords that are not bound, among them the ones other code keeps.
        assert_eq!(run("c", true, true, true), "");
        assert_eq!(run("b", true, false, false), "");
        assert_eq!(run("x", true, true, false), "");
        assert_eq!(run("e", false, false, false), "");
    }

    #[test]
    fn rebinding_moves_the_action_and_frees_the_old_chord() {
        let mut b = Bindings::defaults();
        assert_eq!(b.rebind("new-chat", chord("Ctrl+Alt+N")), Ok(()));
        assert_eq!(b.action(&chord("Ctrl+Alt+N")), "new-chat");
        assert_eq!(b.action(&chord("Alt+Shift+N")), "", "the old chord is free");
        assert_eq!(b.display("new-chat").as_deref(), Some("Ctrl+Alt+N"));
        assert_eq!(b.overrides().len(), 1);
        assert_eq!(b.overrides()["new-chat"], "Ctrl+Alt+N");
        // The freed chord can go to another action.
        assert_eq!(b.rebind("search", chord("Alt+Shift+N")), Ok(()));
        // Binding an action to the chord it already has is fine.
        assert_eq!(b.rebind("search", chord("Alt+Shift+N")), Ok(()));
    }

    #[test]
    fn a_chord_another_action_has_is_refused_and_nothing_changes() {
        let mut b = Bindings::defaults();
        // Ctrl+Alt+G is "goto"; Search may not take it.
        assert_eq!(
            b.rebind("search", chord("Ctrl+Alt+G")),
            Err(Reject::Taken("goto"))
        );
        assert_eq!(b.display("search").as_deref(), Some("Ctrl+Alt+E"));
        assert_eq!(b.action(&chord("Ctrl+Alt+G")), "goto");
        assert!(b.overrides().is_empty());
        assert_eq!(Reject::Taken("goto").code(), "taken");
    }

    #[test]
    fn keys_that_would_break_typing_or_belong_to_others_are_refused() {
        let mut b = Bindings::defaults();
        // A letter alone, or with Shift, would take over typing.
        assert_eq!(b.rebind("find", chord("F")), Err(Reject::NeedsModifier));
        assert_eq!(
            b.rebind("find", chord("Shift+F")),
            Err(Reject::NeedsModifier)
        );
        assert_eq!(b.rebind("find", chord("Tab")), Err(Reject::NeedsModifier));
        // A function key needs no modifier.
        assert_eq!(b.rebind("find", chord("F3")), Ok(()));
        // The text boxes, formatting, the voice recorder, the call keys and the app bar keep theirs.
        for reserved in [
            "Ctrl+C",
            "Ctrl+V",
            "Ctrl+Z",
            "Ctrl+B",
            "Ctrl+Alt+1",
            "Ctrl+Shift+R",
            "Ctrl+Shift+M",
            "Ctrl+Shift+1",
            "Ctrl+Shift+9",
        ] {
            assert_eq!(
                b.rebind("search", chord(reserved)),
                Err(Reject::Reserved),
                "{reserved}"
            );
        }
        assert_eq!(b.display("search").as_deref(), Some("Ctrl+Alt+E"));
        assert_eq!(
            b.rebind("nope", chord("Ctrl+Alt+K")),
            Err(Reject::Unsupported)
        );
    }

    #[test]
    fn capturing_waits_for_the_key_after_a_modifier_and_refuses_what_cannot_be_a_chord() {
        let mut b = Bindings::defaults();
        let none = (false, false, false, false);
        let ctrl = char::from(Key::Control).to_string();
        assert_eq!(
            b.capture("goto", &ctrl, (true, false, false, false)),
            Err(Reject::Wait)
        );
        let esc = char::from(Key::Escape).to_string();
        assert_eq!(b.capture("goto", &esc, none), Err(Reject::Unsupported));
        assert_eq!(
            b.capture("goto", "g", (true, true, false, true)),
            Err(Reject::Unsupported),
            "Meta"
        );
        assert_eq!(b.capture("goto", "j", (true, true, false, false)), Ok(()));
        assert_eq!(b.display("goto").as_deref(), Some("Ctrl+Alt+J"));
    }

    #[test]
    fn reset_brings_back_one_default_and_reset_all_every_default() {
        let mut b = Bindings::defaults();
        b.rebind("goto", chord("Ctrl+Alt+J")).unwrap();
        b.rebind("find", chord("F3")).unwrap();
        b.reset("goto");
        assert_eq!(b.display("goto").as_deref(), Some("Ctrl+Alt+G"));
        assert_eq!(b.display("find").as_deref(), Some("F3"));
        b.reset("not-an-action");
        b.reset_all();
        assert!(b.overrides().is_empty());
        assert_eq!(b.display("find").as_deref(), Some("Ctrl+F"));
    }

    fn prefs(pairs: &[(&str, &str)]) -> ShortcutPrefs {
        ShortcutPrefs {
            bindings: pairs
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn saved_choices_load_and_bad_ones_fall_back_to_the_default() {
        let b = Bindings::from_prefs(&prefs(&[
            ("new-chat", "Ctrl+Alt+N"),
            // Not a chord, a chord that is reserved, one that needs a modifier, an unknown action.
            ("search", "banana"),
            ("goto", "Ctrl+C"),
            ("find", "F"),
            ("not-an-action", "Ctrl+Alt+K"),
        ]));
        assert_eq!(b.display("new-chat").as_deref(), Some("Ctrl+Alt+N"));
        assert_eq!(b.display("search").as_deref(), Some("Ctrl+Alt+E"));
        assert_eq!(b.display("goto").as_deref(), Some("Ctrl+Alt+G"));
        assert_eq!(b.display("find").as_deref(), Some("Ctrl+F"));
        assert_eq!(b.overrides().len(), 1);
    }

    #[test]
    fn two_saved_actions_never_share_a_chord() {
        // Both want Ctrl+Alt+K: the first in the table keeps it.
        let b = Bindings::from_prefs(&prefs(&[("goto", "Ctrl+Alt+K"), ("search", "Ctrl+Alt+K")]));
        assert_eq!(b.display("search").as_deref(), Some("Ctrl+Alt+K"));
        assert_eq!(b.display("goto").as_deref(), Some("Ctrl+Alt+G"));
        // A chord another action gave up in the same file is free by the time it is wanted:
        // "search" wants goto's old chord, "goto" moved away from it.
        let b = Bindings::from_prefs(&prefs(&[("search", "Ctrl+Alt+G"), ("goto", "Ctrl+Alt+J")]));
        assert_eq!(b.display("search").as_deref(), Some("Ctrl+Alt+G"));
        assert_eq!(b.display("goto").as_deref(), Some("Ctrl+Alt+J"));
        // A saved chord that is another action's default and that action did not move: ignored.
        let b = Bindings::from_prefs(&prefs(&[("search", "Ctrl+Alt+G")]));
        assert_eq!(b.display("search").as_deref(), Some("Ctrl+Alt+E"));
        assert_eq!(b.display("goto").as_deref(), Some("Ctrl+Alt+G"));
    }

    #[test]
    fn saved_choices_round_trip_through_the_prefs() {
        let mut b = Bindings::defaults();
        b.rebind("goto", chord("Ctrl+Alt+J")).unwrap();
        b.rebind("shortcuts", chord("F1")).unwrap();
        let saved = ShortcutPrefs {
            bindings: b.overrides(),
        };
        let back = Bindings::from_prefs(&saved);
        assert_eq!(back.display("goto").as_deref(), Some("Ctrl+Alt+J"));
        assert_eq!(back.display("shortcuts").as_deref(), Some("F1"));
        assert_eq!(back.overrides(), b.overrides());
    }
}
