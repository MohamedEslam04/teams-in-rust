//! More of the message menu and the chat around it: Unpin, links, Immersive Reader, Share to
//! Outlook, Report, Pin on channel posts, the file card's menu, Loop cards, presence in the
//! people lists and the pop-out chat window.

use super::*;
use crate::backend::MediaCmd;
use crate::{FileItem, ImageItem, LoopCard, MediaStore, PeopleStore};

/// Records the `MediaStore` and `PeopleStore` commands as `"<store> <name> <argument>"` (the unit
/// separator shown as `|`).
fn store_log(app: &AppWindow) -> Log {
    let log = Log::default();
    let l = log.clone();
    app.global::<MediaStore>().on_command(move |n, a| {
        l.borrow_mut()
            .push(format!("media {n} {}", a.replace('\u{1f}', "|")))
    });
    let l = log.clone();
    app.global::<PeopleStore>()
        .on_command(move |n, a| l.borrow_mut().push(format!("people {n} {a}")));
    log
}

fn urls(list: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        list.iter()
            .map(|u| SharedString::from(*u))
            .collect::<Vec<_>>(),
    ))
}

fn has_label(app: &AppWindow, text: &str) -> bool {
    !find_label(app, text).is_empty()
}

fn tap(app: &AppWindow, text: &str) {
    let e = find_label(app, text)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("an element labelled {text:?}"));
    click(&e);
}

#[test]
fn a_pinned_message_offers_unpin_instead_of_pin() {
    let mut pinned = msg("m1", "pinned one", false);
    pinned.pinned = true;
    let app = app_with_messages(vec![pinned, msg("m2", "plain", false)]);
    let log = store_log(&app);

    open_menu(&app, &rows(&app)[0]);
    let labels = menu_labels(&app);
    assert!(labels.contains(&"Unpin".to_string()), "{labels:?}");
    assert!(!labels.contains(&"Pin for everyone".to_string()));
    click(&menu_item(&app, "Unpin"));
    assert_eq!(*log.borrow(), ["people pin-unpin m1"]);
    assert!(!app.get_menu_open());

    open_menu(&app, &rows(&app)[1]);
    let labels = menu_labels(&app);
    assert!(labels.contains(&"Pin for everyone".to_string()));
    assert!(!labels.contains(&"Unpin".to_string()));
}

#[test]
fn the_link_of_a_message_can_be_opened_and_copied_from_its_menu() {
    let mut m = msg("m1", "see the docs", false);
    m.urls = urls(&["https://example.com/a"]);
    let app = app_with_messages(vec![m, msg("m2", "no links", false)]);
    let log = store_log(&app);
    let opened = Log::default();
    let o = opened.clone();
    app.on_open_url(move |u| o.borrow_mut().push(u.to_string()));

    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Open link"));
    assert_eq!(*opened.borrow(), ["https://example.com/a"]);
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Copy link"));
    assert_eq!(*log.borrow(), ["media link-copy https://example.com/a"]);

    open_menu(&app, &rows(&app)[1]);
    let labels = menu_labels(&app);
    assert!(!labels.contains(&"Open link".to_string()), "{labels:?}");
}

#[test]
fn several_links_are_listed_under_open_link_and_copy_link() {
    let mut m = msg("m1", "two links", false);
    m.urls = urls(&["https://example.com/a", "https://example.com/b"]);
    let app = app_with_messages(vec![m]);
    let log = store_log(&app);
    let opened = Log::default();
    let o = opened.clone();
    app.on_open_url(move |u| o.borrow_mut().push(u.to_string()));

    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Open link"));
    assert!(app.get_menu_open(), "the links are listed");
    click(&menu_item(&app, "https://example.com/b"));
    assert_eq!(*opened.borrow(), ["https://example.com/b"]);

    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Copy link"));
    click(&menu_item(&app, "https://example.com/a"));
    assert_eq!(*log.borrow(), ["media link-copy https://example.com/a"]);
}

#[test]
fn immersive_reader_shows_the_message_alone_with_size_and_line_focus() {
    let app = app_with_messages(vec![msg("m1", "a long message to read slowly", false)]);
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Immersive Reader"));
    assert!(!app.get_menu_open());
    assert!(app.get_reader_open());
    assert_eq!(app.get_reader_msg().plain, "a long message to read slowly");
    assert!(has_label(&app, "a long message to read slowly"));

    let three = find_label(&app, "3 lines").remove(0);
    assert_eq!(three.accessible_checked(), Some(false));
    click(&three);
    assert_eq!(
        find_label(&app, "3 lines")[0].accessible_checked(),
        Some(true)
    );
    assert_eq!(
        find_label(&app, "Line focus: off")[0].accessible_checked(),
        Some(false)
    );
    tap(&app, "A+");

    tap(&app, "Close Immersive Reader");
    assert!(!app.get_reader_open());

    // Escape closes it too.
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Immersive Reader"));
    assert!(app.get_reader_open());
    key(&app, Key::Escape, true);
    key(&app, Key::Escape, false);
    assert!(!app.get_reader_open());
}

