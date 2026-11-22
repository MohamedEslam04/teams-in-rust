//! The OneDrive view driven through Slint's testing backend. The real engine runs behind the
//! window in demo mode (no network): every `DriveStore.command` goes through `DriveCmd::parse`
//! and the engine, and the rows the window shows are read back from the rendered elements.

use super::*;
use crate::backend::DriveCmd;
use crate::{DriveRow, DriveStore};
use std::sync::atomic::Ordering;

struct DriveHarness {
    h: Harness,
    /// Every `name argument` the view sent, in order.
    log: Log,
    /// While set, commands wait in `queue` instead of running (see `double_click_label`).
    hold: Rc<std::cell::Cell<bool>>,
    queue: Rc<RefCell<Vec<DriveCmd>>>,
}

impl DriveHarness {
    fn run(engine: &Arc<Engine>, rt: &tokio::runtime::Runtime, cmd: DriveCmd) {
        rt.block_on(engine.clone().handle(Cmd::Drive(cmd)));
    }
}

impl DriveHarness {
    /// A window on the OneDrive view, opened like the rail does (`enter`).
    fn open() -> Self {
        let h = Harness::new();
        h.app.window().set_size(LogicalSize::new(1400., 900.));
        h.app.set_view("onedrive".into());
        let log = Log::default();
        let d = DriveHarness {
            h,
            log: log.clone(),
            hold: Rc::default(),
            queue: Rc::default(),
        };
        let (engine, rt) = (d.h.engine.clone(), d.h.rt.clone());
        let (hold, queue) = (d.hold.clone(), d.queue.clone());
        d.h.app.global::<DriveStore>().on_command(move |name, arg| {
            log.borrow_mut()
                .push(format!("{name} {arg}").trim().to_owned());
            let Some(cmd) = DriveCmd::parse(&name, &arg) else {
                return;
            };
            if hold.get() {
                queue.borrow_mut().push(cmd);
            } else {
                DriveHarness::run(&engine, &rt, cmd);
            }
        });
        d.send("enter", "");
        d.settle();
        d
    }

    fn store(&self) -> DriveStore<'_> {
        self.h.app.global::<DriveStore>()
    }

    /// Sends a command as the view would.
    fn send(&self, name: &str, arg: &str) {
        self.store().invoke_command(name.into(), arg.into());
    }

    /// Lets background tasks (libraries and favorites) finish.
    fn settle(&self) {
        self.h
            .rt
            .block_on(async { tokio::time::sleep(std::time::Duration::from_millis(30)).await });
    }

    fn click_label(&self, label: &str) {
        let e = ElementHandle::find_by_accessible_label(&self.h.app, label)
            .next()
            .unwrap_or_else(|| panic!("nothing labelled {label:?}"));
        e.mock_single_click(PointerEventButton::Left);
    }

    /// A row of the open menu. A column header text can carry the same label ("Size").
    fn click_menu_item(&self, label: &str) {
        ElementHandle::find_by_accessible_label(&self.h.app, label)
            .find(|e| e.type_name().as_deref() == Some("MenuItem"))
            .unwrap_or_else(|| panic!("no menu row {label:?}"))
            .mock_single_click(PointerEventButton::Left);
    }

    fn has_label(&self, label: &str) -> bool {
        ElementHandle::find_by_accessible_label(&self.h.app, label)
            .next()
            .is_some()
    }

    fn double_click_label(&self, label: &str) {
        let e = ElementHandle::find_by_accessible_label(&self.h.app, label)
            .next()
            .unwrap_or_else(|| panic!("nothing labelled {label:?}"));
        // The testing platform counts a double click in real time (500 ms), and a debug build
        // can spend longer than that answering the first click, so commands wait until both
        // clicks are in.
        self.hold.set(true);
        e.mock_single_click(PointerEventButton::Left);
        e.mock_single_click(PointerEventButton::Left);
        self.hold.set(false);
        for cmd in self.queue.take() {
            Self::run(&self.h.engine, &self.h.rt, cmd);
        }
    }

    /// The names shown as list rows, top to bottom. Group headers come as `# <label>`.
    fn names(&self) -> Vec<String> {
        let mut v: Vec<ElementHandle> =
            ElementHandle::find_by_element_type_name(&self.h.app, "DriveListRow").collect();
        v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
        v.iter()
            .map(|e| {
                let l = label(e);
                match l.strip_prefix("Group ") {
                    Some(g) => format!("# {g}"),
                    None => l,
                }
            })
            .collect()
    }

    fn tiles(&self) -> usize {
        ElementHandle::find_by_element_type_name(&self.h.app, "DriveTile").count()
    }

    fn crumbs(&self) -> Vec<String> {
        self.store()
            .get_crumbs()
            .iter()
            .map(|c| c.to_string())
            .collect()
    }

    fn status(&self) -> String {
        self.store().get_status().to_string()
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    fn row(&self, name: &str) -> DriveRow {
        self.store()
            .get_rows()
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no row {name}"))
    }

    /// Hover the row so its hover-only buttons appear.
    fn hover_row(&self, name: &str) {
        let row = ElementHandle::find_by_element_type_name(&self.h.app, "DriveListRow")
            .find(|e| label(e) == name)
            .unwrap_or_else(|| panic!("no list row {name}"));
        hover(&self.h.app, &row);
    }
}

#[test]
fn the_rail_opens_onedrive_on_home_with_recent_files() {
    let h = Harness::new();
    h.app.window().set_size(LogicalSize::new(1400., 900.));
    let seen = Log::default();
    let (engine, rt, s) = (h.engine.clone(), h.rt.clone(), seen.clone());
    h.app.global::<DriveStore>().on_command(move |n, a| {
        s.borrow_mut().push(format!("{n} {a}"));
        if let Some(c) = DriveCmd::parse(&n, &a) {
            rt.block_on(engine.clone().handle(Cmd::Drive(c)));
        }
    });
    let mut rail: Vec<ElementHandle> =
        ElementHandle::find_by_element_id(&h.app, "AppRail::ri").collect();
    rail.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    assert_eq!(rail.len(), 5, "Activity, Chat, Teams, Calendar, OneDrive");
    rail[4].mock_single_click(PointerEventButton::Left);
    assert_eq!(h.app.get_view(), "onedrive");
    assert_eq!(seen.borrow()[0].trim(), "enter");
    let d = h.app.global::<DriveStore>();
    assert_eq!(d.get_section(), "home");
    assert!(d.get_rows().row_count() >= 8, "recent files are listed");
    assert_eq!(d.get_title(), "Home");
}

