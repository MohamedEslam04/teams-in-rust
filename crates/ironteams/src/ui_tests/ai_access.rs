//! AI access (docs/designs/mcp-access-control.md, slice 4): the chip in the chat header and its
//! popover, the "AI access…" item of the chat list menu, the overview, the first-time explainer,
//! the connect dialog, the palette actions and the pop-out window's chip. Everything is hidden
//! while `AiAccessStore.available` is false (the default and a build without the `mcp`
//! feature); the tests set it. Only the store's `command` callback is recorded, as the backend
//! is not part of this.

use super::*;
use crate::{AiAccessRow, AiAccessStore, ChatWindow, PaletteStore};
use i_slint_backend_testing::{AccessibleRole, ElementRoot};

const STANDUP: &str = "19:standup@thread.v2";

/// Every `AiAccessStore` command as `"<name> <argument>"` (a tab in the argument shows as " | ").
fn wire(app: &AppWindow) -> Log {
    let log = Log::default();
    let l = log.clone();
    app.global::<AiAccessStore>().on_command(move |name, arg| {
        l.borrow_mut()
            .push(format!("{name} {}", arg.replace('\t', " | ")));
    });
    log
}

/// The feature is on and the chip talks about `id` at `level`.
fn enable(app: &AppWindow, id: &str, level: &str) {
    let s = app.global::<AiAccessStore>();
    s.set_available(true);
    s.set_target_id(id.into());
    s.set_target_level(level.into());
}

fn chips(app: &impl ElementRoot) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_type_name(app, "AiAccessChip").collect()
}

fn chip_label(app: &impl ElementRoot) -> String {
    let c = chips(app);
    assert_eq!(c.len(), 1, "one AI access chip");
    assert_eq!(c[0].accessible_role(), Some(AccessibleRole::Button));
    label(&c[0])
}

fn menu_items(app: &impl ElementRoot) -> Vec<ElementHandle> {
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, "MenuItem").collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

fn menu_labels(app: &impl ElementRoot) -> Vec<String> {
    menu_items(app).iter().map(label).collect()
}

fn menu_item(app: &impl ElementRoot, name: &str) -> ElementHandle {
    menu_items(app)
        .into_iter()
        .find(|i| label(i) == name)
        .unwrap_or_else(|| panic!("menu item {name:?} in {:?}", menu_labels(app)))
}

/// A button (or switch) with this accessible label; there must be exactly one.
fn control(app: &impl ElementRoot, name: &str) -> ElementHandle {
    let mut v: Vec<_> = ElementHandle::find_by_accessible_label(app, name)
        .filter(|e| {
            matches!(
                e.accessible_role(),
                Some(AccessibleRole::Button | AccessibleRole::Switch)
            )
        })
        .collect();
    assert_eq!(v.len(), 1, "one control labelled {name:?}");
    v.remove(0)
}

fn has_text(app: &impl ElementRoot, text: &str) -> bool {
    ElementHandle::find_by_accessible_label(app, text).count() > 0
}

fn open_chat(level: &str) -> Harness {
    let h = Harness::new();
    h.select(BACKEND);
    enable(&h.app, BACKEND, level);
    h
}

fn row(id: &str, title: &str, level: &str, last_read: &str) -> AiAccessRow {
    AiAccessRow {
        id: id.into(),
        title: title.into(),
        level: level.into(),
        last_read: last_read.into(),
    }
}

fn set_rows(app: &AppWindow, rows: Vec<AiAccessRow>) {
    app.global::<AiAccessStore>()
        .set_rows(ModelRc::new(VecModel::from(rows)));
}

// ---- the chip ----------------------------------------------------------------------------

#[test]
fn nothing_shows_while_ai_access_is_unavailable() {
    let h = Harness::new();
    h.select(BACKEND);
    assert!(chips(&h.app).is_empty());
    // The other two stores' values do not bring it back.
    let s = h.app.global::<AiAccessStore>();
    s.set_target_id(BACKEND.into());
    s.set_target_level("read".into());
    s.set_overview_open(true);
    s.set_explainer_open(true);
    s.set_connect_open(true);
    assert!(chips(&h.app).is_empty());
    for dialog in ["AiOverviewCard", "AiExplainerCard", "AiConnectCard"] {
        assert_eq!(
            ElementHandle::find_by_element_type_name(&h.app, dialog).count(),
            0,
            "{dialog} stays hidden"
        );
    }
}

