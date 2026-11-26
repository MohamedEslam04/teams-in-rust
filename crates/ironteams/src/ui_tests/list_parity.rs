//! Teams parity of the chat list and the 1:1 header, driven through the real window: the Quick
//! views section (its rows, what they open, its remembered collapse), channel rows (team picture,
//! private lock, hover card) and the Storyline tab of a 1:1 chat.

use super::responsive::{WIDTHS, assert_fits, assert_shown, on_screen, resize};
use super::*;
use crate::backend::FindCmd;
use crate::backend::engage::PostData;
use crate::backend::storyline::StorylineData;
use crate::{ChatTipStore, FindStore, QuickViewsStore, StorylineStore};
use i_slint_backend_testing::ElementQuery;
use serde_json::json;
use store::{ConvKind, ConversationRow, MessageRow};

const AHMED: &str =
    "19:00000000-0000-0000-0000-000000000001_00000000-0000-0000-0000-000000000002@unq.gbl.spaces";
const TEAM: &str = "19:team1@thread.tacv2";
const LEADS: &str = "19:leads@thread.tacv2";
const DAY: i64 = 24 * 3_600_000;

/// Sends every `FindStore` command to the engine, as `main.rs` does.
fn wire(h: &Harness) {
    let run = h.runner();
    h.app.global::<FindStore>().on_command(move |name, arg| {
        if let Some(c) = FindCmd::parse(&name, &arg) {
            run(Cmd::Find(c));
        }
    });
}

fn fs(app: &AppWindow) -> FindStore<'_> {
    app.global::<FindStore>()
}

fn click(e: &ElementHandle) {
    e.mock_single_click(PointerEventButton::Left);
}

/// The Quick views rows, top to bottom, as their accessible labels.
fn quick_rows(app: &AppWindow) -> Vec<ElementHandle> {
    let mut v: Vec<_> = ElementHandle::find_by_element_type_name(app, "QuickRow").collect();
    v.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    v
}