#[test]
fn the_left_navigation_switches_sections() {
    let d = DriveHarness::open();
    assert_eq!(d.crumbs(), ["Home"]);
    let names = d.names();
    assert!(names.contains(&"Budget 2026.xlsx".to_owned()), "{names:?}");

    d.click_label("Shared");
    assert_eq!(d.store().get_section(), "shared");
    assert_eq!(d.crumbs(), ["Shared"]);
    assert!(d.names().contains(&"Team handbook".to_owned()));

    d.click_label("Favorites");
    assert_eq!(d.names().len(), 3);

    d.click_label("Meetings");
    assert!(d.names()[0].contains(".mp4"));
    assert!(
        d.has_label("People") && d.has_label("Media"),
        "Browse files by"
    );

    d.click_label("Recycle bin");
    assert_eq!(d.store().get_section(), "bin");
    assert_eq!(d.names().len(), 3);

    // Quick access lists the team libraries, which open as their own root.
    d.click_label("Sales team");
    assert_eq!(d.store().get_section(), "drive:b!sales");
    assert_eq!(d.crumbs(), ["Sales team"]);
    assert!(d.names().contains(&"Price list.xlsx".to_owned()));
    assert_eq!(
        d.log().last().map(String::as_str),
        Some("nav drive:b!sales")
    );

    d.click_label("Libraries");
    assert_eq!(d.names().len(), 3, "one row per library");
}

#[test]
fn folders_open_below_breadcrumbs_that_lead_back() {
    let d = DriveHarness::open();
    d.click_label("My files");
    assert_eq!(d.crumbs(), ["My files"]);
    assert_eq!(
        &d.names()[..3],
        ["Brand assets", "Pictures", "Projects"],
        "folders come first"
    );

    d.double_click_label("Projects");
    assert_eq!(d.crumbs(), ["My files", "Projects"]);
    assert_eq!(d.names()[0], "Alpha");
    d.double_click_label("Alpha");
    assert_eq!(d.crumbs(), ["My files", "Projects", "Alpha"]);
    assert_eq!(d.names(), ["build.sh", "Spec.docx", "Timeline.xlsx"]);
    assert!(d.log().contains(&"open-row demo-alpha".to_owned()));

    d.click_label("Breadcrumb Projects");
    assert_eq!(d.crumbs(), ["My files", "Projects"]);
    assert_eq!(d.names().len(), 3);
    d.click_label("Breadcrumb My files");
    assert_eq!(d.crumbs(), ["My files"]);
    assert_eq!(d.names().len(), 10);
    assert!(d.log().contains(&"crumb 0".to_owned()));
}

#[test]
fn a_column_header_sorts_and_clicking_it_again_flips_the_order() {
    let d = DriveHarness::open();
    d.click_label("My files");
    assert_eq!(
        d.names()[3],
        "Budget 2026.xlsx",
        "name order, folders first"
    );

    d.click_label("Sort by Size");
    let by_size = d.names();
    assert_eq!(by_size[3], "Team offsite.pptx", "biggest file first");
    assert_eq!(by_size[0], "Projects", "folders stay on top");
    d.click_label("Sort by Size");
    assert_eq!(d.names()[3], "README.txt", "smallest file first");
    assert_eq!(d.log().iter().filter(|c| *c == "sort size").count(), 2);

    d.click_label("Sort by Modified by");
    assert_eq!(d.store().get_sort(), "modified-by");
}

#[test]
fn the_sort_menu_and_group_menu_send_commands() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("Sort");
    d.click_menu_item("Sort by Modified");
    assert_eq!(d.store().get_sort(), "modified");
    assert!(!d.store().get_ascending(), "modified starts newest first");
    assert_eq!(d.names()[3], "Budget 2026.xlsx", "modified 45 minutes ago");

    d.click_label("Group by");
    d.click_menu_item("Group by Modified");
    let names = d.names();
    assert_eq!(d.store().get_group(), "modified");
    assert_eq!(names[0], "# Today");
    assert!(names.iter().any(|n| n == "# Yesterday"));
    assert!(names.iter().any(|n| n == "# Older"));
    assert!(d.log().contains(&"group modified".to_owned()));

    d.click_label("Group by");
    d.click_menu_item("Group by None");
    assert!(!d.names().iter().any(|n| n.starts_with("# ")));
}

#[test]
fn the_column_picker_adds_and_removes_columns() {
    let d = DriveHarness::open();
    d.click_label("My files");
    assert!(d.store().get_col_size() && !d.store().get_col_location());
    d.click_label("Columns");
    d.click_menu_item("Location");
    assert!(d.store().get_col_location());
    d.click_menu_item("Size");
    assert!(
        !d.store().get_col_size(),
        "the menu stays open for more toggles"
    );
    assert_eq!(
        d.log().iter().filter(|c| c.starts_with("column ")).count(),
        2
    );
    assert!(d.has_label("Sort by Modified"), "other columns remain");
    assert!(!d.has_label("Sort by Size"), "the Size header is gone");
}

