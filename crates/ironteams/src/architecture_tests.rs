//! Layering guard: see `docs/ARCHITECTURE.md`.
//!
//! `ui_api` <- `backend`: the backend reaches the UI only through `crate::ui_api`.
//! `ui` -> `ui_api` + `support` + `backend::Cmd`: nothing below `ui` may name `crate::ui` or
//! `crate::backend`. The scan is plain text and std-only. It is a unit test, not a file under
//! `tests/`: cargo builds the whole binary a second time for an integration test of a bin crate,
//! and the CI test job was killed for memory when this test lived there.

use std::fs;
use std::path::{Path, PathBuf};

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `path` (or `path` itself, when it is a file), sorted.
fn rust_files(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![path.to_path_buf()];
    while let Some(p) = todo.pop() {
        if p.is_dir() {
            for entry in fs::read_dir(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display())) {
                todo.push(entry.expect("directory entry").path());
            }
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// The 1-based lines of `text` that contain `needle` where `rejects(next)` holds for the char
/// after it (`None` at the end of the text).
fn lines_with(text: &str, needle: &str, rejects: impl Fn(Option<char>) -> bool) -> Vec<usize> {
    let mut lines = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let mut from = 0;
        while let Some(at) = line[from..].find(needle) {
            let end = from + at + needle.len();
            if rejects(line[end..].chars().next()) {
                lines.push(n + 1);
                break;
            }
            from = end;
        }
    }
    lines
}

fn rel(p: &Path) -> String {
    p.strip_prefix(src()).unwrap_or(p).display().to_string()
}

#[test]
fn the_backend_never_names_the_ui() {
    let files = rust_files(&src().join("backend"));
    assert!(!files.is_empty(), "no files found under src/backend");
    let mut found = Vec::new();
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        // `crate::ui_api` (and `crate::ui_tests`) are fine; `crate::ui::x`, `crate::ui;` are not.
        let is_ident = |c: Option<char>| c.is_some_and(|c| c == '_' || c.is_ascii_alphanumeric());
        for n in lines_with(&text, "crate::ui", |next| !is_ident(next)) {
            found.push(format!("{}:{n}", rel(&f)));
        }
    }
    assert!(
        found.is_empty(),
        "backend/ must reach the UI through crate::ui_api only:\n{}",
        found.join("\n")
    );
}

#[test]
fn ui_api_and_support_depend_on_nothing_above_them() {
    let mut files = rust_files(&src().join("support"));
    files.push(src().join("support.rs"));
    files.push(src().join("ui_api.rs"));
    let mut found = Vec::new();
    for f in files {
        let text = fs::read_to_string(&f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
        for (needle, rejects) in [
            ("crate::backend", (|_| true) as fn(Option<char>) -> bool),
            ("crate::ui::", |_| true),
        ] {
            for n in lines_with(&text, needle, rejects) {
                found.push(format!("{}:{n}: {needle}", rel(&f)));
            }
        }
    }
    assert!(
        found.is_empty(),
        "ui_api and support sit below backend and ui:\n{}",
        found.join("\n")
    );
}