fn quick_row(app: &AppWindow, label: &str) -> ElementHandle {
    quick_rows(app)
        .into_iter()
        .find(|r| r.accessible_label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no Quick views row {label:?}"))
}

fn channel(id: &str, title: &str, ago: i64) -> ConversationRow {
    ConversationRow {
        id: id.into(),
        kind: ConvKind::Channel,
        title: title.into(),
        team_id: Some(TEAM.into()),
        team_name: Some("Contoso Ltd.".into()),
        last_activity_ms: teams_proto::time::now_ms() - ago,
        last_preview: String::new(),
        unread: false,
        favorite: false,
        read_horizon_ms: 0,
    }
}

fn post(conv: &str, id: &str, ago: i64, text: &str, reactions: usize) -> MessageRow {
    let users: Vec<_> = (0..reactions)
        .map(|i| json!({"mri": format!("8:orgid:r{i}")}))
        .collect();
    MessageRow {
        id: id.into(),
        conversation_id: conv.into(),
        parent_id: None,
        sender_mri: "8:orgid:00000000-0000-0000-0000-000000000003".into(),
        sender_name: "Sara Hassan".into(),
        compose_ms: teams_proto::time::now_ms() - ago,
        edited_ms: None,
        deleted: false,
        message_type: "RichText/Html".into(),
        content_html: format!("<p>{text}</p>"),
        client_message_id: None,
        reactions_json: json!({"emotions": [{"key": "like", "users": users}]}).to_string(),
    }
}

// ---- Quick views ------------------------------------------------------------------------------

#[test]
fn quick_views_rows_open_their_views_and_drafts_filters_the_list() {
    let h = Harness::new();
    wire(&h);
    h.app.global::<QuickViewsStore>().set_open(true);
    // A channel with a popular post, a quiet one and one from last month.
    h.engine.test_store(|st| {
        st.upsert_conversation(&channel("19:launch@thread.tacv2", "Launch", 3_600_000))
            .unwrap();
        for m in [
            post(
                "19:launch@thread.tacv2",
                "p-popular",
                DAY,
                "the launch plan",
                9,
            ),
            post("19:launch@thread.tacv2", "p-quiet", 3_600_000, "lunch?", 0),
            post(
                "19:launch@thread.tacv2",
                "p-old",
                30 * DAY,
                "last month",
                20,
            ),
        ] {
            st.upsert_message(&m).unwrap();
        }
        st.set_draft(AHMED, "half a thought", None).unwrap();
    });
    h.runner()(Cmd::SetFilter("all".into()));

    // The section sits above Favorites with its five rows.
    let labels: Vec<_> = quick_rows(&h.app)
        .iter()
        .map(|r| r.accessible_label().unwrap().to_string())
        .collect();
    assert_eq!(
        labels,
        [
            "Unreads",
            "Mentions",
            "Followed threads",
            "Discover: popular posts from your teams",
            "Drafts"
        ]
    );
    // Its header comes first, then the rows, then Favorites.
    let y = |label: &str| find_label(&h.app, label)[0].absolute_position().y;
    assert!(y("Quick views") < quick_row(&h.app, "Mentions").absolute_position().y);
    assert!(quick_row(&h.app, "Drafts").absolute_position().y < y("Favorites"));
    // The Drafts row counts the chats with a draft.
    assert_eq!(h.app.global::<QuickViewsStore>().get_drafts(), 1);
    assert_eq!(
        quick_row(&h.app, "Drafts").accessible_value().as_deref(),
        Some("1")
    );

    click(&quick_row(&h.app, "Mentions"));
    assert!(fs(&h.app).get_quick_open());
    assert_eq!(fs(&h.app).get_quick_kind(), "mentions");

    click(&quick_row(&h.app, "Followed threads"));
    assert_eq!(fs(&h.app).get_quick_kind(), "followed");

    // Discover: this week's channel posts, the most reacted first, with an honest label.
    click(&quick_row(
        &h.app,
        "Discover: popular posts from your teams",
    ));
    assert_eq!(fs(&h.app).get_quick_kind(), "discover");
    let rows: Vec<_> = fs(&h.app).get_quick_rows().iter().collect();
    let texts: Vec<_> = rows.iter().map(|r| r.snippet_plain.to_string()).collect();
    let popular = texts.iter().position(|t| t.contains("the launch plan"));
    let quiet = texts.iter().position(|t| t.contains("lunch?"));
    assert!(popular.is_some() && quiet.is_some(), "{texts:?}");
    assert!(popular < quiet, "the post with reactions ranks first");
    assert!(
        !texts.iter().any(|t| t.contains("last month")),
        "7 days only"
    );
    assert!(
        rows[popular.unwrap()].subtitle.ends_with("· 9 reactions"),
        "{}",
        rows[popular.unwrap()].subtitle
    );
    assert!(
        ElementQuery::from_root(&h.app)
            .match_descendants()
            .match_type_name("Text")
            .find_all()
            .iter()
            .any(|t| t
                .accessible_label()
                .is_some_and(|l| l.contains("not from Teams' Discover feed"))),
        "the view says where its posts come from"
    );

    // Drafts is the drafts filter: only chats with a draft, and the quick view closes.
    click(&quick_row(&h.app, "Drafts"));
    assert!(!fs(&h.app).get_quick_open());
    assert_eq!(h.app.get_filter(), "drafts");
    let ids: Vec<_> = h.chats().iter().map(|c| c.id.to_string()).collect();
    assert_eq!(ids, [AHMED]);
    click(&quick_row(&h.app, "Drafts"));
    assert_eq!(h.app.get_filter(), "all");
}

#[test]
fn collapsing_quick_views_hides_its_rows_and_is_remembered() {
    let app = app_with_messages(vec![]);
    assert_eq!(quick_rows(&app).len(), 5);
    let header = ElementHandle::find_by_element_id(&app, "QuickViewsSection::qv-header")
        .next()
        .expect("Quick views header");
    click(&header);
    assert!(!app.global::<QuickViewsStore>().get_open());
    assert!(quick_rows(&app).is_empty());

    // The window state keeps it, and a new window starts collapsed.
    let mut state = session::window_state::WindowState::default();
    crate::window_state::remember_quick_views(&app, &mut state);
    assert!(!crate::window_state::quick_views_open(&state));
    let again = app_with_messages(vec![]);
    assert_eq!(quick_rows(&again).len(), 5, "open before the restore");
    crate::window_state::restore_quick_views(&again, &state);
    assert!(quick_rows(&again).is_empty());
    click(
        &ElementHandle::find_by_element_id(&again, "QuickViewsSection::qv-header")
            .next()
            .unwrap(),
    );
    assert_eq!(quick_rows(&again).len(), 5);
}

// ---- Channel rows -----------------------------------------------------------------------------

fn team_entry() -> teams_proto::teams::TeamEntry {
    let v = json!({"teams": [{
        "id": TEAM, "displayName": "Contoso Ltd.",
        "teamSiteInformation": {"groupId": "g-1"}, "isFavorite": true,
        "channels": [
            {"id": TEAM, "displayName": "General", "isGeneral": true, "isFavorite": true},
            {"id": "19:tech@thread.tacv2", "displayName": "tech only", "isFavorite": true},
            {"id": LEADS, "displayName": "Leads", "channelType": 1, "isFavorite": true}
        ]
    }]});
    teams_proto::teams::parse_teams(&v).remove(0)
}

/// Rests the pointer on the middle of an element.
fn hover(app: &AppWindow, e: &ElementHandle) {
    let (p, s) = (e.absolute_position(), e.size());
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.),
    });
}

