//! Live checks of the app screens against the real account. All `#[ignore]`d; run by hand:
//! `cargo test -p IronTeams live_apps -- --ignored --nocapture --test-threads=1`.
//!
//! Everything here is a read: the commands run are the ones that only load (app bar, flyout,
//! Store home, search, Manage, settings, Planner and To Do, Workflows, OneNote, Lists). Nothing
//! is pinned, installed, run or edited, and no link is opened. Output is counts only.

use super::AppsCmd;
use crate::backend::{Cmd, Engine};
use std::sync::Arc;

async fn engine() -> Arc<Engine> {
    let paths = session::Paths::new().unwrap();
    let cfg = paths.load_config().unwrap();
    let sess = session::stored_session(&cfg).unwrap().expect("signed in");
    let client = teams_proto::Client::resume(sess).await.unwrap();
    session::persist(&cfg, &client).await.unwrap();
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

async fn run(e: &Arc<Engine>, name: &str, arg: &str) {
    let cmd = AppsCmd::parse(name, arg).unwrap();
    e.clone().handle(Cmd::Apps(cmd)).await;
}

#[tokio::test]
#[ignore = "live: reads the app bar, the Store and the built-in apps"]
async fn live_apps_reads() {
    let e = engine().await;

    run(&e, "load", "").await;
    let (bar, flyout, policy) = {
        let a = e.apps.lock().unwrap();
        let bar = a.installed.as_ref().map_or(0, |i| i.app_bar().len());
        let flyout = a.installed.as_ref().map_or(0, |i| i.more_apps("").len());
        (bar, flyout, a.policy.is_some())
    };
    let rail = e.apps.lock().unwrap().rail_items().len();
    println!(
        "app bar: {bar} pinned, {rail} on the rail beyond IronTeams's own screens, {flyout} in the flyout, policy read: {policy}"
    );
    assert!(bar > 0, "the app bar is not empty");

    run(&e, "store-open", "").await;
    {
        let mut a = e.apps.lock().unwrap();
        let d = a.store_data();
        println!(
            "store home: {} categories, {} popular cards, {} org cards, status {:?}",
            d.categories.len(),
            d.popular.len(),
            d.org.len(),
            d.status
        );
        assert!(!d.categories.is_empty());
    }
    run(&e, "store-search", "planner").await;
    {
        let mut a = e.apps.lock().unwrap();
        let d = a.store_data();
        println!("search 'planner': {} cards", d.cards.len());
        assert!(!d.cards.is_empty());
    }
    run(&e, "manage", "").await;
    println!(
        "manage your apps: {} apps",
        e.apps.lock().unwrap().store_data().manage.len()
    );
    run(&e, "store-settings", "").await;
    {
        let mut a = e.apps.lock().unwrap();
        let d = a.store_data();
        println!(
            "store settings: policy read {}, {} permission rows",
            !d.policy.is_empty(),
            d.perms.len()
        );
    }

    run(&e, "tasks-enter", "").await;
    {
        let a = e.apps.lock().unwrap();
        let d = a.tasks.data(teams_proto::time::now_ms());
        println!(
            "tasks: {} nav rows, {} tasks in the open list, status {:?}",
            d.nav.len(),
            d.tasks.len(),
            d.status
        );
    }
    run(&e, "flows-enter", "").await;
    {
        let a = e.apps.lock().unwrap();
        let d = a.flows.data();
        println!(
            "workflows: {} flows, {} runs of the first, status {:?}",
            d.flows.len(),
            d.runs.len(),
            d.status
        );
    }
    run(&e, "notes-enter", "").await;
    {
        let a = e.apps.lock().unwrap();
        let d = a.notes.data();
        println!(
            "onenote: {} notebooks, {} sections, {} pages, status {:?}",
            d.notebooks.len(),
            d.sections.len(),
            d.pages.len(),
            d.status
        );
    }
    run(&e, "lists-enter", "").await;
    {
        let a = e.apps.lock().unwrap();
        let d = a.lists.data();
        println!(
            "lists: {} sites, {} lists, {} columns, {} items, status {:?}",
            d.sites.len(),
            d.lists.len(),
            d.columns.len(),
            d.items.len(),
            d.status
        );
    }
}
