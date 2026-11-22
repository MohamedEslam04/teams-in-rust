//! The app bar, the flyout, the Store and the built-in app screens (Planner and To Do, Workflows,
//! OneNote, Lists, Meeting Recaps) driven through the real window. The engine runs behind it in
//! demo mode (no network): every `command` goes through `AppsCmd::parse` and the engine, and what
//! the window shows is read back from the globals and from the rendered elements. Nothing here
//! can reach a server; a change the screens confirm is only logged (`Engine::apps_sent`).

use super::*;
use crate::backend::AppsCmd;
use crate::{AppsStore, FlowsStore, ListsStore, NotesStore, RecapsStore, TasksStore};
use i_slint_backend_testing::AccessibleRole;

struct AppsHarness {
    h: Harness,
    /// Every `name argument` the screens sent, in order.
    log: Log,
}

impl AppsHarness {
    /// A window with the app globals wired to the engine, and the app bar loaded like at start.
    fn open() -> Self {
        let h = Harness::new();
        h.app.window().set_size(LogicalSize::new(1400., 900.));
        let log = Log::default();
        macro_rules! wire {
            ($global:ident) => {{
                let (engine, rt, log) = (h.engine.clone(), h.rt.clone(), log.clone());
                h.app.global::<$global>().on_command(move |name, arg| {
                    log.borrow_mut()
                        .push(format!("{name} {arg}").trim().to_owned());
                    if let Some(c) = AppsCmd::parse(&name, &arg) {
                        rt.block_on(engine.clone().handle(Cmd::Apps(c)));
                    }
                });
            }};
        }
        wire!(AppsStore);
        wire!(TasksStore);
        wire!(FlowsStore);
        wire!(NotesStore);
        wire!(ListsStore);
        wire!(RecapsStore);
        let a = AppsHarness { h, log };
        a.run(AppsCmd::Load);
        a
    }

    fn run(&self, cmd: AppsCmd) {
        self.h
            .rt
            .block_on(self.h.engine.clone().handle(Cmd::Apps(cmd)));
    }

    fn send(&self, name: &str, arg: &str) {
        self.run(AppsCmd::parse(name, arg).unwrap_or_else(|| panic!("no command {name}")));
    }

    fn store(&self) -> AppsStore<'_> {
        self.h.app.global::<AppsStore>()
    }

    fn view(&self) -> String {
        self.h.app.get_view().to_string()
    }

    fn find(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.h.app, label).collect()
    }

    fn has_label(&self, label: &str) -> bool {
        !self.find(label).is_empty()
    }

    /// Clicks the first button with this label (a heading or text can carry the same label).
    fn click(&self, label: &str) {
        self.click_with(label, PointerEventButton::Left);
    }

    fn click_with(&self, label: &str, button: PointerEventButton) {
        self.find(label)
            .into_iter()
            .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
            .unwrap_or_else(|| panic!("no button labelled {label:?}"))
            .mock_single_click(button);
    }

    /// Clicks the last button with this label: a dialog's buttons come after the page's own
    /// button of the same name (Add, Remove, Update).
    fn click_last(&self, label: &str) {
        self.find(label)
            .into_iter()
            .rfind(|e| e.accessible_role() == Some(AccessibleRole::Button))
            .unwrap_or_else(|| panic!("no button labelled {label:?}"))
            .mock_single_click(PointerEventButton::Left);
    }

    /// The changes the screens sent (offline: would have sent), oldest first.
    fn sent(&self) -> Vec<String> {
        self.h.engine.apps_sent()
    }

    /// Clicks the first element of a component type with this label.
    fn click_in(&self, label: &str, component: &str) {
        self.find(label)
            .into_iter()
            .find(|e| e.type_name().as_deref() == Some(component))
            .unwrap_or_else(|| panic!("no {component} labelled {label:?}"))
            .mock_single_click(PointerEventButton::Left);
    }

    /// Clicks whatever carries the label (a text field, a check box).
    fn click_any(&self, label: &str) {
        self.find(label)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("nothing labelled {label:?}"))
            .mock_single_click(PointerEventButton::Left);
    }

    /// A menu row (a rail entry or a page can carry the same label).
    fn click_menu(&self, label: &str) {
        self.find(label)
            .into_iter()
            .find(|e| e.type_name().as_deref() == Some("MenuItem"))
            .unwrap_or_else(|| panic!("no menu row {label:?}"))
            .mock_single_click(PointerEventButton::Left);
    }

    /// The names on the rail after the fixed entries.
    fn rail(&self) -> Vec<String> {
        self.store()
            .get_rail()
            .iter()
            .map(|r| r.name.to_string())
            .collect()
    }

    fn card_names(cards: ModelRc<crate::AppCard>) -> Vec<String> {
        cards.iter().map(|c| c.name.to_string()).collect()
    }

    fn banner(&self) -> String {
        self.h.app.get_banner().to_string()
    }
}

#[test]
fn an_app_view_loads_itself_when_it_appears() {
    // Like `IRONTEAMS_VIEW=store`: the view is switched without a rail click.
    let a = AppsHarness::open();
    a.h.app.set_view("store".into());
    assert!(a.has_label("Search the Store"), "the view is on screen");
    assert_eq!(a.store().get_popular().row_count(), 4);
    a.h.app.set_view("planner".into());
    assert!(a.has_label("Refresh tasks"));
    assert_eq!(a.h.app.global::<TasksStore>().get_title(), "Tasks");
    // Appearing again does not load it twice.
    let loads = |a: &AppsHarness| {
        a.log
            .borrow()
            .iter()
            .filter(|l| l.starts_with("tasks-enter"))
            .count()
    };
    a.h.app.set_view("chat".into());
    assert!(!a.has_label("Refresh tasks"), "the screen is gone");
    a.h.app.set_view("planner".into());
    assert!(a.has_label("Refresh tasks"));
    assert_eq!(loads(&a), 2, "asked twice, answered from what was loaded");
    assert_eq!(a.h.app.global::<TasksStore>().get_tasks().row_count(), 3);
}

// -- S1.1 the app bar ----------------------------------------------------------------------

