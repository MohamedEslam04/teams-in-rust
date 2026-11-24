//! Apps: the app bar on the rail, the "View more apps" flyout, the Store, and the built-in app
//! screens (Planner and To Do, Workflows, OneNote, Lists, Meeting Recaps).
//!
//! Reads go to the live services through `crates/apps` and `teams_proto`; in demo mode they come
//! from `fixtures`. Changes (add, remove or update an app, pin, tasks, flows, pages, list items)
//! are sent only after the user confirms in the dialog of `form.rs`, and the message that follows
//! is the service's answer. In demo mode and in the tests nothing is sent: `Apps::sent` logs the
//! requests that would have been.

mod bar;
mod cmd;
mod fixtures;
mod flows;
mod form;
#[cfg(test)]
mod live;
mod notes;
mod player;
mod recaps;
mod shop;
mod source;
mod tasks;
pub(crate) mod view;
mod webhost_open;

use super::Engine;
use crate::data::Pixels;
use app_store::{AppDefinition, Grants, InstalledApps};
pub use cmd::AppsCmd;
use std::collections::HashMap;
use std::sync::Arc;

/// The apps part of the engine's state.
#[derive(Default)]
pub struct Apps {
    installed: Option<InstalledApps>,
    policy: Option<app_store::AppPolicy>,
    collapsed: bool,
    flyout: bool,
    flyout_query: String,
    /// Bumped by every Open app, so one still waiting for the installed apps does not open
    /// after a later one.
    open_seq: u64,
    /// The pinned app that is open on the rail.
    active: String,
    /// Icons and screenshots by URL. `None` while loading, or when the download failed.
    images: HashMap<String, Option<Pixels>>,
    wanted: Vec<String>,
    /// Definitions fetched for the Store and the app pages.
    defs: HashMap<String, AppDefinition>,
    shop: shop::Shop,
    tasks: tasks::Tasks,
    flows: flows::Flows,
    notes: notes::Notes,
    lists: notes::Lists,
    recaps: recaps::Recaps,
    form: form::Form,
    /// The changes that were (offline: would have been) sent, as `METHOD url`.
    sent: Vec<String>,
    grants: Grants,
    /// Links that were opened (the tests read these; nothing is opened offline).
    opened: Vec<String>,
    /// The recording player window while it is open (see `player.rs`).
    player: Option<Arc<webhost::WebWindow>>,
    /// The submit callback of the last task module opened in tests.
    #[cfg(test)]
    task_submit: Option<webhost_open::TaskSubmit>,
}

impl Apps {
    /// The image at `url`, if it is loaded; asks for it otherwise.
    fn image(&mut self, url: &str) -> Option<Pixels> {
        // Only https icons: the download adds credentials for Microsoft hosts.
        if !url.starts_with("https://") {
            return None;
        }
        match self.images.get(url) {
            Some(p) => p.clone(),
            None => {
                self.images.insert(url.to_owned(), None);
                self.wanted.push(url.to_owned());
                None
            }
        }
    }

    /// The definition of an installed or Store app.
    fn def(&self, id: &str) -> Option<&AppDefinition> {
        self.installed
            .as_ref()
            .and_then(|i| i.definition(id))
            .or_else(|| self.defs.get(id))
    }
}