#[test]
fn the_chip_says_the_level_in_words() {
    let h = open_chat("blocked");
    assert_eq!(chip_label(&h.app), "AI: Blocked");
    for (level, text) in [
        ("read", "AI: Can read"),
        ("read_draft", "AI: Can read and draft"),
        ("blocked", "AI: Blocked"),
    ] {
        h.app
            .global::<AiAccessStore>()
            .set_target_level(level.into());
        assert_eq!(chip_label(&h.app), text, "{level}");
    }
}

#[test]
fn a_pause_greys_the_chip_and_says_why_but_keeps_the_level() {
    let h = open_chat("read");
    let s = h.app.global::<AiAccessStore>();
    s.set_account_paused(true);
    assert_eq!(chip_label(&h.app), "AI: Can read. Paused for this account");
    s.set_master_paused(true);
    assert_eq!(chip_label(&h.app), "AI: Can read. Paused by master switch");
    s.set_account_paused(false);
    s.set_target_level("read_draft".into());
    assert_eq!(
        chip_label(&h.app),
        "AI: Can read and draft. Paused by master switch"
    );
    s.set_master_paused(false);
    assert_eq!(chip_label(&h.app), "AI: Can read and draft");
}

// ---- channels (slice 6): read-only ----------------------------------------------------------

const GENERAL: &str = "19:team1@thread.tacv2";

fn open_channel(level: &str) -> Harness {
    let h = Harness::new();
    h.runner()(Cmd::SelectChannel(GENERAL.into(), "General".into()));
    enable(&h.app, GENERAL, level);
    h
}

#[test]
fn a_channel_has_a_chip_only_while_ai_access_is_available() {
    let h = Harness::new();
    h.runner()(Cmd::SelectChannel(GENERAL.into(), "General".into()));
    assert!(chips(&h.app).is_empty());
    h.app.global::<AiAccessStore>().set_available(true);
    assert_eq!(chip_label(&h.app), "AI: Blocked");
}

#[test]
fn a_channels_popover_offers_blocked_and_can_read_but_no_drafting() {
    let h = open_channel("read");
    let log = wire(&h.app);
    assert_eq!(chip_label(&h.app), "AI: Can read");
    click(&chips(&h.app)[0]);
    assert_eq!(
        menu_labels(&h.app),
        [
            "Blocked",
            "Can read",
            "This account: Active",
            "AI access: Active",
            "Review all AI access…"
        ]
    );
    for (name, note, on) in [
        ("Blocked", "The AI cannot see this channel.", false),
        (
            "Can read",
            "The AI can read this channel. It cannot draft in channels.",
            true,
        ),
    ] {
        let item = menu_item(&h.app, name);
        assert_eq!(item.accessible_description().as_deref(), Some(note));
        assert_eq!(item.accessible_checked(), Some(on), "{name}");
    }
    click(&menu_item(&h.app, "Blocked"));
    assert_eq!(*log.borrow(), [format!("set-level {GENERAL} | blocked")]);
    assert!(menu_items(&h.app).is_empty(), "the popover closed");
}

#[test]
fn going_back_from_a_channel_to_a_chat_brings_the_three_levels_back() {
    let h = open_channel("blocked");
    h.select(BACKEND);
    enable(&h.app, BACKEND, "blocked");
    click(&chips(&h.app)[0]);
    assert!(menu_labels(&h.app).contains(&"Can read and draft".to_string()));
}

// ---- the Team menu's bulk change: confirmation and undo ---------------------------------------

fn bulk_cards(app: &AppWindow) -> usize {
    ElementHandle::find_by_element_type_name(app, "AiBulkCard").count()
}

/// The backend asked about `names` of the team Contoso Ltd.: the confirmation is open.
fn ask_bulk(app: &AppWindow, names: &[&str]) {
    let s = app.global::<AiAccessStore>();
    s.set_bulk_team("Contoso Ltd.".into());
    let names: Vec<SharedString> = names.iter().map(|n| SharedString::from(*n)).collect();
    s.set_bulk_count(names.len() as i32);
    s.set_bulk_names(ModelRc::new(VecModel::from(names)));
    s.set_bulk_open(true);
}