/// The hover card of a channel row, as (title, "Private" | "Standard").
fn hover_card(app: &AppWindow) -> Option<(String, String)> {
    ElementQuery::from_root(app)
        .match_descendants()
        .match_accessible_role(i_slint_backend_testing::AccessibleRole::Text)
        .find_all()
        .into_iter()
        .find_map(|e| {
            let d = e.accessible_description()?;
            (d == "Private" || d == "Standard").then(|| {
                (
                    e.accessible_label().unwrap_or_default().to_string(),
                    d.to_string(),
                )
            })
        })
}

#[test]
fn a_channel_row_shows_its_team_picture_a_private_lock_and_a_hover_card() {
    let h = Harness::new();
    // Every channel row in view (scrolled-out rows are not found).
    h.app.window().set_size(LogicalSize::new(1200., 1000.));
    h.engine.test_store(|st| {
        st.upsert_conversation(&channel(LEADS, "Leads", 60_000))
            .unwrap()
    });
    h.engine.test_set_teams(&[team_entry()]);
    h.engine
        .test_set_avatar("team:g-1", crate::data::Pixels::new(4, 4));
    h.runner()(Cmd::SetFilter("all".into()));

    let rows = h.chats();
    let leads = rows.iter().find(|c| c.id == LEADS).expect("Leads row");
    let tech = rows
        .iter()
        .find(|c| c.id == "19:tech@thread.tacv2")
        .expect("tech row");
    assert!(leads.private && !tech.private);
    assert!(
        leads.has_avatar && tech.has_avatar,
        "both show the team picture"
    );
    // Chats keep their own photo or initials.
    assert!(rows.iter().filter(|c| !c.channel).all(|c| !c.private));
    // Drawn: the team pictures and one lock, after the private channel's name.
    assert_eq!(find_label(&h.app, "Team picture").len(), 3);
    let lock = find_label(&h.app, "Private channel");
    assert_eq!(lock.len(), 1);
    let name = find_label(&h.app, "Leads")
        .into_iter()
        .find(|e| e.absolute_position().y < lock[0].absolute_position().y + 20.)
        .expect("the name");
    assert!(
        (lock[0].absolute_position().x - (name.absolute_position().x + name.size().width)).abs()
            < 10.,
        "the lock follows the name"
    );

    // Resting on the row shows its name and kind after a moment.
    hover(&h.app, &name);
    // Change handlers run with the next tick.
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert_eq!(h.app.global::<ChatTipStore>().get_id(), LEADS);
    assert_eq!(hover_card(&h.app), None, "not at once");
    testing::mock_elapsed_time(std::time::Duration::from_millis(600));
    assert_eq!(
        hover_card(&h.app),
        Some(("Leads".to_string(), "Private".to_string()))
    );
    let tech_name = find_label(&h.app, "tech only")
        .into_iter()
        .next()
        .expect("tech only row");
    hover(&h.app, &tech_name);
    testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    testing::mock_elapsed_time(std::time::Duration::from_millis(600));
    assert_eq!(
        hover_card(&h.app),
        Some(("tech only".to_string(), "Standard".to_string()))
    );
}

