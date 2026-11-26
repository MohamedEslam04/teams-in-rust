//! The custom theme editor and sharing (docs/designs/theme-engine.md, PR 4): New, Edit and Import
//! open one editor over the page; each change derives the tokens again, runs the contrast
//! adjustment and pushes the rows, the preview and the report. Save stores the theme and applies
//! it. Copy theme puts the share line on the clipboard.
//!
//! Commands: theme-new, theme-edit, theme-import, editor-hex, editor-lch, editor-name,
//! editor-keep, editor-save, editor-cancel, theme-copy, theme-rename, theme-delete.

use super::*;
use appearance::oklch::{Oklch, from_oklch, to_oklch};
use appearance::share;
use appearance::themes::{self, Base, ThemeChoice, ThemeError};
use slint::Model;

/// A picker slider unit is a hundredth of lightness, 0.004 of chroma and 3.6 degrees of hue.
const C_UNIT: f64 = 0.004;
const H_UNIT: f64 = 3.6;
/// The longest part of a refused line the editor shows back.
const SHOWN_LINE: usize = 200;

#[derive(Clone)]
pub(super) enum EditorMode {
    New,
    Edit(String),
    Import,
}

/// The theme being built: the colours as the user picked them, the name and "Keep my colours".
pub(super) struct Editor {
    mode: EditorMode,
    name: String,
    base: Base,
    keep: bool,
    /// The pasted line (`Import`).
    line: String,
    /// The code of the sentence to show (`share-error` in the store), or empty.
    error: String,
}

fn start_base(file: &appearance::ThemesFile) -> Base {
    themes::find(file, themes::DARK_ID)
        .unwrap_or(&themes::builtins()[0])
        .base
}

fn colour(c: Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(c.r, c.g, c.b)
}

fn error_code(e: ThemeError) -> &'static str {
    match e {
        ThemeError::Limit => "limit",
        ThemeError::Name(n) => n.code(),
        ThemeError::Unknown => "not-a-theme",
    }
}

/// One base colour as the editor shows it. `asked` is the slider position the user dragged to:
/// it stays while the colour stored is what it stands for, and snaps to the stored colour when
/// the colour was out of gamut (the chroma was cut).
fn row(c: Rgb, asked: Option<[f32; 3]>) -> SBaseRow {
    let o = to_oklch(c);
    let actual = [
        (o.l * 100.0) as f32,
        (o.c / C_UNIT) as f32,
        (o.h / H_UNIT) as f32,
    ];
    let units = match asked {
        None => actual.map(f32::round),
        Some(req) => {
            let near = |i: usize, tol: f32| (actual[i] - req[i]).abs() <= tol;
            [
                if near(0, 1.0) {
                    req[0]
                } else {
                    actual[0].round()
                },
                if near(1, 1.0) {
                    req[1]
                } else {
                    actual[1].round()
                },
                // The hue of a grey is meaningless and a small chroma loses it to rounding.
                if o.c < 0.01 || near(2, 1.5) {
                    req[2]
                } else {
                    actual[2].round()
                },
            ]
        }
    };
    SBaseRow {
        hex: s(c.to_hex()),
        colour: colour(c),
        l: units[0].clamp(0.0, 100.0),
        c: units[1].clamp(0.0, 100.0),
        h: units[2].clamp(0.0, 100.0),
    }
}

