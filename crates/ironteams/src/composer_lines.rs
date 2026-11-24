//! Rust side of the composer's line editor (ui/molecules/line_editor.slint): the `LineApi` global.
//! The composer draws one input per line but keeps the draft as one string; `render::lines` maps
//! between the two.

use crate::{AppWindow, LineApi};
use render::{Dir, lines};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

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
    api.on_line_text(|text: SharedString, line| lines::line_text(&text, index(line)).into());
    api.on_line_count(|text: SharedString| int(lines::line_count(&text)));
    api.on_line_of(|text: SharedString, offset| int(lines::locate(&text, index(offset)).0));
    api.on_column_of(|text: SharedString, offset| int(lines::locate(&text, index(offset)).1));
    api.on_offset_of(|text: SharedString, line, column| {
        int(lines::offset_of(&text, index(line), index(column)))
    });
    api.on_line_length(|text: SharedString, line| int(lines::line_len(&text, index(line))));
    api.on_replace_line(|text: SharedString, line, new: SharedString| {
        lines::replace_line(&text, index(line), &new).into()
    });
    api.on_splice(|text: SharedString, start, end, insert: SharedString| {
        lines::splice(&text, index(start), index(end), &insert).into()
    });
    api.on_vertical(|text: SharedString, offset, delta| {
        let delta = if delta < 0 { -1 } else { 1 };
        lines::vertical(&text, index(offset), delta).map_or(-1, int)
    });
    api.on_direction(
        |text: SharedString, line| match lines::line_direction(&text, index(line)) {
            Some(Dir::Rtl) => 1,
            Some(Dir::Ltr) => -1,
            None => 0,
        },
    );
}