#[test]
fn the_rail_lists_the_pinned_apps_after_the_fixed_entries_with_store_and_more() {
    let a = AppsHarness::open();
    // Activity, Chat, Teams, Calendar and OneDrive are IronTeams's own screens; the rest of the
    // user's bar follows in the bar's order.
    assert_eq!(a.rail(), ["Calling", "Planner", "Workflows"]);
    for label in [
        "Activity",
        "Calling",
        "Planner",
        "Workflows",
        "Store",
        "More apps",
    ] {
        assert!(a.has_label(label), "rail entry {label}");
    }
}

#[test]
fn right_click_unpins_and_sends_the_new_bar_to_teams() {
    let a = AppsHarness::open();
    a.click_with("Planner", PointerEventButton::Right);
    a.click_menu("Unpin");
    assert_eq!(a.rail(), ["Calling", "Workflows"]);
    assert_eq!(a.banner(), "Unpinned Planner.");
    assert_eq!(
        a.sent(),
        ["POST https://teams.example.invalid/api/mt/demo/beta/users/apps/pinned"]
    );
    assert_eq!(
        *a.log.borrow(),
        vec!["unpin com.microsoft.teamspace.tab.planner"]
    );
}

#[test]
fn an_app_the_admin_pinned_cannot_be_unpinned() {
    let a = AppsHarness::open();
    a.click_with("Calling", PointerEventButton::Right);
    assert!(
        a.find("Unpin").is_empty(),
        "no Unpin for an admin pinned app"
    );
    a.click_menu("Open");
    assert_eq!(a.view(), "app");
    a.send("unpin", "demo-calling");
    assert!(
        a.banner().contains("pinned by your admin"),
        "{}",
        a.banner()
    );
    assert_eq!(a.rail(), ["Calling", "Planner", "Workflows"]);
}

#[test]
fn move_down_and_move_up_reorder_the_pinned_apps() {
    let a = AppsHarness::open();
    a.click_with("Workflows", PointerEventButton::Right);
    a.click_menu("Move up");
    assert_eq!(a.rail(), ["Calling", "Workflows", "Planner"]);
    a.click_with("Workflows", PointerEventButton::Right);
    a.click_menu("Move down");
    assert_eq!(a.rail(), ["Calling", "Planner", "Workflows"]);
}

#[test]
fn dragging_a_rail_entry_reorders_it() {
    let a = AppsHarness::open();
    let planner = a.find("Planner").into_iter().next().expect("Planner entry");
    let (pos, size) = (planner.absolute_position(), planner.size());
    let start = LogicalPosition::new(pos.x + size.width / 2., pos.y + size.height / 2.);
    let window = a.h.app.window();
    window.dispatch_event(WindowEvent::PointerMoved { position: start });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    // One slot (an entry and its gap) down.
    let end = LogicalPosition::new(start.x, start.y + 60.);
    window.dispatch_event(WindowEvent::PointerMoved { position: end });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: end,
        button: PointerEventButton::Left,
    });
    assert_eq!(a.rail(), ["Calling", "Workflows", "Planner"]);
    assert!(
        a.log.borrow().iter().any(|l| l.starts_with("move ")),
        "{:?}",
        a.log.borrow()
    );
    assert_ne!(a.view(), "planner", "a drag is not a click");
}

#[test]
fn ctrl_shift_digits_open_the_nth_entry_and_the_bar_collapses() {
    let a = AppsHarness::open();
    a.h.select(NOTES);
    a.h.type_in_composer("");
    // 1 to 5 are the fixed entries, 6 is the first pinned app.
    a.send("shortcut", "1");
    assert_eq!(a.view(), "activity");
    a.send("shortcut", "7");
    assert_eq!(a.view(), "planner");
    a.send("shortcut", "9");
    assert_eq!(a.view(), "planner", "no ninth entry: nothing changes");

    // The key chord itself: Shift turns the 6 into ^ on a US layout.
    let w = a.h.app.window();
    w.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    w.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    w.dispatch_event(WindowEvent::KeyPressed { text: "^".into() });
    assert_eq!(a.view(), "app", "Ctrl+Shift+6 opens Calling");
    w.dispatch_event(WindowEvent::KeyReleased { text: "^".into() });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    w.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });

    assert_eq!(a.store().get_rail_width(), 68.);
    a.send("rail-collapse", "");
    assert_eq!(a.store().get_rail_width(), 52.);
    a.send("rail-collapse", "");
    assert_eq!(a.store().get_rail_width(), 68.);
}

// -- S1.2 the flyout ------------------------------------------------------------------------

#[test]
fn more_opens_a_searchable_list_of_installed_apps_that_are_not_pinned() {
    let a = AppsHarness::open();
    a.click("More apps");
    assert!(a.store().get_flyout_open());
    // Installed, not pinned, with a personal tab: not Planner (pinned) or Contoso Tickets (a
    // message extension only).
    let names = AppsHarness::card_names(a.store().get_flyout_apps());
    assert_eq!(names, ["Lists", "Meeting Recaps", "OneNote", "Shifts"]);

    type_text(&a.h.app, "note");
    assert_eq!(a.store().get_flyout_query(), "note");
    assert_eq!(
        AppsHarness::card_names(a.store().get_flyout_apps()),
        ["OneNote"]
    );

    type_text(&a.h.app, "zzz");
    assert!(a.store().get_flyout_apps().row_count() == 0);
    assert!(a.has_label("Search apps"));
}

#[test]
fn opening_an_app_from_the_flyout_shows_it_in_the_main_area() {
    let a = AppsHarness::open();
    a.click("More apps");
    a.click("OneNote");
    assert!(!a.store().get_flyout_open(), "the flyout closes");
    assert_eq!(a.view(), "onenote");
    // Shifts has no screen of its own: its page opens in Teams on the web.
    a.click("More apps");
    a.click("Shifts");
    assert_eq!(a.view(), "app");
    assert_eq!(a.store().get_host_name(), "Shifts");
    a.click("Open in Teams on the web");
    let opened = a.h.engine.apps_opened();
    assert!(
        opened
            .iter()
            .any(|u| u.starts_with("https://teams.microsoft.com/l/entity/demo-shifts/")),
        "{opened:?}"
    );
}

#[test]
fn an_app_opened_from_the_flyout_shows_on_the_rail_while_it_is_open() {
    let a = AppsHarness::open();
    a.click("More apps");
    a.click("Shifts");
    assert_eq!(a.view(), "app");
    assert!(a.store().get_has_temp_app());
    assert_eq!(a.store().get_temp_app().name, "Shifts");
    let on_rail = |a: &AppsHarness| {
        a.find("Shifts")
            .iter()
            .any(|e| e.accessible_description().is_some_and(|d| d == "Current"))
    };
    assert!(on_rail(&a), "the rail shows the open app");
    // Another page: it leaves the rail.
    a.h.app.set_view("chat".into());
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(!on_rail(&a));
}

