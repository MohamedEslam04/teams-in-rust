//! Pictures of the screens a colour or theme change must not alter, for the screenshot comparison
//! (.github/workflows/ui-compare.yml): the real window drawn by Slint's software renderer into
//! memory and saved as PNG files, in the dark and the light theme. The workflow draws them at the
//! base commit and at the pull request, and `scripts/ci/compare-shots.sh` compares the pixels.
//! By hand:
//! `IRONTEAMS_SHOTS_DIR=/tmp/shots cargo test -p ironteams --bin ironteams -- --ignored theme_screens_to_png`.
//!
//! Files: `<theme>-<screen>.png` (`dark`, `light`, and `harbor` for one derived theme) for the
//! chat view with the nav (`chat`), an open message menu (`message-menu`), the call screen
//! (`call`), a calendar day (`calendar-day`) and one dialog (`dialog`), each at 1280x800. The
//! Settings pages are `settings_shots`.
//!
//! The pictures must not depend on the day, the machine or the network: nothing here goes through
//! the engine (its chat list and message times follow the clock), the models are filled by hand
//! with fixed text and times, the calendar is built for a fixed day and "now", animations run on
//! the mocked clock, and the fonts are the ones `SLINT_FONT_PATH` names.

use super::settings::{Home, env};
use super::settings_shots::{ShotPlatform, settle, shoot};
use super::*;
use crate::backend::{Ctx, DEFAULT_PREFS, build};
use crate::ui::settings::install;
use crate::{CalStore, CallPerson, CallStore, ChatRow, SettingsStore, Theme};
use serde_json::json;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::{Color, PhysicalSize, Rgb8Pixel, StyledText};

const W: u32 = 1280;
const H: u32 = 800;
const HOUR: i64 = 3_600_000;