#[test]
fn selecting_a_row_shows_the_toolbar_and_its_buttons_fire_commands() {
    let d = DriveHarness::open();
    d.click_label("My files");
    assert!(
        d.has_label("New") && !d.has_label("Share"),
        "no selection: New and Upload"
    );

    d.click_label("Q4 plan.docx");
    assert!(d.row("Q4 plan.docx").selected);
    assert!(d.has_label("1 selected"));
    for b in [
        "Open",
        "Share",
        "Copy link",
        "Favorite",
        "Rename",
        "Move to",
        "Copy to",
        "Download",
        "Delete",
        "More",
    ] {
        assert!(d.has_label(b), "toolbar has {b}");
    }
    assert!(!d.has_label("New"), "the selection toolbar replaces New");

    d.click_label("Copy link");
    d.click_label("Favorite");
    d.click_label("More");
    d.click_menu_item("Add shortcut");
    d.click_label("Download");
    d.click_label("Copy to");
    let sent = d.log();
    for want in [
        "select demo-q4",
        "copy-link",
        "favorite",
        "shortcut",
        "download",
        "copy-to",
    ] {
        assert!(sent.contains(&want.to_owned()), "{want} in {sent:?}");
    }
    assert!(d.store().get_sel_all_favorite() || d.row("Q4 plan.docx").favorite);

    // Two rows: single-item actions are disabled, Move to and Delete still work.
    d.click_label("Cancel");
    d.click_label("Select Meeting notes.md");
    assert!(d.has_label("2 selected"));
    let before = d.log().len();
    d.click_label("Rename");
    d.click_label("Share");
    assert_eq!(
        d.log().len(),
        before,
        "Rename and Share need exactly one item"
    );
    d.click_label("Delete");
    assert!(!d.names().contains(&"Q4 plan.docx".to_owned()));
    assert!(!d.names().contains(&"Meeting notes.md".to_owned()));
    assert!(d.status().starts_with("Deleted 2 items"), "{}", d.status());
    assert!(
        !d.has_label("Delete"),
        "the selection is gone with the rows"
    );
}

#[test]
fn clear_selection_brings_back_the_new_menu() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("README.txt");
    d.click_label("Clear selection");
    assert!(d.has_label("New"));
    assert_eq!(d.store().get_sel_count(), 0);
}

#[test]
fn new_folder_asks_for_a_name_and_creates_it() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("New");
    for item in [
        "Folder",
        "Word document",
        "Excel workbook",
        "PowerPoint presentation",
        "Markdown document",
        "Text document",
    ] {
        assert!(d.has_label(item), "New menu has {item}");
    }
    d.click_label("Folder");
    assert_eq!(d.store().get_dialog(), "prompt");
    assert_eq!(d.store().get_prompt_text(), "Untitled folder");
    d.store().set_prompt_text("".into());
    type_text(&d.h.app, "Invoices");
    d.click_label("OK");
    assert_eq!(d.store().get_dialog(), "");
    assert!(d.log().contains(&"new-folder Invoices".to_owned()));
    assert!(d.names().contains(&"Invoices".to_owned()));
    assert!(d.row("Invoices").selected, "the new folder is selected");
}

#[test]
fn the_new_menu_creates_documents_and_uploads_through_commands() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("New");
    d.click_label("Excel workbook");
    d.click_label("New");
    d.click_label("Markdown document");
    d.click_label("Upload");
    d.click_label("Files upload");
    d.click_label("Upload");
    d.click_label("Folder upload");
    let sent = d.log();
    for want in [
        "new-doc excel",
        "new-doc markdown",
        "upload-files",
        "upload-folder",
    ] {
        assert!(sent.contains(&want.to_owned()), "{want} in {sent:?}");
    }
    // Demo mode never writes.
    assert!(d.status().starts_with("Demo mode"));
}

#[test]
fn a_dropped_path_asks_to_upload_it() {
    let d = DriveHarness::open();
    d.send("drop", "/tmp/notes.txt");
    assert!(d.status().starts_with("Demo mode"), "{}", d.status());
    assert_eq!(
        DriveCmd::parse("drop", "/tmp/a.txt"),
        Some(DriveCmd::Drop("/tmp/a.txt".into()))
    );
}

#[test]
fn rename_opens_a_prefilled_prompt_and_renames() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("README.txt");
    d.click_label("Rename");
    assert_eq!(d.store().get_dialog(), "prompt");
    assert_eq!(d.store().get_prompt_text(), "README.txt");
    d.store().set_prompt_text("CHANGELOG.txt".into());
    d.click_label("OK");
    assert!(d.log().contains(&"rename CHANGELOG.txt".to_owned()));
    assert!(d.names().contains(&"CHANGELOG.txt".to_owned()));
    assert!(!d.names().contains(&"README.txt".to_owned()));
}

#[test]
fn the_row_menu_holds_every_action_and_acts_on_that_row() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.hover_row("Q4 plan.docx");
    d.click_label("More actions for Q4 plan.docx");
    assert!(d.row("Q4 plan.docx").selected, "the menu selects its row");
    for item in [
        "Open in IronTeams",
        "Open in browser",
        "Open in desktop app",
        "Open location",
        "Share",
        "Copy link",
        "Add shortcut",
        "Rename",
        "Move to",
        "Copy to",
        "Download",
        "Delete",
        "Version history",
        "Details",
    ] {
        assert!(d.has_label(item), "row menu has {item}");
    }
    d.click_label("Details");
    assert_eq!(d.store().get_dialog(), "info");
    assert_eq!(d.store().get_info_title(), "Details");
    let text = d.store().get_info_text().to_string();
    assert!(
        text.contains("Name: Q4 plan.docx") && text.contains("Type: word"),
        "{text}"
    );
    d.click_label("Close");
    assert_eq!(d.store().get_dialog(), "");
}

#[test]
fn the_star_toggles_a_favorite_and_favorites_drops_it() {
    let d = DriveHarness::open();
    d.settle();
    d.click_label("My files");
    assert!(!d.row("Q4 plan.docx").favorite);
    d.hover_row("Q4 plan.docx");
    d.click_label("Add to favorites Q4 plan.docx");
    assert!(d.row("Q4 plan.docx").favorite);
    assert!(d.log().contains(&"favorite demo-q4".to_owned()));
    d.click_label("Favorites");
    d.hover_row("Budget 2026.xlsx");
    d.click_label("Remove from favorites Budget 2026.xlsx");
    assert!(!d.names().contains(&"Budget 2026.xlsx".to_owned()));
}

