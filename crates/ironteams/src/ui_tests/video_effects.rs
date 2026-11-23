//! The camera preview, the camera picker and the Video effects panel, through the real window and
//! engine with the fake call driver: its preview takes the pictures a test hands it through the
//! same effects and mirror as the live camera's.

use super::*;
use crate::backend::call::{CallCmd, Event, FakeDriver};
use crate::{CallStore, VideoFxStore};
use calling::video::I420Frame;
use calling::video::effects::{Background, Filter};
use i_slint_backend_testing::AccessibleRole;

const THREAD: &str = "19:meeting_fx@thread.v2";

struct Fx {
    h: Harness,
    fake: FakeDriver,
    log: Log,
}

impl Fx {
    fn new() -> Self {
        let h = Harness::new();
        let fake = FakeDriver::manual();
        h.engine.calls.set_driver(Arc::new(fake.clone()));
        crate::ui::call::wire(&h.app);
        // A build with video.
        h.app.global::<CallStore>().set_camera_available(true);
        let log = Log::default();
        let (l, run) = (log.clone(), h.runner());
        h.app.global::<CallStore>().on_command(move |name, arg| {
            l.borrow_mut()
                .push(format!("{name} {arg}").trim().to_owned());
            if let Some(c) = CallCmd::parse(&name, &arg) {
                run(Cmd::Call(c));
            }
        });
        Fx { h, fake, log }
    }

    fn cs(&self) -> CallStore<'_> {
        self.h.app.global::<CallStore>()
    }

    fn fx(&self) -> VideoFxStore<'_> {
        self.h.app.global::<VideoFxStore>()
    }

    fn all(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.h.app, label).collect()
    }

    fn with_role(&self, label: &str, role: AccessibleRole) -> ElementHandle {
        self.all(label)
            .into_iter()
            .find(|e| e.accessible_role() == Some(role))
            .unwrap_or_else(|| panic!("no {role:?} {label:?}"))
    }

    fn button(&self, label: &str) -> ElementHandle {
        self.with_role(label, AccessibleRole::Button)
    }

    fn shows(&self, label: &str) -> bool {
        !self.all(label).is_empty()
    }

    fn prejoin(&self) {
        self.h.app.set_current_id(THREAD.into());
        click(&self.button("Join meeting"));
        assert_eq!(self.cs().get_phase(), "prejoin");
    }

    fn radio(&self, label: &str) -> ElementHandle {
        self.with_role(label, AccessibleRole::RadioButton)
    }

    fn tab(&self, label: &str) -> ElementHandle {
        self.with_role(label, AccessibleRole::Tab)
    }

    /// Pre-join, Join now, then the call connects.
    fn connected(&self) {
        self.prejoin();
        click(&self.button("Join now"));
        self.fake.emit(Event::Connected);
        assert_eq!(self.cs().get_phase(), "connected");
    }

    fn camera_on(&self) {
        click(&self.button("Camera"));
        assert!(self.cs().get_prejoin_camera());
    }

    fn sent(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// The preview's left-most pixel's red.
    fn preview_left_red(&self) -> u8 {
        let px = self.fx().get_preview().to_rgb8().expect("an RGB picture");
        px.as_slice()[0].r
    }
}

/// An 8x4 camera picture: white on the left half, black on the right.
fn half_white() -> I420Frame {
    let (w, h) = (8, 4);
    let mut data = vec![128u8; I420Frame::len_for(w, h)];
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = if x < w / 2 { 235 } else { 16 };
        }
    }
    I420Frame::new(w, h, data).unwrap()
}

