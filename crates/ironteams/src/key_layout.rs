//! Shortcuts on any keyboard layout. Slint reads a key by the character the layout gives it: on
//! the Arabic layout Ctrl+V arrives as Ctrl+"ر", so neither the text boxes' own Copy, Paste, Cut,
//! Select all and Undo nor the app's chords see their letter. Native apps go by the key's place
//! instead. With Ctrl, Cmd or Alt held on a key whose layout character is not Latin, the window
//! hook (`main.rs`) sends the event on with the US letter of that key and drops the original.
//! Typing without those modifiers, Latin layouts, AltGr and the input method stay as they are.

use slint::SharedString;
use slint::platform::WindowEvent as SlintEvent;
use slint::winit_030::EventResult;
use slint::winit_030::winit::event::{ElementState, KeyEvent, WindowEvent};
use slint::winit_030::winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use std::cell::Cell;

/// The modifiers held with a key.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The logo key; Cmd on macOS.
    pub meta: bool,
}

impl From<ModifiersState> for Mods {
    fn from(m: ModifiersState) -> Self {
        Mods {
            ctrl: m.control_key(),
            alt: m.alt_key(),
            shift: m.shift_key(),
            meta: m.super_key(),
        }
    }
}

/// macOS' Option types characters, so Option alone does not make a shortcut there.
const OPTION_TYPES: bool = cfg!(target_os = "macos");

/// The character a physical key types on the US layout, without Shift.
pub(crate) fn us_key(code: KeyCode) -> Option<char> {
    use KeyCode as K;
    Some(match code {
        K::KeyA => 'a',
        K::KeyB => 'b',
        K::KeyC => 'c',
        K::KeyD => 'd',
        K::KeyE => 'e',
        K::KeyF => 'f',
        K::KeyG => 'g',
        K::KeyH => 'h',
        K::KeyI => 'i',
        K::KeyJ => 'j',
        K::KeyK => 'k',
        K::KeyL => 'l',
        K::KeyM => 'm',
        K::KeyN => 'n',
        K::KeyO => 'o',
        K::KeyP => 'p',
        K::KeyQ => 'q',
        K::KeyR => 'r',
        K::KeyS => 's',
        K::KeyT => 't',
        K::KeyU => 'u',
        K::KeyV => 'v',
        K::KeyW => 'w',
        K::KeyX => 'x',
        K::KeyY => 'y',
        K::KeyZ => 'z',
        K::Digit0 => '0',
        K::Digit1 => '1',
        K::Digit2 => '2',
        K::Digit3 => '3',
        K::Digit4 => '4',
        K::Digit5 => '5',
        K::Digit6 => '6',
        K::Digit7 => '7',
        K::Digit8 => '8',
        K::Digit9 => '9',
        K::Minus => '-',
        K::Equal => '=',
        K::BracketLeft => '[',
        K::BracketRight => ']',
        K::Backslash => '\\',
        K::Semicolon => ';',
        K::Quote => '\'',
        K::Comma => ',',
        K::Period => '.',
        K::Slash => '/',
        K::Backquote => '`',
        _ => return None,
    })
}

/// The US key of an Arabic (101) layout character, for a key whose physical place is unknown.
fn arabic_key(base: &str) -> Option<char> {
    const KEYS: &[(&str, char)] = &[
        ("ض", 'q'),
        ("ص", 'w'),
        ("ث", 'e'),
        ("ق", 'r'),
        ("ف", 't'),
        ("غ", 'y'),
        ("ع", 'u'),
        ("ه", 'i'),
        ("خ", 'o'),
        ("ح", 'p'),
        ("ج", '['),
        ("د", ']'),
        ("ش", 'a'),
        ("س", 's'),
        ("ي", 'd'),
        ("ب", 'f'),
        ("ل", 'g'),
        ("ا", 'h'),
        ("ت", 'j'),
        ("ن", 'k'),
        ("م", 'l'),
        ("ك", ';'),
        ("ط", '\''),
        ("ئ", 'z'),
        ("ء", 'x'),
        ("ؤ", 'c'),
        ("ر", 'v'),
        ("لا", 'b'),
        ("ﻻ", 'b'),
        ("ى", 'n'),
        ("ة", 'm'),
        ("و", ','),
        ("ز", '.'),
        ("ظ", '/'),
        ("ذ", '`'),
    ];
    KEYS.iter().find(|(k, _)| *k == base).map(|(_, c)| *c)
}

/// The US character of a key with Shift held.
fn shifted(c: char) -> char {
    match c {
        'a'..='z' => c.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        _ => c,
    }
}

