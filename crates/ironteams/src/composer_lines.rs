//! Rust side of the composer's line editor (ui/molecules/line_editor.slint): the `LineApi` global.
//! The composer draws one input per line but keeps the draft as one string; `render::lines` maps
//! between the two.

use crate::{AppWindow, LineApi};
use render::{Dir, lines};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

fn index(v: i32) -> usize {
    usize::try_from(v).unwrap_or(0)
}

fn int(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Attaches the line functions to a window.
pub fn wire(window: &AppWindow) {
    let api = window.global::<LineApi>();
    api.on_split(|text: SharedString| {
        let rows: Vec<SharedString> = text.split('\n').map(SharedString::from).collect();
        ModelRc::new(VecModel::from(rows))
    });
    api.on_join(|rows: ModelRc<SharedString>| {
        let rows: Vec<String> = rows.iter().map(|row| row.to_string()).collect();
        rows.join("\n").into()
    });
    api.on_update(|rows: ModelRc<SharedString>, text: SharedString| update(&rows, &text));
    api.on_line_count(|text: SharedString| int(lines::line_count(&text)));
    api.on_line_of(|text: SharedString, offset| int(lines::locate(&text, index(offset)).0));
    api.on_column_of(|text: SharedString, offset| int(lines::locate(&text, index(offset)).1));
    api.on_offset_of(|text: SharedString, line, column| {
        int(lines::offset_of(&text, index(line), index(column)))
    });
    api.on_line_length(|text: SharedString, line| int(lines::line_len(&text, index(line))));
    api.on_splice(|text: SharedString, start, end, insert: SharedString| {
        lines::splice(&text, index(start), index(end), &insert).into()
    });
    api.on_vertical(|text: SharedString, offset, delta| {
        let delta = if delta < 0 { -1 } else { 1 };
        lines::vertical(&text, index(offset), delta).map_or(-1, int)
    });
    api.on_step(|text: SharedString, offset, delta, by_word| {
        let delta = if delta < 0 { -1 } else { 1 };
        int(lines::step(&text, index(offset), delta, by_word))
    });
    api.on_printable(|text: SharedString| {
        !text.is_empty()
            && text
                .chars()
                .all(|c| !c.is_control() && !('\u{f700}'..='\u{f7ff}').contains(&c))
    });
    api.on_direction(
        |text: SharedString, line| match lines::line_direction(&text, index(line)) {
            Some(Dir::Rtl) => 1,
            Some(Dir::Ltr) => -1,
            None => 0,
        },
    );
}

/// Makes the rows of `rows` the lines of `text`: a row that differs is set, missing rows are added
/// and extra rows removed, so the inputs of the rows that stay keep their focus.
fn update(rows: &ModelRc<SharedString>, text: &str) {
    let lines: Vec<&str> = text.split('\n').collect();
    while rows.row_count() > lines.len() {
        let _ = rows.remove_row(rows.row_count() - 1);
    }
    for (i, line) in lines.iter().enumerate() {
        if i >= rows.row_count() {
            let _ = rows.push_row((*line).into());
        } else if rows.row_data(i).as_deref() != Some(*line) {
            rows.set_row_data(i, (*line).into());
        }
    }
}