#[test]
fn share_to_outlook_and_report_go_to_the_engine() {
    let app = app_with_messages(vec![msg("m1", "theirs", false), msg("m2", "mine", true)]);
    let log = store_log(&app);
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Share to Outlook"));
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Report this message"));
    assert_eq!(*log.borrow(), ["media outlook m1", "media report m1"]);

    open_menu(&app, &rows(&app)[1]);
    let labels = menu_labels(&app);
    assert!(
        !labels.contains(&"Report this message".to_string()),
        "no report of my own message: {labels:?}"
    );
}

#[test]
fn channel_posts_offer_pin_for_everyone() {
    let app = app_with_messages(vec![msg("m1", "a post", false)]);
    app.set_current_is_channel(true);
    let log = record_actions(&app);
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Pin for everyone"));
    assert_eq!(*log.borrow(), ["pin m1"]);
}

const SP_FILE: &str = "https://contoso-my.sharepoint.com/personal/u/Documents/plan.docx";

#[test]
fn a_file_card_says_where_it_opens_and_its_menu_downloads_copies_shares_and_opens_it_online() {
    let mut m = msg("m1", "the plan", false);
    m.files = ModelRc::new(VecModel::from(vec![FileItem {
        name: "plan.docx".into(),
        url: SP_FILE.into(),
        ext: "DOCX".into(),
        place: "OneDrive".into(),
        detail: "Opens in the desktop app".into(),
    }]));
    let app = app_with_messages(vec![m]);
    let log = store_log(&app);
    let opened = Log::default();
    let o = opened.clone();
    app.on_open_url(move |u| o.borrow_mut().push(u.to_string()));

    assert!(has_label(&app, "Opens in the desktop app"));
    assert!(!has_label(&app, "Open in browser"));

    for (item, cmd) in [
        ("Download", "download"),
        ("Copy link", "copy"),
        ("Share", "share"),
        ("Open in OneDrive", "web"),
    ] {
        tap(&app, "More options for plan.docx");
        tap(&app, item);
        assert_eq!(
            log.borrow().last().unwrap(),
            &format!("media file {cmd}|{SP_FILE}")
        );
    }
    tap(&app, "More options for plan.docx");
    tap(&app, "Open");
    assert_eq!(*opened.borrow(), [format!("kelma-file:DOCX:{SP_FILE}")]);
}