#[test]
fn the_share_dialog_picks_people_sets_access_and_lists_who_has_it() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("Q4 plan.docx");
    d.click_label("Share");
    assert_eq!(d.store().get_dialog(), "share");
    assert_eq!(d.store().get_share_name(), "Q4 plan.docx");
    assert!(d.has_label("Mohamed Eslam"), "the owner is listed");
    assert!(d.has_label("Sara Hassan"));
    assert!(d.has_label("People in your organization with the link"));

    d.click_label("Search for people");
    type_text(&d.h.app, "ahm");
    assert!(d.log().iter().any(|c| c.starts_with("share-search ahm")));
    assert!(d.has_label("Ahmed Ali"), "the search result shows");
    d.click_label("Ahmed Ali");
    assert_eq!(d.store().get_share_picked().row_count(), 1);
    assert!(d.has_label("Remove Ahmed Ali"));

    d.click_label("Can view");
    assert_eq!(d.store().get_share_role(), "read");
    d.click_label("Send");
    assert!(d.log().iter().any(|c| c.starts_with("share-send")));
    assert!(d.store().get_share_status().starts_with("Demo mode"));

    // Link settings are sent as one argument.
    d.click_label("Specific people");
    d.click_label("Edit");
    d.click_label("30 days");
    d.click_label("Create link");
    assert!(d.log().contains(&"share-link users,edit,0,30".to_owned()));
    d.click_label("Anyone");
    d.click_label("View");
    d.click_label("Block download");
    d.click_label("Create link");
    assert!(d.log().contains(&"share-link anyone,view,1,30".to_owned()));

    // Access of another person can be changed or removed; the owner cannot.
    assert!(d.has_label("Remove access for Sara Hassan"));
    assert!(!d.has_label("Remove access for Mohamed Eslam"));
    d.click_label("Remove access for Sara Hassan");
    assert!(d.log().contains(&"share-perm-remove perm-sara".to_owned()));

    d.click_label("Close");
    assert_eq!(d.store().get_dialog(), "");
    assert_eq!(d.log().last().map(String::as_str), Some("dialog-close"));
}

#[test]
fn move_to_opens_the_folder_picker_which_browses_and_confirms() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("Q4 plan.docx");
    d.click_label("Move to");
    let s = d.store();
    assert_eq!(s.get_dialog(), "picker");
    assert_eq!(s.get_picker_title(), "Move to");
    assert!(s.get_picker_folders_only());
    assert!(!s.get_picker_can_choose(), "nothing chosen at the top");
    let top: Vec<String> = s
        .get_picker_rows()
        .iter()
        .map(|r| r.name.to_string())
        .collect();
    assert_eq!(top[0], "My files");
    assert!(top.contains(&"Sales team".to_owned()));

    d.click_label("Folder My files");
    let s = d.store();
    assert_eq!(
        s.get_picker_crumbs()
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>(),
        ["OneDrive", "My files"]
    );
    let inside: Vec<String> = s
        .get_picker_rows()
        .iter()
        .map(|r| r.name.to_string())
        .collect();
    assert!(inside.contains(&"Projects".to_owned()));
    assert!(!inside.contains(&"Q4 plan.docx".to_owned()), "folders only");
    assert!(s.get_picker_can_choose());

    d.click_label("Folder Projects");
    assert_eq!(d.store().get_picker_crumbs().row_count(), 3);
    d.click_label("Picker location My files");
    assert_eq!(d.store().get_picker_crumbs().row_count(), 2);

    d.click_label("Choose this folder");
    assert_eq!(d.store().get_dialog(), "");
    assert!(d.log().contains(&"pick-choose".to_owned()));
    // Demo mode never moves anything.
    assert!(d.names().contains(&"Q4 plan.docx".to_owned()));
    assert!(d.status().starts_with("Demo mode"));
}

#[test]
fn cancelling_the_picker_closes_it() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("Q4 plan.docx");
    d.click_label("Copy to");
    assert_eq!(d.store().get_picker_title(), "Copy to");
    d.click_label("Cancel");
    assert_eq!(d.store().get_dialog(), "");
    assert!(d.log().contains(&"pick-cancel".to_owned()));
}

#[test]
fn the_picker_can_pick_a_file_for_other_features() {
    let d = DriveHarness::open();
    d.send("pick-begin", "attach");
    let s = d.store();
    assert_eq!(s.get_dialog(), "picker");
    assert!(!s.get_picker_folders_only(), "files can be chosen");
    d.click_label("Folder My files");
    assert!(!d.store().get_picker_can_choose());
    d.click_label("File Budget 2026.xlsx");
    assert!(d.store().get_picker_can_choose());
    assert!(d.store().get_picker_rows().iter().any(|r| r.selected));
    d.click_label("Select");
    assert_eq!(d.store().get_dialog(), "");
}

#[test]
fn home_shows_for_you_cards_that_open_their_file() {
    let d = DriveHarness::open();
    let titles = |d: &DriveHarness| -> Vec<String> {
        d.store()
            .get_cards()
            .iter()
            .map(|c| c.title.to_string())
            .collect()
    };
    assert_eq!(
        titles(&d),
        ["Team offsite.pptx", "Budget 2026.xlsx", "Roadmap.docx"]
    );
    let first = d.store().get_cards().row_data(0).unwrap();
    assert_eq!(first.reason, "Sara Hassan shared this in a Teams chat");
    assert!(d.has_label("For you"), "the row has its heading");

    d.click_label("For you Budget 2026.xlsx");
    assert!(
        d.log()
            .contains(&"card-open card:b!demo/demo-budget".to_owned()),
        "{:?}",
        d.log()
    );

    // The card opened the workbook in the page viewer; close it again.
    assert_eq!(d.store().get_dialog(), "preview");
    d.click_label("Close");

    // Typing a filter hides the cards, leaving the section does too.
    d.click_label("Filter by name or person");
    type_text(&d.h.app, "sara");
    assert_eq!(d.store().get_cards().row_count(), 0);
    d.click_label("My files");
    assert_eq!(d.store().get_cards().row_count(), 0);
    d.click_label("Home");
    assert_eq!(titles(&d).len(), 3, "back on Home they return");
}