#[test]
fn pin_from_the_flyout_adds_the_app_to_the_rail() {
    let a = AppsHarness::open();
    a.click("More apps");
    a.click("Pin Shifts");
    assert_eq!(a.rail(), ["Calling", "Planner", "Workflows", "Shifts"]);
    assert_eq!(a.banner(), "Pinned Shifts.");
    assert_eq!(a.sent().len(), 1, "the new bar went to Teams");
    // It is on the bar now, so it leaves the flyout.
    a.send("flyout-open", "");
    assert!(!AppsHarness::card_names(a.store().get_flyout_apps()).contains(&"Shifts".to_owned()));
}

// -- S2.1 to S2.4 the Store -------------------------------------------------------------------

#[test]
fn the_store_home_shows_popular_apps_with_their_facts() {
    let a = AppsHarness::open();
    a.click("Store");
    assert_eq!(a.view(), "store");
    assert_eq!(a.store().get_store_page(), "home");
    let popular = a.store().get_popular();
    let names: Vec<String> = popular.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(
        names,
        ["Planner", "OneNote", "Contoso Polls", "Fabrikam Agent"]
    );
    let planner = popular.row_data(0).unwrap();
    assert!(planner.meta.contains("★ 4.5"), "{}", planner.meta);
    assert!(
        planner.installed && planner.update,
        "installed one version behind"
    );
    assert_eq!(
        a.store().get_org_apps().row_count(),
        1,
        "Built for your org"
    );
    assert!(a.has_label("Search the Store"));
    // Devices links out; the gear and Manage open their pages.
    a.click("Devices");
    assert!(
        a.h.engine
            .apps_opened()
            .iter()
            .any(|u| u.contains("microsoft.com"))
    );
}

#[test]
fn search_filters_by_works_in_and_by_apps_or_agents() {
    let a = AppsHarness::open();
    a.send("store-search", "contoso");
    assert_eq!(a.store().get_store_page(), "results");
    assert_eq!(
        AppsHarness::card_names(a.store().get_cards()),
        ["Contoso Tickets", "Contoso Polls"]
    );
    a.send("store-scope", "team");
    assert_eq!(
        AppsHarness::card_names(a.store().get_cards()),
        ["Contoso Polls"]
    );
    a.send("store-scope", "all");

    a.send("store-search", "agent");
    assert_eq!(
        AppsHarness::card_names(a.store().get_cards()),
        ["Fabrikam Agent"]
    );
    a.send("store-kind", "apps");
    assert!(
        a.store().get_cards().row_count() == 0,
        "an agent is not an app"
    );
    assert_eq!(a.store().get_store_status(), "No apps match.");
    a.send("store-kind", "agents");
    assert_eq!(a.store().get_cards().row_count(), 1);

    // The search box sends the query on Enter.
    a.send("store-kind", "all");
    a.send("store-home", "");
    a.click_any("Search the Store");
    type_text(&a.h.app, "polls");
    enter(&a.h.app);
    assert_eq!(a.store().get_store_page(), "results");
    assert_eq!(
        AppsHarness::card_names(a.store().get_cards()),
        ["Contoso Polls"]
    );
    assert!(a.log.borrow().iter().any(|l| l == "store-search polls"));
}

#[test]
fn categories_and_featured_open_a_listing() {
    let a = AppsHarness::open();
    a.send("store-open", "");
    let cats: Vec<String> = a
        .store()
        .get_categories()
        .iter()
        .map(|n| n.label.to_string())
        .collect();
    assert!(cats.contains(&"Productivity".to_owned()), "{cats:?}");
    assert!(!cats.contains(&"All".to_owned()));
    a.send("store-category", "productivity");
    assert_eq!(a.store().get_store_page(), "category");
    assert_eq!(a.store().get_store_title(), "Productivity");
    assert_eq!(
        AppsHarness::card_names(a.store().get_cards()),
        ["Planner", "OneNote", "Lists", "Shifts"]
    );
    a.send("store-category", "org");
    assert_eq!(
        AppsHarness::card_names(a.store().get_cards()),
        ["Contoso Intranet"]
    );
    a.send("store-category", "agents");
    assert_eq!(a.store().get_kind_filter(), "agents");
    assert_eq!(
        AppsHarness::card_names(a.store().get_cards()),
        ["Fabrikam Agent"]
    );
}

#[test]
fn an_app_page_shows_the_listing_facts() {
    let a = AppsHarness::open();
    a.send("store-open", "");
    a.click_in("Planner", "AppCardView");
    let s = a.store();
    assert_eq!(s.get_store_page(), "app");
    assert_eq!(s.get_app_name(), "Planner");
    assert_eq!(s.get_app_developer(), "Microsoft Corporation");
    assert_eq!(s.get_app_rating(), "★ 4.5 (119 ratings)");
    assert_eq!(s.get_app_works_in(), "Personal, Team");
    assert!(s.get_app_installed() && s.get_app_update() && s.get_app_pinned());
    assert_eq!(s.get_app_rsc().row_count(), 1);
    assert!(!s.get_app_privacy().is_empty() && !s.get_app_terms().is_empty());
    for label in [
        "Add to another place",
        "Update",
        "Remove",
        "Share",
        "Privacy policy",
        "Terms of use",
    ] {
        assert!(a.has_label(label), "{label}");
    }

    a.click("Share");
    assert!(a.h.engine.apps_opened().iter().any(
        |u| u == "copied:https://teams.microsoft.com/l/app/com.microsoft.teamspace.tab.planner"
    ));
    a.click("Privacy policy");
    assert!(
        a.h.engine
            .apps_opened()
            .iter()
            .any(|u| u.ends_with("/privacy"))
    );
}

