//! The chat list beyond Favorites / Chats / Teams: sections (plan 8.1), the chat item menu (8.2),
//! filters and list options (8.3) and the channel menu (8.5).
//!
//! Sections of the account are read from Teams (`conversationFolders`) and shown in Teams' order.
//! The Teams web client writes them through calls whose bodies IronTeams could not capture (see
//! `docs/protocol.md`, "Chat list sections"), so what the user changes here stays on this
//! computer: sections made in IronTeams, where each item sits, which chats are hidden and the list
//! options. A move made here wins over what Teams has until it is removed.
//!
//! Mute, hide and the channel items (hide, notifications) are real Teams settings: they are
//! written to the account (`teams_proto::actions` and `teams_proto::teams`) and mirrored here.
//! Teams' mute is on or off with no end time (see `docs/protocol.md`), so "Mute for 1 hour" and
//! the like keep the end time here and turn the Teams mute off when it passes.

use super::Engine;
use crate::data::ChatData;
use crate::lists_data::{ChannelMenuData, ChoiceData, ListOptions, SectionData};
use chrono::{Duration as Days, NaiveTime, TimeZone};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use teams_proto::sections::{FolderKind, Folders};
use teams_proto::teams::{ChannelKind, ChannelNotify, ChannelRef, NotifyLevel, TeamEntry};
use teams_proto::time::now_ms;

/// Key of the saved state in the store's key/value table.
const KV_KEY: &str = "lists";
/// Key of the account's sections and favorites as last read from Teams: the list draws them at
/// start-up, before `conversationFolders` answers.
const KV_SERVER: &str = "lists-server";
/// The id "Move to section" uses for Favorites.
pub(super) const FAVORITES: &str = "favorites";
/// The account's lists are fetched again at most this often.
const REFRESH_EVERY_MS: i64 = 60_000;
/// Longest section name.
const NAME_MAX: usize = 64;
/// How often the timed mutes are looked at.
const MUTE_TICK_EVERY: std::time::Duration = std::time::Duration::from_secs(15);
/// The hour "until tomorrow" means for a mute.
const MUTE_MORNING: (u32, u32) = (8, 0);

/// What "Manage apps" says: IronTeams knows no read of the apps installed in one chat.
pub const MANAGE_APPS_NOTE: &str =
    "Managing a chat's apps isn't available in IronTeams yet. Use Teams to add or remove them.";

/// Commands of the list UI (`ListStore.command(name, argument)`).
#[derive(Debug, Clone, PartialEq)]
pub enum ListsCmd {
    /// A section made here; the chat is moved into it when given.
    NewSection {
        move_chat: Option<String>,
        name: String,
    },
    RenameSection(String, String),
    DeleteSection(String),
    ToggleSection(String),
    CollapseAll,
    /// Move an item to a section (`favorites` or a section id).
    Move(String, String),
    RemoveFromSection(String),
    Mute(String),
    /// Mute a chat for a while (`1h`, `8h`, `tomorrow`, `week`; see [`mute_until_for`]).
    MuteFor(String, String),
    Unmute(String),
    CopyLink(String),
    Hide(String),
    /// The chat header's "Remove chat history": the messages so far are gone for this user.
    RemoveHistory(String),
    /// The chat item menu's "Manage apps": no read of a chat's installed apps is known, so it
    /// says so (see `MANAGE_APPS_NOTE`).
    ManageApps(String),
    /// The chat item menu's "Notify when available" (or "Turn off notifications") for the person
    /// of a 1:1 chat, by their MRI (see `notify_available`).
    NotifyWhenAvailable(String, bool),
    /// `previews`, `times` or `combined`, on or off.
    Option(String, bool),
    ChannelMenu(String),
    ChannelShow(String),
    ChannelHide(String),
    ChannelNotify(String, String),
    ChannelLink(String),
    ChannelTeam(String),
    ChannelLeave(String),
}

impl ListsCmd {
    pub fn parse(name: &str, arg: &str) -> Option<ListsCmd> {
        let (a, b) = arg.split_once('\t').unwrap_or((arg, ""));
        let (a, b) = (a.to_string(), b.to_string());
        Some(match name {
            "new-section" => ListsCmd::NewSection {
                move_chat: (!a.is_empty()).then_some(a),
                name: b,
            },
            "rename-section" => ListsCmd::RenameSection(a, b),
            "delete-section" => ListsCmd::DeleteSection(a),
            "section-toggle" => ListsCmd::ToggleSection(a),
            "collapse-all" => ListsCmd::CollapseAll,
            "move" => ListsCmd::Move(a, b),
            "remove-section" => ListsCmd::RemoveFromSection(a),
            "mute" => ListsCmd::Mute(a),
            "mute-for" => ListsCmd::MuteFor(a, b),
            "unmute" => ListsCmd::Unmute(a),
            "copy-link" => ListsCmd::CopyLink(a),
            "hide" => ListsCmd::Hide(a),
            "remove-history" => ListsCmd::RemoveHistory(a),
            "manage-apps" => ListsCmd::ManageApps(a),
            "notify-available" => ListsCmd::NotifyWhenAvailable(a, true),
            "notify-available-off" => ListsCmd::NotifyWhenAvailable(a, false),
            "option" => ListsCmd::Option(a, b == "on"),
            "channel-menu" => ListsCmd::ChannelMenu(a),
            "channel-show" => ListsCmd::ChannelShow(a),
            "channel-hide" => ListsCmd::ChannelHide(a),
            "channel-notify" => ListsCmd::ChannelNotify(a, b),
            "channel-link" => ListsCmd::ChannelLink(a),
            "channel-team" => ListsCmd::ChannelTeam(a),
            "channel-leave" => ListsCmd::ChannelLeave(a),
            _ => return None,
        })
    }
}

/// What the user changed on this computer. Saved as JSON in the store.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Local {
    /// Sections made here: `(id, name)`, in creation order.
    pub sections: Vec<(String, String)>,
    /// Where an item was moved to: a section id, or `""` for "removed from its section".
    pub assign: HashMap<String, String>,
    /// Whether a section is open, when the user chose.
    pub expanded: HashMap<String, bool>,
    /// Chats hidden from the list until they get a newer message: id, time of hiding.
    pub hidden: HashMap<String, i64>,
    /// People to notify when they turn Available (`notify_available`), by MRI.
    pub watched: BTreeSet<String>,
    /// Chats muted for a while: id, time the mute ends. Teams only knows muted or not.
    pub mute_until: HashMap<String, i64>,
    pub opts: ListOptions,
}