/// A letter or digit of a script other than Latin (Arabic, Persian, Hebrew, Cyrillic, Greek…).
fn non_latin(c: char) -> bool {
    c.is_alphanumeric()
        && !c.is_ascii()
        && !matches!(c, '\u{80}'..='\u{24F}' | '\u{1E00}'..='\u{1EFF}')
}

/// The character a shortcut should carry instead of the layout's, or `None` to leave the key as
/// it is. `text` is what the key typed with the modifiers (the layout's level for Shift or
/// AltGr), `base` what it types without any ("" when unknown), `us` the key's US character.
pub(crate) fn latin(text: &str, base: &str, us: Option<char>, m: Mods) -> Option<char> {
    let command = m.ctrl || m.meta || (m.alt && !OPTION_TYPES);
    if !command || text.is_empty() {
        return None;
    }
    let base = if base.is_empty() { text } else { base };
    if !base.chars().next().is_some_and(non_latin) {
        return None;
    }
    // Windows' AltGr is Ctrl+Alt (macOS' Option is Alt): a character other than the key's own
    // means the chord typed it, and it stays a character.
    if m.alt && !m.shift && text.to_lowercase() != base.to_lowercase() {
        return None;
    }
    let plain = us.or_else(|| arabic_key(base))?;
    Some(if m.shift { shifted(plain) } else { plain })
}

/// Sends a remapped key on to Slint: a press, a held key's repeat or a release.
pub(crate) fn send(win: &slint::Window, c: char, state: ElementState, repeat: bool) {
    let text = SharedString::from(c.to_string());
    win.dispatch_event(match (state, repeat) {
        (ElementState::Released, _) => SlintEvent::KeyReleased { text },
        (ElementState::Pressed, true) => SlintEvent::KeyPressRepeated { text },
        (ElementState::Pressed, false) => SlintEvent::KeyPressed { text },
    });
}

/// What the layout types on this key without modifiers, where winit can tell.
fn base_of(event: &KeyEvent) -> String {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        use slint::winit_030::winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        if let Key::Character(s) = event.key_without_modifiers() {
            return s.to_string();
        }
    }
    let _ = event;
    String::new()
}

/// The window hook's part: follows the modifiers and turns shortcuts typed on a non-Latin
/// layout into their US letter.
#[derive(Default)]
pub(crate) struct Shortcuts {
    mods: Cell<ModifiersState>,
}