#[test]
fn add_asks_first_and_then_sends_the_install() {
    let a = AppsHarness::open();
    a.send("store-search", "polls");
    a.click("Contoso Polls");
    assert_eq!(a.store().get_app_name(), "Contoso Polls");
    assert!(!a.store().get_app_installed());
    let targets: Vec<String> = a
        .store()
        .get_app_targets()
        .iter()
        .map(|n| n.label.to_string())
        .collect();
    assert_eq!(
        targets,
        ["Add to a team", "Add to a chat"],
        "its works-in decides the menu"
    );

    a.click("Add");
    a.click_menu("Add to a team");
    let s = a.store();
    assert_eq!(s.get_form_title(), "Add Contoso Polls to a team");
    let teams: Vec<String> = s
        .get_form_choices()
        .iter()
        .map(|c| c.title.to_string())
        .collect();
    assert!(!teams.is_empty(), "the teams to pick from");
    assert!(a.sent().is_empty(), "nothing is sent before Confirm");

    // Confirming without a pick keeps the dialog open and says why.
    a.click_last("Add");
    assert_eq!(s.get_form_error(), "Pick one first.");
    assert_eq!(s.get_form_title(), "Add Contoso Polls to a team");
    assert!(a.sent().is_empty());

    // The link button opens the app in Teams on the web and leaves the dialog open.
    a.click("Open in Teams on the web");
    assert!(
        a.h.engine
            .apps_opened()
            .iter()
            .any(|u| u.ends_with("/l/app/demo-polls"))
    );
    assert_eq!(s.get_form_title(), "Add Contoso Polls to a team");

    a.click_last(&teams[0]);
    assert!(s.get_form_choices().row_data(0).unwrap().selected);
    a.click_last("Add");
    let sent = a.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].starts_with("POST ")
            && sent[0].contains("/beta/teams/")
            && sent[0].ends_with("/apps/entitlements"),
        "{sent:?}"
    );
    assert_eq!(s.get_form_title(), "", "the dialog closes");
    assert_eq!(a.banner(), "Added Contoso Polls to the team.");

    // Add for me asks once and posts to the user's own scope.
    a.send("app-page", "demo-agent");
    a.send("app-add", "me");
    assert_eq!(s.get_form_title(), "Add Fabrikam Agent for you");
    a.click("Cancel");
    assert_eq!(s.get_form_title(), "");
    assert_eq!(a.sent().len(), 1, "Cancel sends nothing");
    a.send("app-add", "me");
    a.click_last("Add");
    let sent = a.sent();
    assert!(
        sent[1].ends_with("/beta/users/apps/entitlements"),
        "{sent:?}"
    );
    assert_eq!(a.banner(), "Added Fabrikam Agent.");
}

#[test]
fn manage_your_apps_lists_installed_apps_with_updates_and_removes_after_a_confirmation() {
    let a = AppsHarness::open();
    a.send("store-open", "");
    a.click("Manage your apps");
    assert_eq!(a.store().get_store_page(), "manage");
    let manage = a.store().get_manage();
    let planner = manage
        .iter()
        .find(|c| c.name == "Planner")
        .expect("Planner is installed");
    assert!(planner.update, "1.8.0 installed, 1.9.0 available");
    assert!(manage.iter().any(|c| c.name == "OneNote" && !c.update));

    a.send("app-remove", "com.microsoft.teamspace.tab.planner");
    let s = a.store();
    assert_eq!(s.get_form_title(), "Remove Planner?");
    assert!(s.get_form_danger());
    assert!(a.sent().is_empty());
    a.click_last("Remove");
    assert_eq!(
        a.sent(),
        [
            "DELETE https://teams.example.invalid/api/mt/demo/beta/users/apps/entitlements/com.microsoft.teamspace.tab.planner"
        ]
    );
    assert_eq!(a.banner(), "Removed Planner.");
    assert_eq!(s.get_form_title(), "");

    a.send("app-update", "com.microsoft.teamspace.tab.planner");
    assert_eq!(s.get_form_title(), "Update Planner?");
    a.click_last("Update");
    let sent = a.sent();
    assert!(
        sent[1].starts_with("POST ")
            && sent[1].ends_with("/apps/Upgrade/com.microsoft.teamspace.tab.planner?version=1.9.0"),
        "{sent:?}"
    );
    assert_eq!(a.banner(), "Updated Planner to version 1.9.0.");

    // A built-in app cannot be removed: no dialog, a message.
    a.send("app-remove", "demo-calling");
    assert_eq!(s.get_form_title(), "");
    assert_eq!(a.banner(), "Calling is built in and cannot be removed.");
    assert_eq!(a.sent().len(), 2);
}

#[test]
fn store_settings_show_the_policy_and_per_app_permissions() {
    let a = AppsHarness::open();
    a.send("store-open", "");
    a.send("store-settings", "");
    let s = a.store();
    assert_eq!(s.get_store_page(), "settings");
    assert_eq!(
        s.get_policy_text(),
        "Custom app upload: not allowed. Pinning apps to the bar: allowed."
    );
    let rows: Vec<(String, String, bool)> = s
        .get_perms()
        .iter()
        .map(|p| (p.app_name.to_string(), p.label.to_string(), p.allowed))
        .collect();
    assert_eq!(
        rows,
        [
            ("OneNote".into(), "Camera and microphone".into(), false),
            ("Shifts".into(), "Location".into(), false),
            ("Shifts".into(), "Notifications".into(), false),
        ],
        "everything starts denied"
    );
    a.click_any("Shifts: Location");
    let after: Vec<bool> = a.store().get_perms().iter().map(|p| p.allowed).collect();
    assert_eq!(after, [false, true, false]);
    assert_eq!(
        a.log.borrow().last().unwrap(),
        "grant demo-shifts|geolocation|on"
    );
    a.click_any("Shifts: Location");
    assert!(!a.store().get_perms().row_data(1).unwrap().allowed);
}

// -- S3 the built-in apps ---------------------------------------------------------------------