/// Shows the editor (or hides it when there is none). `asked` is the row the user is dragging.
fn push_editor(w: &AppWindow, c: &Ctl, asked: Option<(usize, [f32; 3])>) {
    let g = w.global::<SettingsStore>();
    let Some(e) = &c.editor else {
        g.set_editor_open(false);
        return;
    };
    let (mode, id) = match &e.mode {
        EditorMode::New => ("new", String::new()),
        EditorMode::Edit(id) => ("edit", id.clone()),
        EditorMode::Import => ("import", String::new()),
    };
    let mut set = themes::derive(&e.base);
    let report = themes::adjust(&mut set, e.keep);
    let rows: Vec<SBaseRow> = e
        .base
        .0
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let asked = asked.and_then(|(row, units)| (row == i).then_some(units));
            row(c, asked)
        })
        .collect();
    let shown = g.get_editor_rows();
    if shown.row_count() == rows.len() {
        // In place, so a slider that is being dragged keeps its grip.
        for (i, r) in rows.into_iter().enumerate() {
            shown.set_row_data(i, r);
        }
    } else {
        g.set_editor_rows(model(rows));
    }
    let region = |r: themes::Region, name: &str| match r {
        themes::Region::Content => name.to_string(),
        themes::Region::Nav => format!("nav {name}"),
    };
    g.set_editor_failing(model(
        report
            .pairs
            .iter()
            .filter(|p| p.ratio < p.min)
            .map(|p| SPairRow {
                fg: s(region(p.region, p.fg)),
                bg: s(region(p.region, p.bg)),
                ratio: s(format!("{:.1}", p.ratio)),
                min: s(format!("{:.1}", p.min)),
            })
            .collect(),
    ));
    g.set_editor_adjusted(model(
        report
            .adjusted
            .iter()
            .map(|a| SAdjusted {
                token: s(region(a.region, a.token)),
                picked: rgba(a.picked),
                drawn: rgba(a.drawn),
            })
            .collect(),
    ));
    g.set_editor_content(ui_tokens(&set.content));
    g.set_editor_nav(ui_tokens(&set.nav));
    g.set_editor_low_contrast(report.low_contrast);
    g.set_editor_name(s(&e.name));
    g.set_editor_keep(e.keep);
    g.set_editor_mode(s(mode));
    g.set_editor_id(s(id));
    g.set_editor_line(s(e.line.chars().take(SHOWN_LINE).collect::<String>()));
    g.set_editor_error(s(&e.error));
    let room =
        matches!(e.mode, EditorMode::Edit(_)) || c.local.themes.custom.len() < themes::MAX_CUSTOM;
    g.set_editor_can_save(share::check_name(&e.name).is_ok() && e.error.is_empty() && room);
    g.set_editor_open(true);
}

fn open(w: &AppWindow, c: &mut Ctl, mode: EditorMode, name: String, base: Base, keep: bool) {
    let full = c.local.themes.custom.len() >= themes::MAX_CUSTOM;
    let error = if full && !matches!(mode, EditorMode::Edit(_)) {
        "limit"
    } else {
        ""
    };
    c.editor = Some(Editor {
        mode,
        name,
        base,
        keep,
        line: String::new(),
        error: error.into(),
    });
    notice(w, "");
    push_editor(w, c, None);
}

fn parse_units(parts: &[&str]) -> Option<(usize, [f32; 3])> {
    if parts.len() != 4 {
        return None;
    }
    let index: usize = parts.first()?.parse().ok()?;
    let mut units = [0.0_f32; 3];
    for (slot, p) in units.iter_mut().zip(parts.get(1..)?) {
        *slot = p.trim().parse::<f32>().ok()?.clamp(0.0, 100.0);
    }
    (index < 8).then_some((index, units))
}

