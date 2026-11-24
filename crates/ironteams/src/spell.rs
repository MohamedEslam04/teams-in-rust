//! Spell check for the composer. Slint's `TextInput` has no spell-check hook, so the draft is
//! checked here and the composer draws the red underlines itself (`ui/molecules/spelling.slint`): it puts
//! the cursor of a hidden copy of the text input at each word's ends to learn where the word is.
//!
//! The dictionaries are the system's Hunspell ones (`/usr/share/hunspell`, `/usr/share/myspell`;
//! on macOS `~/Library/Spelling` and `/Library/Spelling`), the ones packaged next to the program
//! (`dictionaries/` beside the executable, `Contents/Resources/dictionaries` in the macOS app; the
//! Windows and macOS packages ship en_US and ar there) or `dictionaries/` in the config dir:
//! English (the locale's variant first, then en_US, en_GB, any other en_*) and Arabic (ar, then
//! any ar_*). A word is checked against the dictionary of its script; with no dictionary for it,
//! nothing is flagged. "Add to dictionary" keeps words in `spelling-words.txt` in the config dir.
//! Mentions, links, email addresses, paths, `code`, acronyms and camelCase names are not checked.

use crate::{AppWindow, Edited, SpellApi, SpellWord};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use spellbook::Dictionary;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where distributions install Hunspell dictionaries.
const DIRS: [&str; 3] = [
    "/usr/share/hunspell",
    "/usr/share/myspell",
    "/usr/share/myspell/dicts",
];
const MAX_SUGGESTIONS: usize = 5;
const WORDS_FILE: &str = "spelling-words.txt";

/// Where dictionaries are looked for, in order: the system's, then the packaged ones, then the
/// user's.
fn dictionary_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = DIRS.iter().map(PathBuf::from).collect();
    if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Spelling"));
        }
        dirs.push("/Library/Spelling".into());
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        dirs.push(exe_dir.join("dictionaries"));
        dirs.push(exe_dir.join("../Resources/dictionaries"));
    }
    if let Ok(dir) = appearance::config_dir() {
        dirs.push(dir.join("dictionaries"));
    }
    dirs
}

/// The loaded dictionaries and the words the user added.
#[derive(Default)]
pub struct Speller {
    english: Option<Dictionary>,
    arabic: Option<Dictionary>,
    personal: HashSet<String>,
    /// Where added words are appended; `None` keeps them in memory only.
    file: Option<PathBuf>,
}

/// A misspelled word: its byte range in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Miss {
    pub start: usize,
    pub end: usize,
    pub word: String,
}

fn is_arabic(c: char) -> bool {
    matches!(c,
        '\u{0600}'..='\u{06FF}'
        | '\u{0750}'..='\u{077F}'
        | '\u{08A0}'..='\u{08FF}'
        | '\u{FB50}'..='\u{FDFF}'
        | '\u{FE70}'..='\u{FEFF}')
}

/// Combining marks (Latin accents, Arabic harakat), which belong to the word around them.
fn is_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{06D6}'..='\u{06ED}')
}