#[test]
fn planner_opens_to_do_with_a_board_and_plans() {
    let a = AppsHarness::open();
    a.click("Planner");
    assert_eq!(a.view(), "planner");
    let t = a.h.app.global::<TasksStore>();
    let nav: Vec<String> = t.get_nav().iter().map(|r| r.title.to_string()).collect();
    assert_eq!(
        nav,
        [
            "Planner",
            "My tasks",
            "Sprint board",
            "Launch plan",
            "To Do",
            "Tasks",
            "Groceries",
            "Flagged email"
        ]
    );
    // The default To Do list opens first.
    assert_eq!(t.get_title(), "Tasks");
    let tasks: Vec<String> = t.get_tasks().iter().map(|r| r.title.to_string()).collect();
    assert_eq!(
        tasks,
        [
            "Send the quarterly report",
            "Book the team offsite",
            "Review the design doc"
        ]
    );
    let badges: Vec<String> = t.get_tasks().iter().map(|r| r.badge.to_string()).collect();
    assert_eq!(badges, ["Not started", "In progress", "Completed"]);
    assert!(t.get_summary().starts_with("3 tasks"));

    a.click("Sprint board");
    assert_eq!(t.get_title(), "Sprint board");
    a.click("Board");
    assert_eq!(t.get_mode(), "board");
    let cols: Vec<String> = t
        .get_columns()
        .iter()
        .map(|c| c.title.to_string())
        .collect();
    assert_eq!(cols, ["To do", "Doing", "Done"]);
    a.click("List");
    assert_eq!(t.get_mode(), "list");

    a.click("My tasks");
    assert_eq!(t.get_tasks().row_count(), 4);
    // An empty list says so.
    a.click("Flagged email");
    assert_eq!(t.get_tasks().row_count(), 0);
    assert_eq!(t.get_status(), "No tasks here.");
}

#[test]
fn workflows_list_flows_with_their_run_history_and_run_or_switch_them_after_a_confirmation() {
    let a = AppsHarness::open();
    a.click("Workflows");
    assert_eq!(a.view(), "workflows");
    let f = a.h.app.global::<FlowsStore>();
    let flows: Vec<(String, String)> = f
        .get_flows()
        .iter()
        .map(|r| (r.title.to_string(), r.badge.to_string()))
        .collect();
    assert_eq!(flows.len(), 4);
    assert_eq!(flows[0].1, "On");
    assert_eq!(flows[2].1, "Off");
    // The first flow is selected and its runs shown, newest first.
    let runs: Vec<(String, String)> = f
        .get_runs()
        .iter()
        .map(|r| (r.badge.to_string(), r.tone.to_string()))
        .collect();
    assert_eq!(
        runs,
        [
            ("Succeeded".into(), "ok".into()),
            ("Failed".into(), "bad".into()),
            ("Succeeded".into(), "ok".into())
        ]
    );
    assert_eq!(f.get_environment(), "Demo (default)");
    // Its trigger is not a button, so it cannot be run by hand; it can be turned off.
    assert!(!f.get_can_run());
    assert!(f.get_can_toggle() && f.get_selected_on());
    // A flow that never ran says so.
    a.click("Copy a file to OneDrive");
    assert_eq!(f.get_runs().row_count(), 0);
    assert_eq!(f.get_status(), "This flow has not run yet.");
    assert!(!f.get_can_run(), "a flow that is off is not run");

    // Turn it on: confirm first, then the request.
    a.click("Turn on");
    assert_eq!(
        a.store().get_form_title(),
        "Turn on Copy a file to OneDrive?"
    );
    assert!(a.sent().is_empty());
    a.click_last("Turn on");
    assert_eq!(
        a.sent(),
        [
            "POST https://api.flow.microsoft.com/providers/Microsoft.ProcessSimple/environments/Default-demo/flows/flow-3/start?api-version=2016-11-01"
        ]
    );
    assert_eq!(a.banner(), "Turned on Copy a file to OneDrive.");

    // A manual flow that is on can be run, after asking.
    a.click("Send my weekly status");
    assert!(f.get_can_run());
    a.click("Run");
    assert_eq!(a.store().get_form_title(), "Run Send my weekly status?");
    a.click("Cancel");
    assert_eq!(a.sent().len(), 1, "Cancel sends nothing");
    a.click("Run");
    a.click_last("Run");
    let sent = a.sent();
    assert!(
        sent[1].starts_with("POST https://api.flow.microsoft.com/")
            && sent[1].contains("/flows/flow-4/triggers/manual/run?api-version="),
        "{sent:?}"
    );
    assert_eq!(a.banner(), "Started Send my weekly status.");
    a.click("Turn off");
    a.click_last("Turn off");
    assert!(
        a.sent()[2].contains("/flows/flow-4/stop?"),
        "{:?}",
        a.sent()
    );
    assert_eq!(a.banner(), "Turned off Send my weekly status.");
    assert!(
        a.log
            .borrow()
            .iter()
            .all(|l| l.starts_with("open-app") || l.starts_with("flow") || l.starts_with("form-")),
        "{:?}",
        a.log.borrow()
    );
}

#[test]
fn tasks_can_be_added_completed_edited_and_deleted() {
    let a = AppsHarness::open();
    a.click("Planner");
    let t = a.h.app.global::<TasksStore>();
    let s = a.store();
    assert_eq!(t.get_title(), "Tasks");
    assert!(t.get_can_add());

    // New task: a title is needed and the due date has to be a date.
    a.click("New task");
    assert_eq!(s.get_form_title(), "New task in Tasks");
    a.click_last("Add");
    assert_eq!(s.get_form_error(), "Give the task a title.");
    s.set_form_text1("Plan the offsite".into());
    s.set_form_text2("tomorrow".into());
    a.click_last("Add");
    assert_eq!(
        s.get_form_error(),
        "The due date must look like 2026-11-25."
    );
    assert!(a.sent().is_empty());
    s.set_form_text2("2026-11-25".into());
    a.click_last("Add");
    let sent = a.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].starts_with("POST https://graph.microsoft.com/v1.0/me/todo/lists/todo-1/tasks ")
            && sent[0].contains("dueDateTime")
            && !sent[0].contains("Plan the offsite"),
        "{sent:?}"
    );
    assert_eq!(a.banner(), "Task added.");
    assert_eq!(s.get_form_title(), "");

    // Complete, then undo.
    a.click_any("Complete Send the quarterly report");
    assert!(
        a.sent()[1].starts_with("PATCH ") && a.sent()[1].contains("/todo/lists/todo-1/tasks/t1 "),
        "{:?}",
        a.sent()
    );
    assert_eq!(a.banner(), "Task completed.");
    a.click_any("Mark Review the design doc as not completed");
    assert!(a.sent()[2].contains("/tasks/t3 "), "{:?}", a.sent());
    assert_eq!(a.banner(), "Marked as not completed.");

    // Edit starts from what the task has.
    a.click("Edit Book the team offsite");
    assert_eq!(s.get_form_title(), "Edit task");
    assert_eq!(s.get_form_text1(), "Book the team offsite");
    assert_eq!(s.get_form_text2(), "");
    s.set_form_text1("Book the offsite".into());
    a.click_last("Save");
    assert!(
        a.sent()[3].starts_with("PATCH ") && a.sent()[3].contains("/tasks/t2 "),
        "{:?}",
        a.sent()
    );
    assert_eq!(a.banner(), "Task saved.");

    // Delete asks first.
    a.click("Groceries");
    a.click("Delete Coffee beans");
    assert_eq!(s.get_form_title(), "Delete this task?");
    assert_eq!(a.sent().len(), 4);
    a.click_last("Delete");
    assert!(
        a.sent()[4].starts_with("DELETE ") && a.sent()[4].ends_with("/todo/lists/todo-2/tasks/g1"),
        "{:?}",
        a.sent()
    );
    assert_eq!(a.banner(), "Task deleted.");

    // A plan: the task goes to the plan, and Planner edits carry the task's etag.
    a.click("Sprint board");
    a.click("New task");
    assert_eq!(s.get_form_title(), "New task in Sprint board");
    s.set_form_text1("Write the spec".into());
    a.click_last("Add");
    assert!(
        a.sent()[5].starts_with("POST https://graph.microsoft.com/v1.0/planner/tasks "),
        "{:?}",
        a.sent()
    );
    a.click_any("Complete Draft the release notes");
    assert!(
        a.sent()[6].starts_with("PATCH ")
            && a.sent()[6].ends_with("/planner/tasks/p1 (json: percentComplete)"),
        "{:?}",
        a.sent()
    );

    // My tasks has no list to add to.
    a.click("My tasks");
    assert!(!t.get_can_add());
}

