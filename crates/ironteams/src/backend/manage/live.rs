//! Live checks of the Teams management commands against the real account. All `#[ignore]`d; run
//! by hand:
//! `IRONTEAMS_SANDBOX_FILE=<ids file> cargo test -p ironteams live_teams -- --ignored --nocapture --test-threads=1`
//!
//! Rules: with `IRONTEAMS_SANDBOX_FILE` set the engine refuses every write that does not target the
//! sandbox team (see `State::allow_team`), so nothing else can change. The test creates a few
//! channels and tags in the sandbox and removes them again. Output is counts and booleans only.

use super::*;
use std::time::Duration;

const SEP_S: &str = "\u{1f}";

async fn engine() -> Arc<Engine> {
    let paths = session::Paths::new().unwrap();
    let cfg = paths.load_config().unwrap();
    // The keyring D-Bus service is flaky on back-to-back runs: try a few times.
    let mut sess = None;
    for _ in 0..4 {
        match session::stored_session(&cfg) {
            Ok(s) => {
                sess = s;
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(5)).await,
        }
    }
    let sess = sess.expect("signed in");
    let client = teams_proto::Client::resume(sess).await.unwrap();
    let _ = session::persist(&cfg, &client).await;
    let me = cfg.self_mri.clone().expect("self mri in config");
    i_slint_backend_testing::init_no_event_loop();
    let window = Box::leak(Box::new(crate::AppWindow::new().unwrap()));
    let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(window));
    Engine::new(
        client,
        store::Store::open_in_memory().unwrap(),
        cfg,
        ui,
        me,
        "ironteams live test".into(),
    )
}

async fn run(e: &Arc<Engine>, name: &str, args: &[&str]) -> String {
    let c = TeamsCmd::parse(name, &args.join(SEP_S)).expect("known command");
    e.manage_cmd(c).await.unwrap();
    let err = e.manage_state(|s| {
        let d = std::mem::take(&mut s.dialog.error);
        format!("{}{d}", std::mem::take(&mut s.error))
    });
    println!("{name}: {}", if err.is_empty() { "ok" } else { &err });
    err
}

fn channel_id(e: &Arc<Engine>, team: &str, name: &str) -> Option<String> {
    e.manage_state(|s| {
        s.page
            .team(team)
            .and_then(|t| t.channels.iter().find(|c| c.name == name && !c.is_deleted))
            .map(|c| c.id.clone())
    })
}

fn flag(e: &Arc<Engine>, ch: &str, f: impl Fn(&teams_proto::teams::ChannelEntry) -> bool) -> bool {
    e.manage_state(|s| s.page.channel(ch).is_some_and(|(_, c)| f(c)))
}