fn word_char(c: char) -> bool {
    c.is_alphabetic() || is_mark(c)
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

/// A run of text without spaces that is a link, an email address, a path or a file name.
fn link_like(chunk: &str) -> bool {
    let b = chunk.as_bytes();
    let dotted = (1..b.len().saturating_sub(1)).any(|i| {
        b[i] == b'.' && b[i - 1].is_ascii_alphanumeric() && b[i + 1].is_ascii_alphanumeric()
    });
    chunk.contains("://")
        || chunk.starts_with("www.")
        || chunk.contains('/')
        || chunk.contains('\\')
        || (chunk.contains('@') && dotted)
        || dotted
}

/// The byte ranges of the words worth checking, in text order.
fn words(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte = |i: usize| chars.get(i).map_or(text.len(), |&(b, _)| b);
    let mut out = Vec::new();
    let mut code = false;
    let mut skip_chunk = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        if i == 0 || chars[i - 1].1.is_whitespace() {
            let start = byte(i);
            let end = text[start..]
                .find(char::is_whitespace)
                .map_or(text.len(), |n| start + n);
            skip_chunk = link_like(&text[start..end]);
        }
        if c == '`' {
            code = !code;
            i += 1;
            continue;
        }
        if !word_char(c) {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        while j < chars.len()
            && (word_char(chars[j].1)
                || (is_apostrophe(chars[j].1) && chars.get(j + 1).is_some_and(|x| word_char(x.1))))
        {
            j += 1;
        }
        let before = start.checked_sub(1).map(|k| chars[k].1);
        let after = chars.get(j).map(|x| x.1);
        // Glued to digits or symbols: an id, a #tag, an @name or an `:emoji:` code.
        let glued = |c: Option<char>| {
            c.is_some_and(|c| c.is_numeric() || matches!(c, '_' | '@' | '#' | ':' | '$' | '%'))
        };
        if !code && !skip_chunk && !glued(before) && !(glued(after) && after != Some(':')) {
            out.push((byte(start), byte(j)));
        }
        i = j;
    }
    out
}

/// Byte ranges of the picked `@Name` mentions (`names`: one per line).
fn mention_ranges(text: &str, names: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for name in names.lines().filter(|n| !n.is_empty()) {
        let token = format!("@{name}");
        let mut from = 0;
        while let Some(rel) = text[from..].find(&token) {
            let start = from + rel;
            out.push((start, start + token.len()));
            from = start + token.len();
        }
    }
    out
}

/// Acronyms (`API`) and names with a capital inside (`GitHub`, `iPhone`) are left alone.
fn skip_case(word: &str) -> bool {
    let letters: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
    let upper = letters.iter().filter(|c| c.is_uppercase()).count();
    (upper > 0 && upper == letters.len()) || letters.iter().skip(1).any(|c| c.is_uppercase())
}

impl Speller {
    pub fn new(english: Option<Dictionary>, arabic: Option<Dictionary>) -> Speller {
        Speller {
            english,
            arabic,
            ..Default::default()
        }
    }

    /// The system dictionaries and the words added earlier. Slow (parses the dictionaries).
    pub fn system() -> Speller {
        let dirs = dictionary_dirs();
        let stems = dictionary_stems(&dirs);
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .unwrap_or_default();
        let load = |lang: &str| {
            let stem = pick(&stems, lang, &locale)?;
            let found = dirs.iter().find_map(|d| {
                let aff = d.join(format!("{stem}.aff"));
                let dic = d.join(format!("{stem}.dic"));
                (aff.is_file() && dic.is_file()).then_some((aff, dic))
            })?;
            let dict = read_dictionary(&found.0, &found.1);
            if dict.is_none() {
                tracing::info!(%stem, "spelling dictionary could not be read");
            }
            dict
        };
        let mut s = Speller::new(load("en"), load("ar"));
        if let Ok(dir) = appearance::config_dir() {
            let file = dir.join(WORDS_FILE);
            if let Ok(text) = std::fs::read_to_string(&file) {
                s.personal = text
                    .lines()
                    .map(str::trim)
                    .filter(|w| !w.is_empty())
                    .map(String::from)
                    .collect();
            }
            s.file = Some(file);
        }
        s
    }

    fn dictionary_for(&self, word: &str) -> Option<&Dictionary> {
        if word.chars().any(is_arabic) {
            self.arabic.as_ref()
        } else if word
            .chars()
            .all(|c| (c as u32) < 0x250 || is_mark(c) || is_apostrophe(c))
        {
            self.english.as_ref()
        } else {
            None
        }
    }

    fn correct(&self, word: &str) -> bool {
        let word = word.replace('\u{2019}', "'");
        if self.personal.contains(&word) || self.personal.contains(&word.to_lowercase()) {
            return true;
        }
        self.dictionary_for(&word).is_none_or(|d| d.check(&word))
    }

    /// The misspelled words of `text`. `names` are the picked mentions (one per line); the word
    /// that ends at `cursor` is still being typed and is not flagged.
    pub fn misspelled(&self, text: &str, names: &str, cursor: Option<usize>) -> Vec<Miss> {
        if self.english.is_none() && self.arabic.is_none() {
            return Vec::new();
        }
        let mentions = mention_ranges(text, names);
        words(text)
            .into_iter()
            .filter(|&(s, e)| !mentions.iter().any(|&(ms, me)| s < me && ms < e))
            .filter(|&(_, e)| cursor != Some(e))
            .filter_map(|(start, end)| {
                let word = &text[start..end];
                let short = word.chars().filter(|c| c.is_alphabetic()).count() < 2;
                (!short && !skip_case(word) && !self.correct(word)).then(|| Miss {
                    start,
                    end,
                    word: word.to_string(),
                })
            })
            .collect()
    }

    /// Corrections for a misspelled word, best first (see [`rank`]).
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let word = word.replace('\u{2019}', "'");
        let mut out = Vec::new();
        if let Some(d) = self.dictionary_for(&word) {
            d.suggest(&word, &mut out);
        }
        let mut out = rank(&word, out);
        out.truncate(MAX_SUGGESTIONS);
        out
    }

    /// "Add to dictionary": the word is never flagged again, here and after a restart.
    pub fn add_word(&mut self, word: &str) {
        let word = word.trim();
        if word.is_empty() || !self.personal.insert(word.to_string()) {
            return;
        }
        if let Some(file) = &self.file {
            let saved = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(file)
                })
                .and_then(|mut f| writeln!(f, "{word}"));
            if let Err(e) = saved {
                tracing::warn!(error = %e, "could not save the spelling word");
            }
        }
    }
}