#[test]
fn onenote_adds_a_page_to_the_open_section() {
    let a = AppsHarness::open();
    a.send("open-app", "demo-onenote");
    let s = a.store();
    a.click("New page");
    assert_eq!(s.get_form_title(), "New page in Meeting notes");
    a.click_last("Add");
    assert_eq!(s.get_form_error(), "Give the page a title.");
    s.set_form_text1("Planning".into());
    s.set_form_text2("First item".into());
    a.click_last("Add");
    let sent = a.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].starts_with("POST https://graph.microsoft.com/v1.0/demo/sec-1/pages (html, "),
        "{sent:?}"
    );
    assert_eq!(a.banner(), "Page \"Planning\" added.");
    assert_eq!(s.get_form_title(), "");
}

#[test]
fn lists_add_and_edit_items_with_typed_columns() {
    let a = AppsHarness::open();
    a.send("open-app", "demo-lists");
    let l = a.h.app.global::<ListsStore>();
    let s = a.store();
    assert!(l.get_can_edit());
    a.click("New item");
    assert_eq!(s.get_form_title(), "New item in Issue tracker");
    assert_eq!(s.get_form_label1(), "Title");
    assert_eq!(s.get_form_label2(), "Status");
    assert_eq!(s.get_form_label3(), "Owner");
    a.click_last("Add");
    assert_eq!(s.get_form_error(), "Give the item a title.");
    s.set_form_text1("Crash on save".into());
    s.set_form_text2("Open".into());
    a.click_last("Add");
    let sent = a.sent();
    assert!(
        sent.len() == 1
            && sent[0].starts_with(
                "POST https://graph.microsoft.com/v1.0/sites/site-1/lists/list-1/items (json: fields)"
            ),
        "{sent:?}"
    );
    assert_eq!(a.banner(), "Item added.");

    // Edit starts from the row's values.
    a.click("Edit Login fails on mobile");
    assert_eq!(s.get_form_title(), "Edit item in Issue tracker");
    assert_eq!(s.get_form_text1(), "Login fails on mobile");
    assert_eq!(s.get_form_text2(), "Open");
    assert_eq!(s.get_form_text3(), "Ada");
    s.set_form_text2("Closed".into());
    a.click_last("Save");
    let sent = a.sent();
    assert!(
        sent[1].starts_with(
            "PATCH https://graph.microsoft.com/v1.0/sites/site-1/lists/list-1/items/1/fields "
        ),
        "{sent:?}"
    );
    assert_eq!(a.banner(), "Item saved.");

    // A document library is not edited here.
    a.click("Documents");
    assert!(!l.get_can_edit());
}

/// The Meeting Recaps app opened from the More apps flyout.
fn recaps_open() -> AppsHarness {
    let a = AppsHarness::open();
    a.click("More apps");
    a.click("Meeting Recaps");
    assert_eq!(a.view(), "recaps");
    a
}

/// The home's lines as `(heading?, title)`.
fn recap_lines(r: &RecapsStore<'_>) -> Vec<(bool, String)> {
    r.get_meetings()
        .iter()
        .map(|m| (m.header, m.title.to_string()))
        .collect()
}

#[test]
fn meeting_recaps_home_lists_the_calendars_online_meetings_with_recap_buttons() {
    let a = recaps_open();
    let r = a.h.app.global::<RecapsStore>();
    assert_eq!(r.get_page(), "home");
    assert!(a.has_label("Search for meetings"));
    assert!(a.has_label("All"), "only All: Followed has no source");
    assert!(!a.has_label("Followed"));
    // From the calendar: online meetings of the last 30 days, the canceled one marked, the
    // in-person, older and coming meetings left out. Nothing here asked a meeting service.
    let titles: Vec<String> = recap_lines(&r)
        .into_iter()
        .filter(|(header, _)| !header)
        .map(|(_, t)| t)
        .collect();
    assert_eq!(
        titles,
        [
            "Weekly sync",
            "Canceled: Budget review",
            "Design review",
            "Retro"
        ]
    );
    assert!(r.get_meetings().iter().any(|m| m.header), "week headings");
    for t in &titles {
        assert!(
            a.has_label(&format!("Recap: {t}")),
            "a Recap button for {t}"
        );
    }
    assert_eq!(r.get_footer(), "Loaded all meetings in the last 30 days.");
    assert!(a.has_label("Loaded all meetings in the last 30 days."));
    assert_eq!(r.get_note(), "");

    // Search by title or organizer.
    a.send("recaps-query", "sam");
    let found: Vec<String> = recap_lines(&r)
        .into_iter()
        .filter(|(h, _)| !h)
        .map(|(_, t)| t)
        .collect();
    assert_eq!(found, ["Design review"]);
    a.send("recaps-query", "no such meeting");
    assert_eq!(r.get_note(), "No meeting matches your search.");
    a.send("recaps-query", "");
    assert_eq!(r.get_note(), "");
}