#[test]
fn the_bulk_confirmation_stays_hidden_while_ai_access_is_unavailable() {
    let h = Harness::new();
    h.select(BACKEND);
    ask_bulk(&h.app, &["Roadmap"]);
    assert_eq!(bulk_cards(&h.app), 0);
    h.app.global::<AiAccessStore>().set_available(true);
    assert_eq!(bulk_cards(&h.app), 1);
}

#[test]
fn the_bulk_confirmation_names_the_channels_and_both_buttons_answer_and_close_it() {
    for (button, command) in [
        ("Let AI read", "team-channels-confirm "),
        ("Cancel", "team-channels-cancel "),
    ] {
        let h = open_chat("blocked");
        let log = wire(&h.app);
        ask_bulk(&h.app, &["Roadmap", "Launch plans"]);
        assert_eq!(bulk_cards(&h.app), 1);
        assert!(has_text(&h.app, "Let AI read 2 channels of Contoso Ltd.?"));
        assert!(has_text(&h.app, "Roadmap") && has_text(&h.app, "Launch plans"));
        click(&control(&h.app, button));
        assert_eq!(*log.borrow(), [command]);
        assert!(!h.app.global::<AiAccessStore>().get_bulk_open());
        assert_eq!(bulk_cards(&h.app), 0, "{button} closes it");
    }
}

#[test]
fn the_bulk_question_counts_one_channel() {
    let h = open_chat("blocked");
    ask_bulk(&h.app, &["Roadmap"]);
    assert!(has_text(&h.app, "Let AI read 1 channel of Contoso Ltd.?"));
}

#[test]
fn the_overview_offers_undo_last_change_only_after_a_bulk_change() {
    let h = open_chat("blocked");
    let log = wire(&h.app);
    let s = h.app.global::<AiAccessStore>();
    s.set_overview_open(true);
    let undo = |app: &AppWindow| {
        ElementHandle::find_by_accessible_label(app, "Undo last change")
            .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
            .count()
    };
    assert_eq!(undo(&h.app), 0);
    s.set_can_undo_bulk(true);
    click(&control(&h.app, "Undo last change"));
    assert_eq!(*log.borrow(), ["team-channels-undo "]);
    // The backend answers that it is used up.
    s.set_can_undo_bulk(false);
    assert_eq!(undo(&h.app), 0);
}

// ---- opening the overview and the connect dialog ----------------------------------------------

#[test]
fn the_review_link_and_the_connect_button_open_their_dialogs() {
    let h = open_chat("read");
    let log = wire(&h.app);
    let s = h.app.global::<AiAccessStore>();
    click(&chips(&h.app)[0]);
    click(&menu_item(&h.app, "Review all AI access…"));
    assert!(s.get_overview_open());
    assert_eq!(
        ElementHandle::find_by_element_type_name(&h.app, "AiOverviewCard").count(),
        1
    );
    click(&control(&h.app, "Connect an AI app…"));
    assert!(s.get_connect_open());
    assert_eq!(
        ElementHandle::find_by_element_type_name(&h.app, "AiConnectCard").count(),
        1
    );
    assert_eq!(*log.borrow(), ["overview ", "connect "]);
}

// ---- the popover -------------------------------------------------------------------------

#[test]
fn the_popover_has_three_levels_with_what_each_means_and_the_switches() {
    let h = open_chat("read");
    click(&chips(&h.app)[0]);
    assert_eq!(
        menu_labels(&h.app),
        [
            "Blocked",
            "Can read",
            "Can read and draft",
            "This account: Active",
            "AI access: Active",
            "Review all AI access…"
        ]
    );
    for (name, note) in [
        ("Blocked", "The AI cannot see this chat."),
        ("Can read", "The AI can read this chat."),
        (
            "Can read and draft",
            "The AI can read this chat and draft replies. You approve every send.",
        ),
    ] {
        assert_eq!(
            menu_item(&h.app, name).accessible_description().as_deref(),
            Some(note)
        );
    }
    // One of the three is checked: the chat's level.
    for (name, on) in [
        ("Blocked", false),
        ("Can read", true),
        ("Can read and draft", false),
    ] {
        assert_eq!(
            menu_item(&h.app, name).accessible_checked(),
            Some(on),
            "{name}"
        );
    }
}