#[test]
fn shared_by_you_lists_what_the_user_shared() {
    let d = DriveHarness::open();
    d.click_label("Shared by you");
    assert_eq!(d.store().get_section(), "shared-by");
    assert_eq!(d.crumbs(), ["Shared by you"]);
    assert_eq!(d.names().len(), 3);
    assert!(d.names().contains(&"Q4 plan.docx".to_owned()));
    // The sharer is the user, so no "modified by" name is shown against each file.
    assert_eq!(d.row("Q4 plan.docx").modified_by, "");
    // The type pills narrow it like the other lists.
    d.click_label("Word");
    assert_eq!(d.names(), ["Q4 plan.docx"]);
}

#[test]
fn people_lists_who_shared_and_opens_their_files() {
    let d = DriveHarness::open();
    d.click_label("People");
    assert_eq!(d.store().get_section(), "people");
    assert_eq!(d.crumbs(), ["People"]);
    assert_eq!(
        d.names(),
        ["Sara Hassan", "Ahmed Ali"],
        "newest share first"
    );
    assert_eq!(d.row("Sara Hassan").size, "2 items");
    // People are not files: selecting one does nothing, so no toolbar of file actions.
    d.click_label("Sara Hassan");
    assert_eq!(d.store().get_sel_count(), 0);

    d.double_click_label("Sara Hassan");
    assert_eq!(d.store().get_section(), "person:Sara Hassan");
    assert_eq!(d.crumbs(), ["People", "Sara Hassan"]);
    assert_eq!(d.names(), ["Team offsite.pptx", "Price list.xlsx"]);
    // Files are acted on like in any other list.
    d.click_label("Price list.xlsx");
    assert_eq!(d.store().get_sel_count(), 1);

    d.click_label("Breadcrumb People");
    assert_eq!(d.store().get_section(), "people");
    assert_eq!(d.names().len(), 2);
}

#[test]
fn media_lists_photos_and_videos_as_tiles() {
    let d = DriveHarness::open();
    d.click_label("Media");
    assert_eq!(d.store().get_section(), "media");
    assert_eq!(d.crumbs(), ["Media"]);
    assert!(d.store().get_grid(), "Media starts as tiles");
    assert_eq!(d.tiles(), 3);
    let kinds: Vec<String> = d
        .store()
        .get_rows()
        .iter()
        .map(|r| r.kind.to_string())
        .collect();
    assert_eq!(kinds, ["image", "image", "video"]);
}

#[test]
fn meetings_adds_the_channel_recordings_in_the_background() {
    let d = DriveHarness::open();
    d.click_label("Meetings");
    // The command returned with My files' recordings; the channels arrive right after.
    d.settle();
    let names = d.names();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(
        d.row("Sprint review-20261110.mp4").location,
        "Sales team \u{203a} General"
    );
    assert_eq!(d.row("Weekly sync-20261112.mp4").location, "Recordings");
    assert!(
        d.store().get_col_location(),
        "the place column is always on"
    );
}

#[test]
fn the_new_menu_offers_visio_notebook_forms_and_link() {
    let d = DriveHarness::open();
    d.click_label("My files");

    d.click_label("New");
    d.click_label("Visio drawing");
    assert!(d.log().contains(&"new-doc visio".to_owned()));

    d.click_label("New");
    d.click_label("OneNote notebook");
    assert_eq!(d.store().get_dialog(), "prompt");
    assert_eq!(d.store().get_prompt_text(), "Untitled notebook");
    d.click_label("OK");
    assert!(
        d.log()
            .contains(&"new-notebook Untitled notebook".to_owned())
    );

    d.click_label("New");
    d.click_label("Forms survey");
    assert!(d.log().contains(&"new-forms".to_owned()));
    assert!(d.status().contains("Forms"), "{}", d.status());

    // A link asks for a name and an address and sends both.
    d.click_label("New");
    d.click_label("Link");
    assert_eq!(d.store().get_dialog(), "prompt");
    assert_eq!(d.store().get_prompt_kind(), "new-link");
    assert!(d.has_label("Web address"));
    type_text(&d.h.app, "Wiki");
    d.click_label("Web address");
    // The address starts as "https://".
    type_text(&d.h.app, "wiki.example/home");
    d.click_label("OK");
    assert!(
        d.log()
            .contains(&"new-link Wiki\nhttps://wiki.example/home".to_owned()),
        "{:?}",
        d.log()
    );
    assert_eq!(d.store().get_dialog(), "");
}

#[test]
fn a_link_without_a_web_address_is_refused_before_anything_is_sent() {
    let d = DriveHarness::open();
    d.send("new-link", "Wiki\nnot a link");
    assert!(d.status().contains("https://"), "{}", d.status());
    d.send("new-link", "\nhttps://x.example");
    assert!(d.status().contains("name"), "{}", d.status());
}

#[test]
fn a_pdf_opens_page_by_page_inside_ironteams() {
    let d = DriveHarness::open();
    d.double_click_label("Vendor contract.pdf");
    let s = d.store();
    assert_eq!(s.get_dialog(), "preview");
    assert_eq!(s.get_preview_kind(), "pages");
    assert_eq!((s.get_preview_page(), s.get_preview_pages()), (1, 3));
    let img = s.get_preview_image();
    assert!(
        img.size().width >= 900,
        "the page is drawn: {:?}",
        img.size()
    );
    assert!(d.has_label("Page 1 of 3"));

    d.click_label("Next page");
    assert_eq!(d.store().get_preview_page(), 2);
    d.click_label("Next page");
    d.click_label("Next page");
    assert_eq!(d.store().get_preview_page(), 3, "stops at the last page");
    d.click_label("Previous page");
    assert_eq!(d.store().get_preview_page(), 2);
    assert!(d.log().contains(&"preview-page next".to_owned()));
    assert!(d.log().contains(&"preview-page prev".to_owned()));
    d.click_label("Close");
    assert_eq!(d.store().get_dialog(), "");
}