#[test]
fn a_meeting_recap_shows_its_parts_shared_files_notes_and_transcript() {
    let a = recaps_open();
    let r = a.h.app.global::<RecapsStore>();
    a.click("Recap: Weekly sync");
    assert_eq!(r.get_page(), "detail");
    assert_eq!(r.get_detail_title(), "Weekly sync");
    assert!(!r.get_in_chat());
    assert!(a.has_label("Back to Meeting Recaps"));
    // Two recordings in the meeting chat: two parts; the agenda sent there is a shared file.
    let parts: Vec<String> = r.get_parts().iter().map(|p| p.label.to_string()).collect();
    assert_eq!(parts, ["Part 1", "Part 2"]);
    assert_eq!(r.get_part(), 0);
    assert!(r.get_has_recording());
    assert_eq!(r.get_part_label(), "Recording · 18 min");
    assert!(a.has_label("Agenda.docx"));
    assert_eq!(r.get_files_note(), "");
    assert_eq!(r.get_detail_note(), "", "no error line");
    // Notes: the meeting chat's Notes tab opens in the browser.
    assert_eq!(r.get_tab(), "notes");
    assert!(r.get_has_notes());
    a.click_last("Open meeting notes");

    // Watch and the play card play the part's recording in IronTeams's player window (demo
    // mode only records it), never in the browser; Share copies its link.
    a.click_last("Watch");
    a.click_last("Play the recording");
    a.click("Agenda.docx");
    let opened = a.h.engine.apps_opened();
    assert!(opened.iter().any(|u| u.ends_with("/notes")), "{opened:?}");
    let played = |opened: &[String]| {
        opened
            .iter()
            .filter(|u| u.starts_with("player:https://") && u.ends_with("/01DEMO1.mp4"))
            .count()
    };
    let in_browser = |opened: &[String]| {
        opened
            .iter()
            .filter(|u| u.starts_with("https://") && u.ends_with("/01DEMO1.mp4"))
            .count()
    };
    assert_eq!(played(&opened), 2, "{opened:?}");
    assert_eq!(in_browser(&opened), 0, "not in the browser: {opened:?}");
    assert!(
        crate::external::suppressed_targets()
            .iter()
            .all(|t| !t.ends_with("/01DEMO1.mp4")),
        "nothing handed to the desktop"
    );
    // More options keeps Open in browser as the fallback.
    assert!(a.has_label("More options"));
    a.send("recaps-watch-web", "");
    assert_eq!(in_browser(&a.h.engine.apps_opened()), 1);
    assert!(
        opened.iter().any(|u| u.ends_with("/Agenda.docx")),
        "{opened:?}"
    );
    a.click_last("Share");
    assert_eq!(a.banner(), "Link to the recording copied.");

    // Part 1 has no transcript; Part 2 has one, read as text.
    a.click_any("Transcript");
    assert_eq!(r.get_tab(), "transcript");
    assert_eq!(r.get_transcript_note(), "This recording has no transcript.");
    a.click("Part 2");
    assert_eq!(r.get_part(), 1);
    assert_eq!(r.get_part_label(), "Recording · 10 min");
    assert!(
        r.get_transcript().contains("Ada: Welcome everyone."),
        "{}",
        r.get_transcript()
    );
    assert!(a.has_label("Transcript text"));

    // A canceled meeting with nothing in its chat says so plainly.
    a.click("Back to Meeting Recaps");
    assert_eq!(r.get_page(), "home");
    a.click("Recap: Canceled: Budget review");
    assert_eq!(r.get_detail_title(), "Canceled: Budget review");
    assert!(!r.get_has_recording());
    assert_eq!(r.get_parts().row_count(), 0);
    assert_eq!(
        r.get_part_label(),
        "No recording was found in the meeting chat."
    );
    assert_eq!(r.get_files_note(), "No files were shared.");
    // No AI summary and no Speakers: Copilot features are not offered.
    assert!(!a.has_label("AI summary") && !a.has_label("Speakers"));
}

#[test]
fn a_meeting_chat_has_a_recap_tab_that_shows_the_same_recap() {
    let a = AppsHarness::open();
    crate::ui::call::wire(&a.h.app);
    a.h.app.set_view("chat".into());
    a.h.app.set_current_id("19:meeting_demo2@thread.v2".into());
    let tab = a
        .find("Recap")
        .into_iter()
        .find(|e| e.accessible_role() == Some(AccessibleRole::Tab))
        .expect("a Recap tab in the meeting chat's header");
    tab.mock_single_click(PointerEventButton::Left);
    let f = a.h.app.global::<crate::FindStore>();
    assert_eq!(f.get_chat_tab(), "recap");
    let r = a.h.app.global::<RecapsStore>();
    assert!(r.get_detail_open() && r.get_in_chat());
    assert_eq!(r.get_detail_title(), "Design review");
    assert!(r.get_has_recording());
    assert!(a.has_label("Meeting recap"));
    assert!(
        !a.has_label("Back to Meeting Recaps"),
        "the tab has no way back to the home"
    );
    // A chat that is not a meeting chat has no Recap tab.
    a.h.app.set_current_id("19:group@thread.v2".into());
    assert!(
        !a.find("Recap")
            .into_iter()
            .any(|e| e.accessible_role() == Some(AccessibleRole::Tab))
    );
}

