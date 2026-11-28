//! The Teams management UI (plan `plans/teams-and-channels.md`, T1.1 to T3.1) driven through the
//! real window. Every command the UI sends is recorded as `"<name> <argument>"` (the unit
//! separator shown as `|`). `TeamsStore` is wired to the real engine only for the commands that
//! need no network (filter, sort, search, expand); the engine has no network, so the rest are
//! checked as the commands the UI sends.

use super::*;
use crate::backend::TeamsCmd;
use crate::{AiAccessStore, TCommunity, TDeleted, TPerson, TPost, TSetting, TTag, TeamsStore};
use teams_proto::teams::{Access, ChannelEntry, ChannelKind, Role, TeamEntry};

fn ts(app: &AppWindow) -> TeamsStore<'_> {
    app.global::<TeamsStore>()
}

fn channel(id: &str, name: &str, kind: ChannelKind, shown: bool) -> ChannelEntry {
    ChannelEntry {
        id: id.into(),
        name: name.into(),
        kind,
        is_general: name == "General",
        is_shown: shown,
        is_pinned: false,
        is_muted: false,
        is_archived: false,
        is_deleted: false,
        is_member: true,
        is_moderator: false,
        role: Role::Owner,
        created_ms: 0,
    }
}

fn team(id: &str, name: &str, role: Role, people: u32, channels: Vec<ChannelEntry>) -> TeamEntry {
    TeamEntry {
        id: id.into(),
        group_id: format!("g-{id}"),
        name: name.into(),
        description: String::new(),
        access: Access::Private,
        is_org_wide: false,
        is_shown: true,
        is_collapsed: false,
        is_archived: false,
        is_deleted: false,
        role,
        member_count: people,
        owner_count: 1,
        email: String::new(),
        channels,
    }
}

fn entries() -> Vec<TeamEntry> {
    vec![
        team(
            "t1",
            "Zebra",
            Role::Owner,
            9,
            vec![
                channel("t1", "General", ChannelKind::Standard, true),
                channel("c2", "Roadmap", ChannelKind::Standard, false),
                channel("c3", "Secret", ChannelKind::Private, true),
            ],
        ),
        team(
            "t2",
            "Alpha",
            Role::Member,
            30,
            vec![channel("t2", "General", ChannelKind::Standard, true)],
        ),
    ]
}

/// The real engine behind the window, with the page open on `entries()`. `TeamsStore` commands are
/// recorded; the ones that need no network also run in the engine.
fn engine_page() -> (Harness, Log) {
    let h = Harness::new();
    ts(&h.app).set_open(true);
    let log = Log::default();
    let (l, run) = (log.clone(), h.runner());
    ts(&h.app).on_command(move |n, a| {
        l.borrow_mut()
            .push(format!("{n} {}", a.replace('\u{1f}', "|")));
        if matches!(n.as_str(), "filter" | "sort" | "query" | "expand")
            && let Some(c) = TeamsCmd::parse(&n, &a)
        {
            run(Cmd::Teams(c));
        }
    });
    h.engine.manage_set_entries(entries());
    (h, log)
}

/// Elements with this accessible label. A control and the text inside it share a label, so the
/// text is left out when something else has it too.
fn by_label(app: &AppWindow, label: &str) -> Vec<ElementHandle> {
    use testing::AccessibleRole;
    let mut v: Vec<ElementHandle> = ElementHandle::find_by_accessible_label(app, label).collect();
    if v.iter()
        .any(|e| e.accessible_role() != Some(AccessibleRole::Text))
    {
        v.retain(|e| e.accessible_role() != Some(AccessibleRole::Text));
    }
    v
}

/// The one element with this accessible label.
fn el(app: &AppWindow, label: &str) -> ElementHandle {
    let mut v = by_label(app, label);
    assert_eq!(
        v.len(),
        1,
        "expected one element labelled {label:?}, found {}",
        v.len()
    );
    v.remove(0)
}

fn has(app: &AppWindow, label: &str) -> bool {
    !by_label(app, label).is_empty()
}

fn tap(app: &AppWindow, label: &str) {
    click(&el(app, label));
}

fn team_rows(app: &AppWindow) -> Vec<String> {
    let mut heads: Vec<ElementHandle> =
        ElementHandle::find_by_element_id(app, "TeamLine::head").collect();
    heads.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    heads.iter().map(label).collect()
}