/// True once `f` holds. The list lags a few seconds behind renames and deletions, so it is
/// reloaded between looks.
async fn eventually(e: &Arc<Engine>, f: impl Fn(&Arc<Engine>) -> bool) -> bool {
    for _ in 0..8 {
        if f(e) {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = e.manage_reload().await;
    }
    f(e)
}

/// Deletes every channel of the sandbox team that a run of this test made.
async fn remove_test_channels(e: &Arc<Engine>, team: &str) {
    let _ = e.manage_reload().await;
    let found: Vec<(TeamRef, ChannelRef)> = e.manage_state(|s| {
        s.page
            .team(team)
            .map(|t| {
                t.channels
                    .iter()
                    .filter(|c| c.name.starts_with("ironteams-ui-live") && !c.is_deleted)
                    .map(|c| (t.team_ref(), ChannelRef::from_entry(c)))
                    .collect()
            })
            .unwrap_or_default()
    });
    for (t, c) in found {
        let _ = e.client.delete_channel(&t, &c).await;
    }
}

fn sandbox() -> Option<Guard> {
    Guard::from_env()
}

#[tokio::test]
#[ignore = "live: creates and removes channels and tags in the sandbox team"]
async fn live_teams_ui_commands_in_the_sandbox_team() {
    let Some(g) = sandbox() else {
        println!("no IRONTEAMS_SANDBOX_FILE: skipped");
        return;
    };
    let e = engine().await;
    let team = g.team.clone();
    let mut failures: Vec<String> = vec![];
    let mut check = |what: &str, ok: bool| {
        println!("check {what}: {ok}");
        if !ok {
            failures.push(what.into());
        }
    };

    // T1.1: the list.
    check("open loads the list", run(&e, "open", &[]).await.is_empty());
    let rows = e.manage_state(|s| s.page.rows());
    check("sandbox team is listed", rows.iter().any(|r| r.id == team));
    check(
        "analytics has counts",
        e.manage_state(|s| s.page.stats().len() == 10),
    );
    run(&e, "filter", &["owned"]).await;
    check(
        "owned filter keeps the sandbox team",
        e.manage_state(|s| s.page.rows().iter().any(|r| r.id == team)),
    );
    run(&e, "filter", &["all"]).await;
    run(&e, "query", &["tests"]).await;
    check(
        "search by channel name finds the team",
        e.manage_state(|s| s.page.rows().iter().any(|r| r.id == team)),
    );
    run(&e, "query", &[""]).await;

    // T1.2: discover (read only) and the sandbox team's own code.
    check(
        "discover suggestions",
        run(&e, "discover", &[""]).await.is_empty(),
    );
    println!("suggested teams: {}", e.manage_state(|s| s.discover.len()));
    check(
        "discover search",
        run(&e, "discover", &["a"]).await.is_empty(),
    );
    let none_pending = e.manage_state(|s| s.requested.is_empty());
    check("no join request was made", none_pending);
    let refused = run(&e, "join", &["not-a-real-group"]).await;
    check(
        "joining another team is refused here",
        refused.starts_with("refused"),
    );
    let refused = run(&e, "join-code", &["not-our-code"]).await;
    check(
        "a foreign code is refused here",
        refused.starts_with("refused"),
    );
    let code = e.client.team_join_code(&team).await.unwrap();
    let own = match code {
        Some(c) => c,
        None => e.client.reset_team_join_code(&team).await.unwrap(),
    };
    check(
        "own code goes through join-code",
        run(&e, "join-code", &[&own]).await.is_empty(),
    );
    println!(
        "join info present: {}",
        !e.manage_state(|s| s.join_info.clone()).is_empty()
    );

    // T2.1: new channels (standard in the threaded layout, and private).
    // A deleted channel keeps its SharePoint folder name for a while, so each run uses new names.
    let stamp = teams_proto::time::now_ms() % 1_000_000;
    let n1 = format!("ironteams-ui-live-{stamp}");
    let n2 = format!("ironteams-ui-live-{stamp}-private");
    let (n1, n2) = (n1.as_str(), n2.as_str());
    let renamed = format!("{n1}-renamed");
    let renamed_private = format!("{n2}-renamed");
    check(
        "create standard channel",
        run(
            &e,
            "channel-create",
            &[&team, n1, "made by the UI test", "standard", "1", "0"],
        )
        .await
        .is_empty(),
    );
    let c1 = channel_id(&e, &team, n1);
    check("standard channel is listed", c1.is_some());
    let c1 = c1.unwrap_or_default();
    check(
        "create private channel",
        run(&e, "channel-create", &[&team, n2, "", "private", "0", "0"])
            .await
            .is_empty(),
    );
    let c2 = channel_id(&e, &team, n2);
    check("private channel is listed", c2.is_some());
    let c2 = c2.unwrap_or_default();
    if c1.is_empty() || c2.is_empty() {
        remove_test_channels(&e, &team).await;
        panic!("a test channel was not created, the rest would only repeat that: {failures:?}");
    }

    // T1.3: hide, show, pin, unpin.
    run(&e, "chan-show", &[&c1, "0"]).await;
    check("hide", flag(&e, &c1, |c| !c.is_shown));
    run(&e, "chan-show", &[&c1, "1"]).await;
    check("show", flag(&e, &c1, |c| c.is_shown));
    run(&e, "chan-pin", &[&c1, "1"]).await;
    check("pin", e.manage_state(|s| s.page.pinned.contains(&c1)));
    run(&e, "chan-pin", &[&c1, "0"]).await;
    check("unpin", e.manage_state(|s| !s.page.pinned.contains(&c1)));
    run(&e, "chan-show", &[&c2, "0"]).await;
    check("hide private", flag(&e, &c2, |c| !c.is_shown));
    run(&e, "chan-show", &[&c2, "1"]).await;
    check("show private", flag(&e, &c2, |c| c.is_shown));
    check(
        "show for everyone on",
        run(&e, "chan-default", &[&c1, "1"]).await.is_empty(),
    );
    check(
        "show for everyone off",
        run(&e, "chan-default", &[&c1, "0"]).await.is_empty(),
    );
    let other = e.manage_state(|s| {
        s.page
            .entries
            .iter()
            .find(|t| t.id != team)
            .and_then(|t| t.channels.first().map(|c| c.id.clone()))
    });
    if let Some(o) = other {
        let r = run(&e, "chan-show", &[&o, "0"]).await;
        check("hiding in a real team is refused", r.starts_with("refused"));
    }

    // T2.2: edit, moderation, archive, delete, restore.
    run(&e, "channel-edit", &[&team, &c1, &renamed, "edited"]).await;
    check(
        "rename",
        eventually(&e, |e| flag(e, &c1, |c| c.name == renamed)).await,
    );
    run(&e, "channel-edit", &[&team, &c2, &renamed_private, ""]).await;
    check(
        "rename private",
        eventually(&e, |e| flag(e, &c2, |c| c.name == renamed_private)).await,
    );
    run(&e, "dialog", &["moderation", &team, &c1]).await;
    run(&e, "mod-set", &["post", "Moderators"]).await;
    run(&e, "mod-set", &["bots", "0"]).await;
    let m = e.client.moderation(&c1).await.unwrap();
    check(
        "moderation stored",
        m.post_permissions == "Moderators" && !m.allow_bots && m.reply_permissions == "Everyone",
    );
    run(&e, "mod-set", &["post", "Everyone"]).await;
    run(&e, "mod-set", &["bots", "1"]).await;
    run(&e, "channel-archive", &[&c1, "1"]).await;
    check("archive", flag(&e, &c1, |c| c.is_archived));
    run(&e, "channel-archive", &[&c1, "0"]).await;
    check("unarchive", flag(&e, &c1, |c| !c.is_archived));
    run(&e, "channel-delete", &[&team, &c1]).await;
    check(
        "delete",
        eventually(&e, |e| channel_id(e, &team, &renamed).is_none()).await,
    );
    let mut listed = false;
    for _ in 0..5 {
        run(&e, "dialog", &["deleted", &team, ""]).await;
        listed = e.manage_state(|s| s.dialog.deleted.iter().any(|d| d.id == c1));
        if listed {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    check("deleted list has it", listed);
    run(&e, "channel-restore", &[&team, &c1]).await;
    check(
        "restore",
        eventually(&e, |e| channel_id(e, &team, &renamed).is_some()).await,
    );

    // T2.5 email and T2.8 notifications (dialog state first, like the UI).
    run(&e, "dialog", &["email", &team, &c1]).await;
    check(
        "no address yet",
        e.manage_state(|s| s.dialog.email.is_empty()),
    );
    run(&e, "email-create", &[]).await;
    check(
        "address made",
        e.manage_state(|s| !s.dialog.email.is_empty()),
    );
    run(&e, "email-senders", &["Members"]).await;
    let got = e.client.channel_email(&c1).await.unwrap();
    check(
        "senders stored",
        got.is_some_and(|x| x.senders == EmailSenders::Members),
    );
    run(&e, "email-remove", &[]).await;
    check(
        "address removed",
        e.client.channel_email(&c1).await.unwrap().is_none(),
    );
    run(&e, "dialog", &["notify", &team, &c1]).await;
    for (level, name) in [
        ("banner", NotifyLevel::BannerAndFeed),
        ("off", NotifyLevel::Off),
        ("feed", NotifyLevel::Feed),
    ] {
        run(&e, "notify-set", &[level, "0"]).await;
        check(
            &format!("notify {level}"),
            e.client.channel_notifications(&c1).await.unwrap().level == name,
        );
    }

    // T2.3 members: the roster and the people search (read only), a role round trip on nobody.
    run(&e, "dialog", &["members", &team, ""]).await;
    let n = e.manage_state(|s| s.dialog.people.len());
    println!("team members listed: {n}");
    check(
        "roster lists people with names",
        n >= 1 && e.manage_state(|s| s.dialog.people.iter().all(|p| p.name != "Unknown person")),
    );
    run(&e, "member-search", &["ab"]).await;
    println!(
        "search results: {}",
        e.manage_state(|s| s.dialog.candidates.len())
    );
    run(&e, "dialog", &["members", &team, &c2]).await;
    check(
        "private channel roster",
        e.manage_state(|s| !s.dialog.people.is_empty()),
    );

    // T2.4 tags.
    run(&e, "dialog", &["tags", &team, ""]).await;
    // A deleted tag keeps its name until it is purged, so each run names its tag anew.
    let tag_name = format!("ironteams-ui-tag-{stamp}");
    check(
        "create tag",
        run(&e, "tag-create", &[&tag_name, ""]).await.is_empty(),
    );
    let tag = e.manage_state(|s| {
        s.dialog
            .tags
            .iter()
            .find(|t| t.name == tag_name)
            .map(|t| t.id.clone())
    });
    check("tag is listed", tag.is_some());
    let tag = tag.unwrap_or_default();
    check(
        "tag has its first member",
        e.manage_state(|s| s.dialog.tag_people.len() == 1),
    );
    // Adding ourselves again changes nothing, but it exercises the add call.
    let me = e.me.clone();
    check("tag add", run(&e, "tag-add", &[&me]).await.is_empty());
    check(
        "still one member",
        e.manage_state(|s| s.dialog.tag_people.len() == 1),
    );
    let tag_renamed = format!("{tag_name}-2");
    run(&e, "tag-rename", &[&tag, &tag_renamed]).await;
    check(
        "tag renamed",
        e.manage_state(|s| s.dialog.tags.iter().any(|t| t.name == tag_renamed)),
    );
    run(&e, "tag-delete", &[&tag]).await;
    check(
        "tag deleted",
        e.manage_state(|s| s.dialog.tags.iter().all(|t| t.id != tag)),
    );

    // T2.7 manage team.
    run(&e, "dialog", &["manage-team", &team, ""]).await;
    let (user, guest) = e.manage_state(|s| {
        (
            s.dialog
                .settings
                .iter()
                .filter(|x| x.group == "user")
                .count(),
            s.dialog
                .settings
                .iter()
                .filter(|x| x.group == "guest")
                .count(),
        )
    });
    println!("settings rows: user {user}, guest {guest}");
    check("settings are listed", user > 0);
    let first = e.manage_state(|s| {
        s.dialog
            .settings
            .iter()
            .find(|x| x.group == "user")
            .cloned()
    });
    if let Some(s0) = first {
        run(
            &e,
            "team-setting",
            &["user", &s0.key, if s0.on { "0" } else { "1" }],
        )
        .await;
        let flipped = e.manage_state(|s| {
            s.dialog
                .settings
                .iter()
                .any(|x| x.key == s0.key && x.group == "user" && x.on != s0.on)
        });
        check("a permission flips", flipped);
        run(
            &e,
            "team-setting",
            &["user", &s0.key, if s0.on { "1" } else { "0" }],
        )
        .await;
        check(
            "and flips back",
            e.manage_state(|s| {
                s.dialog
                    .settings
                    .iter()
                    .any(|x| x.key == s0.key && x.group == "user" && x.on == s0.on)
            }),
        );
    }
    let before = e.manage_state(|s| {
        s.page
            .team(&team)
            .map(|t| t.description.clone())
            .unwrap_or_default()
    });
    run(&e, "team-edit", &["ironteams-sandbox", "ironteams ui test"]).await;
    run(&e, "team-code", &["remove"]).await;
    check(
        "code removed",
        e.manage_state(|s| s.dialog.team_code.is_empty()),
    );
    run(&e, "team-code", &["reset"]).await;
    check(
        "code made",
        e.manage_state(|s| !s.dialog.team_code.is_empty()),
    );
    run(&e, "team-code", &["remove"]).await;
    run(&e, "team-edit", &["ironteams-sandbox", &before]).await;
    for attempt in 1..=3 {
        let r = run(&e, "team-archive", &[&team, "1"]).await;
        println!("team archive attempt {attempt}: failed={}", !r.is_empty());
        if r.is_empty() {
            run(&e, "team-archive", &[&team, "0"]).await;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let refused = run(&e, "team-create", &["should-not-exist", "", "private"]).await;
    check(
        "creating a team is refused here",
        refused.starts_with("refused"),
    );

    // T3.1 communities (read only).
    check(
        "communities load",
        run(&e, "communities", &[]).await.is_empty(),
    );
    let first = e.manage_state(|s| s.communities.first().map(|c| c.id));
    println!("communities: {}", e.manage_state(|s| s.communities.len()));
    if let Some(id) = first {
        check(
            "feed loads",
            run(&e, "community-open", &[&id.to_string()])
                .await
                .is_empty(),
        );
        println!("posts: {}", e.manage_state(|s| s.posts.len()));
    }

    // Cleanup: whatever the checks said, take the test channels away again.
    remove_test_channels(&e, &team).await;
    // The list lags a few seconds behind a deletion.
    let mut left = usize::MAX;
    for _ in 0..8 {
        let _ = e.manage_reload().await;
        left = e.manage_state(|s| {
            s.page
                .team(&team)
                .map(|t| {
                    t.channels
                        .iter()
                        .filter(|c| c.name.starts_with("ironteams-ui-live") && !c.is_deleted)
                        .count()
                })
                .unwrap_or(0)
        });
        if left == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    println!("test channels left: {left}");
    assert_eq!(left, 0, "cleanup");
    assert!(failures.is_empty(), "failed checks: {failures:?}");
}