impl Local {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "sections": self.sections.iter().map(|(i, n)| json!([i, n])).collect::<Vec<_>>(),
            "assign": self.assign,
            "expanded": self.expanded,
            "hidden": self.hidden,
            "watched": self.watched,
            "mute_until": self.mute_until,
            "opts": {"previews": self.opts.previews, "times": self.opts.times, "combined": self.opts.combined},
        })
    }

    pub(super) fn from_json(v: &Value) -> Local {
        let map = |k: &str| -> Vec<(String, Value)> {
            v.get(k)
                .and_then(Value::as_object)
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default()
        };
        let flag = |k: &str, default: bool| {
            v.pointer(&format!("/opts/{k}"))
                .and_then(Value::as_bool)
                .unwrap_or(default)
        };
        Local {
            sections: v
                .get("sections")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some((
                        p.get(0)?.as_str()?.to_string(),
                        p.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect(),
            assign: map("assign")
                .into_iter()
                .filter_map(|(k, v)| Some((k, v.as_str()?.to_string())))
                .collect(),
            expanded: map("expanded")
                .into_iter()
                .filter_map(|(k, v)| Some((k, v.as_bool()?)))
                .collect(),
            hidden: map("hidden")
                .into_iter()
                .filter_map(|(k, v)| Some((k, v.as_i64()?)))
                .collect(),
            mute_until: map("mute_until")
                .into_iter()
                .filter_map(|(k, v)| Some((k, v.as_i64()?)))
                .collect(),
            watched: v
                .get("watched")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|m| m.as_str().map(str::to_owned))
                .collect(),
            opts: ListOptions {
                previews: flag("previews", true),
                times: flag("times", true),
                combined: flag("combined", false),
            },
        }
    }
}

/// A section of the account, as read from Teams.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ServerSection {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub items: Vec<String>,
}

/// The account's sections and favorites as saved for the next start.
fn server_to_json(sections: &[ServerSection], favs: &HashSet<String>) -> Value {
    let mut favs: Vec<&String> = favs.iter().collect();
    favs.sort();
    json!({
        "sections": sections
            .iter()
            .map(|s| json!({"id": s.id, "name": s.name, "expanded": s.expanded, "items": s.items}))
            .collect::<Vec<_>>(),
        "favorites": favs,
    })
}

/// The saved sections and favorites; anything unreadable is left out.
fn server_from_json(v: &Value) -> (Vec<ServerSection>, HashSet<String>) {
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect()
    };
    let sections = v
        .get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(ServerSection {
                id: s.get("id")?.as_str()?.to_string(),
                name: s.get("name")?.as_str()?.to_string(),
                expanded: s.get("expanded").and_then(Value::as_bool).unwrap_or(true),
                items: strings(s.get("items")),
            })
        })
        .collect();
    (sections, strings(v.get("favorites")).into_iter().collect())
}

/// What IronTeams knows about a channel (from the aggregator).
#[derive(Debug, Clone)]
pub(super) struct ChannelInfo {
    pub team_id: String,
    pub group_id: String,
    pub name: String,
    pub kind: ChannelKind,
    pub shown: bool,
    pub general: bool,
    pub notify: Option<NotifyLevel>,
}

/// Engine state of the list pieces.
#[derive(Default)]
pub(super) struct State {
    local: Local,
    loaded: bool,
    server: Vec<ServerSection>,
    server_favs: HashSet<String>,
    /// `server` and `server_favs` came from Teams in this run (not from the saved copy).
    server_known: bool,
    muted: HashSet<String>,
    channels: HashMap<String, ChannelInfo>,
    last_refresh_ms: i64,
}

/// Where an item sits in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Placement {
    /// Chats, or Teams and channels.
    Default,
    Favorites,
    Section(String),
}

/// A snapshot of the list state for one render.
pub(super) struct Overlay {
    local: Local,
    server_of: HashMap<String, String>,
    server_favs: HashSet<String>,
    muted: HashSet<String>,
    hidden_channels: HashSet<String>,
    unknown_sections: HashSet<String>,
}

impl Overlay {
    pub(super) fn is_muted(&self, id: &str) -> bool {
        self.muted.contains(id)
    }

    /// When a timed mute of the chat ends, as Unix milliseconds.
    pub(super) fn mute_ends(&self, id: &str) -> Option<i64> {
        self.local.mute_until.get(id).copied()
    }

    /// A chat hidden by the user stays hidden until it gets a newer message; a hidden channel
    /// stays out of the chat list (the Teams view shows it dimmed).
    pub(super) fn is_hidden(&self, id: &str, last_activity_ms: i64) -> bool {
        self.hidden_channels.contains(id)
            || self
                .local
                .hidden
                .get(id)
                .is_some_and(|at| last_activity_ms <= *at)
    }

    /// The person is on the "Notify when available" list.
    pub(super) fn is_watching(&self, person: &str) -> bool {
        self.local.watched.contains(person)
    }

    /// The group Teams-and-channels rows belong to when the user chose to combine them.
    pub(super) fn combined(&self) -> bool {
        self.local.opts.combined
    }

    /// Where `id` goes. A move made here wins; then the account's sections; then Favorites.
    pub(super) fn placement(&self, id: &str, starred: bool) -> Placement {
        let known = |s: &str| {
            !self.unknown_sections.contains(s)
                && (s == FAVORITES
                    || self.local.sections.iter().any(|(i, _)| i == s)
                    || self.server_of.values().any(|v| v == s))
        };
        match self.local.assign.get(id).map(String::as_str) {
            Some("") => return Placement::Default,
            Some(FAVORITES) => return Placement::Favorites,
            Some(s) if known(s) => return Placement::Section(s.to_string()),
            _ => {}
        }
        if let Some(s) = self.server_of.get(id) {
            return Placement::Section(s.clone());
        }
        if starred || self.server_favs.contains(id) {
            return Placement::Favorites;
        }
        Placement::Default
    }
}

/// Percent-encodes everything but letters, digits and `-_.~`.
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `https://teams.microsoft.com/l/chat/<id>/0`
pub(super) fn chat_link(conversation_id: &str) -> String {
    format!(
        "https://teams.microsoft.com/l/chat/{}/0",
        enc(conversation_id)
    )
}

/// `https://teams.microsoft.com/l/channel/<id>/<name>?groupId=<group>&tenantId=<tenant>`
pub(super) fn channel_link(
    channel_id: &str,
    name: &str,
    group_id: &str,
    tenant_id: &str,
) -> String {
    let mut link = format!(
        "https://teams.microsoft.com/l/channel/{}/{}",
        enc(channel_id),
        enc(name)
    );
    let mut query = Vec::new();
    if !group_id.is_empty() {
        query.push(format!("groupId={}", enc(group_id)));
    }
    if !tenant_id.is_empty() {
        query.push(format!("tenantId={}", enc(tenant_id)));
    }
    if !query.is_empty() {
        link.push('?');
        link.push_str(&query.join("&"));
    }
    link
}

fn notify_level(s: &str) -> Option<NotifyLevel> {
    Some(match s {
        "all" => NotifyLevel::BannerAndFeed,
        "feed" => NotifyLevel::Feed,
        "off" => NotifyLevel::Off,
        _ => return None,
    })
}

fn level_name(l: NotifyLevel) -> &'static str {
    match l {
        NotifyLevel::BannerAndFeed => "all",
        NotifyLevel::Feed => "feed",
        NotifyLevel::Off => "off",
    }
}

/// A usable section name: trimmed, not empty, not too long.
pub(super) fn clean_name(name: &str) -> Option<String> {
    let n = name.trim();
    (!n.is_empty() && n.chars().count() <= NAME_MAX).then(|| n.to_string())
}