fn pictures(urls: &[&str]) -> ModelRc<ImageItem> {
    ModelRc::new(VecModel::from(
        urls.iter()
            .map(|u| ImageItem {
                url: (*u).into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    ))
}

#[test]
fn a_picture_offers_copy_open_in_browser_download_and_share_in_the_message_menu() {
    let mut m = msg("m1", "a screenshot", false);
    m.images = pictures(&["https://pics.invalid/a.png"]);
    let app = app_with_messages(vec![m, msg("m2", "no picture", false)]);
    let log = store_log(&app);

    open_menu(&app, &rows(&app)[0]);
    let labels = menu_labels(&app);
    for entry in ["Copy image", "Open in Browser", "Download", "Share"] {
        assert!(labels.contains(&entry.to_string()), "{entry}: {labels:?}");
    }
    click(&menu_item(&app, "Copy image"));
    assert!(!app.get_menu_open());
    for (entry, cmd) in [("Open in Browser", "browser"), ("Download", "download")] {
        open_menu(&app, &rows(&app)[0]);
        click(&menu_item(&app, entry));
        assert_eq!(
            log.borrow().last().unwrap(),
            &format!("media image-do {cmd}|m1|https://pics.invalid/a.png")
        );
    }
    assert_eq!(
        log.borrow()[0],
        "media image-do copy|m1|https://pics.invalid/a.png"
    );

    open_menu(&app, &rows(&app)[1]);
    let labels = menu_labels(&app);
    assert!(!labels.contains(&"Copy image".to_string()), "{labels:?}");
    assert!(
        !labels.contains(&"Open in Browser".to_string()),
        "{labels:?}"
    );
}

#[test]
fn several_pictures_are_listed_under_each_picture_entry() {
    let mut m = msg("m1", "two screenshots", false);
    m.images = pictures(&["https://pics.invalid/a.png", "https://pics.invalid/b.png"]);
    let app = app_with_messages(vec![m]);
    let log = store_log(&app);

    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Download"));
    assert!(app.get_menu_open(), "the pictures are listed");
    click(&menu_item(&app, "Image 2"));
    assert_eq!(
        *log.borrow(),
        ["media image-do download|m1|https://pics.invalid/b.png"]
    );
}

#[test]
fn a_file_offers_open_download_copy_link_and_share_in_the_message_menu() {
    let mut m = msg("m1", "the plan", false);
    m.files = ModelRc::new(VecModel::from(vec![FileItem {
        name: "plan.docx".into(),
        url: SP_FILE.into(),
        ext: "DOCX".into(),
        place: "OneDrive".into(),
        detail: "Opens in the desktop app".into(),
    }]));
    let app = app_with_messages(vec![m, msg("m2", "no file", false)]);
    let log = store_log(&app);
    let opened = Log::default();
    let o = opened.clone();
    app.on_open_url(move |u| o.borrow_mut().push(u.to_string()));

    for (entry, cmd) in [
        ("Download file", "download"),
        ("Copy file link", "copy"),
        ("Share file", "share"),
    ] {
        open_menu(&app, &rows(&app)[0]);
        click(&menu_item(&app, entry));
        assert!(!app.get_menu_open());
        assert_eq!(
            log.borrow().last().unwrap(),
            &format!("media file {cmd}|{SP_FILE}")
        );
    }
    open_menu(&app, &rows(&app)[0]);
    click(&menu_item(&app, "Open file"));
    assert_eq!(*opened.borrow(), [format!("kelma-file:DOCX:{SP_FILE}")]);

    open_menu(&app, &rows(&app)[1]);
    let labels = menu_labels(&app);
    assert!(!labels.contains(&"Open file".to_string()), "{labels:?}");
}

#[test]
fn a_file_elsewhere_has_no_open_in_entry() {
    let mut m = msg("m1", "a zip", false);
    m.files = ModelRc::new(VecModel::from(vec![FileItem {
        name: "logs.zip".into(),
        url: "https://example.com/logs.zip".into(),
        ext: "ZIP".into(),
        place: "".into(),
        detail: "Opens in the browser".into(),
    }]));
    let app = app_with_messages(vec![m]);
    tap(&app, "More options for logs.zip");
    assert!(has_label(&app, "Download"));
    assert!(!has_label(&app, "Open in OneDrive") && !has_label(&app, "Open in SharePoint"));
}

#[test]
fn a_loop_link_is_drawn_as_a_card_that_opens_in_loop() {
    let mut m = msg("m1", "our plan", false);
    m.loops = ModelRc::new(VecModel::from(vec![LoopCard {
        url: "https://loop.cloud.microsoft/p/abc".into(),
        title: "Sprint plan".into(),
    }]));
    let app = app_with_messages(vec![m]);
    let opened = Log::default();
    let o = opened.clone();
    app.on_open_url(move |u| o.borrow_mut().push(u.to_string()));
    assert!(has_label(&app, "Open in Loop"));
    tap(&app, "Open Sprint plan in Loop");
    assert_eq!(*opened.borrow(), ["https://loop.cloud.microsoft/p/abc"]);
}

#[test]
fn a_meeting_link_is_a_card_whose_join_runs_the_calendar_join_with_that_link() {
    let url = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0";
    let mut m = msg("m1", "join us", false);
    m.joins = urls(&[url]);
    let app = app_with_messages(vec![m]);
    let asked = Log::default();
    let a = asked.clone();
    app.global::<crate::CalStore>()
        .on_command(move |n, arg| a.borrow_mut().push(format!("{n} {arg}")));
    assert!(has_label(&app, "Meeting link"));
    find_label(&app, "Join meeting")
        .into_iter()
        .find(|e| e.accessible_role() == Some(testing::AccessibleRole::Button))
        .expect("a Join button")
        .invoke_accessible_default_action();
    assert_eq!(*asked.borrow(), [format!("join {url}")]);
}

#[test]
fn a_message_without_a_meeting_link_has_no_join_card() {
    let app = app_with_messages(vec![msg("m1", "see https://example.com/a", false)]);
    assert!(!has_label(&app, "Meeting link"));
    assert!(!has_label(&app, "Join meeting"));
}

// ---- engine-backed ----------------------------------------------------------------------

#[test]
fn a_pasted_meeting_link_gets_a_join_card_and_is_a_link_of_the_message() {
    let url = "https://teams.cloud.microsoft/l/meetup-join/19%3ameeting_x%40thread.v2/0";
    let other = "https://example.invalid/docs";
    let h = Harness::new();
    h.engine.test_store(|st| {
        st.upsert_message(&store::MessageRow {
            id: "link-1".into(),
            conversation_id: NOTES.into(),
            parent_id: None,
            sender_mri: "8:orgid:00000000-0000-0000-0000-000000000002".into(),
            sender_name: "Ahmed Ali".into(),
            compose_ms: 1_795_000_000_000,
            edited_ms: None,
            deleted: false,
            message_type: "RichText/Html".into(),
            // Plain text, not an <a>: the way a pasted link can arrive.
            content_html: format!("<p>join {url} and read {other}.</p>"),
            client_message_id: None,
            reactions_json: "{}".into(),
        })
        .unwrap();
    });
    h.select(NOTES);
    let m = h
        .app
        .get_messages()
        .iter()
        .find(|m| m.id == "link-1")
        .expect("listed");
    assert_eq!(
        m.joins.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        [url]
    );
    assert_eq!(
        m.urls.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        [url, other],
        "both are links of the message, the full stop is not part of the second"
    );
    assert!(has_label(&h.app, "Meeting link"));
}

/// Sends `MediaStore` commands to the engine the way `main.rs` wires them.
fn wire_media(h: &Harness) {
    let run = h.runner();
    h.app.global::<MediaStore>().on_command(move |n, a| {
        if let Some(c) = MediaCmd::parse(&n, &a) {
            run(Cmd::Media(c));
        }
    });
}

#[test]
fn the_participants_list_shows_each_persons_presence() {
    let h = Harness::new();
    let other = "8:orgid:presence-test-participant";
    h.engine.test_set_presence(other, "Busy");
    h.engine.test_set_members(BACKEND, &[demo::ME, other]);
    h.select(BACKEND);
    h.runner()(Cmd::People(
        crate::backend::PeopleCmd::parse("participants-open", "").unwrap(),
    ));
    let members = h.app.global::<PeopleStore>().get_members();
    let presence: Vec<String> = members.iter().map(|p| p.presence.to_string()).collect();
    assert_eq!(presence.len(), 2);
    assert_eq!(presence[1], "Busy");
}

#[test]
fn pop_out_opens_the_chat_in_its_own_window_and_sends_from_there() {
    let h = Harness::new();
    wire_media(&h);
    h.app
        .global::<MediaStore>()
        .invoke_command("popout".into(), BACKEND.into());
    let cw = crate::ui::popout::popout_window().expect("the pop-out window");
    assert_eq!(cw.get_conv_id(), BACKEND);
    assert_eq!(cw.get_chat_title(), "Backend Team");
    let before = cw.get_messages().row_count();
    assert!(before > 0, "the chat's messages are shown");

    // Its composer sends through the outbox (offline here, so the message waits there).
    cw.invoke_send("hello from the pop-out".into());
    let msgs = cw.get_messages();
    assert_eq!(msgs.row_count(), before + 1);
    assert_eq!(
        msgs.row_data(before).unwrap().plain,
        "hello from the pop-out"
    );

    // A channel stays in the main window.
    h.app
        .global::<MediaStore>()
        .invoke_command("popout".into(), "19:team1@thread.tacv2".into());
    assert_eq!(cw.get_conv_id(), BACKEND);
}
#[test]
fn the_pop_out_opens_at_the_newest_message_follows_new_ones_and_has_a_send_button() {
    use i_slint_backend_testing::AccessibleRole;
    let h = Harness::new();
    wire_media(&h);
    let run = h.runner();
    h.select(BACKEND);
    // A long chat: far more than one window of messages.
    for i in 0..40 {
        // Rows of different heights, so the list's height estimate moves as rows show.
        let text = if i % 5 == 0 {
            format!("line {i} {}", super::overflow::LONG)
        } else {
            format!("line {i}")
        };
        run(Cmd::Send(text, String::new()));
    }
    h.app
        .global::<MediaStore>()
        .invoke_command("popout".into(), BACKEND.into());
    let cw = crate::ui::popout::popout_window().expect("the pop-out window");
    // The window gets its size once it is mapped, after the messages were set.
    cw.window().set_size(LogicalSize::new(560., 680.));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(
        cw.get_at_newest(),
        "the pop-out opens at the newest message"
    );

    // A Send button next to the box (Enter still sends).
    let send = ElementHandle::find_by_accessible_label(&cw, "Send")
        .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .expect("a Send button");
    assert!(
        super::responsive::on_screen(&send, 560.),
        "Send on screen at {:?} {:?}",
        send.absolute_position(),
        send.size()
    );
    cw.set_draft("from the button".into());
    click(&send);
    assert_eq!(cw.get_draft(), "");
    let msgs = cw.get_messages();
    let last = msgs.row_data(msgs.row_count() - 1).unwrap();
    assert_eq!(last.plain, "from the button");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(
        cw.get_at_newest(),
        "still at the newest after a new message"
    );

    for m in h
        .app
        .get_messages()
        .iter()
        .filter(|m| m.id.starts_with("pending-"))
    {
        run(Cmd::DiscardSend(m.id.to_string()));
    }
}