#[test]
fn picking_a_level_sends_set_level_for_the_open_chat_and_closes() {
    let h = open_chat("blocked");
    let log = wire(&h.app);
    click(&chips(&h.app)[0]);
    click(&menu_item(&h.app, "Can read and draft"));
    assert_eq!(*log.borrow(), [format!("set-level {BACKEND} | read_draft")]);
    assert!(menu_items(&h.app).is_empty(), "the popover closed");
    click(&chips(&h.app)[0]);
    click(&menu_item(&h.app, "Blocked"));
    assert_eq!(log.borrow()[1], format!("set-level {BACKEND} | blocked"));
}

#[test]
fn the_switch_rows_send_master_and_account_pause_and_the_link_opens_the_overview() {
    let h = open_chat("read");
    let log = wire(&h.app);
    click(&chips(&h.app)[0]);
    click(&menu_item(&h.app, "AI access: Active"));
    click(&menu_item(&h.app, "This account: Active"));
    assert_eq!(*log.borrow(), ["master paused", "account-pause on"]);
    // The rows show the state they are in, and the next click undoes it.
    let s = h.app.global::<AiAccessStore>();
    s.set_master_paused(true);
    s.set_account_paused(true);
    click(&menu_item(&h.app, "AI access: Paused"));
    click(&menu_item(&h.app, "This account: Paused"));
    assert_eq!(log.borrow()[2..], ["master active", "account-pause off"]);
    click(&menu_item(&h.app, "Review all AI access…"));
    assert_eq!(log.borrow().last().map(String::as_str), Some("overview "));
    assert!(menu_items(&h.app).is_empty(), "the link closes the popover");
}

// ---- the chat list's menu ----------------------------------------------------------------

fn list_harness() -> Harness {
    let h = Harness::new();
    h.app
        .window()
        .set_size(slint::LogicalSize::new(1200., 900.));
    h.runner()(Cmd::SetFilter("all".into()));
    h
}

fn right_click_row(app: &AppWindow, title: &str) {
    let rows = find_label(app, title);
    assert!(!rows.is_empty(), "a row titled {title:?}");
    let p = rows[0].absolute_position();
    let s = rows[0].size();
    let at = LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.);
    app.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Right,
    });
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Right,
    });
}

#[test]
fn the_list_menu_offers_ai_access_for_a_group_chat_but_not_a_channel_or_when_unavailable() {
    let h = list_harness();
    right_click_row(&h.app, "Morning Standup");
    assert!(!menu_labels(&h.app).contains(&"AI access…".to_string()));
    h.app.global::<AiAccessStore>().set_available(true);
    right_click_row(&h.app, "Morning Standup");
    assert!(menu_labels(&h.app).contains(&"AI access…".to_string()));
    right_click_row(&h.app, "tech only");
    assert!(
        !menu_labels(&h.app).contains(&"AI access…".to_string()),
        "channels get it in a later slice: {:?}",
        menu_labels(&h.app)
    );
}

#[test]
fn ai_access_in_the_list_menu_opens_the_row_and_sets_its_level() {
    let h = list_harness();
    let log = wire(&h.app);
    h.app.global::<AiAccessStore>().set_available(true);
    right_click_row(&h.app, "Morning Standup");
    click(&menu_item(&h.app, "AI access…"));
    assert_eq!(*log.borrow(), [format!("open {STANDUP}")]);
    assert_eq!(
        menu_labels(&h.app),
        ["‹ AI access", "Blocked", "Can read", "Can read and draft"]
    );
    // The backend answers "open" with the row's level; the rows show it.
    enable(&h.app, STANDUP, "read");
    assert_eq!(
        menu_item(&h.app, "Can read").accessible_checked(),
        Some(true)
    );
    // The level is set for the row, not for whatever chat is open.
    click(&menu_item(&h.app, "Can read and draft"));
    assert_eq!(log.borrow()[1], format!("set-level {STANDUP} | read_draft"));
    assert!(menu_items(&h.app).is_empty(), "the menu closed");
}

#[test]
fn the_back_row_of_the_ai_view_returns_to_the_list_menu() {
    let h = list_harness();
    wire(&h.app);
    h.app.global::<AiAccessStore>().set_available(true);
    right_click_row(&h.app, "Morning Standup");
    click(&menu_item(&h.app, "AI access…"));
    click(&menu_item(&h.app, "‹ AI access"));
    assert!(menu_labels(&h.app).contains(&"Mark as unread".to_string()));
}