#[test]
fn an_office_file_previews_through_its_pdf_conversion() {
    let d = DriveHarness::open();
    d.double_click_label("Customer pitch.pptx");
    assert_eq!(d.store().get_dialog(), "preview");
    assert_eq!(d.store().get_preview_kind(), "pages");
    assert!(d.store().get_preview_pages() >= 1);
}

#[test]
fn the_recycle_bin_offers_restore_and_asks_before_deleting_for_good() {
    let d = DriveHarness::open();
    d.click_label("Recycle bin");
    d.click_label("Old draft.docx");
    assert!(d.has_label("Restore") && d.has_label("Delete forever"));
    assert!(!d.has_label("Share"), "no sharing from the bin");
    d.click_label("Restore");
    assert!(d.log().contains(&"restore".to_owned()));
    assert!(!d.names().contains(&"Old draft.docx".to_owned()));
    assert!(d.status().starts_with("Restored 1 item"), "{}", d.status());

    d.click_label("Scratch.txt");
    d.click_label("Delete forever");
    assert_eq!(d.store().get_dialog(), "confirm");
    assert!(
        !d.log().contains(&"delete-forever".to_owned()),
        "not before the user confirms"
    );
    d.click_label("Cancel");
    assert!(d.names().contains(&"Scratch.txt".to_owned()));
    d.click_label("Delete forever");
    d.click_label("Yes, delete forever");
    assert!(d.log().contains(&"delete-forever".to_owned()));
    assert!(!d.names().contains(&"Scratch.txt".to_owned()));
}

#[test]
fn list_and_grid_views_show_the_same_items() {
    let d = DriveHarness::open();
    d.click_label("My files");
    assert_eq!(d.tiles(), 0);
    d.click_label("Grid view");
    assert!(d.store().get_grid());
    assert_eq!(d.tiles(), 10);
    d.click_label("Budget 2026.xlsx");
    assert!(d.row("Budget 2026.xlsx").selected);
    d.click_label("List view");
    assert!(!d.store().get_grid());
    assert_eq!(d.tiles(), 0);
    assert_eq!(d.names().len(), 10);
    assert!(d.log().contains(&"view grid".to_owned()) && d.log().contains(&"view list".to_owned()));
}

#[test]
fn type_pills_and_the_filter_box_narrow_home() {
    let d = DriveHarness::open();
    let all = d.names().len();
    assert_eq!(all, 8);
    for pill in ["All", "Word", "Excel", "PowerPoint", "PDF", "More"] {
        assert!(d.has_label(pill), "pill {pill}");
    }
    d.click_label("Excel");
    assert_eq!(d.names(), ["Budget 2026.xlsx", "Roadmap.xlsx"]);
    d.click_label("PDF");
    assert_eq!(d.names(), ["Vendor contract.pdf"]);
    d.click_label("All");
    assert_eq!(d.names().len(), all);

    d.click_label("Filter by name or person");
    type_text(&d.h.app, "sara");
    assert_eq!(
        d.names(),
        ["Customer pitch.pptx"],
        "matches the person who changed it"
    );
    assert!(d.log().contains(&"filter sara".to_owned()));
    // Leaving the section clears the filter.
    d.click_label("Shared");
    assert_eq!(d.store().get_filter(), "");
}

#[test]
fn searching_runs_a_search_section_and_an_empty_search_returns_home() {
    let d = DriveHarness::open();
    d.click_label("Search files");
    type_text(&d.h.app, "plan");
    enter(&d.h.app);
    assert_eq!(d.store().get_section(), "search");
    assert_eq!(d.crumbs(), ["Search: plan"]);
    assert!(!d.names().is_empty());
    d.send("search", "");
    assert_eq!(d.store().get_section(), "home");
}

#[test]
fn opening_a_text_file_previews_it_inside_ironteams() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.double_click_label("README.txt");
    assert_eq!(d.store().get_dialog(), "preview");
    assert_eq!(d.store().get_preview_kind(), "text");
    assert!(d.store().get_preview_text().contains("README.txt"));
    assert!(d.has_label("Open in browser"));
    d.click_label("Close");
    assert_eq!(d.store().get_dialog(), "");

    d.double_click_label("Meeting notes.md");
    assert_eq!(d.store().get_preview_kind(), "markdown");
    assert!(d.store().get_preview_text().starts_with("# Meeting notes"));
    d.click_label("Close");
}

#[test]
fn version_history_lists_versions_and_restores_an_earlier_one() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("Q4 plan.docx");
    d.click_label("More");
    d.click_label("Version history");
    assert_eq!(d.store().get_dialog(), "versions");
    assert_eq!(d.store().get_versions().row_count(), 3);
    assert!(d.has_label("Restore 2.0"));
    assert!(
        !d.has_label("Restore 3.0"),
        "the current version has no Restore"
    );
    d.click_label("Restore 2.0");
    assert!(d.log().contains(&"version-restore 2.0".to_owned()));
    assert_eq!(d.store().get_dialog(), "");
}

#[test]
fn sync_explains_that_ironteams_does_not_sync() {
    let d = DriveHarness::open();
    d.click_label("Sync");
    assert_eq!(d.store().get_dialog(), "info");
    let text = d.store().get_info_text().to_string();
    assert_eq!(text, onedrive::sync_help());
    assert!(text.contains("does not sync") && text.contains("rclone"));
}

#[test]
fn open_menu_offers_ironteams_browser_desktop_and_location() {
    let d = DriveHarness::open();
    d.click_label("My files");
    d.click_label("README.txt");
    d.click_label("Open");
    for item in [
        "Open in IronTeams",
        "Open in browser",
        "Open in desktop app",
        "Open location",
    ] {
        assert!(d.has_label(item), "{item}");
    }
    d.click_label("Open in IronTeams");
    assert!(d.log().contains(&"open-ironteams".to_owned()));
    assert_eq!(d.store().get_dialog(), "preview");
}