fn color(name: &str) -> Color {
    let c = crate::data::color_for(name);
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn chat(id: &str, title: &str, kind: &str, preview: &str, time: &str) -> ChatRow {
    ChatRow {
        id: id.into(),
        title: title.into(),
        preview: preview.into(),
        time: time.into(),
        initials: crate::data::initials(title).into(),
        color: color(title),
        section: "chats".into(),
        kind: kind.into(),
        group: kind == "group",
        ..Default::default()
    }
}

/// A message as the chat shows it, its body made the way the app makes it: from Teams HTML, so a
/// mention takes the colour of the theme drawn now (`markdown::set_mention_color`).
fn message(id: &str, sender: &str, mine: bool, time: &str, html: &str) -> Msg {
    let rt = render::parse_teams_html(html);
    let r = crate::markdown::render_blocks(&rt);
    Msg {
        id: id.into(),
        sender: sender.into(),
        initials: crate::data::initials(sender).into(),
        color: color(sender),
        mine,
        time: time.into(),
        show_header: true,
        body: StyledText::from_markdown(&r.markdown)
            .unwrap_or_else(|_| StyledText::from_plain_text(&r.plain)),
        plain: r.plain.into(),
        ..Default::default()
    }
}

fn messages() -> Vec<Msg> {
    let mention = r#"<p>Hi <span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">Mohamed Eslam</span>, can you review the cache PR today?</p>"#;
    vec![
        Msg {
            day: "Wednesday, March 12".into(),
            ..message(
                "m1",
                "Sara Hassan",
                false,
                "09:15",
                "<p>Morning all, the build is green again.</p>",
            )
        },
        Msg {
            mentions_me: true,
            ..message("m2", "Ahmed Ali", false, "09:40", mention)
        },
        Msg {
            reactions: ModelRc::new(VecModel::from(vec![crate::ReactionItem {
                key: "like".into(),
                emoji: "👍".into(),
                name: "Like".into(),
                count: 2,
                mine: true,
                names: "You, Sara Hassan".into(),
                people: ModelRc::default(),
            }])),
            ..message(
                "m3",
                "Me",
                true,
                "09:41",
                "<p>On it, thanks for the ping.</p>",
            )
        },
        message(
            "m4",
            "Sara Hassan",
            false,
            "09:52",
            "<p>Great. I will merge the other branch after lunch.</p>",
        ),
    ]
}

fn open_chat(app: &AppWindow) {
    app.set_view("chat".into());
    app.set_synced(true);
    app.set_connected(true);
    app.set_chats_main(ModelRc::new(VecModel::from(vec![
        ChatRow {
            unread: true,
            mention: true,
            ..chat(
                "c1",
                "Backend Team",
                "group",
                "Ahmed: Mohamed, can you review the cache PR today?",
                "09:40",
            )
        },
        chat("c2", "Sara Hassan", "chat", "Great, thanks!", "Tue"),
        chat(
            "c3",
            "Omar Khaled",
            "chat",
            "You: see you at the demo",
            "Mon",
        ),
        chat(
            "c4",
            "Design Crit",
            "group",
            "Mona: new mockups are up",
            "Mar 7",
        ),
    ])));
    app.set_current_id("c1".into());
    app.set_current_title("Backend Team".into());
    app.set_current_initials("BT".into());
    app.set_current_color(color("Backend Team"));
    app.set_current_kind("group".into());
    app.set_messages(ModelRc::new(VecModel::from(messages())));
}

fn in_call(app: &AppWindow) {
    let person = |name: &str| CallPerson {
        id: name.into(),
        name: name.into(),
        initials: crate::data::initials(name).into(),
        ..Default::default()
    };
    let people = vec![
        CallPerson {
            me: true,
            role: "organizer".into(),
            ..person("Mohamed Eslam")
        },
        CallPerson {
            speaking: true,
            ..person("Ada Lovelace")
        },
        CallPerson {
            muted: true,
            ..person("Bo Ström")
        },
        CallPerson {
            hand: true,
            ..person("Sara Hassan")
        },
        person("Omar Khaled"),
    ];
    let s = app.global::<CallStore>();
    s.set_kind("meeting".into());
    s.set_title("Sprint planning".into());
    s.set_my_name("Mohamed Eslam".into());
    s.set_my_initials("ME".into());
    s.set_timer("12:34".into());
    s.set_chat_id("c1".into());
    s.set_can_share(true);
    s.set_in_call(people.len() as i32);
    s.set_people(ModelRc::new(VecModel::from(people)));
    s.set_phase("connected".into());
}

fn calendar_day(app: &AppWindow) {
    let day = ::calendar::Date::new(2026, 11, 18);
    let at = |h: &str| json!({"dateTime": format!("2026-11-18T{h}:00.0000000"), "timeZone": "UTC"});
    let ev = |id: &str, from: &str, to: &str| json!({"id": id, "subject": id, "start": at(from), "end": at(to)});
    let events = ::calendar::parse_events(&json!({ "value": [
        ev("Stand-up", "09:00", "09:15"),
        ev("Design review", "10:00", "11:00"),
        ev("Lunch with Sara", "12:30", "13:30"),
        ev("Sprint review", "15:00", "16:00"),
    ]}));
    let refs: Vec<&::calendar::Event> = events.iter().collect();
    let data = build(
        &refs,
        &Ctx {
            date: day,
            view: ::calendar::View::Day(1),
            scale: 30,
            now_ms: day.midnight_ms() + 11 * HOUR,
            first_day: ::calendar::Weekday::Monday,
            mini_anchor: day,
            off: &|_| 0,
            prefs: &DEFAULT_PREFS,
        },
    );
    UiHandle::new(app.as_weak()).calendar(data);
    app.set_view("calendar".into());
}

/// Draws the window once without keeping the picture. Slint makes what an `if` or a timer adds (the
/// message menu, which fades in after a timer) when it first draws it; `shoot` then lets the
/// animations run before the picture that is kept.
fn warm_up(window: &MinimalSoftwareWindow) {
    window.set_size(PhysicalSize::new(W, H));
    let mut buf = vec![Rgb8Pixel::default(); (W * H) as usize];
    window.request_redraw();
    window.draw_if_needed(|r| {
        r.render(&mut buf, W as usize);
    });
}

#[test]
#[ignore = "writes PNGs; run by .github/workflows/ui-compare.yml"]
fn theme_screens_to_png() {
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
    window.set_size(PhysicalSize::new(W, H));

    let home = Home::new("theme-shots");
    let log = Log::default();
    let tokens = Arc::new(session::MemoryStore::default());
    let app = AppWindow::new().unwrap();
    crate::ui::install_local(&app);
    crate::composer::wire(&app);
    app.set_login_state("ready".into());
    app.set_me_initials("ME".into());
    install(&app, None, env(&home, &log, tokens));
    app.show().unwrap();
    crate::language::apply(&appearance::GeneralPrefs {
        language: Some("en".into()),
        ..Default::default()
    });
    let settings = app.global::<SettingsStore>();
    let shot = |name: &str| {
        warm_up(&window);
        shoot(&window, W, H, false, &dir.join(format!("{name}.png")));
    };

    // IronTeams Dark and Light, then Harbor (a built-in theme with a dark nav and a light content
    // area; its pictures are new, so the comparison has nothing to hold them against).
    for (theme, cmd, arg) in [
        ("dark", "theme", "dark"),
        ("light", "theme", "light"),
        ("harbor", "theme-pick", "builtin:harbor"),
    ] {
        // Applying the look also sets the colour mentions are drawn in, so the messages below
        // are made after it.
        settings.invoke_command(cmd.into(), arg.into());
        assert_eq!(app.global::<Theme>().get_dark(), theme == "dark");

        open_chat(&app);
        shot(&format!("{theme}-chat"));

        // The menu of the second message, at a fixed place.
        app.set_menu_msg(messages().swap_remove(1));
        app.set_menu_x(520.);
        app.set_menu_y(260.);
        app.set_menu_open(true);
        shot(&format!("{theme}-message-menu"));
        app.set_menu_open(false);

        in_call(&app);
        shot(&format!("{theme}-call"));
        app.global::<CallStore>().set_phase("".into());
        settle();

        calendar_day(&app);
        shot(&format!("{theme}-calendar-day"));
        let cal = app.global::<CalStore>();
        cal.invoke_ask(
            "meet-now".into(),
            "Meet now".into(),
            "Starts a Teams meeting right away and gives you the link.".into(),
            "Meeting title (optional)".into(),
            "".into(),
            "".into(),
            "Start meeting".into(),
        );
        shot(&format!("{theme}-dialog"));
        cal.invoke_close_all();
        app.set_view("chat".into());
    }

    // A light app with a dark call (PR 5): the call screen and the audio menu opened from it take
    // the call theme; the pictures are new, so the comparison has nothing to hold them against.
    settings.invoke_command("theme".into(), "light".into());
    settings.invoke_command("call-theme".into(), "builtin:dark".into());
    assert!(app.global::<Theme>().get_call().dark);
    assert!(!app.global::<Theme>().get_content().dark);
    open_chat(&app);
    in_call(&app);
    shot("callthemed-call");
    let call = app.global::<CallStore>();
    call.set_menu("devices".into());
    shot("callthemed-audio-menu");
    call.set_menu("".into());
    call.set_phase("".into());
    settle();
}