#[test]
fn the_camera_switch_starts_the_preview_and_the_prejoin_picks_a_camera() {
    let f = Fx::new();
    f.prejoin();
    assert!(f.shows("Your camera is off"));
    assert!(f.fake.previews().is_empty(), "no camera before the switch");

    f.camera_on();
    assert_eq!(f.fake.previews(), ["on default"]);
    assert!(
        f.shows("Your camera turns on when you join"),
        "no picture yet"
    );
    assert!(f.fake.preview_frame(&half_white()));
    assert!(f.fx().get_has_preview());
    let preview = f.with_role("Your camera preview", AccessibleRole::Image);
    assert!(preview.size().width > 300.0, "the preview fills its box");

    // The camera picker sits with the audio ones and restarts the preview on the new camera.
    let picker = f.with_role("Camera", AccessibleRole::Combobox);
    assert_eq!(picker.accessible_value().unwrap(), "Default camera");
    click(&picker);
    click(&f.with_role("USB Webcam", AccessibleRole::ListItem));
    assert_eq!(f.fx().get_camera(), 2);
    assert!(f.sent().contains(&"camera-device 2".to_owned()));
    assert_eq!(f.fake.previews(), ["on default", "off", "on cam-usb"]);
    assert!(
        !f.fx().get_has_preview(),
        "the old camera's picture is gone"
    );

    // Off: the camera is let go and the picture cleared.
    click(&f.button("Camera"));
    assert_eq!(f.fake.previews().last().unwrap(), "off");
    assert!(!f.fx().get_has_preview());
    assert!(f.shows("Your camera is off"));
    assert!(
        !f.fake.preview_frame(&half_white()),
        "nothing listens any more"
    );
}

#[test]
fn leaving_the_prejoin_lets_the_camera_go() {
    let f = Fx::new();
    f.prejoin();
    f.camera_on();
    click(&f.button("Join now"));
    assert_eq!(f.fake.previews(), ["on default", "off"]);
    f.fake.emit(Event::Connected);
    assert_eq!(f.cs().get_phase(), "connected");
    assert_eq!(f.fake.previews().len(), 2, "no preview in the call");

    let g = Fx::new();
    g.prejoin();
    g.camera_on();
    click(&g.button("Cancel"));
    assert_eq!(g.fake.previews(), ["on default", "off"]);
}

#[test]
fn the_effects_panel_opens_and_blur_applies_to_what_is_sent() {
    let f = Fx::new();
    f.prejoin();
    f.camera_on();
    assert!(!f.fx().get_open());
    click(&f.button("Video effects"));
    assert!(f.fx().get_open());
    assert!(f.shows("Background"));
    for tile in ["None", "Standard blur", "Bookshelf", "Bright office"] {
        assert!(f.shows(tile), "{tile}");
    }
    assert!(!f.shows("Strong blur"), "the strength is not a tile");
    assert!(f.shows("Add new background"));
    let none = f.button("None");
    assert_eq!(none.accessible_checked(), Some(true));

    click(&f.button("Standard blur"));
    assert!(f.sent().contains(&"fx-background blur".to_owned()));
    assert_eq!(f.fx().get_background(), "blur");
    assert_eq!(f.button("Standard blur").accessible_checked(), Some(true));
    assert_eq!(f.h.engine.calls.effects().background, Background::Blur);

    // The strength shows under the tiles once a blur is chosen.
    assert_eq!(f.radio("Standard").accessible_checked(), Some(true));
    click(&f.radio("Strong"));
    assert_eq!(f.fx().get_background(), "strong-blur");
    assert_eq!(
        f.button("Standard blur").accessible_checked(),
        Some(true),
        "the tile stays checked for either strength"
    );
    assert_eq!(
        f.h.engine.calls.effects().background,
        Background::StrongBlur
    );

    // Brightness and the filters are in the Settings tab.
    click(&f.tab("Settings"));
    click(&f.button("Black and white"));
    assert_eq!(f.fx().get_filter(), "mono");
    assert_eq!(f.h.engine.calls.effects().filter, Filter::Mono);

    let bright = f.with_role("Adjust brightness", AccessibleRole::Switch);
    click(&bright);
    assert!(f.fx().get_brighten());
    assert!(f.h.engine.calls.effects().brighten);

    click(&f.button("Close video effects"));
    assert!(!f.fx().get_open());
    assert_eq!(
        f.fx().get_background(),
        "strong-blur",
        "closing keeps the choice"
    );
}