/// Handles the editor and sharing commands; false for any other name.
pub(super) fn command(shared: &Shared, w: &AppWindow, name: &str, arg: &str) -> bool {
    if !matches!(
        name,
        "theme-new"
            | "theme-edit"
            | "theme-import"
            | "editor-hex"
            | "editor-lch"
            | "editor-name"
            | "editor-keep"
            | "editor-save"
            | "editor-cancel"
            | "theme-copy"
            | "theme-rename"
            | "theme-delete"
    ) {
        return false;
    }
    let g = w.global::<SettingsStore>();
    let parts: Vec<&str> = arg.split('\u{1f}').collect();
    let mut c = shared.borrow_mut();
    let c = &mut *c;
    match name {
        "theme-new" => {
            let base = start_base(&c.local.themes);
            let wanted = c.local.themes.unique_name("My theme", None);
            open(w, c, EditorMode::New, wanted, base, false);
        }
        "theme-edit" => {
            let def = c
                .local
                .themes
                .custom
                .iter()
                .find(|d| d.id == arg)
                .map(|d| (d.name.clone(), d.base, d.keep_original));
            if let Some((name, base, keep)) = def {
                open(w, c, EditorMode::Edit(arg.to_string()), name, base, keep);
            }
        }
        "theme-import" => {
            g.set_import_open(false);
            let base = start_base(&c.local.themes);
            match share::parse(arg) {
                Ok((name, base)) => open(w, c, EditorMode::Import, name, base, false),
                Err(e) => {
                    open(w, c, EditorMode::Import, String::new(), base, false);
                    if let Some(ed) = &mut c.editor {
                        ed.error = e.code().into();
                    }
                }
            }
            if let Some(ed) = &mut c.editor {
                ed.line = arg.trim().to_string();
                push_editor(w, c, None);
            }
        }
        "editor-hex" | "editor-lch" | "editor-name" | "editor-keep" => {
            let Some(ed) = &mut c.editor else {
                return true;
            };
            let mut asked = None;
            match name {
                "editor-name" => ed.name = share::clean_name(arg),
                "editor-keep" => ed.keep = arg == "1",
                _ if matches!(ed.mode, EditorMode::Import) => {}
                "editor-hex" => {
                    let picked = parts
                        .first()
                        .and_then(|i| i.parse::<usize>().ok())
                        .filter(|&i| i < 8)
                        .zip(parts.get(1).and_then(|h| Rgb::from_hex(h)));
                    if let Some((i, rgb)) = picked {
                        ed.base.0[i] = rgb;
                    }
                }
                _ => {
                    if let Some((i, u)) = parse_units(&parts) {
                        ed.base.0[i] = from_oklch(Oklch {
                            l: f64::from(u[0]) / 100.0,
                            c: f64::from(u[1]) * C_UNIT,
                            h: f64::from(u[2]) * H_UNIT,
                        });
                        asked = Some((i, u));
                    }
                }
            }
            push_editor(w, c, asked);
        }
        "editor-cancel" => {
            c.editor = None;
            g.set_import_open(false);
            push_editor(w, c, None);
        }
        "editor-save" => {
            let Some(ed) = &c.editor else {
                return true;
            };
            if !ed.error.is_empty() {
                return true;
            }
            let (mode, name, base, keep) = (ed.mode.clone(), ed.name.clone(), ed.base, ed.keep);
            let themes = &mut c.local.themes;
            let saved_as = match &mode {
                EditorMode::New | EditorMode::Import => themes.add(&name, base, keep),
                EditorMode::Edit(id) => themes
                    .update(id, base, keep)
                    .and_then(|()| themes.rename(id, &name))
                    .map(|_| id.clone()),
            };
            match saved_as {
                Ok(id) => {
                    c.local.themes.choice = Some(ThemeChoice::Fixed(id));
                    let r = c.local.save_themes();
                    c.editor = None;
                    look_changed(w, c);
                    push_themes(w, c);
                    push_editor(w, c, None);
                    saved(w, r);
                }
                Err(e) => {
                    if let Some(ed) = &mut c.editor {
                        ed.error = error_code(e).into();
                    }
                    push_editor(w, c, None);
                }
            }
        }
        "theme-copy" => {
            let line = c
                .local
                .themes
                .custom
                .iter()
                .find(|d| d.id == arg)
                .map(|d| share::export(&d.name, &d.base));
            if let Some(line) = line {
                match (c.env.hooks.copy_text)(&line) {
                    Ok(()) => notice(w, "Theme copied."),
                    Err(e) => notice(w, format!("Could not copy the theme: {e}")),
                }
            }
        }
        "theme-rename" => {
            let id = parts.first().copied().unwrap_or("");
            let wanted = parts.get(1).copied().unwrap_or("");
            match c.local.themes.rename(id, wanted) {
                Ok(_) => {
                    let r = c.local.save_themes();
                    g.set_rename_id(s(""));
                    g.set_editor_error(s(""));
                    look_changed(w, c);
                    push_themes(w, c);
                    saved(w, r);
                }
                Err(ThemeError::Name(n)) => g.set_editor_error(s(n.code())),
                Err(_) => g.set_rename_id(s("")),
            }
        }
        _ => {
            // theme-delete
            if c.local.themes.delete(arg) {
                let r = c.local.save_themes();
                look_changed(w, c);
                push_themes(w, c);
                saved(w, r);
            }
        }
    }
    true
}

/// A chat message's "Preview" on a theme chip: Settings > Appearance with the import preview of
/// the line. Nothing is saved until Save. False for any other name.
pub fn route_theme_preview(w: &AppWindow, name: &str, arg: &str) -> bool {
    if name != "theme-preview" {
        return false;
    }
    let g = w.global::<SettingsStore>();
    g.invoke_show(s("appearance"));
    g.invoke_command(s("theme-import"), s(arg));
    let _ = w.show();
    true
}