/// The `.dic` file names (without extension) found in `dirs` that have an `.aff` next to them.
fn dictionary_stems(dirs: &[PathBuf]) -> Vec<String> {
    let mut out: Vec<String> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension()? == "dic" && p.with_extension("aff").is_file())
                .then(|| p.file_stem()?.to_str().map(String::from))?
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The dictionary to use for `lang` (`en`, `ar`) among `stems`: the locale's variant
/// (`en_GB.UTF-8` gives `en_GB`), then the usual one, then any of the language.
fn pick(stems: &[String], lang: &str, locale: &str) -> Option<String> {
    let region = locale.split(['.', '@']).next().unwrap_or_default();
    let usual: &[&str] = match lang {
        "en" => &["en_US", "en_GB", "en"],
        _ => &["ar", "ar_SA", "ar_EG"],
    };
    let of_lang = |s: &str| {
        s == lang || s.starts_with(&format!("{lang}_")) || s.starts_with(&format!("{lang}-"))
    };
    std::iter::once(region)
        .filter(|r| of_lang(r))
        .chain(usual.iter().copied())
        .find(|want| stems.iter().any(|s| s == want))
        .map(String::from)
        .or_else(|| stems.iter().find(|s| of_lang(s)).cloned())
}

/// Hunspell files are UTF-8 or Latin-1 (`SET ISO8859-1`); other encodings are skipped.
fn decode(bytes: Vec<u8>, latin1: bool) -> Option<String> {
    if latin1 {
        Some(bytes.into_iter().map(char::from).collect())
    } else {
        String::from_utf8(bytes).ok()
    }
}

fn read_dictionary(aff: &Path, dic: &Path) -> Option<Dictionary> {
    let aff_bytes = std::fs::read(aff).ok()?;
    let set = aff_bytes
        .split(|&b| b == b'\n')
        .find_map(|l| l.strip_prefix(b"SET "))
        .map(|v| String::from_utf8_lossy(v).trim().to_uppercase())
        .unwrap_or_else(|| "UTF-8".into());
    let latin1 = match set.as_str() {
        "UTF-8" => false,
        "ISO8859-1" | "ISO-8859-1" => true,
        _ => return None,
    };
    let mut aff_text = decode(aff_bytes, latin1)?;
    if latin1 {
        aff_text = aff_text.replacen("SET ISO8859-1", "SET UTF-8", 1);
    }
    let dic_text = decode(std::fs::read(dic).ok()?, latin1)?;
    Dictionary::new(&aff_text, &dic_text).ok()
}

/// The QWERTY rows, for "the finger slipped to the next key".
const QWERTY: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];

/// Whether `a` and `b` are next to each other on a QWERTY keyboard (same row or the row
/// above/below, at most one column apart).
fn neighbor_keys(a: char, b: char) -> bool {
    let at = |c: char| {
        QWERTY
            .iter()
            .enumerate()
            .find_map(|(r, row)| row.find(c).map(|col| (r as i32, col as i32)))
    };
    match (at(a), at(b)) {
        (Some((ra, ca)), Some((rb, cb))) => a != b && (ra - rb).abs() <= 1 && (ca - cb).abs() <= 1,
        _ => false,
    }
}

