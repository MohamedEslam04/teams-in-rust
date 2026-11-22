//! Reaction chips under a message as in Teams: the count only past one, an add-reaction chip after
//! them, and a card below the hovered chip with the reaction's name and who reacted.

use super::*;
use crate::{ReactionItem, Reactor};

fn reactor(name: &str) -> Reactor {
    Reactor {
        name: name.into(),
        initials: crate::data::initials(name).into(),
        color: slint::Color::from_rgb_u8(0x5b, 0x5f, 0xc7),
        img: Default::default(),
        has_img: false,
    }
}

fn reaction(key: &str, emoji: &str, name: &str, mine: bool, people: &[&str]) -> ReactionItem {
    ReactionItem {
        key: key.into(),
        emoji: emoji.into(),
        name: name.into(),
        count: people.len() as i32,
        mine,
        names: people.join(", ").into(),
        people: ModelRc::new(VecModel::from(
            people.iter().map(|p| reactor(p)).collect::<Vec<_>>(),
        )),
    }
}

fn with_reactions(id: &str, reactions: Vec<ReactionItem>) -> Msg {
    let mut m = msg(id, "react to me", false);
    m.reactions = ModelRc::new(VecModel::from(reactions));
    m
}

fn chips(app: &AppWindow) -> Vec<ElementHandle> {
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, "ReactionChip").collect();
    v.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
    v
}

fn card(app: &AppWindow) -> Option<ElementHandle> {
    ElementHandle::find_by_element_type_name(app, "ReactionCard").next()
}

/// Lets the hover bookkeeping (a `changed` handler) run.
fn settle() {
    for _ in 0..4 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(50));
    }
}

fn header(app: &AppWindow) -> String {
    let h = ElementHandle::find_by_element_id(app, "ReactionCard::header")
        .next()
        .expect("the card's header");
    label(&h)
}

fn reactor_names(app: &AppWindow) -> Vec<String> {
    let mut rows: Vec<_> = ElementHandle::find_by_element_type_name(app, "ReactorRow").collect();
    rows.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    rows.iter().map(label).collect()
}

#[test]
fn the_count_shows_only_when_more_than_one_person_reacted() {
    let app = app_with_messages(vec![with_reactions(
        "m1",
        vec![
            reaction("laugh", "😆", "Laugh", false, &["Sara Hassan"]),
            reaction("like", "👍", "Like", true, &["Sara Hassan", "Omar Khaled"]),
        ],
    )]);
    assert_eq!(chips(&app).len(), 2);
    assert_eq!(
        ElementHandle::find_by_element_id(&app, "ReactionChip::count").count(),
        1,
        "only the chip with two people shows a count"
    );
}

#[test]
fn hovering_a_chip_lists_the_reaction_name_and_each_person() {
    let app = app_with_messages(vec![with_reactions(
        "m1",
        vec![reaction(
            "laugh",
            "😆",
            "Laugh",
            true,
            &["Sara Hassan", "Omar Khaled", "Mona Adel"],
        )],
    )]);
    assert!(card(&app).is_none(), "no card before hovering");
    let chip = chips(&app).remove(0);
    hover(&app, &chip);
    settle();
    let c = card(&app).expect("the card while hovering");
    assert_eq!(header(&app), "Laugh");
    assert_eq!(
        reactor_names(&app),
        ["Sara Hassan", "Omar Khaled", "Mona Adel"]
    );

    // Below the chip, a few pixels away, and inside the window.
    let (chip_p, chip_s) = (chip.absolute_position(), chip.size());
    let (p, s) = (c.absolute_position(), c.size());
    assert!(
        p.y >= chip_p.y + chip_s.height && p.y <= chip_p.y + chip_s.height + 12.,
        "card at {} below the chip ending at {}",
        p.y,
        chip_p.y + chip_s.height
    );
    assert!(
        (s.width - 200.).abs() < 1.,
        "about 200px wide, not {}",
        s.width
    );
    let win = app.window().size().to_logical(app.window().scale_factor());
    assert!(p.x >= 0. && p.x + s.width <= win.width);
    assert!(p.y + s.height <= win.height);
}

#[test]
fn leaving_the_chip_closes_the_card() {
    let app = app_with_messages(vec![with_reactions(
        "m1",
        vec![reaction("like", "👍", "Like", false, &["Sara Hassan"])],
    )]);
    hover(&app, &chips(&app)[0]);
    settle();
    assert!(card(&app).is_some());
    let win = app.window().size().to_logical(app.window().scale_factor());
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(win.width - 4., win.height / 2.),
    });
    for _ in 0..10 {
        testing::mock_elapsed_time(std::time::Duration::from_millis(100));
    }
    assert!(card(&app).is_none(), "the card goes once the pointer left");
}

#[test]
fn near_the_bottom_the_card_flips_above_the_chip_and_stays_in_the_window() {
    let many: Vec<String> = (1..=12).map(|n| format!("Person Number{n}")).collect();
    let names: Vec<&str> = many.iter().map(String::as_str).collect();
    let mut msgs: Vec<Msg> = (0..30)
        .map(|n| msg(&format!("m{n}"), "filler", false))
        .collect();
    msgs.push(with_reactions(
        "last",
        vec![reaction("heart", "❤️", "Heart", false, &names)],
    ));
    let app = app_with_messages(msgs);
    for _ in 0..6 {
        settle();
    }
    let win = app.window().size().to_logical(app.window().scale_factor());
    // The lowest chip on screen is the newest message's.
    let chip = chips(&app)
        .into_iter()
        .filter(|c| c.absolute_position().y < win.height)
        .max_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y))
        .expect("the newest message's chip");
    hover(&app, &chip);
    settle();
    let c = card(&app).expect("the card");
    let (chip_p, chip_s) = (chip.absolute_position(), chip.size());
    let (p, s) = (c.absolute_position(), c.size());
    assert!(
        chip_p.y + chip_s.height + 4. + s.height > win.height - 8.,
        "the test needs the chip too low for a card below it"
    );
    assert!(
        p.y + s.height <= chip_p.y,
        "flipped above: card {}..{} over the chip at {}",
        p.y,
        p.y + s.height,
        chip_p.y
    );
    assert!(p.y >= 0. && p.y + s.height <= win.height);
    // More than ten people scroll: ten rows fit, so the twelve are not all laid out at once.
    assert_eq!(header(&app), "Heart");
    assert!(reactor_names(&app).len() <= 12);
    assert_eq!(reactor_names(&app)[0], "Person Number1");
}

#[test]
fn the_add_reaction_chip_follows_the_reactions_and_opens_the_picker() {
    let app = app_with_messages(vec![with_reactions(
        "m1",
        vec![reaction("like", "👍", "Like", false, &["Sara Hassan"])],
    )]);
    let add = ElementHandle::find_by_accessible_label(&app, "Add reaction")
        .next()
        .expect("the add-reaction chip");
    let chip = chips(&app).remove(0);
    assert!(
        add.absolute_position().x >= chip.absolute_position().x + chip.size().width,
        "after the reactions"
    );
    click(&add);
    assert!(app.get_picker_open());
    assert_eq!(app.get_picker_msg_id(), "m1");
}

#[test]
fn a_message_without_reactions_has_no_add_reaction_chip() {
    let app = app_with_messages(vec![msg("m1", "plain", false)]);
    assert_eq!(
        ElementHandle::find_by_accessible_label(&app, "Add reaction").count(),
        0
    );
}