impl Shortcuts {
    /// `PreventDefault` when the key went on remapped, else `Propagate`.
    pub(crate) fn event(&self, win: &slint::Window, event: &WindowEvent) -> EventResult {
        match event {
            WindowEvent::ModifiersChanged(m) => self.mods.set(m.state()),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                let Key::Character(text) = &event.logical_key else {
                    return EventResult::Propagate;
                };
                let us = match event.physical_key {
                    PhysicalKey::Code(code) => us_key(code),
                    PhysicalKey::Unidentified(_) => None,
                };
                if let Some(c) = latin(text, &base_of(event), us, self.mods.get().into()) {
                    send(win, c, event.state, event.repeat);
                    return EventResult::PreventDefault;
                }
            }
            _ => {}
        }
        EventResult::Propagate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Mods = Mods {
        ctrl: true,
        alt: false,
        shift: false,
        meta: false,
    };
    const CTRL_SHIFT: Mods = Mods {
        shift: true,
        ..CTRL
    };
    const CTRL_ALT: Mods = Mods { alt: true, ..CTRL };
    const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };

    fn key(code: KeyCode) -> Option<char> {
        us_key(code)
    }

    #[test]
    fn ctrl_with_arabic_letters_gives_the_latin_letter_of_the_key() {
        for (ar, code, latin_c) in [
            ("ر", KeyCode::KeyV, 'v'),
            ("ؤ", KeyCode::KeyC, 'c'),
            ("ء", KeyCode::KeyX, 'x'),
            ("ش", KeyCode::KeyA, 'a'),
            ("ئ", KeyCode::KeyZ, 'z'),
            ("غ", KeyCode::KeyY, 'y'),
            ("ب", KeyCode::KeyF, 'f'),
            ("ن", KeyCode::KeyK, 'k'),
            ("ث", KeyCode::KeyE, 'e'),
        ] {
            assert_eq!(latin(ar, ar, key(code), CTRL), Some(latin_c), "Ctrl+{ar}");
        }
    }

    #[test]
    fn punctuation_keys_give_their_us_character() {
        assert_eq!(latin("ز", "ز", key(KeyCode::Period), CTRL), Some('.'));
        assert_eq!(latin("ظ", "ظ", key(KeyCode::Slash), CTRL), Some('/'));
        // Ctrl+Shift+, (Settings) arrives as Ctrl+Shift+< on the US layout.
        assert_eq!(latin("،", "و", key(KeyCode::Comma), CTRL_SHIFT), Some('<'));
    }

    #[test]
    fn ctrl_shift_gives_the_capital_even_where_the_shifted_level_is_ascii() {
        // Shift+V types "{" on the Arabic layout; the key is still V.
        assert_eq!(latin("{", "ر", key(KeyCode::KeyV), CTRL_SHIFT), Some('V'));
        // Shift+X types a sukun.
        assert_eq!(
            latin("\u{652}", "ء", key(KeyCode::KeyX), CTRL_SHIFT),
            Some('X')
        );
        assert_eq!(latin("ض", "ض", key(KeyCode::KeyQ), CTRL_SHIFT), Some('Q'));
    }

    #[test]
    fn typing_arabic_without_a_command_modifier_is_untouched() {
        assert_eq!(latin("ر", "ر", key(KeyCode::KeyV), NONE), None);
        let shift = Mods {
            shift: true,
            ..NONE
        };
        assert_eq!(latin("{", "ر", key(KeyCode::KeyV), shift), None);
        assert_eq!(latin("لا", "لا", key(KeyCode::KeyB), NONE), None);
    }

    #[test]
    fn latin_layouts_are_untouched() {
        assert_eq!(latin("v", "v", key(KeyCode::KeyV), CTRL), None);
        assert_eq!(latin("V", "v", key(KeyCode::KeyV), CTRL_SHIFT), None);
        // AZERTY: the A key is Q; Ctrl+A stays Ctrl+A, not Ctrl+Q.
        assert_eq!(latin("a", "a", key(KeyCode::KeyQ), CTRL), None);
        // Latin letters beyond ASCII (French é, German ö) are the layout's own keys.
        assert_eq!(latin("é", "é", key(KeyCode::Digit2), CTRL), None);
        assert_eq!(latin("ö", "ö", key(KeyCode::Semicolon), CTRL), None);
    }

    #[test]
    fn altgr_characters_stay_characters() {
        // Windows reports AltGr as Ctrl+Alt: Polish AltGr+A types ą, Greek AltGr+E types €.
        assert_eq!(latin("ą", "a", key(KeyCode::KeyA), CTRL_ALT), None);
        assert_eq!(latin("€", "ε", key(KeyCode::KeyE), CTRL_ALT), None);
        // A real Ctrl+Alt chord on the Arabic layout types nothing else and is remapped.
        assert_eq!(latin("ع", "ع", key(KeyCode::KeyU), CTRL_ALT), Some('u'));
    }

    #[test]
    fn alt_shift_chords_are_remapped_where_alt_is_not_a_typing_key() {
        let alt_shift = Mods {
            alt: true,
            shift: true,
            ..NONE
        };
        // Alt+Shift+R (focus the compose box): Shift+R types a fathatan on the Arabic layout.
        let got = latin("\u{64B}", "ق", key(KeyCode::KeyR), alt_shift);
        assert_eq!(got, (!OPTION_TYPES).then_some('R'));
    }

    #[test]
    fn cmd_counts_like_ctrl() {
        let cmd = Mods { meta: true, ..NONE };
        assert_eq!(latin("ر", "ر", key(KeyCode::KeyV), cmd), Some('v'));
    }

    #[test]
    fn other_scripts_work_the_same() {
        // Russian ЙЦУКЕН: the V key types м, the C key с.
        assert_eq!(latin("м", "м", key(KeyCode::KeyV), CTRL), Some('v'));
        assert_eq!(latin("с", "с", key(KeyCode::KeyC), CTRL), Some('c'));
        // Hebrew: the V key types ה.
        assert_eq!(latin("ה", "ה", key(KeyCode::KeyV), CTRL), Some('v'));
    }

    #[test]
    fn an_unknown_physical_key_falls_back_to_the_arabic_layout() {
        assert_eq!(latin("ر", "ر", None, CTRL), Some('v'));
        assert_eq!(latin("ﻻ", "ﻻ", None, CTRL), Some('b'));
        assert_eq!(latin("ر", "", None, CTRL_SHIFT), Some('V'));
        // Another script with no known key stays as it is.
        assert_eq!(latin("м", "м", None, CTRL), None);
    }

    #[test]
    fn keys_without_a_us_character_are_not_remapped_by_place() {
        assert_eq!(us_key(KeyCode::Enter), None);
        assert_eq!(us_key(KeyCode::F5), None);
        assert_eq!(us_key(KeyCode::KeyV), Some('v'));
        assert_eq!(us_key(KeyCode::Backquote), Some('`'));
    }
}