#[test]
fn meeting_recaps_keep_long_text_inside_at_760_px() {
    use super::overflow::{LONG, LONG_AR, LONG_WORD, assert_texts_inside};
    let a = recaps_open();
    a.h.app.window().set_size(LogicalSize::new(760., 700.));
    let r = a.h.app.global::<RecapsStore>();
    let long_row = |id: &str, title: &str| crate::RecapRow {
        id: id.into(),
        title: title.into(),
        when: LONG.into(),
        organizer: LONG_AR.into(),
        month: "Nov".into(),
        day: "18".into(),
        header: false,
        canceled: false,
    };
    r.set_meetings(slint::ModelRc::new(slint::VecModel::from(vec![
        crate::RecapRow {
            id: "week".into(),
            title: LONG.into(),
            header: true,
            ..Default::default()
        },
        long_row("a", LONG),
        long_row("b", LONG_AR),
        long_row("c", LONG_WORD),
    ])));
    let rows: Vec<_> = ElementHandle::find_by_element_type_name(&a.h.app, "MeetingRow").collect();
    assert_eq!(rows.len(), 3);
    for (i, row) in rows.iter().enumerate() {
        assert_texts_inside(row, &format!("meeting row {i}"));
    }
    for h in ElementHandle::find_by_element_type_name(&a.h.app, "WeekHeading") {
        assert_texts_inside(&h, "week heading");
    }

    // The recap page: long title, date line, parts, files and transcript.
    a.send("recaps-open", "ev-sync");
    assert_eq!(r.get_page(), "detail");
    r.set_detail_title(LONG.into());
    r.set_detail_organizer(LONG_AR.into());
    r.set_files(slint::ModelRc::new(slint::VecModel::from(vec![
        crate::AppRow {
            id: "https://example.invalid/f".into(),
            title: LONG_WORD.into(),
            subtitle: LONG.into(),
            meta: "DOCX".into(),
            ..Default::default()
        },
    ])));
    r.set_transcript(format!("{LONG}\n{LONG_WORD}\n{LONG_AR}").into());
    r.set_tab("transcript".into());
    // A meeting recorded in many parts: the parts scroll and Share / Watch / More options stay
    // on the page.
    r.set_parts(slint::ModelRc::new(slint::VecModel::from(
        (1..=9)
            .map(|i| crate::AppNav {
                label: format!("Part {i}").into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    )));
    let page = ElementHandle::find_by_element_type_name(&a.h.app, "RecapDetailPage")
        .next()
        .expect("the recap page");
    assert!(page.size().width <= 760.5, "{:?}", page.size());
    for label in ["Share", "Watch", "More options"] {
        let b = find_label(&a.h.app, label);
        assert!(!b.is_empty(), "{label} is shown");
        let (p, s) = (b[0].absolute_position(), b[0].size());
        assert!(
            p.x >= page.absolute_position().x - 0.5
                && p.x + s.width <= page.absolute_position().x + page.size().width + 0.5,
            "{label} at {p:?} size {s:?} leaves the page"
        );
    }
    // Texts that wrap may scroll below the fold, so only the header and the file row are
    // checked against their own boxes, and the page against the window's width.
    for f in ElementHandle::find_by_element_type_name(&a.h.app, "FileRow") {
        assert_texts_inside(&f, "shared file row");
    }
    for t in super::overflow::texts(&page) {
        let (p, s) = (t.absolute_position(), t.size());
        let pp = page.absolute_position();
        assert!(
            p.x >= pp.x - 0.5 && p.x + s.width <= pp.x + page.size().width + 0.5,
            "text {:?} at {p:?} size {s:?} leaves the page",
            t.accessible_label()
        );
    }
}

#[test]
fn onenote_reads_notebooks_sections_pages_and_page_text() {
    let a = AppsHarness::open();
    a.send("open-app", "demo-onenote");
    assert_eq!(a.view(), "onenote");
    let n = a.h.app.global::<NotesStore>();
    assert_eq!(n.get_notebooks().row_count(), 1);
    let sections: Vec<String> = n
        .get_sections()
        .iter()
        .map(|r| r.title.to_string())
        .collect();
    assert_eq!(sections, ["Meeting notes", "Ideas"]);
    // The first section's pages are listed.
    let pages: Vec<String> = n.get_pages().iter().map(|r| r.title.to_string()).collect();
    assert_eq!(pages, ["Weekly sync", "Retro"]);
    a.click("Weekly sync");
    assert_eq!(n.get_page_title(), "Weekly sync");
    assert!(
        n.get_page_text().contains("Ship the Store view & Planner"),
        "{}",
        n.get_page_text()
    );
    // An empty section says so.
    a.click("Ideas");
    assert_eq!(n.get_pages().row_count(), 0);
    assert_eq!(n.get_status(), "This section has no pages.");
}

#[test]
fn lists_show_a_lists_columns_and_items() {
    let a = AppsHarness::open();
    a.send("open-app", "demo-lists");
    assert_eq!(a.view(), "lists");
    let l = a.h.app.global::<ListsStore>();
    assert_eq!(l.get_sites().row_count(), 1);
    // The user list opens before the document library.
    assert_eq!(l.get_title(), "Issue tracker");
    let cols: Vec<String> = l.get_columns().iter().map(|c| c.to_string()).collect();
    assert_eq!(cols, ["Title", "Status", "Owner", "Done"]);
    assert_eq!(l.get_items().row_count(), 3);
    let first = l.get_items().row_data(0).unwrap().cells;
    assert_eq!(first.row_data(0).unwrap(), "Login fails on mobile");
    a.click("Documents");
    assert_eq!(l.get_title(), "Documents");
}

#[test]
fn a_w_apps_tab_opens_in_the_web_host_and_the_plain_link_stays_in_the_browser() {
    let a = AppsHarness::open();
    a.click("More apps");
    a.click("Shifts");
    assert_eq!(a.view(), "app");
    // The tab goes to the web host (demo mode only records it, no Chromium starts).
    a.send("app-link", "tab:home");
    let opened = a.h.engine.apps_opened();
    assert!(
        opened
            .iter()
            .any(|u| u.starts_with("web-host:https://apps.example.invalid/demo-shifts/")),
        "{opened:?}"
    );
    assert!(
        !opened
            .iter()
            .any(|u| u.starts_with("https://apps.example.invalid/")),
        "the tab URL is not sent to the browser: {opened:?}"
    );
    // The about tab has no web URL, so it falls back to the Teams link in the browser.
    a.send("app-link", "tab:about");
    // The plain link keeps using the browser.
    a.send("app-link", "host");
    let opened = a.h.engine.apps_opened();
    assert!(
        opened
            .iter()
            .any(|u| u.starts_with("https://teams.microsoft.com/l/entity/demo-shifts/home")),
        "{opened:?}"
    );
}
#[test]
fn the_store_and_an_app_fit_a_narrow_window() {
    use super::responsive::{WIDTHS, assert_fits, resize};
    for width in WIDTHS {
        let a = AppsHarness::open();
        resize(&a.h.app, width);
        a.send("store-open", "");
        assert_fits(&a.h.app, width, "the apps store");
        a.click_in("Planner", "AppCardView");
        assert_eq!(a.store().get_store_page(), "app");
        assert_fits(&a.h.app, width, "an app in the store");
    }
}