/// The Teams view draws the team's picture on its row (initials until it is known) and a lock
/// after a private channel's name, as the chat list does.
#[test]
fn the_teams_view_shows_the_team_picture_and_a_lock_after_private_channel_names() {
    let h = Harness::new();
    h.app.window().set_size(LogicalSize::new(1200., 1000.));
    h.app.set_view("teams".into());
    h.engine.test_store(|st| {
        st.upsert_conversation(&channel(LEADS, "Leads", 60_000))
            .unwrap()
    });
    h.engine.test_set_teams(&[team_entry()]);
    h.runner()(Cmd::SetFilter("all".into()));
    let teams = h.app.get_teams();
    let team = teams.row_data(0).expect("a team row");
    assert!(!team.has_avatar, "no picture yet: the initials show");
    assert!(find_label(&h.app, "Team picture").is_empty());
    let private: Vec<_> = team
        .channels
        .iter()
        .filter(|c| c.private)
        .map(|c| c.name.to_string())
        .collect();
    assert_eq!(private, ["Leads"], "only the private channel gets a lock");

    // The picture arrives: the row shows it instead of the initials.
    h.engine
        .test_set_avatar("team:g-1", crate::data::Pixels::new(4, 4));
    h.engine.test_set_teams(&[team_entry()]);
    h.runner()(Cmd::SetFilter("all".into()));
    assert!(h.app.get_teams().row_data(0).unwrap().has_avatar);
    assert_eq!(find_label(&h.app, "Team picture").len(), 1);
    let lock = find_label(&h.app, "Private channel");
    assert_eq!(lock.len(), 1);
    let name = find_label(&h.app, "Leads")
        .into_iter()
        .find(|e| (e.absolute_position().y - lock[0].absolute_position().y).abs() < 20.)
        .expect("the channel name");
    assert!(
        (lock[0].absolute_position().x - (name.absolute_position().x + name.size().width)).abs()
            < 10.,
        "the lock follows the name"
    );
}

#[test]
fn a_long_private_channel_name_elides_before_its_lock() {
    for width in [220., 320.] {
        let app = app_with_messages(vec![]);
        app.set_sidebar_width(width);
        app.set_chats_channels(ModelRc::new(VecModel::from(vec![crate::ChatRow {
            id: "c".into(),
            title: super::overflow::LONG.into(),
            channel: true,
            private: true,
            section: "channels".into(),
            ..Default::default()
        }])));
        let rows: Vec<_> = ElementHandle::find_by_element_type_name(&app, "ChatItem").collect();
        assert_eq!(rows.len(), 1);
        super::overflow::assert_texts_inside(&rows[0], "private channel row");
        let lock = &find_label(&app, "Private channel")[0];
        let row_end = rows[0].absolute_position().x + rows[0].size().width;
        let lock_end = lock.absolute_position().x + lock.size().width;
        assert!(
            lock_end <= row_end + 0.5,
            "list {width}px: the lock ends at {lock_end}, the row at {row_end}"
        );
    }
}

// ---- Storyline tab -----------------------------------------------------------------------------

