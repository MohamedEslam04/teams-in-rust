//! Where the cards sit: the bottom-right corner of the work area, newest at the bottom, at most
//! three. Pure arithmetic in physical pixels, used by the window backends (X11, Windows, macOS);
//! the layer shell is placed by the compositor from the same margin.

/// A rectangle on the screen, physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Cards on the screen at once; a newer card pushes the oldest one out.
pub const MAX_CARDS: usize = 3;
/// From the edges of the work area to the cards, logical pixels.
pub const MARGIN: u32 = 16;

/// The part of a monitor that panels leave free. The window backends only know the monitor, so
/// the usual taskbar is set aside: at the bottom on Windows (48 px) and on Linux desktops (KDE's
/// default panel is 44 px), the menu bar (25 px) and the Dock (about 70 px) on macOS. Logical
/// pixels times `scale`.
pub fn work_area(monitor: Rect, scale: f64, os: super::select::Os) -> Rect {
    use super::select::Os;
    let (top, bottom) = match os {
        Os::Windows => (0.0, 48.0),
        Os::Mac => (25.0, 70.0),
        Os::Linux | Os::Other => (0.0, 44.0),
    };
    let (top, bottom) = (
        (top * scale).round() as u32,
        (bottom * scale).round() as u32,
    );
    let h = monitor.h.saturating_sub(top + bottom);
    Rect {
        x: monitor.x,
        y: monitor.y + top as i32,
        w: monitor.w,
        h,
    }
}

/// The top-left corner of each card, oldest first, for cards of these heights stacked upwards
/// from the bottom-right corner of `area` (`margin` from its right and bottom edges, `spacing`
/// apart). A stack taller than the area keeps its newest card on the screen.
pub fn stack(
    area: Rect,
    width: u32,
    heights: &[u32],
    margin: u32,
    spacing: u32,
) -> Vec<(i32, i32)> {
    let x = area.x + area.w as i32 - width as i32 - margin as i32;
    let mut bottom = area.y + area.h as i32 - margin as i32;
    let mut out: Vec<(i32, i32)> = heights
        .iter()
        .rev()
        .map(|h| {
            let top = bottom - *h as i32;
            bottom = top - spacing as i32;
            (x, top)
        })
        .collect();
    out.reverse();
    out
}

/// The top-left corner of one window of `size` in the bottom-right corner of `area`.
pub fn corner(area: Rect, size: (u32, u32), margin: u32) -> (i32, i32) {
    stack(area, size.0, &[size.1], margin, 0)[0]
}

#[cfg(test)]
mod tests {
    use super::super::select::Os;
    use super::*;

    const FULL_HD: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    #[test]
    fn one_card_sits_in_the_bottom_right_corner() {
        assert_eq!(stack(FULL_HD, 340, &[150], 16, 8), vec![(1564, 914)]);
        assert_eq!(corner(FULL_HD, (340, 150), 16), (1564, 914));
    }

    #[test]
    fn two_and_three_cards_stack_upwards_with_the_newest_at_the_bottom() {
        assert_eq!(
            stack(FULL_HD, 340, &[150, 100], 16, 8),
            vec![(1564, 806), (1564, 964)]
        );
        let three = stack(FULL_HD, 340, &[150, 100, 120], 16, 8);
        assert_eq!(three, vec![(1564, 678), (1564, 836), (1564, 944)]);
        // Each card ends `spacing` above the next one.
        assert_eq!(three[0].1 + 150 + 8, three[1].1);
        assert_eq!(three[1].1 + 100 + 8, three[2].1);
        assert_eq!(three[2].1 + 120 + 16, 1080);
    }

    #[test]
    fn a_window_grown_for_a_new_card_moves_up_and_keeps_its_bottom_edge() {
        // The window backends place the whole stack as one window by its top-left corner: a
        // call card (164 px with its shadow), a reminder under it, then the call gone.
        let area = work_area(FULL_HD, 1.0, Os::Linux);
        let one = corner(area, (372, 164), 16);
        let two = corner(area, (372, 164 + 8 + 120), 16);
        let after = corner(area, (372, 32 + 120), 16);
        assert_eq!(one, (1920 - 372 - 16, 1080 - 44 - 16 - 164));
        assert_eq!(
            two,
            (one.0, one.1 - 128),
            "taller: moved up by the new card"
        );
        assert_eq!(after, (one.0, 1080 - 44 - 16 - 152), "shorter: moved down");
        for (top, h) in [(one.1, 164), (two.1, 292), (after.1, 152)] {
            assert_eq!(top + h, 1080 - 44 - 16, "the bottom edge stays");
        }
    }

    #[test]
    fn a_second_monitor_and_a_scaled_screen_keep_the_corner() {
        let right = Rect {
            x: 1920,
            y: -200,
            w: 2560,
            h: 1440,
        };
        assert_eq!(
            corner(right, (680, 300), 32),
            (1920 + 2560 - 680 - 32, -200 + 1440 - 300 - 32)
        );
    }

    #[test]
    fn the_work_area_leaves_room_for_the_taskbar() {
        assert_eq!(
            work_area(FULL_HD, 1.0, Os::Windows),
            Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1032
            }
        );
        assert_eq!(work_area(FULL_HD, 2.0, Os::Linux).h, 1080 - 88);
        let mac = work_area(FULL_HD, 1.0, Os::Mac);
        assert_eq!((mac.y, mac.h), (25, 1080 - 25 - 70));
        // A tiny monitor never underflows.
        let tiny = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 20,
        };
        assert_eq!(work_area(tiny, 1.0, Os::Mac).h, 0);
    }
}
