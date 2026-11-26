//! The installed font families for the code and interface font pickers. The only module that uses
//! Slint's `unstable-fontique-012` API, so a Slint upgrade that renames it breaks only this file.
//!
//! The font spike (docs/designs/theme-engine.md, Open Question 1) found outcome (a): a family set
//! through `Window.default-font-family` (or a `Text`'s `font-family`) is followed by Slint's
//! `system-ui` and `sans-serif` chain, which on Linux is the one `main.rs::configure_fonts` builds
//! with the bundled colour emoji fonts. Emoji, bold and Arabic keep working without registering
//! anything here, and a change applies at once.

use slint::fontique_012::{fontique, shared_collection};
use std::sync::OnceLock;

/// Listed once per run. Reading every family's faces took 6 s on a Windows CI runner (23 ms on
/// Linux, 29 ms on macOS), so it runs on a thread of its own, with a collection of its own: Slint's
/// collection belongs to the UI thread.
static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();

/// Hands the families a user can pick to `ready` on the UI thread: at once when they are listed,
/// else once a worker has listed them. Nothing is handed when listing fails, so the pickers keep
/// only "Default".
pub(super) fn families(ready: impl FnOnce(&[String]) + Send + 'static) {
    if let Some(list) = FAMILIES.get() {
        ready(list.as_slice());
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("font-list".into())
        .spawn(move || {
            let list = FAMILIES.get_or_init(list);
            let _ = slint::invoke_from_event_loop(move || ready(list.as_slice()));
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "settings: the installed fonts are not listed");
    }
}

/// The installed families that cover basic Latin and are not emoji fonts, sorted and without
/// duplicates.
pub(super) fn list() -> Vec<String> {
    let mut collection = fontique::Collection::new(fontique::CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let ids: Vec<fontique::FamilyId> = collection.family_ids().collect();
    let mut names: Vec<String> = ids
        .into_iter()
        .filter_map(|id| {
            let name = collection.family_name(id)?.to_string();
            let latin = covers_latin(&mut collection, id);
            usable(&name, latin).then_some(name)
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// Whether the family's default face maps every letter A to Z and a to z.
fn covers_latin(collection: &mut fontique::Collection, id: fontique::FamilyId) -> bool {
    let Some(family) = collection.family(id) else {
        return false;
    };
    let Some(font) = family.default_font() else {
        return false;
    };
    let Some(blob) = font.load(None) else {
        return false;
    };
    let Some(map) = font.charmap_index().charmap(blob.data()) else {
        return false;
    };
    ('A'..='Z').chain('a'..='z').all(|c| map.map(c).is_some())
}

/// A family the pickers list: it covers basic Latin and is not an emoji font (some emoji fonts
/// map the letters to blank glyphs).
pub(super) fn usable(name: &str, covers_latin: bool) -> bool {
    covers_latin && !name.to_lowercase().contains("emoji")
}

/// Whether Slint can draw a family of this name.
pub(super) fn installed(name: &str) -> bool {
    !name.is_empty() && shared_collection().family_id(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::usable;

    #[test]
    fn usable_keeps_latin_families_and_drops_symbol_and_emoji_fonts() {
        let fonts = [
            ("DejaVu Sans Mono", true),
            ("JetBrains Mono", true),
            ("Noto Sans Arabic", false),
            ("Symbol", false),
            ("Noto Color Emoji", false),
            ("Twemoji Mozilla", true),
            ("Fluent Emoji 3D", true),
            ("Segoe UI Emoji", true),
        ];
        let kept: Vec<&str> = fonts
            .iter()
            .filter(|(n, latin)| usable(n, *latin))
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(kept, ["DejaVu Sans Mono", "JetBrains Mono"]);
    }
}