fn tab(app: &AppWindow, label: &str) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(app, label)
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Tab))
}

#[test]
fn a_one_to_one_chat_has_a_storyline_tab_with_the_card_and_posting_disabled() {
    let h = Harness::new();
    wire(&h);
    // A group chat has no Storyline tab.
    h.select(BACKEND);
    assert!(tab(&h.app, "Shared").is_some());
    assert!(tab(&h.app, "Storyline").is_none());
    // The self chat and a 1:1 chat have one.
    h.select(NOTES);
    assert!(tab(&h.app, "Storyline").is_some());
    h.select(AHMED);
    click(&tab(&h.app, "Storyline").expect("Storyline tab"));
    assert_eq!(fs(&h.app).get_chat_tab(), "storyline");
    assert_eq!(
        tab(&h.app, "Storyline").unwrap().accessible_checked(),
        Some(true)
    );
    // The card shows at once from what IronTeams knows; offline, the tab says why counts are missing.
    let sl = h.app.global::<StorylineStore>();
    assert_eq!(sl.get_name(), "Ahmed Ali");
    assert_eq!(sl.get_followers(), -1);
    assert!(sl.get_note().contains("offline"), "{}", sl.get_note());
    assert!(!find_label(&h.app, "Followers: unknown").is_empty());

    // With Engage's answer (fixture data): the counts and the posts.
    UiHandle::new(h.app.as_weak()).storyline(StorylineData {
        name: "Ahmed Ali".into(),
        initials: "AA".into(),
        followers: Some(12),
        following: Some(3),
        posts_count: Some(7),
        posts: vec![PostData {
            id: "1".into(),
            sender: "Ahmed Ali".into(),
            initials: "AA".into(),
            time: "Mon".into(),
            text: "Shipped the new onboarding flow".into(),
            likes: 4,
            replies: 2,
            ..Default::default()
        }],
        ..Default::default()
    });
    for label in ["Followers: 12", "Following: 3", "Posts: 7"] {
        assert_eq!(find_label(&h.app, label).len(), 1, "{label}");
    }
    assert!(!find_label(&h.app, "Shipped the new onboarding flow").is_empty());
    assert!(!find_label(&h.app, "Recent Viva Engage posts by Ahmed Ali").is_empty());

    // Every write control is there but disabled, and says why.
    for label in [
        "Share thoughts…",
        "Discussion",
        "Question",
        "Praise",
        "Poll",
    ] {
        let e = find_label(&h.app, label)
            .into_iter()
            .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
            .unwrap_or_else(|| panic!("{label}"));
        assert_eq!(e.accessible_enabled(), Some(false), "{label}");
        assert_eq!(
            e.accessible_description().as_deref(),
            Some("Posting to Storyline isn't available yet"),
            "{label}"
        );
    }

    // Back to the conversation; another chat starts on its Chat tab.
    click(&tab(&h.app, "Chat").unwrap());
    assert_eq!(fs(&h.app).get_chat_tab(), "chat");
    assert!(find_label(&h.app, "Followers: 12").is_empty());
}

#[test]
fn quick_views_and_the_storyline_tab_fit_narrow_windows() {
    for width in WIDTHS {
        let h = Harness::new();
        wire(&h);
        h.app.global::<QuickViewsStore>().set_open(true);
        resize(&h.app, width);
        // The list (one pane at 760) with the Quick views.
        assert_shown(&h.app, "Followed threads", None, width, "Quick views");
        assert_fits(&h.app, width, "chat list");
        h.select(AHMED);
        let storyline = tab(&h.app, "Storyline").expect("Storyline tab");
        assert!(on_screen(&storyline, width), "the tab at {width}px");
        assert_fits(&h.app, width, "1:1 chat");
        click(&storyline);
        assert_shown(&h.app, "Followers: unknown", None, width, "Storyline");
        assert_shown(&h.app, "Poll", None, width, "Storyline");
        assert_fits(&h.app, width, "Storyline");
    }
}