#[test]
fn the_panel_has_two_tabs_the_effects_and_the_settings() {
    let f = Fx::new();
    f.prejoin();
    f.camera_on();
    click(&f.button("Video effects"));
    assert_eq!(f.tab("Video effects").accessible_checked(), Some(true));
    assert_eq!(f.tab("Settings").accessible_checked(), Some(false));
    assert!(f.shows("Add new background"));
    assert!(f.shows("Others won't see your video while you preview."));
    assert!(!f.shows("Mirror my video"));

    click(&f.tab("Settings"));
    assert_eq!(f.tab("Settings").accessible_checked(), Some(true));
    assert!(f.shows("Mirror my video"));
    assert!(f.shows("Adjust brightness"));
    assert!(f.shows("Appearance section"));
    assert!(f.shows("Camera section"));
    assert!(!f.shows("Add new background"));
    assert!(!f.shows("Others won't see your video while you preview."));
    // The camera's own section lists the cameras by name; choosing one restarts the preview.
    assert_eq!(f.radio("Default camera").accessible_checked(), Some(true));
    click(&f.radio("Integrated Camera"));
    assert!(f.sent().contains(&"camera-device 1".to_owned()));
    assert_eq!(f.fx().get_camera(), 1);
    assert_eq!(
        f.radio("Integrated Camera").accessible_checked(),
        Some(true)
    );

    click(&f.tab("Video effects"));
    assert!(f.shows("Add new background"));
    assert!(!f.shows("Mirror my video"));
}

#[test]
fn preview_shows_our_camera_to_us_only_and_apply_turns_the_video_on() {
    let f = Fx::new();
    f.prejoin();
    click(&f.button("Video effects"));
    assert!(f.fake.previews().is_empty());

    // Preview: the camera runs for the preview, the camera switch (what we join with) stays off.
    click(&f.button("Preview"));
    assert!(f.sent().contains(&"video-preview on".to_owned()));
    assert_eq!(f.fake.previews(), ["on default"]);
    assert!(
        !f.cs().get_prejoin_camera(),
        "nothing is sent for a preview"
    );
    assert!(f.fake.preview_frame(&half_white()));
    assert!(f.fx().get_has_preview(), "the pre-join picture shows it");

    // Apply and turn on video: the switch goes on, the same camera keeps running.
    click(&f.button("Apply and turn on video"));
    assert!(f.sent().contains(&"video-apply".to_owned()));
    assert!(f.cs().get_prejoin_camera());
    assert!(!f.fx().get_open(), "the panel closes");
    assert_eq!(f.fake.previews(), ["on default"]);
}

#[test]
fn in_a_call_preview_runs_while_the_camera_is_off_and_apply_sends_it() {
    let f = Fx::new();
    f.connected();
    f.fake.emit(Event::Camera {
        available: true,
        on: false,
    });
    click(&f.button("Camera options"));
    // The menu shows our camera at once: the preview starts with the menu.
    crate::ui::call::watch(&f.h.app);
    assert_eq!(f.fake.previews(), ["on default"]);
    click(&f.button("More video effects and settings"));
    assert!(f.fx().get_open());
    assert_eq!(f.cs().get_menu(), "");
    crate::ui::call::watch(&f.h.app);
    assert_eq!(
        f.fake.previews(),
        ["on default", "off"],
        "the menu closed with it"
    );

    click(&f.button("Preview"));
    assert_eq!(f.fake.previews().last().unwrap(), "on default");
    assert!(f.fake.preview_frame(&half_white()));
    assert!(f.shows("Your video preview"));
    assert!(
        !f.fake.calls().contains(&"camera on".to_owned()),
        "a preview sends nothing"
    );

    click(&f.button("Apply and turn on video"));
    assert_eq!(f.fake.calls().last().unwrap(), "camera on");
    assert!(!f.fx().get_open());
    assert_eq!(
        f.fake.previews().last().unwrap(),
        "off",
        "the preview lets the camera go before it is sent"
    );
}

