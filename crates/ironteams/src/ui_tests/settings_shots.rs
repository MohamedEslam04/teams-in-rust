//! Pictures of every Settings page, for looking at the design without a display: the real window
//! drawn by Slint's software renderer into memory and saved as PNG files. Run by
//! .github/workflows/ui-shots.yml, or by hand:
//! `IRONTEAMS_SHOTS_DIR=/tmp/shots cargo test -p ironteams --bin ironteams -- --ignored settings_pages_to_png`.
//!
//! Each page is drawn in the dark and the light theme at 1920x1080 (what the window shows) and
//! whole at 900 px wide, whole at 760 px wide (the one-pane width) in the dark theme, and whole
//! at 900 px in Arabic. Files: `<theme>-<width>-<page>.png`. Then the keyboard's focus ring on a
//! switch row, a pill, a button and a radio row (`focus-<control>-dark-900-<page>.png`), and Find in
//! Settings: its results in English and Arabic and the page a result opens (`find-*.png`).

use super::settings::{Home, env, server};
use super::*;
use crate::ui::settings::install;
use crate::{SettingsStore, Theme};
use i_slint_backend_testing::AccessibleRole;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{PhysicalSize, Rgb8Pixel};
use std::time::Duration;
use teams_proto::user_settings::PrivacySettings;

const PAGES: [&str; 13] = [
    "general",
    "chats",
    "accounts",
    "privacy",
    "notifications",
    "appearance",
    "accessibility",
    "devices",
    "calls",
    "files",
    "storage",
    "permissions",
    "about",
];

/// A whole page is drawn into a window this tall, then cut below its last row.
const TALL: u32 = 6000;

struct ShotPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for ShotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
    // Animations and timers run on the mocked clock (`testing::mock_elapsed_time`).
    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(testing::get_mocked_time())
    }
}

fn settle() {
    for _ in 0..20 {
        testing::mock_elapsed_time(Duration::from_millis(50));
        slint::platform::update_timers_and_animations();
    }
}

/// Draws the window at `w` x `h` and saves it; `whole` cuts the rows below the page's content.
fn shoot(window: &MinimalSoftwareWindow, w: u32, h: u32, whole: bool, path: &std::path::Path) {
    window.set_size(PhysicalSize::new(w, h));
    settle();
    let mut buf = vec![Rgb8Pixel::default(); (w * h) as usize];
    window.request_redraw();
    window.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let mut rows = h;
    if whole {
        // The page's own column (right of the page list): cut the rows that only show its
        // background, keeping a margin.
        let x0 = (w / 2) as usize;
        let bg = buf[((h - 1) * w) as usize + x0];
        let blank = |y: u32| {
            let row = &buf[(y * w) as usize..((y + 1) * w) as usize];
            row[x0..].iter().all(|p| *p == bg)
        };
        while rows > 400 && blank(rows - 1) {
            rows -= 1;
        }
        rows = (rows + 24).min(h);
    }
    let raw: Vec<u8> = buf[..(rows * w) as usize]
        .iter()
        .flat_map(|p| [p.r, p.g, p.b])
        .collect();
    image::save_buffer(path, &raw, w, rows, image::ColorType::Rgb8).unwrap();
}