/// How `word` becomes `fix`: the edit distance (Levenshtein with adjacent transpositions, by
/// character) and, for one edit, which kind it is: 0 two neighbours swapped, 1 a letter replaced
/// by a neighbouring key, 2 another letter replaced, 3 a letter added or dropped.
fn edit(word: &[char], fix: &[char]) -> (usize, u8) {
    let (n, m) = (word.len(), fix.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(word[i - 1] != fix[j - 1]);
            let mut best = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && word[i - 1] == fix[j - 2] && word[i - 2] == fix[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    let kind = if n != m {
        3
    } else {
        let diff: Vec<usize> = (0..n).filter(|&i| word[i] != fix[i]).collect();
        match diff[..] {
            [a, b] if b == a + 1 && word[a] == fix[b] && word[b] == fix[a] => 0,
            [a] if neighbor_keys(word[a], fix[a]) => 1,
            _ => 2,
        }
    };
    (d[n][m], kind)
}

/// The dictionary's suggestions in the order a typo most likely meant them: fewest edits first;
/// for one edit, two swapped neighbours, then a slip to the next key, then another letter of the
/// same length, then a letter added or dropped; a suggestion that keeps the first letter (it is
/// rarely the mistyped one) before one that changes it; splitting into two words last. Ties keep
/// the dictionary's order. So "tset" gives "test" before "stet".
fn rank(word: &str, suggestions: Vec<String>) -> Vec<String> {
    let lower = |w: &str| w.to_lowercase().chars().collect::<Vec<char>>();
    let typed = lower(word);
    let mut keyed: Vec<_> = suggestions
        .into_iter()
        .map(|fix| {
            let f = lower(&fix);
            let (distance, kind) = edit(&typed, &f);
            let split = fix.contains(' ') || fix.contains('-');
            let first_changed = typed.first() != f.first();
            ((split, distance, kind, first_changed), fix)
        })
        .collect();
    keyed.sort_by_key(|(k, _)| *k);
    keyed.into_iter().map(|(_, fix)| fix).collect()
}

/// Bytes at the start that `a` and `b` share, at a character boundary. Underlines of words in
/// that part are still in place while the draft changes.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((i, _), _)| i)
}

/// `text` with `start..end` replaced by `with`, the cursor after it.
fn replace(text: &str, start: i32, end: i32, with: &str) -> Edited {
    let ok = |n: i32| {
        usize::try_from(n)
            .ok()
            .filter(|&n| text.is_char_boundary(n))
    };
    match (ok(start), ok(end)) {
        (Some(s), Some(e)) if s <= e => Edited {
            text: format!("{}{with}{}", &text[..s], &text[e..]).into(),
            cursor: (s + with.len()) as i32,
        },
        _ => Edited {
            text: text.into(),
            cursor: -1,
        },
    }
}

thread_local! {
    /// The speller of the window's thread (Slint calls the pure callbacks there).
    static SPELLER: RefCell<Speller> = RefCell::new(Speller::default());
    static LOADING: Cell<bool> = const { Cell::new(false) };
}

fn bump(w: &AppWindow) {
    let api = w.global::<SpellApi>();
    api.set_generation(api.get_generation() + 1);
}

/// Attaches the spelling functions to a window.
pub fn wire(window: &AppWindow) {
    let api = window.global::<SpellApi>();
    api.on_check(
        |text: SharedString, names: SharedString, cursor, _generation| {
            let found = SPELLER.with(|s| {
                s.borrow()
                    .misspelled(&text, &names, usize::try_from(cursor).ok())
            });
            // The underlines are drawn for left-aligned lines: a word in a right-to-left line (it is
            // right-aligned in the composer) gets none.
            let rows: Vec<SpellWord> = found
                .into_iter()
                .filter(|m| {
                    let (line, _) = render::lines::locate(&text, m.start);
                    render::lines::line_direction(&text, line) != Some(render::Dir::Rtl)
                })
                .map(|m| SpellWord {
                    start: m.start as i32,
                    end: m.end as i32,
                    word: m.word.into(),
                })
                .collect();
            ModelRc::new(VecModel::from(rows))
        },
    );
    api.on_suggest(|word: SharedString| {
        let found: Vec<SharedString> = SPELLER
            .with(|s| s.borrow().suggest(&word))
            .into_iter()
            .map(SharedString::from)
            .collect();
        ModelRc::new(VecModel::from(found))
    });
    api.on_stable(|a: SharedString, b: SharedString| common_prefix(&a, &b) as i32);
    api.on_replace(|text: SharedString, start, end, with: SharedString| {
        replace(&text, start, end, &with)
    });
    let weak = window.as_weak();
    api.on_add_word(move |word: SharedString| {
        SPELLER.with(|s| s.borrow_mut().add_word(&word));
        if let Some(w) = weak.upgrade() {
            bump(&w);
        }
    });
}

/// Loads the system dictionaries on a worker thread, once (spell check on).
pub fn ensure_loaded(window: &AppWindow) {
    if cfg!(test) || LOADING.replace(true) {
        return;
    }
    let weak = window.as_weak();
    std::thread::spawn(move || {
        let speller = Speller::system();
        let _ = weak.upgrade_in_event_loop(move |w| {
            SPELLER.with(|s| *s.borrow_mut() = speller);
            bump(&w);
        });
    });
}

/// Puts a speller in place of the system one (tests).
#[cfg(test)]
pub fn install(speller: Speller) {
    SPELLER.with(|s| *s.borrow_mut() = speller);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny English dictionary.
    pub(crate) fn english() -> Dictionary {
        let aff = "SET UTF-8\nTRY esianrtolcdugmphbyfvkwz'\n";
        let dic = "16\nhello\nworld\nthe\nmessage\nteam\nsend\nplease\ndon't\nhelp\nsee\nmail\nopen\nrun\nand\nor\nticket\n";
        Dictionary::new(aff, dic).unwrap()
    }

    pub(crate) fn arabic() -> Dictionary {
        Dictionary::new("SET UTF-8\n", "2\nمرحبا\nعالم\n").unwrap()
    }

    pub(crate) fn speller() -> Speller {
        Speller::new(Some(english()), Some(arabic()))
    }

    fn flagged(s: &Speller, text: &str) -> Vec<String> {
        s.misspelled(text, "", None)
            .into_iter()
            .map(|m| m.word)
            .collect()
    }

    #[test]
    fn misspelled_words_come_with_their_byte_range() {
        let s = speller();
        let found = s.misspelled("hello wrold, the mesage", "", None);
        assert_eq!(
            found,
            vec![
                Miss {
                    start: 6,
                    end: 11,
                    word: "wrold".into()
                },
                Miss {
                    start: 17,
                    end: 23,
                    word: "mesage".into()
                },
            ]
        );
        assert!(
            flagged(&s, "Hello World. Don't help!").is_empty(),
            "case and apostrophes"
        );
    }

    #[test]
    fn arabic_words_use_the_arabic_dictionary_and_offsets_are_bytes() {
        let s = speller();
        let text = "مرحبا يا عالمم";
        let found = s.misspelled(text, "", None);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(&text[found[1].start..found[1].end], "عالمم");
        // Without an Arabic dictionary Arabic is not flagged.
        let en_only = Speller::new(Some(english()), None);
        assert!(flagged(&en_only, "عالمم helo") == vec!["helo".to_string()]);
    }

    #[test]
    fn mentions_links_code_names_and_the_word_being_typed_are_skipped() {
        let s = speller();
        assert!(flagged(&s, "@Sara Hassan please help").contains(&"Hassan".to_string()));
        let found = s.misspelled("@Sara Hassan please help", "Sara Hassan", None);
        assert!(found.is_empty(), "{found:?}");
        for text in [
            "see https://exmple.com/pathh",
            "mail sara@contosso.com",
            "open notes.txtt",
            "run `cargoo buildd` please",
            "the API and GitHub",
            "ticket ABC123 or abc_def or #hashtg",
            "x y z", // single letters
        ] {
            assert!(
                flagged(&s, text).is_empty(),
                "{text}: {:?}",
                flagged(&s, text)
            );
        }
        let text = "hello wrold";
        assert!(
            s.misspelled(text, "", Some(text.len())).is_empty(),
            "still typing"
        );
        assert_eq!(s.misspelled(text, "", Some(5)).len(), 1);
    }

    #[test]
    fn no_dictionary_flags_nothing() {
        assert!(Speller::default().misspelled("wrold", "", None).is_empty());
    }

    #[test]
    fn suggestions_and_added_words() {
        let mut s = speller();
        assert_eq!(
            s.suggest("wrold").first().map(String::as_str),
            Some("world")
        );
        s.add_word("ironteams");
        assert!(flagged(&s, "ironteams wrold") == vec!["wrold".to_string()]);
    }

    #[test]
    fn added_words_are_saved_and_read_back() {
        let dir = std::env::temp_dir().join(format!("ironteams-spell-{}", std::process::id()));
        let file = dir.join(WORDS_FILE);
        let mut s = speller();
        s.file = Some(file.clone());
        s.add_word("ironteams");
        s.add_word("ironteams");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "ironteams\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_locale_variant_wins_then_the_usual_one() {
        let stems: Vec<String> = ["ar_EG", "en_AU", "en_GB", "en_US"]
            .map(String::from)
            .to_vec();
        assert_eq!(pick(&stems, "en", "en_GB.UTF-8").as_deref(), Some("en_GB"));
        assert_eq!(pick(&stems, "en", "de_DE.UTF-8").as_deref(), Some("en_US"));
        assert_eq!(pick(&stems, "ar", "").as_deref(), Some("ar_EG"));
        assert_eq!(
            pick(&["en_AU".to_string()], "en", "").as_deref(),
            Some("en_AU")
        );
        assert_eq!(pick(&[], "ar", "ar_EG.UTF-8"), None);
    }

    #[test]
    fn latin1_dictionaries_are_decoded() {
        assert_eq!(decode(vec![b'c', 0xE9], true).as_deref(), Some("cé"));
        assert_eq!(decode(vec![0xE9], false), None);
    }

    #[test]
    fn suggestions_put_the_likely_typo_first() {
        // What the menu listed for "tset" in the live run.
        let live = ["stet", "test", "set", "sett", "t set"]
            .map(String::from)
            .to_vec();
        assert_eq!(rank("tset", live), ["test", "stet", "set", "sett", "t set"]);
        // A slip to the next key beats another letter; same length beats a dropped letter.
        let got = rank("tewt", ["tent", "tet", "test"].map(String::from).to_vec());
        assert_eq!(got, ["test", "tent", "tet"]);
        // Case does not change the order and ties keep the dictionary's order.
        let got = rank("Tset", ["Stet", "Test"].map(String::from).to_vec());
        assert_eq!(got, ["Test", "Stet"]);
        let got = rank("abc", ["abq", "abp"].map(String::from).to_vec());
        assert_eq!(got, ["abq", "abp"]);
        let got = rank("abc", ["abp", "abq"].map(String::from).to_vec());
        assert_eq!(got, ["abp", "abq"]);
    }

    #[test]
    fn the_speller_ranks_the_dictionarys_suggestions() {
        let dic = "5\nstet\ntest\nset\nsett\nt\n";
        let d = Dictionary::new("SET UTF-8\nTRY esianrtolcdugmphbyfvkwz'\n", dic).unwrap();
        let s = Speller::new(Some(d), None);
        assert_eq!(s.suggest("tset").first().map(String::as_str), Some("test"));
    }

    #[test]
    fn the_shared_prefix_stops_at_the_first_change() {
        assert_eq!(common_prefix("hello wrold", "hello wrold!"), 11);
        assert_eq!(common_prefix("hello wrold", "hellx"), 4);
        assert_eq!(common_prefix("عالم", "عالب"), "عال".len());
        assert_eq!(common_prefix("", "abc"), 0);
    }

    #[test]
    fn replacing_a_word_puts_the_cursor_after_it() {
        let e = replace("hello wrold!", 6, 11, "world");
        assert_eq!(e.text, "hello world!");
        assert_eq!(e.cursor, 11);
        assert_eq!(replace("ab", 5, 9, "x").cursor, -1, "out of range");
    }
}