#[test]
fn the_camera_chevron_opens_a_menu_with_the_cameras_and_the_backgrounds() {
    let f = Fx::new();
    f.connected();
    assert_eq!(f.cs().get_menu(), "");
    click(&f.button("Camera options"));
    assert_eq!(f.cs().get_menu(), "camera");
    // The cameras by their names, the chosen one checked.
    for name in ["Default camera", "Integrated Camera", "USB Webcam"] {
        assert!(f.shows(name), "{name}");
    }
    assert_eq!(f.radio("Default camera").accessible_checked(), Some(true));
    for row in ["None", "Standard blur", "Adjust brightness"] {
        assert!(f.shows(row), "{row}");
    }
    assert_eq!(f.radio("None").accessible_checked(), Some(true));
    assert!(f.shows("More video effects and settings"));

    click(&f.radio("USB Webcam"));
    assert_eq!(f.fx().get_camera(), 2);
    assert!(f.sent().contains(&"camera-device 2".to_owned()));
    assert_eq!(f.radio("USB Webcam").accessible_checked(), Some(true));

    // Standard blur is a setting of what is sent; its chevron lists the strengths.
    click(&f.radio("Standard blur"));
    assert!(f.sent().contains(&"fx-background blur".to_owned()));
    assert_eq!(f.h.engine.calls.effects().background, Background::Blur);
    assert_eq!(f.radio("Standard blur").accessible_checked(), Some(true));
    assert_eq!(f.radio("None").accessible_checked(), Some(false));
    click(&f.button("Blur strength"));
    click(&f.radio("Strong"));
    assert_eq!(
        f.h.engine.calls.effects().background,
        Background::StrongBlur
    );

    click(&f.with_role("Adjust brightness", AccessibleRole::Switch));
    assert!(f.h.engine.calls.effects().brighten);

    // The whole panel is one click away, and takes the menu's place.
    click(&f.button("More video effects and settings"));
    assert!(f.fx().get_open());
    assert_eq!(f.cs().get_menu(), "");
    assert!(f.shows("Add new background"));
}

#[test]
fn the_camera_menu_stays_clear_of_the_side_panel_and_inside_the_window() {
    let f = Fx::new();
    f.connected();
    click(&f.button("More"));
    click(&f.button("Camera options"));
    let menu = f.with_role("Camera settings", AccessibleRole::List);
    let (pos, size) = (menu.absolute_position(), menu.size());
    // The harness window is 1200x800.
    assert!(pos.x >= 0.0 && pos.y >= 0.0, "{pos:?}");
    assert!(
        pos.x + size.width <= 1200.0 - 320.0 + 0.5,
        "left of the More panel: {pos:?} {size:?}"
    );
    assert!(pos.y + size.height <= 800.0, "{pos:?} {size:?}");
}

#[test]
fn the_choices_are_kept_between_runs() {
    let dir = std::env::temp_dir().join(format!(
        "ironteams-ui-video-fx-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = Fx::new();
    f.h.engine.calls.keep_effects_in(dir.clone());
    f.prejoin();
    f.camera_on();
    click(&f.button("Video effects"));
    click(&f.button("Standard blur"));
    click(&f.tab("Settings"));
    click(&f.with_role("Mirror my video", AccessibleRole::Switch));
    click(&f.with_role("Adjust brightness", AccessibleRole::Switch));

    let kept = crate::backend::call::video_fx::VideoFxPrefs::load(&dir);
    assert_eq!(kept.background, "blur");
    assert!(!kept.mirror, "the mirror was turned off");
    assert!(kept.brighten);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mirror_changes_only_the_self_view() {
    let f = Fx::new();
    f.prejoin();
    f.camera_on();
    assert!(f.fx().get_mirror(), "mirrored by default, as Teams does");
    f.fake.preview_frame(&half_white());
    assert!(
        f.preview_left_red() < 40,
        "mirrored: the right (black) half shows on the left"
    );
    let sent = f.h.engine.calls.effects();

    click(&f.button("Video effects"));
    click(&f.tab("Settings"));
    click(&f.with_role("Mirror my video", AccessibleRole::Switch));
    assert!(f.sent().contains(&"fx-mirror off".to_owned()));
    assert!(!f.fx().get_mirror());
    f.fake.preview_frame(&half_white());
    assert!(f.preview_left_red() > 200, "not mirrored now");
    assert_eq!(
        f.h.engine.calls.effects(),
        sent,
        "what the others see is untouched"
    );
}

#[test]
fn the_call_s_more_panel_opens_the_effects() {
    let f = Fx::new();
    f.prejoin();
    click(&f.button("Join now"));
    f.fake.emit(Event::Connected);
    click(&f.button("More"));
    assert!(
        f.all("Camera")
            .iter()
            .any(|e| e.accessible_role() == Some(AccessibleRole::Combobox)),
        "More lists the camera with the audio devices"
    );
    click(&f.button("Video effects"));
    assert!(f.fx().get_open());
    click(&f.button("Standard blur"));
    assert_eq!(f.fx().get_background(), "blur", "{:?}", f.sent());
    click(&f.radio("Strong"));
    assert_eq!(
        f.h.engine.calls.effects().background,
        Background::StrongBlur
    );
    click(&f.button("Close video effects"));
    assert!(f.shows("Device settings"), "back to More");
}