// ---- the overview ------------------------------------------------------------------------

#[test]
fn the_overview_lists_what_the_ai_can_see_with_a_counter_and_blocks_one_click() {
    let h = open_chat("read");
    let log = wire(&h.app);
    let s = h.app.global::<AiAccessStore>();
    set_rows(
        &h.app,
        vec![
            row(
                BACKEND,
                "Backend Team",
                "read_draft",
                "last read by AI: 10:20",
            ),
            row(STANDUP, "Morning Standup", "read", ""),
        ],
    );
    s.set_overview_open(true);
    assert_eq!(
        ElementHandle::find_by_element_type_name(&h.app, "AiOverviewCard").count(),
        1
    );
    assert!(has_text(&h.app, "2 conversations visible to AI"));
    assert!(has_text(
        &h.app,
        "Backend Team, Can read and draft, last read by AI: 10:20"
    ));
    assert!(has_text(&h.app, "Morning Standup, Can read"));

    click(&control(&h.app, "Block Morning Standup"));
    assert_eq!(*log.borrow(), [format!("block {STANDUP}")]);
    // The backend answers with the shorter list.
    set_rows(&h.app, vec![row(BACKEND, "Backend Team", "read_draft", "")]);
    assert!(has_text(&h.app, "1 conversation visible to AI"));
    click(&control(&h.app, "Block all"));
    click(&control(&h.app, "Connect an AI app…"));
    assert_eq!(log.borrow()[1..], ["block-all ", "connect "]);
}

#[test]
fn the_overview_holds_the_account_and_calendar_switches_and_closes() {
    let h = open_chat("blocked");
    let log = wire(&h.app);
    let s = h.app.global::<AiAccessStore>();
    s.set_overview_open(true);
    assert_eq!(
        control(&h.app, "Pause AI access for this account").accessible_checked(),
        Some(false)
    );
    click(&control(&h.app, "Pause AI access for this account"));
    click(&control(&h.app, "Calendar and people: Blocked"));
    assert_eq!(*log.borrow(), ["account-pause on", "calendar-people on"]);
    s.set_account_paused(true);
    s.set_calendar_people(true);
    click(&control(&h.app, "Pause AI access for this account"));
    click(&control(&h.app, "Calendar and people: Allowed"));
    assert_eq!(
        log.borrow()[2..],
        ["account-pause off", "calendar-people off"]
    );
    click(&control(&h.app, "Close"));
    assert!(!s.get_overview_open());
    assert_eq!(
        ElementHandle::find_by_element_type_name(&h.app, "AiOverviewCard").count(),
        0
    );
}

// ---- the explainer -----------------------------------------------------------------------

#[test]
fn the_explainer_says_where_the_text_goes_and_both_buttons_close_it() {
    for (button, command) in [
        ("Continue", "explainer-confirm "),
        ("Cancel", "explainer-cancel "),
    ] {
        let h = open_chat("blocked");
        let log = wire(&h.app);
        let s = h.app.global::<AiAccessStore>();
        s.set_explainer_open(true);
        for text in [
            "What the AI app reads goes to its AI provider (Anthropic for Claude).",
            "A work account's policy may forbid this.",
            "Content that is sent cannot be recalled.",
            "Turning access off stops new reads. It does not remove what the AI app already read.",
        ] {
            assert!(has_text(&h.app, text), "{text}");
        }
        click(&control(&h.app, button));
        assert_eq!(*log.borrow(), [command]);
        assert!(!s.get_explainer_open(), "{button} closes it");
    }
}

// ---- the connect dialog ------------------------------------------------------------------

#[test]
fn the_connect_dialog_shows_the_text_to_copy_and_closes() {
    let h = open_chat("blocked");
    let s = h.app.global::<AiAccessStore>();
    s.set_connect_text("claude mcp add --scope user ironteams".into());
    s.set_connect_open(true);
    let boxes: Vec<_> =
        ElementHandle::find_by_accessible_label(&h.app, "Connect commands").collect();
    assert_eq!(boxes.len(), 1, "the selectable text");
    assert_eq!(
        boxes[0].accessible_value().as_deref(),
        Some("claude mcp add --scope user ironteams")
    );
    // Copy selects the text and copies it through the window's clipboard.
    click(&control(&h.app, "Copy"));
    click(&control(&h.app, "Close"));
    assert!(!s.get_connect_open());
}