#[test]
#[ignore = "writes PNGs; run by .github/workflows/ui-shots.yml"]
fn settings_pages_to_png() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("IRONTEAMS_SHOTS_DIR").expect("set IRONTEAMS_SHOTS_DIR to a directory"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    assert!(
        !PLATFORM_SET.replace(true),
        "draw the pictures on a thread of their own"
    );
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(ShotPlatform(window.clone()))).unwrap();
    window.set_size(PhysicalSize::new(1920, 1080));

    let home = Home::new("shots").with_accounts();
    let log = Log::default();
    let tokens = Arc::new(session::MemoryStore::default());
    let app = AppWindow::new().unwrap();
    crate::ui::install_local(&app);
    crate::composer::wire(&app);
    app.set_login_state("ready".into());
    app.set_me_initials("TU".into());
    install(&app, None, env(&home, &log, tokens));
    app.show().unwrap();
    // What Teams would have answered: the privacy switches, a blocked person, the
    // notification types.
    UiHandle::new(app.as_weak()).settings_server(server(PrivacySettings::default()));
    let g = app.global::<SettingsStore>();

    let lang = |tag: &str| {
        crate::language::apply(&appearance::GeneralPrefs {
            language: Some(tag.into()),
            ..Default::default()
        })
    };
    lang("en");
    for theme in ["dark", "light"] {
        g.invoke_command("theme".into(), theme.into());
        assert_eq!(app.global::<Theme>().get_dark(), theme == "dark");
        for page in PAGES {
            g.invoke_show(page.into());
            shoot(
                &window,
                1920,
                1080,
                false,
                &dir.join(format!("{theme}-1920-{page}.png")),
            );
            shoot(
                &window,
                900,
                TALL,
                true,
                &dir.join(format!("{theme}-900-{page}.png")),
            );
            if theme == "dark" {
                shoot(
                    &window,
                    760,
                    TALL,
                    true,
                    &dir.join(format!("{theme}-760-{page}.png")),
                );
            }
        }
    }
    g.invoke_command("theme".into(), "dark".into());
    lang("ar");
    for page in PAGES {
        g.invoke_show(page.into());
        shoot(
            &window,
            900,
            TALL,
            true,
            &dir.join(format!("ar-900-{page}.png")),
        );
    }
    lang("en");

    // The keyboard's focus ring: a click on one control, then Tab (or Shift+Tab) to the next.
    let find = |label: &str| {
        ElementHandle::find_by_accessible_label(&app, label)
            .find(|e| e.size().height > 0. && e.accessible_role() != Some(AccessibleRole::Text))
            .unwrap_or_else(|| panic!("{label:?} on screen"))
    };
    let click = |label: &str| {
        let e = find(label);
        let (p, s) = (e.absolute_position(), e.size());
        let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position: at });
        app.window().dispatch_event(WindowEvent::PointerPressed {
            position: at,
            button: PointerEventButton::Left,
        });
        settle();
        app.window().dispatch_event(WindowEvent::PointerReleased {
            position: at,
            button: PointerEventButton::Left,
        });
        settle();
        // The pointer leaves, so no hover tint mixes with the ring.
        app.window().dispatch_event(WindowEvent::PointerExited);
    };
    let tab = |back: bool| {
        if back {
            key(&app, Key::Shift, true);
        }
        key(&app, Key::Tab, true);
        key(&app, Key::Tab, false);
        if back {
            key(&app, Key::Shift, false);
        }
    };
    for (page, first, back, name) in [
        ("notifications", "Show the sender's photo", false, "switch"),
        ("appearance", "Message density: Comfy", false, "pill"),
        ("appearance", "Reset font size", true, "button"),
        ("devices", "Speaker: System default", false, "radio"),
    ] {
        g.invoke_show(page.into());
        window.set_size(PhysicalSize::new(900, 1080));
        settle();
        click(first);
        // A switch was flipped by the click: Tab, then put it back with the mouse after the shot.
        tab(back);
        shoot(
            &window,
            900,
            1080,
            false,
            &dir.join(format!("focus-{name}-dark-900-{page}.png")),
        );
        if name == "switch" {
            click(first);
        }
    }

    // Find in Settings: the results, then the page a result opens with the setting tinted.
    g.invoke_show("general".into());
    find("Find in Settings").set_accessible_value(SharedString::from("sound"));
    shoot(
        &window,
        900,
        1080,
        false,
        &dir.join("find-dark-900-results.png"),
    );
    lang("ar");
    shoot(
        &window,
        900,
        1080,
        false,
        &dir.join("find-ar-900-results.png"),
    );
    lang("en");
    let first = g.get_found().row_data(0).expect("a setting about sound");
    find(&format!("Result: {} in {}", first.label, first.page_title))
        .invoke_accessible_default_action();
    // The page's rows are made when the tree is next read (here), and the tint starts then.
    find(&first.label);
    shoot(
        &window,
        1920,
        1080,
        false,
        &dir.join("find-dark-1920-landed.png"),
    );
}
