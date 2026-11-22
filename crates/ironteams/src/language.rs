//! The interface language (plan X1.2).
//!
//! The translations are Slint bundled translations: the `.po` catalogues under
//! `translations/<lang>/LC_MESSAGES/ironteams.po` are compiled into the app, and every `@tr("...")`
//! string in `ui/*.slint` follows the language selected here. English is the source text, so a
//! language without a catalogue (or a string without an entry) simply stays English.
//!
//! Settings calls [`apply`] with the saved general preferences at start and on every change.
//! `Theme.rtl` needs no extra wiring: the Arabic catalogue answers "rtl" to the "text direction"
//! message in `ui/tokens/theme.slint`, so it flips together with the translation.

use appearance::{GeneralPrefs, Language};

/// Resolves the language from the preferences (the system language when none is chosen),
/// records it as the shared setting and selects its translation. Call it on the UI thread after
/// the first window exists.
pub fn apply(prefs: &GeneralPrefs) {
    select(appearance::language::set_from(prefs));
}

fn select(lang: &Language) {
    // English is the source text: "en" selects it, other tags need a catalogue.
    if let Err(e) = slint::select_bundled_translation(lang.tag) {
        tracing::warn!(language = lang.tag, error = %e, "language: no translation to select");
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn catalogue(tag: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("translations")
            .join(tag)
            .join("LC_MESSAGES/ironteams.po");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn every_offered_language_but_english_has_a_catalogue() {
        for lang in appearance::available_languages() {
            if lang.tag != "en" {
                assert!(catalogue(lang.tag).contains("msgid"), "{}", lang.tag);
            }
        }
    }

    /// A translation must keep the `{}` placeholders of its source, or the argument is lost.
    #[test]
    fn translations_keep_their_placeholders() {
        let po = catalogue("ar");
        let (mut id, mut checked) = (String::new(), 0);
        for line in po.lines() {
            if let Some(v) = line.strip_prefix("msgid ") {
                id = v.to_owned();
            } else if let Some(v) = line.strip_prefix("msgstr ")
                && !id.is_empty()
                && id != "\"\""
            {
                assert_eq!(
                    id.matches("{}").count(),
                    v.matches("{}").count(),
                    "{id} -> {v}"
                );
                checked += 1;
            }
        }
        assert!(checked > 300, "only {checked} entries checked");
    }
}