// ---- the command palette -----------------------------------------------------------------

fn ai_entries(app: &AppWindow) -> Vec<String> {
    let mut rows: Vec<_> =
        ElementHandle::find_by_element_id(app, "CommandPalette::entry").collect();
    rows.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    rows.iter()
        .map(label)
        .filter(|l| l.starts_with("AI access"))
        .collect()
}

fn chord(app: &AppWindow, mods: &[Key], text: &str) {
    for m in mods {
        key(app, *m, true);
    }
    key(app, text, true);
    key(app, text, false);
    for m in mods.iter().rev() {
        key(app, *m, false);
    }
}

/// A window with the palette open (by its chord) and `query` typed.
fn palette_with(query: &str) -> AppWindow {
    let app = app_with_messages(vec![]);
    composer(&app).mock_single_click(PointerEventButton::Left);
    chord(&app, &[Key::Control, Key::Shift], "p");
    assert!(app.global::<PaletteStore>().get_open(), "the chord opens");
    assert!(!find_label(&app, "Command palette").is_empty());
    type_text(&app, query);
    app
}

#[test]
fn the_palette_offers_the_three_ai_actions_only_when_available() {
    let app = palette_with("ai access");
    assert!(ai_entries(&app).is_empty());
    app.global::<AiAccessStore>().set_available(true);
    // Best match first: the shorter label of a shared start wins.
    assert_eq!(
        ai_entries(&app),
        [
            "AI access: pause",
            "AI access: review",
            "AI access: connect an AI app"
        ]
    );
    app.global::<AiAccessStore>().set_master_paused(true);
    assert!(ai_entries(&app).contains(&"AI access: resume".to_string()));
}

#[test]
fn palette_actions_review_pause_resume_and_connect() {
    for (query, paused, command) in [
        ("ai access: review", false, "overview "),
        ("ai access: pause", false, "master paused"),
        ("ai access: resume", true, "master active"),
        ("ai access: connect", false, "connect "),
    ] {
        let app = palette_with(query);
        let log = wire(&app);
        let s = app.global::<AiAccessStore>();
        s.set_available(true);
        s.set_master_paused(paused);
        enter(&app);
        assert_eq!(*log.borrow(), [command], "{query}");
        assert!(!app.global::<PaletteStore>().get_open(), "{query} closes");
    }
}

#[test]
fn the_palette_review_and_connect_open_their_dialogs() {
    for query in ["ai access: review", "ai access: connect"] {
        let app = palette_with(query);
        wire(&app);
        let s = app.global::<AiAccessStore>();
        s.set_available(true);
        enter(&app);
        let open = if query.ends_with("review") {
            s.get_overview_open()
        } else {
            s.get_connect_open()
        };
        assert!(open, "{query}");
    }
}

// ---- the pop-out window ------------------------------------------------------------------

fn popout() -> ChatWindow {
    init_platform();
    let cw = ChatWindow::new().unwrap();
    cw.window().set_size(LogicalSize::new(560., 680.));
    cw.show().unwrap();
    cw
}

#[test]
fn the_popout_chip_follows_its_own_properties_and_reports_the_level() {
    let cw = popout();
    assert!(chips(&cw).is_empty(), "hidden until the main window says");
    cw.set_ai_available(true);
    cw.set_ai_level("read".into());
    assert_eq!(chip_label(&cw), "AI: Can read");
    cw.set_ai_paused_text("Paused by master switch".into());
    assert_eq!(chip_label(&cw), "AI: Can read. Paused by master switch");
    let picked = Log::default();
    let p = picked.clone();
    cw.on_ai_set_level(move |l| p.borrow_mut().push(l.to_string()));
    click(&chips(&cw)[0]);
    // Only the three levels: the switches and the overview live in the main window.
    assert_eq!(
        menu_labels(&cw),
        ["Blocked", "Can read", "Can read and draft"]
    );
    click(&menu_item(&cw, "Can read and draft"));
    assert_eq!(*picked.borrow(), ["read_draft"]);
}