impl Engine {
    /// Loads the app bar in the background (called once the engine starts).
    pub(super) fn start_apps(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            if let Err(err) = e.apps_cmd(AppsCmd::Load).await {
                tracing::warn!(%err, "app bar load failed");
            }
        });
    }

    pub(super) async fn apps_cmd(self: &Arc<Self>, cmd: AppsCmd) -> anyhow::Result<()> {
        use AppsCmd::*;
        match cmd {
            Load => self.apps_load().await,
            RailCollapse => {
                let mut a = self.apps.lock().unwrap();
                a.collapsed = !a.collapsed;
                drop(a);
                self.apps_push_rail();
                Ok(())
            }
            Pin(id) => self.apps_pin(&id).await,
            Unpin(id) => self.apps_unpin(&id).await,
            Move(id, to) => self.apps_move(&id, to).await,
            Shortcut(n) => self.apps_shortcut(n).await,
            FlyoutOpen => self.apps_flyout(true, None).await,
            FlyoutClose => self.apps_flyout(false, None).await,
            FlyoutQuery(q) => self.apps_flyout(true, Some(q)).await,
            OpenApp(id) => self.apps_open(&id).await,
            StoreEnter => self.shop_enter().await,
            StoreOpen | StoreHome => self.shop_home().await,
            StoreSearch(q) => self.shop_search(q).await,
            StoreCategory(key) => self.shop_category(&key).await,
            StoreScope(s) => self.shop_scope(&s).await,
            StoreKind(k) => self.shop_kind(&k).await,
            AppPage(id) => self.shop_app(&id).await,
            AppAdd(target) => self.shop_add(&target).await,
            AppRemove(id) => self.shop_remove(&id).await,
            AppUpdate(id) => self.shop_update(&id).await,
            AppShare(id) => self.shop_share(&id),
            AppLink(what) => self.shop_link(&what),
            Manage => self.shop_manage().await,
            StoreSettings => self.shop_settings().await,
            Grant(app, perm, on) => self.shop_grant(&app, &perm, on),
            TasksEnter => self.tasks_enter().await,
            TasksNav(key) => self.tasks_nav(&key).await,
            TasksMode(mode) => self.tasks_mode(&mode),
            TasksRefresh => self.tasks_refresh().await,
            TaskNew => self.tasks_begin_new(),
            TaskFromMessage(text) => self.tasks_from_message(&text).await,
            TaskToggle(id) => self.tasks_toggle(&id).await,
            TaskEdit(id) => self.tasks_begin_edit(&id),
            TaskDelete(id) => self.tasks_begin_delete(&id),
            FlowsEnter => self.flows_enter().await,
            FlowsSelect(id) => self.flows_select(&id).await,
            FlowsRefresh => self.flows_refresh().await,
            FlowRun(id) => self.flows_begin_run(&id),
            FlowToggle(id) => self.flows_begin_toggle(&id),
            NotesEnter => self.notes_enter().await,
            NotesNotebook(id) => self.notes_notebook(&id).await,
            NotesSection(id) => self.notes_section(&id).await,
            NotesPage(id) => self.notes_page(&id).await,
            NotesOpenWeb => self.notes_open_web(),
            PageNew => self.notes_begin_new_page(),
            ListsEnter => self.lists_enter().await,
            ListsSite(id) => self.lists_site(&id).await,
            ListsList(id) => self.lists_list(&id).await,
            ItemNew => self.lists_begin_item(None),
            ItemEdit(id) => self.lists_begin_item(Some(&id)),
            FormSubmit(text) => self.form_submit(&text).await,
            FormCancel => {
                self.form_cancel();
                Ok(())
            }
            FormChoice(id) => {
                self.form_choice(&id);
                Ok(())
            }
            FormLink => {
                self.form_link();
                Ok(())
            }
            RecapsEnter => self.recaps_enter().await,
            RecapsRefresh => self.recaps_refresh().await,
            RecapsQuery(q) => self.recaps_query(&q),
            RecapsOpen(key) => self.recaps_open(&key).await,
            RecapsHome => self.recaps_home(),
            RecapsPart(i) => self.recaps_part(i).await,
            RecapsTab(t) => self.recaps_tab(&t).await,
            RecapsWatch => self.recaps_watch(),
            RecapsShare => self.recaps_share(),
            RecapsNotes => self.recaps_notes(),
            RecapsWatchWeb => self.recaps_watch_web(),
            RecapsPlay(url) => self.recaps_play_link(&url),
            RecapsTranscript => self.recaps_watch(),
            RecapsFile(url) => self.recaps_file(&url),
            RecapsChat(thread) => self.recaps_chat(&thread).await,
        }
    }

    /// Opens a link in the system browser. Offline (demo and tests) it is only recorded.
    pub(super) fn apps_open_link(&self, url: &str) {
        if !url.starts_with("https://") {
            return;
        }
        self.apps.lock().unwrap().opened.push(url.to_owned());
        if !self.apps_offline() {
            let _ = crate::external::open(url);
        }
    }

    /// Starts the downloads of the icons and screenshots the last build asked for. The views are
    /// pushed again when one arrives.
    pub(super) fn apps_fetch_images(self: &Arc<Self>) {
        if self.apps_offline() {
            return;
        }
        let urls = std::mem::take(&mut self.apps.lock().unwrap().wanted);
        for url in urls {
            let e = self.clone();
            tokio::spawn(async move {
                let bytes = e.client.bytes(&url, &[], 4 * 1024 * 1024).await;
                let px = match bytes {
                    Ok(b) => {
                        super::util::decode_limited(move || super::util::decode_image(&b, 480, 320))
                            .await
                            .flatten()
                    }
                    Err(err) => {
                        tracing::debug!(%err, "app image download failed");
                        None
                    }
                };
                if px.is_some() {
                    e.apps.lock().unwrap().images.insert(url, px);
                    e.apps_push_rail();
                    e.shop_push();
                }
            });
        }
    }
}

#[cfg(test)]
impl Engine {
    /// Links the app screens opened, oldest first.
    pub(crate) fn apps_opened(&self) -> Vec<String> {
        self.apps.lock().unwrap().opened.clone()
    }

    /// The changes the app screens sent (offline: would have sent), oldest first.
    pub(crate) fn apps_sent(&self) -> Vec<String> {
        self.apps.lock().unwrap().sent.clone()
    }
}