#[test]
fn add_shortcut_failing_shows_a_plain_message_not_an_error() {
    let d = DriveHarness::open();
    d.click_label("Shared");
    d.click_label("Team handbook");
    // Leave demo mode so the call goes to the (unreachable) service and fails like the real one.
    d.h.engine.offline.store(false, Ordering::Relaxed);
    d.click_label("More");
    d.click_menu_item("Add shortcut");
    let status = d.status();
    assert!(
        status.starts_with("OneDrive did not accept the shortcut"),
        "{status}"
    );
    assert!(d.has_label("Team handbook"), "the list is untouched");
}

#[test]
fn a_late_listing_does_not_replace_the_section_the_user_moved_to() {
    // The state machine drops listings of an older navigation; here two quick navigations end on
    // the last one.
    let d = DriveHarness::open();
    d.send("nav", "shared");
    d.send("nav", "bin");
    assert_eq!(d.store().get_section(), "bin");
    assert_eq!(d.names().len(), 3);
}

/// Renders the demo Home or a folder offscreen to a PNG, for looking at the design without a
/// display: `IRONTEAMS_SHOT=/tmp/drive.png cargo test -p IronTeams render_demo_drive -- --ignored`
/// (`IRONTEAMS_SHOT_VIEW=grid|files|bin|share|picker` picks the scene).
#[test]
#[ignore = "writes a PNG; run by hand"]
fn render_demo_drive_png() {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::{PhysicalSize, Rgb8Pixel};

    struct Shot(Rc<MinimalSoftwareWindow>);
    impl slint::platform::Platform for Shot {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    let out = compat::var("IRONTEAMS_SHOT").expect("set IRONTEAMS_SHOT to the output PNG path");
    let scene = compat::var("IRONTEAMS_SHOT_VIEW").unwrap_or_default();
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Shot(window.clone()))).unwrap();
    let app = AppWindow::new().unwrap();
    let (w, h) = (1400u32, 900u32);
    window.set_size(PhysicalSize::new(w, h));
    app.set_login_state("ready".into());
    app.set_view("onedrive".into());

    // The real engine in demo mode, driven on a current-thread runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = Store::open_in_memory().unwrap();
    demo::seed(&store, teams_proto::time::now_ms()).unwrap();
    let engine = Engine::new(
        Client::offline("shot").unwrap(),
        store,
        session::Config::default(),
        UiHandle::new(app.as_weak()),
        demo::ME.into(),
        "Me".into(),
    );
    engine.offline.store(true, Ordering::Relaxed);
    let send = |name: &str, arg: &str| {
        if let Some(c) = DriveCmd::parse(name, arg) {
            rt.block_on(engine.clone().handle(Cmd::Drive(c)));
        }
    };
    send("enter", "");
    rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(50)).await });
    match scene.as_str() {
        "grid" => {
            send("nav", "files");
            send("view", "grid");
            send("select", "demo-budget");
        }
        "files" => {
            send("nav", "files");
            send("group", "modified");
            send("select", "demo-q4");
        }
        "bin" => {
            send("nav", "bin");
            send("select", "bin-1");
        }
        "share" => {
            send("nav", "files");
            send("select", "demo-q4");
            send("share", "");
            send("share-search", "sa");
            send("share-pick", "8:orgid:00000000-0000-0000-0000-000000000003");
        }
        "picker" => {
            send("nav", "files");
            send("select", "demo-q4");
            send("move-to", "");
            send("pick-open", "root");
        }
        _ => {
            send("select", "demo-budget");
        }
    }
    app.show().unwrap();

    let mut buf = vec![Rgb8Pixel::default(); (w * h) as usize];
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let raw: Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    image::save_buffer(&out, &raw, w, h, image::ColorType::Rgb8).unwrap();
}