/// When a mute for `kind` (`1h`, `8h`, `tomorrow`, `week`) started at `now_ms` ends, as Unix
/// milliseconds: the hours and the week count from now, `tomorrow` is 08:00 the next local day.
/// `None` for any other name.
pub(crate) fn mute_until_for<Tz: TimeZone>(kind: &str, now_ms: i64, tz: &Tz) -> Option<i64> {
    let hours = |h: i64| Some(now_ms + h * 3_600_000);
    match kind {
        "1h" => hours(1),
        "8h" => hours(8),
        "week" => hours(7 * 24),
        "tomorrow" => {
            let day = tz.timestamp_millis_opt(now_ms).single()?.date_naive() + Days::days(1);
            let (h, m) = MUTE_MORNING;
            let t = day.and_time(NaiveTime::from_hms_opt(h, m, 0)?);
            // 08:00 can fall in a clock change; the first valid time after it will do.
            (0..=3)
                .find_map(|i| tz.from_local_datetime(&(t + Days::hours(i))).earliest())
                .map(|d| d.timestamp_millis())
        }
        _ => None,
    }
}

/// The chats whose timed mute has ended at `now_ms`, in id order.
pub(crate) fn expired_mutes(mute_until: &HashMap<String, i64>, now_ms: i64) -> Vec<String> {
    let mut ids: Vec<_> = mute_until
        .iter()
        .filter(|(_, until)| **until <= now_ms)
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// The end of a mute as the menu and the banner say it: the time when it is today ("14:30"),
/// else the day too ("Tomorrow 08:00").
pub(crate) fn until_label<Tz: TimeZone>(until_ms: i64, now_ms: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (Some(until), Some(now)) = (
        tz.timestamp_millis_opt(until_ms).single(),
        tz.timestamp_millis_opt(now_ms).single(),
    ) else {
        return String::new();
    };
    if until.date_naive() == now.date_naive() {
        appearance::formats::time_of(&until)
    } else {
        super::remind::when_label(until_ms, now_ms, tz)
    }
}

impl Engine {
    /// Loads the saved state once, and tells the UI the options.
    fn lists_load(&self) {
        if self.lists.lock().unwrap().loaded {
            return;
        }
        // Read before taking the lists' lock: the sync and the Teams view take the lists' lock
        // while they hold the store's, so taking them the other way round could deadlock both.
        let (saved, last) = {
            let store = self.store.lock().unwrap();
            let read = |key: &str| {
                store
                    .get_kv(key)
                    .ok()
                    .flatten()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            };
            (read(KV_KEY), read(KV_SERVER))
        };
        let fresh = {
            let mut st = self.lists.lock().unwrap();
            if st.loaded {
                return;
            }
            st.loaded = true;
            if let Some(v) = saved {
                st.local = Local::from_json(&v);
            }
            // The sections Teams had last time show at once; the refresh replaces them.
            if !st.server_known
                && let Some(v) = last
            {
                (st.server, st.server_favs) = server_from_json(&v);
            }
            st.local.opts
        };
        self.ui.list_options(fresh);
    }

    fn lists_save(&self) {
        let json = self.lists.lock().unwrap().local.to_json().to_string();
        if let Err(err) = self.store.lock().unwrap().set_kv(KV_KEY, &json) {
            tracing::warn!(%err, "saving the list state failed");
        }
    }

    /// "Notify when available" was turned on or off for a person. Saved with the list state; a
    /// person turned on is looked up at once, so the first status is known before it changes.
    fn set_watching(&self, person: &str, on: bool) -> anyhow::Result<()> {
        if on && !super::notify_available::can_watch(person, &self.me) {
            return Ok(());
        }
        let changed = {
            let mut st = self.lists.lock().unwrap();
            if on {
                st.local.watched.insert(person.to_string())
            } else {
                st.local.watched.remove(person)
            }
        };
        if !changed {
            return Ok(());
        }
        self.lists_save();
        if on {
            super::people_presence::want(person);
        }
        self.mark(|d| d.chats = true);
        Ok(())
    }

    /// Whether the person is on the "Notify when available" list.
    pub(super) fn is_watching(&self, person: &str) -> bool {
        self.lists_load();
        self.lists.lock().unwrap().local.watched.contains(person)
    }

    /// Everyone on the "Notify when available" list.
    pub(super) fn watched_people(&self) -> Vec<String> {
        self.lists_load();
        self.lists
            .lock()
            .unwrap()
            .local
            .watched
            .iter()
            .cloned()
            .collect()
    }

    /// A snapshot for rendering the chat list.
    pub(super) fn lists_overlay(&self) -> Overlay {
        self.lists_load();
        let st = self.lists.lock().unwrap();
        let mut server_of = HashMap::new();
        for s in &st.server {
            for item in &s.items {
                server_of
                    .entry(item.clone())
                    .or_insert_with(|| s.id.clone());
            }
        }
        Overlay {
            local: st.local.clone(),
            server_of,
            server_favs: st.server_favs.clone(),
            muted: st.muted.clone(),
            hidden_channels: st
                .channels
                .iter()
                .filter(|(_, c)| !c.shown)
                .map(|(id, _)| id.clone())
                .collect(),
            unknown_sections: HashSet::new(),
        }
    }

    /// True when a new message in this chat or channel must not pop a notification.
    pub(super) fn lists_silenced(&self, conversation_id: &str) -> bool {
        // A snoozed chat is silent until it comes back.
        if self.remind_is_snoozed(conversation_id) {
            return true;
        }
        let st = self.lists.lock().unwrap();
        st.muted.contains(conversation_id)
            || st
                .channels
                .get(conversation_id)
                .is_some_and(|c| c.notify == Some(NotifyLevel::Off))
    }

    /// The sections for the UI: the account's, then the ones made here, with their rows.
    pub(super) fn push_sections(&self, mut by_section: HashMap<String, Vec<ChatData>>) {
        let st = self.lists.lock().unwrap();
        let mut sections = Vec::new();
        let mut choices = vec![ChoiceData {
            id: FAVORITES.into(),
            name: "Favorites".into(),
        }];
        let rows: Vec<(String, String, bool, bool)> = st
            .server
            .iter()
            .map(|s| (s.id.clone(), s.name.clone(), s.expanded, false))
            .chain(
                st.local
                    .sections
                    .iter()
                    .map(|(i, n)| (i.clone(), n.clone(), true, true)),
            )
            .collect();
        for (id, name, server_expanded, local) in rows {
            let mut items = by_section.remove(&id).unwrap_or_default();
            // A section of the account keeps Teams' own order; the rest stay newest first.
            if let Some(s) = st.server.iter().find(|s| s.id == id) {
                items.sort_by_key(|c| {
                    s.items
                        .iter()
                        .position(|x| *x == c.id)
                        .unwrap_or(usize::MAX)
                });
            }
            choices.push(ChoiceData {
                id: id.clone(),
                name: name.clone(),
            });
            sections.push(SectionData {
                expanded: st
                    .local
                    .expanded
                    .get(&id)
                    .copied()
                    .unwrap_or(server_expanded),
                unread: items.iter().filter(|c| c.unread).count() as i32,
                id,
                name,
                local,
                items,
            });
        }
        drop(st);
        self.ui.sections(sections, choices);
    }

    /// Reads the mute flags of the chats the aggregator listed.
    pub(super) fn lists_note_chats(&self, chats: &[teams_proto::csa::ChatMeta]) {
        let mut st = self.lists.lock().unwrap();
        for c in chats {
            if c.muted_by.contains(&self.me) {
                st.muted.insert(c.id.clone());
            } else {
                st.muted.remove(&c.id);
            }
        }
    }

    /// Fetches the account's sections and channel state (best effort, at most once a minute).
    pub(super) async fn lists_refresh(self: &Arc<Self>) {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        {
            let mut st = self.lists.lock().unwrap();
            let now = now_ms();
            if now - st.last_refresh_ms < REFRESH_EVERY_MS {
                return;
            }
            st.last_refresh_ms = now;
        }
        match self.client.conversation_folders().await {
            Ok(f) => self.lists_apply_folders(&f),
            Err(err) => tracing::warn!(%err, "reading the chat list sections failed"),
        }
        match self.client.teams_and_channels().await {
            Ok(t) => self.lists_apply_teams(&t),
            Err(err) => tracing::warn!(%err, "reading the channel state failed"),
        }
        self.mark(|d| {
            d.chats = true;
            d.teams = true;
        });
    }

    pub(super) fn lists_apply_folders(&self, f: &Folders) {
        let mut st = self.lists.lock().unwrap();
        st.server_known = true;
        st.server = f
            .folders
            .iter()
            .filter(|x| x.kind == FolderKind::User)
            .map(|x| ServerSection {
                id: x.id.clone(),
                name: x.name.clone(),
                expanded: x.expanded,
                items: x.items.iter().map(|i| i.conversation_id.clone()).collect(),
            })
            .collect();
        st.server_favs = f
            .favorites()
            .map(|x| x.items.iter().map(|i| i.conversation_id.clone()).collect())
            .unwrap_or_default();
        let json = server_to_json(&st.server, &st.server_favs).to_string();
        drop(st);
        if let Err(err) = self.store.lock().unwrap().set_kv(KV_SERVER, &json) {
            tracing::warn!(%err, "saving the chat list sections failed");
        }
    }

    pub(super) fn lists_apply_teams(&self, teams: &[TeamEntry]) {
        let mut st = self.lists.lock().unwrap();
        st.channels.clear();
        let mut names = Vec::new();
        for t in teams.iter().filter(|t| !t.is_archived && !t.is_deleted) {
            for c in t.channels.iter().filter(|c| !c.is_deleted) {
                if c.is_muted {
                    st.muted.insert(c.id.clone());
                }
                names.push((c.id.clone(), c.name.clone()));
                st.channels.insert(
                    c.id.clone(),
                    ChannelInfo {
                        team_id: t.id.clone(),
                        group_id: t.group_id.clone(),
                        name: c.name.clone(),
                        kind: c.kind,
                        shown: c.is_shown,
                        general: c.is_general,
                        notify: None,
                    },
                );
            }
        }
        drop(st);
        // The stored titles come from the sync, which may have had no name for a channel (a
        // private one) and stored a stand-in: the names of this list correct them.
        if let Err(err) = self.store.lock().unwrap().set_channel_titles(&names) {
            tracing::warn!(%err, "correcting the channel titles failed");
        }
    }

    /// The Graph group id of a channel's team, when the aggregator listed the channel.
    pub(super) fn lists_channel_group(&self, id: &str) -> Option<String> {
        self.lists
            .lock()
            .unwrap()
            .channels
            .get(id)
            .map(|c| c.group_id.clone())
            .filter(|g| !g.is_empty())
    }

    /// Whether the channel is a private one (the Teams view draws a lock after its name).
    pub(super) fn lists_channel_private(&self, id: &str) -> bool {
        self.lists
            .lock()
            .unwrap()
            .channels
            .get(id)
            .is_some_and(|c| c.kind == ChannelKind::Private)
    }

    /// Whether the Teams view should draw this channel dimmed (hidden in Teams).
    pub(super) fn lists_channel_hidden(&self, id: &str) -> bool {
        self.lists
            .lock()
            .unwrap()
            .channels
            .get(id)
            .is_some_and(|c| !c.shown)
    }

    pub(super) async fn lists_cmd(self: &Arc<Self>, cmd: ListsCmd) -> anyhow::Result<()> {
        self.lists_load();
        match cmd {
            ListsCmd::NewSection { move_chat, name } => self.new_section(move_chat, &name),
            ListsCmd::RenameSection(id, name) => self.rename_section(&id, &name),
            ListsCmd::DeleteSection(id) => self.delete_section(&id),
            ListsCmd::ToggleSection(id) => self.toggle_section(&id),
            ListsCmd::CollapseAll => self.collapse_all(),
            ListsCmd::Move(conv, to) => self.move_item(&conv, &to),
            ListsCmd::RemoveFromSection(conv) => self.move_item(&conv, ""),
            ListsCmd::Mute(conv) => self.set_mute(&conv, true, None).await,
            ListsCmd::MuteFor(conv, kind) => {
                let now = now_ms();
                let Some(until) = mute_until_for(&kind, now, &chrono::Local) else {
                    return Ok(());
                };
                self.set_mute(&conv, true, Some(until)).await?;
                self.banner(&format!(
                    "Chat muted until {}",
                    until_label(until, now, &chrono::Local)
                ));
                Ok(())
            }
            ListsCmd::Unmute(conv) => self.set_mute(&conv, false, None).await,
            ListsCmd::CopyLink(conv) => self.copy_text(&chat_link(&conv), "Chat link copied"),
            ListsCmd::Hide(conv) => self.hide_chat(&conv).await,
            ListsCmd::RemoveHistory(conv) => self.remove_history(&conv).await,
            ListsCmd::ManageApps(_) => {
                self.ui.banner(MANAGE_APPS_NOTE.into());
                Ok(())
            }
            ListsCmd::NotifyWhenAvailable(person, on) => self.set_watching(&person, on),
            ListsCmd::Option(name, on) => self.set_option(&name, on),
            ListsCmd::ChannelMenu(id) => self.channel_menu(&id).await,
            ListsCmd::ChannelShow(id) => self.set_channel_shown(&id, true).await,
            ListsCmd::ChannelHide(id) => self.set_channel_shown(&id, false).await,
            ListsCmd::ChannelNotify(id, level) => self.set_channel_notify(&id, &level).await,
            ListsCmd::ChannelLink(id) => self.copy_channel_link(&id).await,
            ListsCmd::ChannelTeam(id) => {
                self.go_to_team(&id);
                Ok(())
            }
            ListsCmd::ChannelLeave(id) => self.leave_channel(&id).await,
        }
    }

    fn banner(&self, text: &str) {
        if !self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            self.ui.banner(text.to_string());
        }
    }

    fn copy_text(&self, text: &str, done: &str) -> anyhow::Result<()> {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            self.ui.banner(done.to_string());
            return Ok(());
        }
        arboard::Clipboard::new()?.set_text(text.to_string())?;
        self.ui.banner(done.to_string());
        Ok(())
    }

    fn dirty_lists(&self) {
        self.mark(|d| {
            d.chats = true;
            d.teams = true;
        });
    }

    // ---- sections ----

    fn section_names(&self) -> Vec<String> {
        let st = self.lists.lock().unwrap();
        st.server
            .iter()
            .map(|s| s.name.clone())
            .chain(st.local.sections.iter().map(|(_, n)| n.clone()))
            .collect()
    }

    fn new_section(&self, move_chat: Option<String>, name: &str) -> anyhow::Result<()> {
        let Some(name) = clean_name(name) else {
            self.banner("Give the section a name of up to 64 characters");
            return Ok(());
        };
        if self
            .section_names()
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&name))
            || name.eq_ignore_ascii_case("favorites")
        {
            self.banner("A section with that name already exists");
            return Ok(());
        }
        let id = format!("local-{}", now_ms());
        {
            let mut st = self.lists.lock().unwrap();
            st.local.sections.push((id.clone(), name));
        }
        if let Some(conv) = move_chat {
            self.move_item(&conv, &id)?;
        } else {
            self.lists_save();
            self.dirty_lists();
        }
        Ok(())
    }

    fn rename_section(&self, id: &str, name: &str) -> anyhow::Result<()> {
        let Some(name) = clean_name(name) else {
            self.banner("Give the section a name of up to 64 characters");
            return Ok(());
        };
        {
            let mut st = self.lists.lock().unwrap();
            let Some(s) = st.local.sections.iter_mut().find(|(i, _)| i == id) else {
                drop(st);
                self.banner("Sections made in Teams are renamed in Teams");
                return Ok(());
            };
            s.1 = name;
        }
        self.lists_save();
        self.dirty_lists();
        Ok(())
    }

    fn delete_section(&self, id: &str) -> anyhow::Result<()> {
        {
            let mut st = self.lists.lock().unwrap();
            let before = st.local.sections.len();
            st.local.sections.retain(|(i, _)| i != id);
            if st.local.sections.len() == before {
                drop(st);
                self.banner("Sections made in Teams are deleted in Teams");
                return Ok(());
            }
            // Its items go back to Chats and channels.
            st.local.assign.retain(|_, s| s != id);
            st.local.expanded.remove(id);
        }
        self.lists_save();
        self.dirty_lists();
        Ok(())
    }

    fn toggle_section(&self, id: &str) -> anyhow::Result<()> {
        {
            let mut st = self.lists.lock().unwrap();
            let server_open = st
                .server
                .iter()
                .find(|s| s.id == id)
                .is_none_or(|s| s.expanded);
            let open = st.local.expanded.get(id).copied().unwrap_or(server_open);
            st.local.expanded.insert(id.to_string(), !open);
        }
        self.lists_save();
        self.mark(|d| d.chats = true);
        Ok(())
    }

    /// "Collapse all sections" (Alt+Q).
    fn collapse_all(&self) -> anyhow::Result<()> {
        {
            let mut st = self.lists.lock().unwrap();
            let ids: Vec<String> = st
                .server
                .iter()
                .map(|s| s.id.clone())
                .chain(st.local.sections.iter().map(|(i, _)| i.clone()))
                .collect();
            for id in ids {
                st.local.expanded.insert(id, false);
            }
        }
        self.ui.fixed_sections_open(false);
        self.lists_save();
        self.mark(|d| d.chats = true);
        Ok(())
    }

    /// Moves an item: `to` is `favorites`, a section id, or `""` for "Remove from section".
    fn move_item(&self, conv: &str, to: &str) -> anyhow::Result<()> {
        {
            let st = self.store.lock().unwrap();
            // The star is the local Favorites flag; it follows the move.
            st.set_favorite(conv, to == FAVORITES)?;
        }
        {
            let mut st = self.lists.lock().unwrap();
            st.local.assign.insert(conv.to_string(), to.to_string());
        }
        self.lists_save();
        self.dirty_lists();
        Ok(())
    }

    /// The star toggled: a move made here would hide the change, so it is dropped.
    pub(super) fn lists_clear_assign(&self, conv: &str) {
        self.lists_load();
        let removed = self
            .lists
            .lock()
            .unwrap()
            .local
            .assign
            .remove(conv)
            .is_some();
        if removed {
            self.lists_save();
        }
    }

    // ---- chat item menu ----

    /// Turns the Teams mute of a chat on or off. `until` is when a mute ends (`None`: it lasts
    /// until the user turns it off); it is kept here, as Teams has no end time.
    async fn set_mute(
        self: &Arc<Self>,
        conv: &str,
        muted: bool,
        until: Option<i64>,
    ) -> anyhow::Result<()> {
        let until = until.filter(|_| muted);
        let (was, was_until) = {
            let mut st = self.lists.lock().unwrap();
            let was = st.muted.contains(conv);
            let was_until = st.local.mute_until.get(conv).copied();
            if muted {
                st.muted.insert(conv.to_string());
            } else {
                st.muted.remove(conv);
            }
            match until {
                Some(t) => st.local.mute_until.insert(conv.to_string(), t),
                None => st.local.mute_until.remove(conv),
            };
            (was, was_until)
        };
        if was_until != until {
            self.lists_save();
        }
        // The chat header's "More chat options" shows Mute or Unmute too.
        self.mark(|d| {
            d.chats = true;
            d.header = true;
        });
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        if let Err(err) = self.client.set_muted(conv, muted).await {
            // The account refused: put the old state back.
            let mut st = self.lists.lock().unwrap();
            if was {
                st.muted.insert(conv.to_string());
            } else {
                st.muted.remove(conv);
            }
            match was_until {
                Some(t) => st.local.mute_until.insert(conv.to_string(), t),
                None => st.local.mute_until.remove(conv),
            };
            drop(st);
            if was_until != until {
                self.lists_save();
            }
            self.mark(|d| {
                d.chats = true;
                d.header = true;
            });
            return Err(err.into());
        }
        Ok(())
    }

    /// Unmutes the chats whose timed mute has ended at `now`, the way the user's Unmute does. One
    /// the account refuses keeps its end time and is tried again at the next tick.
    pub(super) async fn lists_expire_mutes(self: &Arc<Self>, now: i64) {
        self.lists_load();
        let ended = expired_mutes(&self.lists.lock().unwrap().local.mute_until, now);
        for conv in ended {
            if let Err(err) = self.set_mute(&conv, false, None).await {
                tracing::warn!(%err, "unmuting a chat whose timed mute ended failed");
            }
        }
    }

    /// Looks at the timed mutes now (those that ended while the app was closed) and then on a
    /// timer. Not while offline: the account's mute could not be turned off.
    pub(super) fn start_mute_expiry(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                if !e.offline.load(std::sync::atomic::Ordering::Relaxed) {
                    e.lists_expire_mutes(now_ms()).await;
                }
                tokio::time::sleep(MUTE_TICK_EVERY).await;
            }
        });
    }

    async fn hide_chat(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
        let at = now_ms();
        self.lists
            .lock()
            .unwrap()
            .local
            .hidden
            .insert(conv.to_string(), at);
        self.lists_save();
        let open_here = self.view.lock().unwrap().current.as_deref() == Some(conv);
        self.mark(|d| d.chats = true);
        if open_here {
            self.banner("Chat hidden. It comes back with its next message.");
        }
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        // Best effort: Teams hides the chat for this account too.
        if let Err(err) = self.client.set_hidden(conv, true).await {
            tracing::warn!(%err, "hiding the chat in Teams failed");
        }
        Ok(())
    }

    /// "Remove chat history" (chat header, "More chat options"): Teams clears the history up to
    /// now for this user only. Here the cached messages and draft go, and the chat leaves the
    /// list until a newer message arrives. The account is written first; when it refuses,
    /// nothing changes here.
    async fn remove_history(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
        let at = now_ms();
        if !self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            self.client.clear_history(conv, at).await?;
        }
        self.store.lock().unwrap().delete_conversation(conv)?;
        self.lists
            .lock()
            .unwrap()
            .local
            .hidden
            .insert(conv.to_string(), at);
        self.lists_save();
        let was_open = {
            let mut v = self.view.lock().unwrap();
            let open = v.current.as_deref() == Some(conv);
            if open {
                v.current = None;
                v.thread_root = None;
            }
            open
        };
        self.mark(|d| {
            d.chats = true;
            d.header = true;
            d.messages = true;
        });
        if was_open {
            self.banner("Chat history removed");
        }
        Ok(())
    }

    // ---- filters and options ----

    /// The previews and times options (tests of the command order).
    #[cfg(test)]
    pub(super) fn list_options_for_test(&self) -> (bool, bool) {
        let st = self.lists.lock().unwrap();
        (st.local.opts.previews, st.local.opts.times)
    }

    fn set_option(&self, name: &str, on: bool) -> anyhow::Result<()> {
        let opts = {
            let mut st = self.lists.lock().unwrap();
            match name {
                "previews" => st.local.opts.previews = on,
                "times" => st.local.opts.times = on,
                "combined" => st.local.opts.combined = on,
                _ => return Ok(()),
            }
            st.local.opts
        };
        self.lists_save();
        self.ui.list_options(opts);
        self.mark(|d| d.chats = true);
        Ok(())
    }

    // ---- channel menu ----

    pub(super) fn channel_info(&self, id: &str) -> Option<ChannelInfo> {
        self.lists.lock().unwrap().channels.get(id).cloned()
    }

    fn push_channel_menu(&self, id: &str) {
        let (info, level) = {
            let st = self.lists.lock().unwrap();
            let info = st.channels.get(id).cloned();
            let level = info
                .as_ref()
                .and_then(|c| c.notify)
                .map_or("feed", level_name);
            (info, level)
        };
        self.ui.channel_menu(ChannelMenuData {
            level,
            private: info
                .as_ref()
                .is_some_and(|c| c.kind == ChannelKind::Private),
            hidden: info.as_ref().is_some_and(|c| !c.shown),
            pinned: self.pinned_ids().iter().any(|p| p == id),
        });
    }

    async fn channel_menu(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        self.push_channel_menu(id);
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        // The notification level is read when the menu opens.
        if let Ok(n) = self.client.channel_notifications(id).await {
            if let Some(c) = self.lists.lock().unwrap().channels.get_mut(id) {
                c.notify = Some(n.level);
            }
            self.push_channel_menu(id);
        }
        Ok(())
    }

    async fn set_channel_shown(self: &Arc<Self>, id: &str, shown: bool) -> anyhow::Result<()> {
        let Some(info) = self.channel_info(id) else {
            self.banner("That channel is not in your team list");
            return Ok(());
        };
        if let Some(c) = self.lists.lock().unwrap().channels.get_mut(id) {
            c.shown = shown;
        }
        self.dirty_lists();
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let r = ChannelRef {
            id: id.to_string(),
            kind: info.kind,
        };
        if let Err(err) = self.client.set_channel_shown(&r, info.general, shown).await {
            if let Some(c) = self.lists.lock().unwrap().channels.get_mut(id) {
                c.shown = !shown;
            }
            self.dirty_lists();
            return Err(err.into());
        }
        Ok(())
    }

    async fn set_channel_notify(self: &Arc<Self>, id: &str, level: &str) -> anyhow::Result<()> {
        let Some(l) = notify_level(level) else {
            return Ok(());
        };
        // Shown at once, so the menu opened right after has it; put back when the service
        // refuses, unless another choice came meanwhile.
        let before = self
            .lists
            .lock()
            .unwrap()
            .channels
            .get_mut(id)
            .and_then(|c| c.notify.replace(l));
        self.push_channel_menu(id);
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let answer = self
            .client
            .set_channel_notifications(
                id,
                ChannelNotify {
                    level: l,
                    include_replies: false,
                },
            )
            .await;
        if let Err(err) = answer {
            if let Some(c) = self.lists.lock().unwrap().channels.get_mut(id)
                && c.notify == Some(l)
            {
                c.notify = before;
            }
            self.push_channel_menu(id);
            return Err(err.into());
        }
        Ok(())
    }

    async fn copy_channel_link(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let info = self.channel_info(id);
        let (name, group) = info
            .map(|c| (c.name, c.group_id))
            .unwrap_or_else(|| (String::new(), String::new()));
        let tenant = if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            String::new()
        } else {
            self.client.tenant_id().await.unwrap_or_default()
        };
        self.copy_text(
            &channel_link(id, &name, &group, &tenant),
            "Channel link copied",
        )
    }

    /// "Go to team": the Teams view with the channel's team open.
    fn go_to_team(&self, channel_id: &str) {
        let team = self.channel_info(channel_id).map(|c| c.team_id);
        if let Some(t) = team {
            self.view.lock().unwrap().collapsed_teams.remove(&t);
        }
        self.mark(|d| d.teams = true);
        self.ui.show_view("teams");
    }

    /// Leaves a private channel (the confirmation was answered in the UI).
    async fn leave_channel(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let Some(info) = self
            .channel_info(id)
            .filter(|c| c.kind == ChannelKind::Private)
        else {
            self.banner("Only private channels can be left from here");
            return Ok(());
        };
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let team = teams_proto::teams::TeamRef {
            thread_id: info.team_id.clone(),
            group_id: info.group_id.clone(),
        };
        let r = ChannelRef {
            id: id.to_string(),
            kind: info.kind,
        };
        // Off the list at once (a second Leave or the channel menu finds it gone); back when the
        // service refuses. The chat's messages are deleted only once the service agreed.
        let kept = self.lists.lock().unwrap().channels.remove(id);
        self.dirty_lists();
        if let Err(err) = self
            .client
            .remove_private_channel_member(&team, &r, &self.me)
            .await
        {
            if let Some(c) = kept {
                self.lists
                    .lock()
                    .unwrap()
                    .channels
                    .insert(id.to_string(), c);
            }
            self.dirty_lists();
            return Err(err.into());
        }
        self.store.lock().unwrap().delete_conversation(id)?;
        self.dirty_lists();
        self.banner("You left the channel");
        Ok(())
    }

    // ---- tests ----

    #[cfg(test)]
    pub(crate) fn test_lists(&self) -> (Local, Vec<ServerSection>) {
        let st = self.lists.lock().unwrap();
        (st.local.clone(), st.server.clone())
    }

    #[cfg(test)]
    pub(crate) fn test_set_folders(&self, f: &Folders) {
        self.lists_apply_folders(f);
    }

    /// The people to notify when available as saved in the store (what the next start reads).
    #[cfg(test)]
    pub(crate) fn test_saved_watchers(&self) -> Vec<String> {
        let saved = self.store.lock().unwrap().get_kv(KV_KEY).ok().flatten();
        saved
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| Local::from_json(&v).watched.into_iter().collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn test_set_teams(&self, teams: &[TeamEntry]) {
        self.lists_apply_teams(teams);
        self.mark(|d| d.teams = true);
    }

    #[cfg(test)]
    pub(crate) fn test_touch_conversation(&self, id: &str, ms: i64) {
        self.store
            .lock()
            .unwrap()
            .touch_conversation(id, ms, "new message", true)
            .unwrap();
        self.mark(|d| d.chats = true);
    }

    #[cfg(test)]
    pub(crate) fn test_silenced(&self, id: &str) -> bool {
        self.lists_silenced(id)
    }

    /// The chat is muted (the header's menu offers Unmute).
    pub(super) fn is_muted(&self, id: &str) -> bool {
        self.lists.lock().unwrap().muted.contains(id)
    }

    #[cfg(test)]
    pub(crate) fn test_is_muted(&self, id: &str) -> bool {
        self.is_muted(id)
    }

    #[cfg(test)]
    pub(crate) fn test_mute_until(&self, id: &str) -> Option<i64> {
        self.lists.lock().unwrap().local.mute_until.get(id).copied()
    }

    #[cfg(test)]
    pub(crate) fn test_expire_mutes(self: &Arc<Self>, now: i64) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(self.lists_expire_mutes(now));
    }

    #[cfg(test)]
    pub(crate) fn test_is_hidden(&self, id: &str) -> bool {
        self.lists.lock().unwrap().local.hidden.contains_key(id)
    }

    #[cfg(test)]
    pub(crate) fn test_channel_shown(&self, id: &str) -> Option<bool> {
        self.channel_info(id).map(|c| c.shown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay() -> Overlay {
        let mut local = Local::default();
        local.sections.push(("local-1".into(), "Ops".into()));
        let mut server_of = HashMap::new();
        server_of.insert("c-server".to_string(), "srv-1".to_string());
        server_of.insert("c-both".to_string(), "srv-1".to_string());
        Overlay {
            local,
            server_of,
            server_favs: ["c-fav".to_string()].into(),
            muted: HashSet::new(),
            hidden_channels: ["chan-hidden".to_string()].into(),
            unknown_sections: HashSet::new(),
        }
    }

    #[test]
    fn parses_manage_apps() {
        assert_eq!(
            ListsCmd::parse("manage-apps", "19:x"),
            Some(ListsCmd::ManageApps("19:x".into()))
        );
    }

    #[test]
    fn parses_notify_when_available_on_and_off() {
        assert_eq!(
            ListsCmd::parse("notify-available", "8:orgid:a"),
            Some(ListsCmd::NotifyWhenAvailable("8:orgid:a".into(), true))
        );
        assert_eq!(
            ListsCmd::parse("notify-available-off", "8:orgid:a"),
            Some(ListsCmd::NotifyWhenAvailable("8:orgid:a".into(), false))
        );
    }

    #[test]
    fn the_people_to_notify_survive_a_round_trip_and_an_old_save_has_none() {
        let mut l = Local::default();
        l.watched.insert("8:orgid:b".into());
        l.watched.insert("8:orgid:a".into());
        let back = Local::from_json(&l.to_json());
        assert_eq!(
            back.watched.iter().collect::<Vec<_>>(),
            ["8:orgid:a", "8:orgid:b"]
        );
        // A list saved before the feature existed has no such key.
        assert!(
            Local::from_json(&json!({"sections": []}))
                .watched
                .is_empty()
        );
        let mut o = overlay();
        assert!(!o.is_watching("8:orgid:a"));
        o.local.watched.insert("8:orgid:a".into());
        assert!(o.is_watching("8:orgid:a"));
    }

    #[test]
    fn parses_remove_history() {
        assert_eq!(
            ListsCmd::parse("remove-history", "19:x"),
            Some(ListsCmd::RemoveHistory("19:x".into()))
        );
    }

    #[test]
    fn parses_commands_with_one_or_two_arguments() {
        assert_eq!(
            ListsCmd::parse("new-section", "19:x\tOps"),
            Some(ListsCmd::NewSection {
                move_chat: Some("19:x".into()),
                name: "Ops".into()
            })
        );
        assert_eq!(
            ListsCmd::parse("new-section", "\tOps"),
            Some(ListsCmd::NewSection {
                move_chat: None,
                name: "Ops".into()
            })
        );
        assert_eq!(
            ListsCmd::parse("move", "19:x\tlocal-1"),
            Some(ListsCmd::Move("19:x".into(), "local-1".into()))
        );
        assert_eq!(
            ListsCmd::parse("option", "previews\toff"),
            Some(ListsCmd::Option("previews".into(), false))
        );
        assert_eq!(ListsCmd::parse("nope", ""), None);
    }

    #[test]
    fn local_state_survives_a_round_trip() {
        let mut l = Local::default();
        l.sections.push(("local-1".into(), "Ops".into()));
        l.assign.insert("c1".into(), "local-1".into());
        l.assign.insert("c2".into(), "".into());
        l.expanded.insert("local-1".into(), false);
        l.hidden.insert("c3".into(), 99);
        l.opts.previews = false;
        l.opts.combined = true;
        assert_eq!(Local::from_json(&l.to_json()), l);
        // Garbage or an empty value gives the defaults.
        assert_eq!(Local::from_json(&json!({})), Local::default());
        assert!(Local::default().opts.previews && Local::default().opts.times);
    }

    fn utc() -> chrono::FixedOffset {
        chrono::FixedOffset::east_opt(0).unwrap()
    }

    fn at(tz: &chrono::FixedOffset, d: u32, h: u32, m: u32) -> i64 {
        tz.with_ymd_and_hms(2026, 11, d, h, m, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn parses_a_timed_mute() {
        assert_eq!(
            ListsCmd::parse("mute-for", "c\t8h"),
            Some(ListsCmd::MuteFor("c".into(), "8h".into()))
        );
    }

    #[test]
    fn mute_durations_count_from_now_and_tomorrow_is_eight_the_next_local_day() {
        let tz = utc();
        let now = at(&tz, 20, 10, 0);
        assert_eq!(mute_until_for("1h", now, &tz), Some(now + 3_600_000));
        assert_eq!(mute_until_for("8h", now, &tz), Some(now + 8 * 3_600_000));
        assert_eq!(mute_until_for("week", now, &tz), Some(at(&tz, 27, 10, 0)));
        assert_eq!(
            mute_until_for("tomorrow", now, &tz),
            Some(at(&tz, 21, 8, 0))
        );
        // Late in the evening, tomorrow is still the next calendar day, in the local zone.
        let tz = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(
            mute_until_for("tomorrow", at(&tz, 20, 23, 50), &tz),
            Some(at(&tz, 21, 8, 0))
        );
        // Just after midnight it is not "later this morning" but the next day.
        assert_eq!(
            mute_until_for("tomorrow", at(&tz, 21, 0, 10), &tz),
            Some(at(&tz, 22, 8, 0))
        );
        // Anything else is no duration (the forever mute has no end time).
        assert_eq!(mute_until_for("", now, &tz), None);
        assert_eq!(mute_until_for("2h", now, &tz), None);
    }

    #[test]
    fn a_mute_has_ended_at_its_time_and_not_before() {
        let until: HashMap<String, i64> =
            [("b".into(), 1000), ("a".into(), 1000), ("c".into(), 2000)].into();
        assert!(expired_mutes(&until, 999).is_empty());
        assert_eq!(expired_mutes(&until, 1000), ["a", "b"]);
        assert_eq!(expired_mutes(&until, 5000), ["a", "b", "c"]);
        assert!(expired_mutes(&HashMap::new(), 5000).is_empty());
    }

    #[test]
    fn the_end_of_a_mute_says_the_time_today_and_the_day_after() {
        let tz = utc();
        let now = at(&tz, 20, 10, 0);
        assert_eq!(
            until_label(at(&tz, 20, 14, 30), now, &tz),
            appearance::formats::clock(14 * 60 + 30)
        );
        assert_eq!(
            until_label(at(&tz, 21, 8, 0), now, &tz),
            format!("Tomorrow {}", appearance::formats::clock(8 * 60))
        );
    }

    #[test]
    fn mute_end_times_survive_a_round_trip_and_an_old_save_has_none() {
        let mut l = Local::default();
        l.mute_until.insert("c1".into(), 1234);
        assert_eq!(Local::from_json(&l.to_json()), l);
        assert!(
            Local::from_json(&json!({"hidden": {"c": 1}}))
                .mute_until
                .is_empty()
        );
    }

    #[test]
    fn a_move_made_here_wins_over_teams_and_the_star() {
        let mut o = overlay();
        assert_eq!(
            o.placement("c-server", false),
            Placement::Section("srv-1".into())
        );
        assert_eq!(
            o.placement("c-fav", false),
            Placement::Favorites,
            "Teams' Favorites"
        );
        assert_eq!(
            o.placement("c-star", true),
            Placement::Favorites,
            "the local star"
        );
        assert_eq!(o.placement("plain", false), Placement::Default);
        o.local.assign.insert("c-server".into(), "local-1".into());
        assert_eq!(
            o.placement("c-server", false),
            Placement::Section("local-1".into())
        );
        o.local.assign.insert("c-both".into(), "".into());
        assert_eq!(
            o.placement("c-both", false),
            Placement::Default,
            "removed from its section"
        );
        o.local.assign.insert("c-fav".into(), FAVORITES.into());
        assert_eq!(o.placement("c-fav", false), Placement::Favorites);
        // A move to a section that no longer exists falls back to where Teams has the item.
        o.local
            .assign
            .insert("c-server".into(), "local-gone".into());
        assert_eq!(
            o.placement("c-server", false),
            Placement::Section("srv-1".into())
        );
    }

    #[test]
    fn hidden_chats_return_with_newer_activity_and_hidden_channels_stay_out() {
        let mut o = overlay();
        o.local.hidden.insert("chat".into(), 1000);
        assert!(o.is_hidden("chat", 1000));
        assert!(o.is_hidden("chat", 500));
        assert!(!o.is_hidden("chat", 1001), "a newer message brings it back");
        assert!(o.is_hidden("chan-hidden", i64::MAX));
        assert!(!o.is_hidden("other", 0));
    }

    #[test]
    fn links() {
        assert_eq!(
            chat_link("19:abc@thread.v2"),
            "https://teams.microsoft.com/l/chat/19%3Aabc%40thread.v2/0"
        );
        assert_eq!(
            channel_link("19:c@thread.tacv2", "Release plans", "g-1", "t-1"),
            "https://teams.microsoft.com/l/channel/19%3Ac%40thread.tacv2/Release%20plans?groupId=g-1&tenantId=t-1"
        );
        assert_eq!(
            channel_link("19:c@thread.tacv2", "x", "", ""),
            "https://teams.microsoft.com/l/channel/19%3Ac%40thread.tacv2/x"
        );
    }

    #[test]
    fn section_names_are_cleaned() {
        assert_eq!(clean_name("  Ops  ").as_deref(), Some("Ops"));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(&"x".repeat(65)), None);
        assert!(clean_name(&"x".repeat(64)).is_some());
    }

    #[test]
    fn notify_names_map_both_ways() {
        for n in ["all", "feed", "off"] {
            assert_eq!(level_name(notify_level(n).unwrap()), n);
        }
        assert_eq!(notify_level("loud"), None);
    }

    /// The sync and the Teams view take the lists' lock while they hold the store's (a channel's
    /// name, its team's picture). The first load of the lists read the store while it held the
    /// lists' lock, the other way round: the two could deadlock each other, and with them every
    /// task and the window waiting on the store. Waiting for the store, it must hold nothing.
    #[test]
    fn loading_the_lists_holds_no_lock_while_it_waits_for_the_store() {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let e = Engine::new(
            teams_proto::Client::offline("test").unwrap(),
            store::Store::open_in_memory().unwrap(),
            session::Config::default(),
            ui,
            "8:me".into(),
            "Me".into(),
        );
        assert!(!e.lists.lock().unwrap().loaded, "not loaded yet");
        // Something else (the sync) holds the store.
        let store = e.store.lock().unwrap();
        let loading = {
            let e = e.clone();
            std::thread::spawn(move || e.lists_load())
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        let lists_free = e.lists.try_lock().is_ok();
        drop(store);
        loading.join().unwrap();
        assert!(
            lists_free,
            "the lists' lock was held while waiting for the store's"
        );
        assert!(
            e.lists.lock().unwrap().loaded,
            "loaded once the store is free"
        );
    }

    /// A chat muted for a while is unmuted, the way Unmute does it, once its time has come, and
    /// the end time is gone from what the next start reads.
    #[test]
    fn a_timed_mute_is_lifted_when_its_time_comes() {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let e = Engine::new(
            teams_proto::Client::offline("test").unwrap(),
            store::Store::open_in_memory().unwrap(),
            session::Config::default(),
            ui,
            "8:me".into(),
            "Me".into(),
        );
        e.offline.store(true, std::sync::atomic::Ordering::Relaxed);
        let run = |cmd| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(e.lists_cmd(cmd))
                .unwrap()
        };
        let saved_until = |e: &Arc<Engine>| {
            let t = e.store.lock().unwrap().get_kv(KV_KEY).unwrap().unwrap();
            Local::from_json(&serde_json::from_str(&t).unwrap()).mute_until
        };
        run(ListsCmd::MuteFor("c1".into(), "1h".into()));
        run(ListsCmd::Mute("c2".into()));
        let until = e.test_mute_until("c1").expect("an end time");
        assert!(e.is_muted("c1") && e.is_muted("c2"));
        assert_eq!(
            e.test_mute_until("c2"),
            None,
            "no end time for a plain mute"
        );
        assert_eq!(
            saved_until(&e).get("c1"),
            Some(&until),
            "saved for the next start"
        );
        // Not yet: nothing changes.
        e.test_expire_mutes(until - 1);
        assert!(e.is_muted("c1"));
        // Its time: c1 is unmuted, the mute with no end stays.
        e.test_expire_mutes(until);
        assert!(!e.is_muted("c1") && e.is_muted("c2"));
        assert_eq!(e.test_mute_until("c1"), None);
        assert!(saved_until(&e).is_empty());
        // Muting again with "until I turn it off" drops an earlier end time.
        run(ListsCmd::MuteFor("c1".into(), "8h".into()));
        run(ListsCmd::Mute("c1".into()));
        assert_eq!(e.test_mute_until("c1"), None);
    }
}