fn text_in(app: &AppWindow, field: &str, text: &str) {
    click(&el(app, field));
    type_text(app, text);
}

fn last(log: &Log) -> String {
    log.borrow().last().cloned().unwrap_or_default()
}

fn sent(log: &Log, cmd: &str) -> bool {
    log.borrow().iter().any(|c| c == cmd)
}

fn people(v: &[(&str, &str, &str, bool)]) -> ModelRc<TPerson> {
    ModelRc::new(VecModel::from(
        v.iter()
            .map(|(mri, name, role, me)| TPerson {
                mri: (*mri).into(),
                name: (*name).into(),
                role: (*role).into(),
                me: *me,
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    ))
}

// ---- T1.1 the page ----------------------------------------------------------------

#[test]
fn page_lists_teams_with_counts_and_filters_sorts_and_searches() {
    let (h, log) = engine_page();
    // A to Z by default; the row says privacy, role, people and channels.
    assert_eq!(team_rows(&h.app), ["Alpha", "Zebra"]);
    assert_eq!(
        el(&h.app, "Zebra").accessible_description().unwrap(),
        "Private, Owner, 9 people, 3 channels"
    );

    tap(&h.app, "Teams you own");
    assert_eq!(team_rows(&h.app), ["Zebra"]);
    assert!(sent(&log, "filter owned"));
    tap(&h.app, "All");
    assert_eq!(team_rows(&h.app), ["Alpha", "Zebra"]);
    tap(&h.app, "Sort: Z to A");
    assert_eq!(team_rows(&h.app), ["Zebra", "Alpha"]);
    tap(&h.app, "Sort: Most people");
    assert_eq!(team_rows(&h.app), ["Alpha", "Zebra"]);
    tap(&h.app, "Hidden");
    assert_eq!(team_rows(&h.app), ["Zebra"], "a team with a hidden channel");

    // A search that matches a channel opens its team and lists only that channel.
    tap(&h.app, "All");
    text_in(&h.app, "Search teams", "road");
    assert_eq!(team_rows(&h.app), ["Zebra"]);
    assert!(has(&h.app, "Roadmap") && !has(&h.app, "Secret"));
    assert!(sent(&log, "query road"));
}

#[test]
fn expanding_a_team_shows_all_its_channels_with_the_hidden_one_flagged() {
    let (h, _) = engine_page();
    assert!(
        !has(&h.app, "Roadmap"),
        "channels stay closed until the team opens"
    );
    tap(&h.app, "Zebra");
    let flags: Vec<String> = ["General", "Roadmap", "Secret"]
        .iter()
        .map(|n| el(&h.app, n).accessible_description().unwrap().to_string())
        .collect();
    assert_eq!(flags, ["", "Hidden ", "private"]);
    tap(&h.app, "Zebra");
    assert!(!has(&h.app, "Roadmap"), "a second click closes it");
}

#[test]
fn tabs_load_their_data_and_analytics_counts_the_list() {
    let (h, log) = engine_page();
    tap(&h.app, "Analytics");
    assert_eq!(ts(&h.app).get_tab(), "analytics");
    assert!(has(&h.app, "Teams: 2"));
    assert!(has(&h.app, "Hidden channels: 1"));
    assert!(has(&h.app, "Private and shared channels: 1"));
    tap(&h.app, "Pending invites");
    assert_eq!(ts(&h.app).get_tab(), "invites");
    assert!(has(&h.app, "You have no pending join requests."));
    tap(&h.app, "Discover");
    assert!(sent(&log, "discover "), "Discover loads suggestions");
    tap(&h.app, "Communities");
    assert!(sent(&log, "communities "));
}

// ---- T1.2 discover and join -------------------------------------------------------

fn discover_row(group: &str, name: &str, access: &str, requested: bool) -> crate::TDiscover {
    crate::TDiscover {
        group: group.into(),
        name: name.into(),
        people: 12,
        access: access.into(),
        requested,
        ..Default::default()
    }
}

#[test]
fn discover_lists_public_teams_and_joins_requests_or_cancels() {
    let (h, log) = engine_page();
    tap(&h.app, "Discover");
    ts(&h.app).set_discover(ModelRc::new(VecModel::from(vec![
        discover_row("g1", "Open team", "Public", false),
        discover_row("g2", "Closed team", "Private", true),
        discover_row("g3", "Gated team", "Private", false),
    ])));
    tap(&h.app, "Join team");
    assert_eq!(last(&log), "join g1");
    tap(&h.app, "Request to join");
    assert_eq!(last(&log), "join g3");
    tap(&h.app, "Cancel request");
    assert_eq!(last(&log), "join-cancel g2");
    text_in(&h.app, "Search public teams", "oper");
    enter(&h.app);
    assert_eq!(last(&log), "discover oper");
}

#[test]
fn pending_invites_lists_requests_with_cancel() {
    let (h, log) = engine_page();
    tap(&h.app, "Pending invites");
    ts(&h.app).set_requests(ModelRc::new(VecModel::from(vec![discover_row(
        "g9",
        "Waiting team",
        "Private",
        true,
    )])));
    tap(&h.app, "Cancel request");
    assert_eq!(last(&log), "join-cancel g9");
}

#[test]
fn join_with_a_code_sends_the_code_and_shows_the_answer() {
    let (h, log) = engine_page();
    tap(&h.app, "Create team  ▾");
    tap(&h.app, "Join a team with a code");
    assert_eq!(ts(&h.app).get_dialog(), "join");
    assert_eq!(last(&log), "dialog join||");
    tap(&h.app, "Join team");
    assert!(
        !log.borrow().iter().any(|c| c.starts_with("join-code")),
        "an empty code sends nothing"
    );
    text_in(&h.app, "Team code", "abc1234");
    tap(&h.app, "Join team");
    assert_eq!(last(&log), "join-code abc1234");
    ts(&h.app).set_join_info("You joined Demo".into());
    assert!(has(&h.app, "You joined Demo"));
    tap(&h.app, "Cancel");
    assert_eq!(last(&log), "dialog-close ");
    assert_eq!(ts(&h.app).get_dialog(), "");
}

// ---- T1.3 channel list control ----------------------------------------------------

fn channel_menu(h: &Harness, channel: &str) {
    if !has(&h.app, channel) {
        tap(&h.app, "Zebra");
    }
    tap(&h.app, &format!("More options for {channel}"));
}

#[test]
fn channel_menu_shows_hides_and_pins() {
    let (h, log) = engine_page();
    channel_menu(&h, "Roadmap");
    assert!(
        has(&h.app, "Show") && !has(&h.app, "Hide"),
        "a hidden channel offers Show"
    );
    tap(&h.app, "Show");
    assert_eq!(last(&log), "chan-show c2|1");
    assert!(!has(&h.app, "Manage notifications"), "the menu closes");

    channel_menu(&h, "Secret");
    assert!(has(&h.app, "Hide") && has(&h.app, "Pin"));
    tap(&h.app, "Pin");
    assert_eq!(last(&log), "chan-pin c3|1");
    channel_menu(&h, "Secret");
    tap(&h.app, "Hide");
    assert_eq!(last(&log), "chan-show c3|0");

    // A private channel can be left; a standard one cannot.
    channel_menu(&h, "Secret");
    assert!(has(&h.app, "Leave channel") && has(&h.app, "Add members"));
    tap(&h.app, "Leave channel");
    assert_eq!(ts(&h.app).get_dialog(), "confirm");
    tap(&h.app, "Yes");
    assert_eq!(last(&log), "leave-channel c3");
    channel_menu(&h, "Roadmap");
    assert!(!has(&h.app, "Leave channel"));
    assert!(has(&h.app, "Show for everyone automatically"));
    tap(&h.app, "Show for everyone automatically");
    assert_eq!(last(&log), "chan-default c2|1");
}

#[test]
fn pinned_and_hidden_state_comes_from_the_engine_rows() {
    let (h, _) = engine_page();
    let mut e = entries();
    e[0].channels[2].is_pinned = true;
    h.engine.manage_set_entries(e);
    tap(&h.app, "Zebra");
    assert_eq!(
        el(&h.app, "Secret").accessible_description().unwrap(),
        "Pinned private"
    );
}

#[test]
fn team_menu_goes_to_the_team_hides_it_and_leaving_asks_first() {
    let (h, log) = engine_page();
    let went = Log::default();
    let w = went.clone();
    h.app
        .on_select_channel(move |id, title| w.borrow_mut().push(format!("{id} {title}")));
    // The app's `go` handler closes the page and opens the channel; the test window has the real
    // app.slint, so this is the real wiring.
    tap(&h.app, "More options for Zebra");
    tap(&h.app, "Go to team");
    assert!(!ts(&h.app).get_open(), "the page closes");
    assert_eq!(*went.borrow(), ["t1 Zebra"]);
    assert_eq!(h.app.get_view(), "teams");

    ts(&h.app).set_open(true);
    tap(&h.app, "More options for Alpha");
    tap(&h.app, "Hide team");
    assert_eq!(last(&log), "team-show t2|0");

    tap(&h.app, "More options for Alpha");
    tap(&h.app, "Leave the team");
    assert_eq!(ts(&h.app).get_dialog(), "confirm");
    assert!(!log.borrow().iter().any(|c| c.starts_with("leave-team")));
    tap(&h.app, "Yes");
    assert_eq!(last(&log), "leave-team t2");
    assert_eq!(ts(&h.app).get_dialog(), "");

    // Only owners see archive and delete.
    tap(&h.app, "More options for Alpha");
    assert!(!has(&h.app, "Archive team") && !has(&h.app, "Delete the team"));
}

#[test]
fn hidden_channels_and_teams_leave_the_sidebar_and_pinned_come_first() {
    let h = Harness::new();
    let names = |h: &Harness| -> Vec<String> {
        let mut v = vec![];
        for t in h.app.get_teams().iter() {
            v.extend(t.channels.iter().map(|c| c.name.to_string()));
        }
        v
    };
    let render = |h: &Harness| h.rt.block_on(async { h.engine.render_dirty() });
    render(&h);
    assert_eq!(names(&h), ["General", "tech only"]);
    let mut t = team(
        "19:team1@thread.tacv2",
        "Contoso Ltd.",
        Role::Owner,
        5,
        vec![
            channel(
                "19:team1@thread.tacv2",
                "General",
                ChannelKind::Standard,
                true,
            ),
            channel(
                "19:tech@thread.tacv2",
                "tech only",
                ChannelKind::Standard,
                false,
            ),
        ],
    );
    h.engine.manage_set_entries(vec![t.clone()]);
    render(&h);
    assert_eq!(names(&h), ["General"], "a hidden channel is not listed");
    t.is_shown = false;
    h.engine.manage_set_entries(vec![t]);
    render(&h);
    assert!(
        h.app.get_teams().iter().next().is_none(),
        "a hidden team is not listed"
    );
}

// ---- T2.1 new channel -------------------------------------------------------------

#[test]
fn new_channel_dialog_sends_name_description_layout_and_default() {
    let (h, log) = engine_page();
    tap(&h.app, "More options for Zebra");
    tap(&h.app, "New channel");
    assert_eq!(ts(&h.app).get_dialog(), "new-channel");
    assert_eq!(last(&log), "dialog new-channel|t1|");
    tap(&h.app, "Create");
    assert!(
        !log.borrow().iter().any(|c| c.starts_with("channel-create")),
        "a nameless channel is not created"
    );
    ts(&h.app).set_d_team("t1".into());
    text_in(&h.app, "Channel name", "Launch");
    text_in(&h.app, "Description", "All about it");
    tap(&h.app, "Threaded chat");
    tap(&h.app, "Show for everyone automatically");
    tap(&h.app, "Create");
    assert_eq!(
        last(&log),
        "channel-create t1|Launch|All about it|standard|1|1"
    );
}

#[test]
fn private_channel_asks_for_the_roster_and_picks_people() {
    let (h, log) = engine_page();
    tap(&h.app, "More options for Zebra");
    tap(&h.app, "New channel");
    ts(&h.app).set_d_team("t1".into());
    tap(&h.app, "Private");
    assert_eq!(last(&log), "roster t1");
    ts(&h.app).set_people(people(&[
        ("8:orgid:me", "Me", "Owner", true),
        ("8:orgid:a", "Sara", "Member", false),
    ]));
    text_in(&h.app, "Channel name", "Quiet");
    tap(&h.app, "Sara");
    assert_eq!(last(&log), "pick 8:orgid:a");
    tap(&h.app, "Create");
    assert_eq!(last(&log), "channel-create t1|Quiet||private|0|0");
    // Shared works the same way.
    tap(&h.app, "Shared");
    tap(&h.app, "Create");
    assert_eq!(last(&log), "channel-create t1|Quiet||shared|0|0");
}

// ---- T2.2 edit, delete, restore, archive, moderation ------------------------------

#[test]
fn edit_channel_sends_the_new_name_and_description() {
    let (h, log) = engine_page();
    channel_menu(&h, "Roadmap");
    // The engine fills the form when it opens the dialog.
    ts(&h.app).set_d_name("Roadmap".into());
    tap(&h.app, "Edit channel");
    assert_eq!(last(&log), "dialog edit-channel|t1|c2");
    ts(&h.app).set_d_team("t1".into());
    ts(&h.app).set_d_channel("c2".into());
    text_in(&h.app, "Description", "New text");
    tap(&h.app, "Save");
    assert_eq!(last(&log), "channel-edit t1|c2|Roadmap|New text");
}

#[test]
fn general_cannot_be_edited_archived_or_deleted() {
    let (h, _) = engine_page();
    channel_menu(&h, "General");
    assert!(has(&h.app, "Manage channel") && has(&h.app, "Manage notifications"));
    assert!(
        !has(&h.app, "Edit channel")
            && !has(&h.app, "Delete channel")
            && !has(&h.app, "Archive channel")
    );
}

#[test]
fn delete_and_archive_ask_first_and_deleted_channels_can_be_restored() {
    let (h, log) = engine_page();
    channel_menu(&h, "Roadmap");
    tap(&h.app, "Delete channel");
    assert_eq!(ts(&h.app).get_dialog(), "confirm");
    assert!(has(
        &h.app,
        "Delete Roadmap? You can restore it for 21 days."
    ));
    assert!(!log.borrow().iter().any(|c| c.starts_with("channel-delete")));
    tap(&h.app, "Yes");
    assert_eq!(last(&log), "channel-delete t1|c2");

    channel_menu(&h, "Roadmap");
    tap(&h.app, "Archive channel");
    tap(&h.app, "Yes");
    assert_eq!(last(&log), "channel-archive c2|1");

    tap(&h.app, "More options for Zebra");
    tap(&h.app, "Deleted channels");
    assert_eq!(last(&log), "dialog deleted|t1|");
    ts(&h.app).set_deleted(ModelRc::new(VecModel::from(vec![TDeleted {
        id: "c9".into(),
        name: "Old plans".into(),
        when: "Deleted Mon".into(),
    }])));
    assert!(has(&h.app, "Old plans"));
    ts(&h.app).set_d_team("t1".into());
    tap(&h.app, "Restore");
    assert_eq!(last(&log), "channel-restore t1|c9");
}

#[test]
fn an_archived_channel_offers_restore_from_the_archive() {
    let (h, log) = engine_page();
    let mut e = entries();
    e[0].channels[1].is_archived = true;
    h.engine.manage_set_entries(e);
    channel_menu(&h, "Roadmap");
    tap(&h.app, "Restore from the archive");
    tap(&h.app, "Yes");
    assert_eq!(last(&log), "channel-archive c2|0");
}

#[test]
fn moderation_dialog_changes_who_can_post_reply_and_pin() {
    let (h, log) = engine_page();
    channel_menu(&h, "Roadmap");
    tap(&h.app, "Manage channel");
    assert_eq!(last(&log), "dialog moderation|t1|c2");
    ts(&h.app).set_allow_bots(true);
    ts(&h.app).set_allow_connectors(true);
    let owners_only = by_label(&h.app, "Owners only");
    assert_eq!(owners_only.len(), 3, "post, reply and pin");
    click(&owners_only[0]);
    assert_eq!(last(&log), "mod-set post|Moderators");
    click(&owners_only[1]);
    assert_eq!(last(&log), "mod-set reply|Moderators");
    click(&owners_only[2]);
    assert_eq!(last(&log), "mod-set pin|Moderators");
    tap(&h.app, "Allow bots to post");
    assert_eq!(last(&log), "mod-set bots|0");
    tap(&h.app, "Allow connectors to post");
    assert_eq!(last(&log), "mod-set connectors|0");
}

// ---- T2.3 members ------------------------------------------------------------------

#[test]
fn members_dialog_changes_roles_removes_and_adds_people() {
    let (h, log) = engine_page();
    tap(&h.app, "More options for Zebra");
    tap(&h.app, "Add members");
    assert_eq!(last(&log), "dialog members|t1|");
    ts(&h.app).set_d_team("t1".into());
    ts(&h.app).set_d_owner(true);
    ts(&h.app).set_people(people(&[
        ("8:orgid:me", "Me", "Owner", true),
        ("8:orgid:s", "Sara", "Member", false),
        ("8:orgid:o", "Omar", "Owner", false),
        ("8:orgid:g", "Gus", "Guest", false),
    ]));
    assert!(has(&h.app, "4 people"));
    tap(&h.app, "Make owner");
    assert_eq!(last(&log), "member-role 8:orgid:s|owner");
    tap(&h.app, "Make member");
    assert_eq!(last(&log), "member-role 8:orgid:o|member");
    let remove = by_label(&h.app, "Remove");
    assert_eq!(remove.len(), 3, "not yourself");
    click(&remove[2]);
    assert_eq!(last(&log), "member-remove 8:orgid:g");

    text_in(&h.app, "Add people", "di");
    assert_eq!(last(&log), "member-search di");
    ts(&h.app).set_candidates(people(&[("8:orgid:d", "Dina", "", false)]));
    tap(&h.app, "Add");
    assert_eq!(last(&log), "member-add 8:orgid:d|member");
}

#[test]
fn a_plain_member_cannot_change_roles_or_remove_team_members() {
    let (h, _) = engine_page();
    tap(&h.app, "More options for Alpha");
    tap(&h.app, "Add members");
    ts(&h.app).set_d_owner(false);
    ts(&h.app).set_people(people(&[("8:orgid:s", "Sara", "Member", false)]));
    assert!(!has(&h.app, "Make owner") && !has(&h.app, "Remove"));
}

#[test]
fn a_standard_channel_shares_its_teams_members() {
    let (h, log) = engine_page();
    channel_menu(&h, "Roadmap");
    // Standard channels have no Add members entry; private ones do.
    assert!(!has(&h.app, "Add members"));
    tap(&h.app, "Manage channel tags");
    assert_eq!(last(&log), "dialog tags|t1|c2");
}

// ---- T2.4 tags ----------------------------------------------------------------------

#[test]
fn tags_dialog_creates_selects_adds_removes_and_deletes() {
    let (h, log) = engine_page();
    tap(&h.app, "More options for Zebra");
    tap(&h.app, "Manage team tags");
    assert_eq!(last(&log), "dialog tags|t1|");
    ts(&h.app).set_tags(ModelRc::new(VecModel::from(vec![TTag {
        id: "tag-1".into(),
        name: "Designers".into(),
        count: 2,
        ..Default::default()
    }])));
    text_in(&h.app, "New tag", "QA");
    tap(&h.app, "Create tag");
    assert_eq!(last(&log), "tag-create QA|");
    tap(&h.app, "Designers");
    assert_eq!(last(&log), "tag-select tag-1");
    ts(&h.app).set_tag_people(people(&[("8:orgid:s", "Sara", "Member", false)]));
    tap(&h.app, "Remove");
    assert_eq!(last(&log), "tag-remove 8:orgid:s");
    text_in(&h.app, "Add people to the selected tag", "di");
    ts(&h.app).set_candidates(people(&[("8:orgid:d", "Dina", "", false)]));
    tap(&h.app, "Add");
    assert_eq!(last(&log), "tag-add 8:orgid:d");
    tap(&h.app, "Delete");
    assert_eq!(ts(&h.app).get_dialog(), "confirm");
    tap(&h.app, "Yes");
    assert_eq!(last(&log), "tag-delete tag-1");
}

// ---- T2.5 channel email address -----------------------------------------------------

#[test]
fn email_dialog_gets_copies_restricts_and_removes_the_address() {
    let (h, log) = engine_page();
    channel_menu(&h, "Roadmap");
    tap(&h.app, "Get email address");
    assert_eq!(last(&log), "dialog email|t1|c2");
    tap(&h.app, "Get email address");
    assert_eq!(last(&log), "email-create ");
    ts(&h.app).set_email("abc.example@amer.teams.ms".into());
    ts(&h.app).set_email_senders("Anyone".into());
    assert!(has(&h.app, "Email address abc.example@amer.teams.ms"));
    tap(&h.app, "Copy address");
    assert_eq!(last(&log), "email-copy ");
    tap(&h.app, "Team members");
    assert_eq!(last(&log), "email-senders Members");
    tap(&h.app, "These domains");
    text_in(&h.app, "Domains, separated by commas", "a.com, b.org");
    tap(&h.app, "Save");
    assert_eq!(last(&log), "email-senders Domains|a.com, b.org");
    tap(&h.app, "Remove email address");
    assert_eq!(last(&log), "email-remove ");
}

// ---- T2.6 new team -------------------------------------------------------------------

#[test]
fn new_team_dialog_sends_name_description_and_privacy() {
    let (h, log) = engine_page();
    tap(&h.app, "Create team  ▾");
    tap(&h.app, "New team");
    assert_eq!(last(&log), "dialog new-team||");
    tap(&h.app, "Create");
    assert!(!log.borrow().iter().any(|c| c.starts_with("team-create")));
    text_in(&h.app, "Team name", "Apollo");
    tap(&h.app, "Create");
    assert_eq!(last(&log), "team-create Apollo||private");
    tap(&h.app, "Public");
    text_in(&h.app, "Description", "Moon");
    tap(&h.app, "Create");
    assert_eq!(last(&log), "team-create Apollo|Moon|public");
    tap(&h.app, "Org-wide");
    tap(&h.app, "Create");
    assert_eq!(last(&log), "team-create Apollo|Moon|org");
}

// ---- T2.7 manage team ----------------------------------------------------------------

#[test]
fn manage_team_edits_details_code_permissions_and_copies_the_link() {
    let (h, log) = engine_page();
    ts(&h.app).set_d_name("Zebra".into());
    ts(&h.app).set_d_owner(true);
    ts(&h.app).set_user_settings(ModelRc::new(VecModel::from(vec![TSetting {
        key: "editEnabled".into(),
        group: "user".into(),
        label: "Members can edit their messages".into(),
        on: true,
    }])));
    ts(&h.app).set_guest_settings(ModelRc::new(VecModel::from(vec![TSetting {
        key: "createTopic".into(),
        group: "guest".into(),
        label: "Guests can create channels".into(),
        on: false,
    }])));
    tap(&h.app, "More options for Zebra");
    tap(&h.app, "Manage team");
    assert_eq!(last(&log), "dialog manage-team|t1|");
    ts(&h.app).set_d_team("t1".into());
    text_in(&h.app, "Description", "Hello");
    tap(&h.app, "Save details");
    assert_eq!(last(&log), "team-edit Zebra|Hello");
    tap(&h.app, "Generate code");
    assert_eq!(last(&log), "team-code reset");
    ts(&h.app).set_team_code("abc1234".into());
    assert!(has(&h.app, "Team code abc1234"));
    tap(&h.app, "Remove code");
    assert_eq!(last(&log), "team-code remove");
    tap(&h.app, "Members can edit their messages");
    assert_eq!(last(&log), "team-setting user|editEnabled|0");
    tap(&h.app, "Guests can create channels");
    assert_eq!(last(&log), "team-setting guest|createTopic|1");
    tap(&h.app, "Copy link to team");
    assert_eq!(last(&log), "team-link t1");
}

#[test]
fn archiving_and_deleting_a_team_are_owner_only_and_ask_first() {
    let (h, log) = engine_page();
    tap(&h.app, "More options for Zebra");
    tap(&h.app, "Archive team");
    assert!(has(&h.app, "Archive Zebra?"));
    tap(&h.app, "Yes");
    assert_eq!(last(&log), "team-archive t1|1");
    tap(&h.app, "More options for Zebra");
    tap(&h.app, "Delete the team");
    tap(&h.app, "Yes");
    assert_eq!(last(&log), "team-delete t1");
}

// ---- T2.8 notifications ---------------------------------------------------------------

#[test]
fn notification_dialog_sets_the_level_and_replies() {
    let (h, log) = engine_page();
    channel_menu(&h, "Roadmap");
    tap(&h.app, "Manage notifications");
    assert_eq!(last(&log), "dialog notify|t1|c2");
    ts(&h.app).set_notify_level("feed".into());
    tap(&h.app, "Banner and feed");
    assert_eq!(last(&log), "notify-set banner|0");
    tap(&h.app, "Include replies");
    assert_eq!(last(&log), "notify-set feed|1");
    tap(&h.app, "Off");
    assert_eq!(last(&log), "notify-set off|0");
}

// ---- T3.1 communities -------------------------------------------------------------------

fn community() -> TCommunity {
    TCommunity {
        id: "42".into(),
        name: "All Company".into(),
        people: 300,
        company: true,
        ..Default::default()
    }
}

#[test]
fn communities_tab_lists_communities_and_opens_a_read_only_feed() {
    let (h, log) = engine_page();
    // The calendar view has no chat list, so only the tab lists the community.
    h.app.set_view("calendar".into());
    ts(&h.app).set_communities(ModelRc::new(VecModel::from(vec![community()])));
    tap(&h.app, "Communities");
    tap(&h.app, "All Company");
    assert_eq!(last(&log), "community-open 42");
    assert_eq!(ts(&h.app).get_community(), "42");
    ts(&h.app).set_community_name("All Company".into());
    ts(&h.app).set_posts_more(true);
    ts(&h.app).set_posts(ModelRc::new(VecModel::from(vec![TPost {
        id: "1".into(),
        sender: "Sara".into(),
        time: "Mon".into(),
        text: "Welcome everyone".into(),
        likes: 3,
        url: "https://example.com/p/1".into(),
        ..Default::default()
    }])));
    let post = el(&h.app, "Sara");
    assert_eq!(post.accessible_description().unwrap(), "Welcome everyone");
    assert!(
        !has(&h.app, "Reply") && !has(&h.app, "Send"),
        "the feed is read only"
    );
    tap(&h.app, "Show older posts");
    assert_eq!(last(&log), "community-more ");
    tap(&h.app, "Back to communities");
    assert_eq!(ts(&h.app).get_community(), "");
}

#[test]
fn the_chat_list_has_a_communities_section_that_opens_the_feed() {
    let app = new_window();
    app.window().set_size(LogicalSize::new(1200., 900.));
    app.set_login_state("ready".into());
    app.show().unwrap();
    assert!(!has(&app, "All Company"), "no community, no section");
    ts(&app).set_communities(ModelRc::new(VecModel::from(vec![community()])));
    // A click opens the Engage feed (`ui_tests::engage`), not the Teams page.
    tap(&app, "All Company");
    assert!(app.global::<crate::EngageStore>().get_open());
    assert!(!ts(&app).get_open());
}

#[test]
fn the_teams_sidebar_opens_the_page() {
    let app = new_window();
    app.window().set_size(LogicalSize::new(1200., 900.));
    app.set_login_state("ready".into());
    app.set_view("teams".into());
    app.show().unwrap();
    let log = Log::default();
    let l = log.clone();
    ts(&app).on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    tap(&app, "Your teams and channels");
    assert!(ts(&app).get_open());
    assert_eq!(last(&log), "open ");
}

#[test]
fn the_page_closes_with_its_close_button() {
    let (h, _) = engine_page();
    tap(&h.app, "Close");
    assert!(!ts(&h.app).get_open());
}

#[test]
fn errors_and_notices_show_on_the_page() {
    let (h, _) = engine_page();
    ts(&h.app).set_error("Not allowed".into());
    ts(&h.app).set_notice("Channel created".into());
    assert!(has(&h.app, "Error: Not allowed"));
    assert!(has(&h.app, "Channel created"));
}

// ---- AI access (docs/designs/mcp-access-control.md, slice 6) ----------------------------------

/// Every `AiAccessStore` command as `"<name> <argument>"`.
fn ai_commands(app: &AppWindow) -> Log {
    let log = Log::default();
    let l = log.clone();
    app.global::<AiAccessStore>()
        .on_command(move |n, a| l.borrow_mut().push(format!("{n} {a}")));
    log
}

#[test]
fn the_team_menu_lets_the_ai_read_all_channels_only_when_available() {
    let (h, _log) = engine_page();
    let ai = ai_commands(&h.app);
    tap(&h.app, "More options for Zebra");
    assert!(has(&h.app, "Go to team"));
    assert!(
        !has(&h.app, "Let AI read all channels…"),
        "hidden without the feature"
    );
    h.app.global::<AiAccessStore>().set_available(true);
    tap(&h.app, "Let AI read all channels…");
    assert_eq!(*ai.borrow(), ["team-channels t1"]);
    assert!(!has(&h.app, "Go to team"), "the menu closed");
}

#[test]
fn the_ai_bulk_confirmation_and_the_explainer_show_over_the_teams_page() {
    let (h, _log) = engine_page();
    let ai = ai_commands(&h.app);
    let s = h.app.global::<AiAccessStore>();
    s.set_available(true);
    s.set_bulk_team("Zebra".into());
    s.set_bulk_count(1);
    s.set_bulk_open(true);
    // A click reaches the dialog, not the page under it.
    tap(&h.app, "Let AI read");
    s.set_explainer_open(true);
    tap(&h.app, "Continue");
    assert_eq!(
        *ai.borrow(),
        ["team-channels-confirm ", "explainer-confirm "]
    );
    assert!(ts(&h.app).get_open(), "the Teams page stays open");
}