/// Live run against the signed-in OneDrive: `cargo test -p IronTeams live_drive_flow -- --ignored
/// --nocapture`. It reads Home, Shared, Favorites, Libraries, Meetings and the recycle bin (counts
/// only), then does every write inside one folder, `ironteams-test-<timestamp>` in My files, and
/// deletes the folder and purges its leftovers from the recycle bin at the end. It never shares
/// anything and never creates a link.
#[test]
#[ignore = "live: needs a signed-in session; writes only inside ironteams-test-<timestamp>"]
fn live_drive_flow() {
    init_platform();
    let app = AppWindow::new().unwrap();
    app.window().set_size(LogicalSize::new(1400., 900.));
    app.set_login_state("ready".into());
    app.set_view("onedrive".into());
    app.show().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let paths = session::Paths::new().unwrap();
    let cfg = paths.load_config().unwrap();
    let sess = session::stored_session(&cfg)
        .unwrap()
        .expect("not signed in: run ironteams-cli login");
    let client = rt.block_on(Client::resume(sess)).unwrap();
    let me = rt.block_on(client.self_mri()).unwrap();
    let engine = Engine::new(
        client,
        Store::open_in_memory().unwrap(),
        session::Config::default(),
        UiHandle::new(app.as_weak()),
        me,
        "Me".into(),
    );
    let send = |name: &str, arg: &str| {
        let cmd = DriveCmd::parse(name, arg).unwrap();
        rt.block_on(engine.clone().handle(Cmd::Drive(cmd)));
    };
    let rows = || -> Vec<DriveRow> { app.global::<DriveStore>().get_rows().iter().collect() };
    let status = || app.global::<DriveStore>().get_status().to_string();
    let id_of = |name: &str| -> String {
        rows()
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no row named {name}"))
            .id
            .to_string()
    };
    let has = |name: &str| rows().iter().any(|r| r.name == name);

    let folder = format!("ironteams-test-{}", teams_proto::time::now_ms());
    eprintln!("live: test folder is {folder}");

    // Read-only: counts only.
    send("enter", "");
    rt.block_on(async { tokio::time::sleep(std::time::Duration::from_secs(4)).await });
    for key in [
        "home",
        "shared",
        "favorites",
        "libraries",
        "meetings",
        "bin",
        "files",
    ] {
        send("nav", key);
        eprintln!(
            "live: {key}: {} rows, status {:?}",
            rows().iter().filter(|r| !r.header).count(),
            status()
        );
    }
    send("search", "report");
    eprintln!("live: search: {} rows", rows().len());

    // Everything below happens in the test folder.
    let flow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        send("nav", "files");
        send("new-folder", &folder);
        assert!(has(&folder), "folder was created: {}", status());
        send("open-row", &id_of(&folder));
        let crumbs = app.global::<DriveStore>().get_crumbs();
        assert_eq!(
            crumbs.row_count(),
            2,
            "breadcrumbs have My files and the folder"
        );

        send("new-folder", "sub");
        send("new-doc", "word");
        send("new-doc", "excel");
        send("new-doc", "markdown");
        send("new-doc", "text");
        for n in ["Document.docx", "Book.xlsx", "Document.md", "Document.txt"] {
            assert!(has(n), "{n} was created: {}", status());
        }

        // Upload a file and a folder from disk (a drop does the same).
        let dir = std::env::temp_dir().join(format!("{folder}-src"));
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        std::fs::write(dir.join("note.txt"), "hello from IronTeams").unwrap();
        std::fs::write(dir.join("inner/deep.txt"), "deep").unwrap();
        send("drop", dir.join("note.txt").to_str().unwrap());
        assert!(has("note.txt"), "file uploaded: {}", status());
        send("drop", dir.to_str().unwrap());
        assert!(
            has(&format!("{folder}-src")),
            "folder uploaded: {}",
            status()
        );
        let _ = std::fs::remove_dir_all(&dir);

        // Rename.
        send("select", &id_of("Document.txt"));
        send("rename", "renamed.txt");
        assert!(
            has("renamed.txt") && !has("Document.txt"),
            "renamed: {}",
            status()
        );

        // Favorite and un-favorite.
        send("favorite", &id_of("renamed.txt"));
        assert!(
            rows()
                .iter()
                .find(|r| r.name == "renamed.txt")
                .unwrap()
                .favorite
        );
        send("favorite", &id_of("renamed.txt"));
        assert!(
            !rows()
                .iter()
                .find(|r| r.name == "renamed.txt")
                .unwrap()
                .favorite
        );

        // Preview: text opens inside ironteams.
        send("open-row", &id_of("note.txt"));
        let d = app.global::<DriveStore>();
        assert_eq!(d.get_dialog(), "preview");
        assert_eq!(d.get_preview_text(), "hello from IronTeams");
        send("dialog-close", "");

        // Details and version history.
        send("select", &id_of("Document.docx"));
        send("details", "");
        assert_eq!(d.get_dialog(), "info");
        send("versions", "");
        assert_eq!(d.get_dialog(), "versions");
        eprintln!(
            "live: versions of a new file: {}",
            d.get_versions().row_count()
        );
        send("dialog-close", "");

        // Copy and move through the picker into "sub".
        for (purpose, name) in [("copy", "Book.xlsx"), ("move", "Document.md")] {
            send("select", &id_of(name));
            send(
                if purpose == "copy" {
                    "copy-to"
                } else {
                    "move-to"
                },
                "",
            );
            assert_eq!(d.get_dialog(), "picker");
            let pick = |n: &str| {
                d.get_picker_rows()
                    .iter()
                    .find(|r| r.name == n)
                    .unwrap_or_else(|| panic!("picker has no {n}"))
                    .id
                    .to_string()
            };
            send("pick-open", &pick("My files"));
            send("pick-open", &pick(&folder));
            send("pick-open", &pick("sub"));
            send("pick-choose", "");
            eprintln!("live: {purpose}: {}", status());
        }
        assert!(
            has("Book.xlsx") && !has("Document.md"),
            "move took the file out, copy kept it"
        );
        send("open-row", &id_of("sub"));
        assert!(has("Book.xlsx") && has("Document.md"), "both landed in sub");
        send("crumb", "1");

        // Add shortcut is expected to be refused; the app must say so in plain words.
        send("select", &id_of("sub"));
        send("shortcut", "");
        eprintln!("live: shortcut: {}", status());

        // Delete, see it in the recycle bin, restore one, delete one for good.
        send("select", &id_of("renamed.txt"));
        send("delete", "");
        assert!(!has("renamed.txt"), "deleted: {}", status());
        send("select", &id_of("note.txt"));
        send("delete", "");
        send("nav", "bin");
        let in_bin = |n: &str| {
            rows()
                .iter()
                .any(|r| r.name == n && r.location.contains(folder.as_str()))
        };
        assert!(
            in_bin("renamed.txt") && in_bin("note.txt"),
            "both are in the bin"
        );
        let find = |n: &str| {
            rows()
                .iter()
                .find(|r| r.name == n && r.location.contains(folder.as_str()))
                .unwrap()
                .id
                .to_string()
        };
        send("select", &find("renamed.txt"));
        send("restore", "");
        send("select", &find("note.txt"));
        send("delete-forever", "");
        assert!(!in_bin("note.txt"), "purged: {}", status());
    }));

    // Cleanup, whatever happened above: delete the folder, then purge what it left in the bin.
    send("nav", "files");
    if has(&folder) {
        send("select", &id_of(&folder));
        send("delete", "");
    }
    send("nav", "bin");
    let mine = || -> Vec<String> {
        rows()
            .iter()
            .filter(|r| r.name == folder || r.location.contains(folder.as_str()))
            .map(|r| r.id.to_string())
            .collect()
    };
    let purged = mine();
    for id in &purged {
        send("select", id);
        send("delete-forever", "");
    }
    let left = mine().len();
    eprintln!("live: purged {} bin entries, {left} left", purged.len());
    send("nav", "files");
    assert!(!has(&folder), "the test folder is gone from My files");
    assert_eq!(left, 0, "nothing of the test is left in the recycle bin");
    if let Err(e) = flow {
        std::panic::resume_unwind(e);
    }
}
