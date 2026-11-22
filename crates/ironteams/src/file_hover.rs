//! Whether a file is being dragged over the window, read from the window system's events. The
//! chat shows a drop zone meanwhile (`AppWindow.file-hover`). Wayland sends no drag events (winit
//! 0.30), so there the zone never shows, as a drop never arrives either.

use slint::winit_030::winit::event::WindowEvent;

/// `Some(true)` when a file is dragged over the window, `Some(false)` when it left again or was
/// dropped, `None` for every other event.
pub(crate) fn of(event: &WindowEvent) -> Option<bool> {
    match event {
        WindowEvent::HoveredFile(_) => Some(true),
        WindowEvent::HoveredFileCancelled | WindowEvent::DroppedFile(_) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_dragged_file_shows_the_zone_until_it_leaves_or_is_dropped() {
        let file = PathBuf::from("/tmp/report.pdf");
        assert_eq!(of(&WindowEvent::HoveredFile(file.clone())), Some(true));
        assert_eq!(of(&WindowEvent::HoveredFileCancelled), Some(false));
        assert_eq!(of(&WindowEvent::DroppedFile(file)), Some(false));
    }

    #[test]
    fn other_events_leave_the_zone_as_it_is() {
        assert_eq!(of(&WindowEvent::Focused(true)), None);
        assert_eq!(of(&WindowEvent::CloseRequested), None);
    }
}
