//! Finding things (plan chat 7.1, 7.2, 7.4): the Search page, Find in chat or channel and the quick
//! views Mentions and Followed threads. The cache is searched first so results show at once; the
//! server search (Substrate), the directory and OneDrive add to them when they answer.

use super::Engine;
use super::util::chat_time;
use crate::data::{color_for, initials};
use crate::find_data::{HitData, PickData, highlight_md, marked_md, matches_all, terms_of};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use store::{ConvFilter, ConvKind, ConversationRow, LocalFilters, LocalHit, MessageRow};
use teams_proto::global_search::Paging;
use teams_proto::search::{MessageHit, SearchFilters, mentions_query};
use teams_proto::time::now_ms;
use teams_proto::views::FollowedThread;

mod discover;
mod global;
mod goto;
mod top_messages;

/// Messages asked from the server per page.
const PAGE: u32 = 25;
/// Rows of one category in the All tab, and in a category tab.
const ALL_MESSAGES: usize = 5;
const ALL_OTHER: usize = 3;
const TAB_MAX: usize = 100;
/// Cached messages scanned for one query.
const CACHE_SCAN: usize = 2000;
/// Followed threads whose posts are loaded.
const FOLLOWED_MAX: usize = 30;
/// Matches kept for Find in chat.
const FIND_MAX: u32 = 50;

/// Commands of the finding UI (`FindStore.command(name, argument)`).
#[derive(Debug, Clone, PartialEq)]
pub enum FindCmd {
    Search(String),
    SearchTab(String),
    Editor(String),
    PickQuery(String),
    Pick(String),
    FilterClear(String),
    Date(String),
    Open(String),
    More,
    Quick(String),
    QuickClose,
    Unfollow(String),
    /// Removes a row of the Saved quick view from the saved list.
    Unsave(String),
    FindOpen,
    FindClose,
    FindQuery(String),
    FindPrev,
    FindNext,
    Tab(String),
    /// A channel tab beyond Posts and Files: opens its address.
    ChannelTabOpen(String),
    /// "Add a tab": a website, as `name` U+001F `url`.
    ChannelTabAdd(String),
    /// "Add a tab" with an installed app, as `app id` U+001F `name`.
    ChannelTabAddApp(String),
    /// "Add a tab" in a chat (website, then app), as the channel ones. UNVERIFIED endpoint.
    ChatTabAdd(String),
    ChatTabAddApp(String),
    /// Rename a tab, as `tab id` U+001F `name`.
    ChannelTabRename(String),
    ChannelTabRemove(String),
    /// The "Add a tab" dialog opened: list the installed apps that have a tab.
    ChannelTabApps,
    SharedFilter(String),
    SharedKeyword(String),
    SharedOpen(String),
    SharedUpload,
    /// The row menu, each with the row's address (Rename adds `name` after U+001F).
    SharedDownload(String),
    SharedCopyLink(String),
    SharedShare(String),
    SharedRename(String),
    SharedDelete(String),
    /// New folder, with its name.
    SharedNewFolder(String),
    /// New document: `word`, `excel` or `powerpoint`.
    SharedNewDoc(String),
    /// Upload from OneDrive: the cloud-file picker, answered like Attach cloud files.
    SharedOneDrive,
    Details(bool),
    Follow(String),
    UnfollowMsg(String),
    MenuFollow(String),
    /// The top search box: suggestions for the text typed so far.
    Suggest(String),
    /// The top search box's Messages group, once typing has paused.
    SuggestMessages(String),
    /// `/goto`, `/chat` and `/call`: the command's name, U+001F and the name typed after it.
    Go(String),
}

impl FindCmd {
    pub fn parse(name: &str, arg: &str) -> Option<FindCmd> {
        let a = arg.to_string();
        Some(match name {
            "search" => FindCmd::Search(a),
            "search-tab" => FindCmd::SearchTab(a),
            "editor" => FindCmd::Editor(a),
            "pick-query" => FindCmd::PickQuery(a),
            "pick" => FindCmd::Pick(a),
            "filter-clear" => FindCmd::FilterClear(a),
            "date" => FindCmd::Date(a),
            "open" => FindCmd::Open(a),
            "more" => FindCmd::More,
            "quick" => FindCmd::Quick(a),
            "quick-close" => FindCmd::QuickClose,
            "unfollow" => FindCmd::Unfollow(a),
            "unsave" => FindCmd::Unsave(a),
            "find-open" => FindCmd::FindOpen,
            "find-close" => FindCmd::FindClose,
            "find-query" => FindCmd::FindQuery(a),
            "find-prev" => FindCmd::FindPrev,
            "find-next" => FindCmd::FindNext,
            "tab" => FindCmd::Tab(a),
            "channel-tab-open" => FindCmd::ChannelTabOpen(a),
            "channel-tab-add" => FindCmd::ChannelTabAdd(a),
            "channel-tab-add-app" => FindCmd::ChannelTabAddApp(a),
            "chat-tab-add" => FindCmd::ChatTabAdd(a),
            "chat-tab-add-app" => FindCmd::ChatTabAddApp(a),
            "channel-tab-rename" => FindCmd::ChannelTabRename(a),
            "channel-tab-remove" => FindCmd::ChannelTabRemove(a),
            "channel-tab-apps" => FindCmd::ChannelTabApps,
            "shared-filter" => FindCmd::SharedFilter(a),
            "shared-keyword" => FindCmd::SharedKeyword(a),
            "shared-open" => FindCmd::SharedOpen(a),
            "shared-upload" => FindCmd::SharedUpload,
            "shared-download" => FindCmd::SharedDownload(a),
            "shared-copy-link" => FindCmd::SharedCopyLink(a),
            "shared-share" => FindCmd::SharedShare(a),
            "shared-rename" => FindCmd::SharedRename(a),
            "shared-delete" => FindCmd::SharedDelete(a),
            "shared-new-folder" => FindCmd::SharedNewFolder(a),
            "shared-new-doc" => FindCmd::SharedNewDoc(a),
            "shared-onedrive" => FindCmd::SharedOneDrive,
            "details" => FindCmd::Details(arg == "open"),
            "follow" => FindCmd::Follow(a),
            "unfollow-msg" => FindCmd::UnfollowMsg(a),
            "menu-follow" => FindCmd::MenuFollow(a),
            "suggest" => FindCmd::Suggest(a),
            "suggest-messages" => FindCmd::SuggestMessages(a),
            "go" => FindCmd::Go(a),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    All,
    Messages,
    Files,
    People,
    Channels,
    Chats,
    Meetings,
}

impl Tab {
    fn parse(s: &str) -> Tab {
        match s {
            "messages" => Tab::Messages,
            "files" => Tab::Files,
            "people" => Tab::People,
            "channels" => Tab::Channels,
            "chats" => Tab::Chats,
            "meetings" => Tab::Meetings,
            _ => Tab::All,
        }
    }

    /// The hit kind a category tab shows.
    fn kind(self) -> Option<&'static str> {
        Some(match self {
            Tab::All => return None,
            Tab::Messages => "message",
            Tab::Files => "file",
            Tab::People => "person",
            Tab::Channels => "channel",
            Tab::Chats => "chat",
            Tab::Meetings => "meeting",
        })
    }
}

/// The "Date" filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Date {
    #[default]
    Any,
    Day,
    Week,
    Month,
}

impl Date {
    fn parse(s: &str) -> Date {
        match s {
            "24h" => Date::Day,
            "7d" => Date::Week,
            "30d" => Date::Month,
            _ => Date::Any,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Date::Any => "",
            Date::Day => "Past 24 hours",
            Date::Week => "Past week",
            Date::Month => "Past month",
        }
    }

    /// The earliest time a message may have been sent, or 0.
    fn since_ms(self, now: i64) -> i64 {
        let day = 86_400_000;
        match self {
            Date::Any => 0,
            Date::Day => now - day,
            Date::Week => now - 7 * day,
            Date::Month => now - 30 * day,
        }
    }
}

/// `YYYY-MM-DD` of a time, for the `sent>=` filter of the server search.
fn day_of(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// One search or quick view result before it becomes a row.
#[derive(Debug, Clone, Default)]
pub(super) struct Hit {
    kind: &'static str,
    conv: String,
    message: String,
    root: String,
    time_ms: i64,
    mri: String,
    /// Who sent it, or the name of a person, file or conversation.
    name: String,
    /// Where it happened (`Team › channel`, a chat's title) or a person's job.
    place: String,
    /// Plain text to show a snippet of.
    text: String,
    /// A snippet the cache's search index cut and marked (`store::MARK_START`…), shown instead
    /// of highlighting `text`; empty for the other hits.
    marked: String,
    url: String,
}

impl Hit {
    fn id(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.kind, self.conv, self.message, self.root, self.mri, self.url
        )
    }
}

/// A match of Find in chat.
#[derive(Debug, Clone, PartialEq)]
struct Match {
    id: String,
    /// The thread root when the match is a channel reply.
    root: Option<String>,
    time_ms: i64,
}

#[derive(Default)]
struct ChatFind {
    open: bool,
    query: String,
    matches: Vec<Match>,
    /// Index into `matches`, in time order.
    index: usize,
    seq: u64,
}

#[derive(Default)]
pub(super) struct State {
    // Search page.
    query: String,
    tab: Tab,
    in_conv: Option<(String, String)>,
    person: Option<(String, String)>,
    date: Date,
    editor: String,
    picks: Vec<(String, String)>,
    /// Bumped per editor query, so a slow people search older than the latest one is dropped.
    pick_seq: u64,
    seq: u64,
    hits: Vec<Hit>,
    msg_from: u32,
    more: bool,
    /// Next page of Files, People and Meetings (the global search), see `global`.
    global_more: Paging,
    searched: bool,
    // Quick views.
    quick: Option<&'static str>,
    quick_hits: Vec<Hit>,
    quick_from: u32,
    quick_more: bool,
    quick_seq: u64,
    followed: Vec<FollowedThread>,
    followed_loaded: bool,
    follow_override: HashMap<(String, String), bool>,
    // Find in chat.
    find: ChatFind,
    /// The Messages group of the top search box's dropdown.
    top: top_messages::TopMessages,
    pub(super) shared: super::shared::State,
}

/// Rows of a category in display order. With a category tab only that category shows; the All tab
/// shows the top of each with a section header, or only messages while a filter is on.
fn rows_for(tab: Tab, filtered: bool, hits: &[Hit]) -> Vec<(Hit, String)> {
    let order: [(&str, &str, usize); 6] = [
        ("message", "Messages", ALL_MESSAGES),
        ("file", "Files", ALL_OTHER),
        ("person", "People", ALL_OTHER),
        ("channel", "Channels", ALL_OTHER),
        ("chat", "Group chats", ALL_OTHER),
        ("meeting", "Meetings", ALL_OTHER),
    ];
    let mut out = vec![];
    for (kind, label, cap) in order {
        let (take, section) = match tab.kind() {
            Some(k) if k == kind => (TAB_MAX, false),
            Some(_) => continue,
            None if filtered && kind != "message" => continue,
            None => (cap, true),
        };
        for (i, h) in hits
            .iter()
            .filter(|h| h.kind == kind)
            .take(take)
            .enumerate()
        {
            out.push((
                h.clone(),
                if section && i == 0 {
                    label.to_string()
                } else {
                    String::new()
                },
            ));
        }
    }
    out
}

/// The text of a stored message without markup.
fn plain_of(m: &MessageRow) -> String {
    ::render::plain_text(&::render::parse_teams_html(&m.content_html))
}

/// The longest term, which is the best prefilter for the SQL scan.
fn longest(terms: &[String]) -> Option<&str> {
    terms
        .iter()
        .max_by_key(|t| t.chars().count())
        .map(String::as_str)
}

impl Engine {
    pub(super) async fn find_cmd(self: &Arc<Self>, cmd: FindCmd) -> anyhow::Result<()> {
        match cmd {
            FindCmd::Search(q) => self.search_start(q).await,
            FindCmd::Suggest(q) => self.suggest_start(q),
            FindCmd::SuggestMessages(q) => self.suggest_messages(q),
            FindCmd::Go(arg) => self.go_to(&arg).await?,
            FindCmd::SearchTab(t) => {
                self.find.lock().unwrap().tab = Tab::parse(&t);
                self.push_search(false);
            }
            FindCmd::Editor(name) => self.editor_open(name).await,
            FindCmd::PickQuery(q) => self.pick_query(q).await,
            FindCmd::Pick(id) => self.pick(&id).await,
            FindCmd::FilterClear(which) => {
                {
                    let mut f = self.find.lock().unwrap();
                    match which.as_str() {
                        "in" => f.in_conv = None,
                        "person" => f.person = None,
                        _ => f.date = Date::Any,
                    }
                }
                self.push_filters();
                self.rerun_search().await;
            }
            FindCmd::Date(d) => {
                {
                    let mut f = self.find.lock().unwrap();
                    f.date = Date::parse(&d);
                    f.editor.clear();
                }
                self.push_filters();
                self.rerun_search().await;
            }
            FindCmd::Open(id) => self.open_hit(&id).await?,
            FindCmd::More => self.more_results().await,
            FindCmd::Quick(kind) => self.quick_open(&kind).await,
            FindCmd::QuickClose => {
                self.find.lock().unwrap().quick = None;
            }
            FindCmd::Unfollow(id) => self.unfollow_row(&id).await?,
            FindCmd::Unsave(id) => self.unsave_row(&id).await?,
            FindCmd::FindOpen => {
                let mut f = self.find.lock().unwrap();
                f.find = ChatFind {
                    open: true,
                    ..Default::default()
                };
            }
            FindCmd::FindClose => self.chat_find_close(),
            FindCmd::FindQuery(q) => self.chat_find_query(q).await,
            FindCmd::FindPrev => self.chat_find_step(false).await,
            FindCmd::FindNext => self.chat_find_step(true).await,
            FindCmd::Tab(t) => {
                self.shared_tab(&t).await;
                if t == "storyline" {
                    self.storyline_open().await;
                }
            }
            FindCmd::ChannelTabOpen(url) => {
                if url.starts_with("https://") {
                    self.clone().handle(super::Cmd::OpenUrl(url)).await;
                }
            }
            FindCmd::ChannelTabAdd(arg) => self.add_channel_tab(&arg).await?,
            FindCmd::ChannelTabAddApp(arg) => self.add_channel_app_tab(&arg).await?,
            FindCmd::ChatTabAdd(arg) => self.add_chat_tab(&arg, false).await?,
            FindCmd::ChatTabAddApp(arg) => self.add_chat_tab(&arg, true).await?,
            FindCmd::ChannelTabRename(arg) => self.rename_channel_tab(&arg).await?,
            FindCmd::ChannelTabRemove(id) => self.remove_channel_tab(&id).await?,
            FindCmd::ChannelTabApps => self.load_tab_apps().await,
            FindCmd::SharedFilter(f) => self.shared_filter(&f),
            FindCmd::SharedKeyword(k) => self.shared_keyword(k),
            FindCmd::SharedOpen(url) => {
                self.clone().handle(super::Cmd::OpenUrl(url)).await;
            }
            FindCmd::SharedUpload => self.shared_upload().await,
            FindCmd::SharedDownload(url) => self.shared_download(&url).await,
            FindCmd::SharedCopyLink(url) => self.shared_copy_link(&url),
            FindCmd::SharedShare(url) => self.shared_share(&url).await,
            FindCmd::SharedRename(arg) => self.shared_rename(&arg).await,
            FindCmd::SharedDelete(url) => self.shared_delete(&url).await,
            FindCmd::SharedNewFolder(name) => self.shared_new_folder(&name).await,
            FindCmd::SharedNewDoc(kind) => self.shared_new_doc(&kind).await,
            FindCmd::SharedOneDrive => self.shared_onedrive().await,
            FindCmd::Details(open) => self.details(open).await,
            FindCmd::Follow(id) => self.set_follow(&id, true).await?,
            FindCmd::UnfollowMsg(id) => self.set_follow(&id, false).await?,
            FindCmd::MenuFollow(id) => self.menu_follow(&id).await,
        }
        Ok(())
    }

    /// A chat was opened: the find bar, the Shared tab and the quick view close.
    pub(super) fn find_chat_opened(self: &Arc<Self>, conv: &str) {
        {
            let mut f = self.find.lock().unwrap();
            f.find = ChatFind::default();
            f.quick = None;
        }
        self.ui.chat_tab_reset();
        self.ui.quick_open(false);
        self.shared_chat_opened(conv);
    }

    // ---- locations and rows -------------------------------------------------------

    fn conv_row(&self, conv: &str) -> Option<ConversationRow> {
        self.store.lock().unwrap().conversation(conv).ok().flatten()
    }

    /// `Team › channel` for channels, the chat's title for chats.
    pub(super) fn location_of(&self, conv: &str, topic: &str, team: &str) -> String {
        match self.conv_row(conv) {
            Some(c) if c.kind == ConvKind::Channel => match &c.team_name {
                Some(t) => format!("{t} › {}", c.title),
                None => c.title,
            },
            Some(c) if c.kind == ConvKind::Notes => "Notes".into(),
            Some(c) => c.title,
            None if !team.is_empty() => format!("{team} › {topic}"),
            None => topic.to_string(),
        }
    }

    fn user_name(&self, mri: &str) -> Option<String> {
        self.store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
    }

    fn hit_from_message(&self, h: &MessageHit) -> Hit {
        let name = if h.sender_name.is_empty() {
            self.user_name(&h.sender_mri).unwrap_or_default()
        } else {
            h.sender_name.clone()
        };
        Hit {
            kind: "message",
            conv: h.conversation_id.clone(),
            message: h.message_id.clone(),
            root: h.root_id.clone(),
            time_ms: h.time_ms,
            mri: h.sender_mri.clone(),
            name,
            place: self.location_of(&h.conversation_id, &h.topic, &h.team_name),
            text: h.snippet.clone(),
            marked: String::new(),
            url: String::new(),
        }
    }

    fn hit_from_row(&self, m: &MessageRow) -> Hit {
        let name = if m.sender_name.is_empty() {
            self.user_name(&m.sender_mri).unwrap_or_default()
        } else {
            m.sender_name.clone()
        };
        Hit {
            kind: "message",
            conv: m.conversation_id.clone(),
            message: m.id.clone(),
            root: m.parent_id.clone().unwrap_or_default(),
            time_ms: m.compose_ms,
            mri: m.sender_mri.clone(),
            name,
            place: self.location_of(&m.conversation_id, "", ""),
            text: plain_of(m),
            marked: String::new(),
            url: String::new(),
        }
    }

    fn row_data(self: &Arc<Self>, h: &Hit, section: String, terms: &[String]) -> HitData {
        let name = if h.name.is_empty() {
            "Someone".to_string()
        } else {
            h.name.clone()
        };
        let (title, subtitle) = match h.kind {
            "message" => (name.clone(), h.place.clone()),
            "thread" => (h.place.clone(), h.name.clone()),
            _ => (name.clone(), h.place.clone()),
        };
        let snippet = match h.kind {
            "message" | "thread" if !h.marked.is_empty() => marked_md(&h.marked),
            "message" | "thread" => highlight_md(&h.text, terms, 160),
            _ => String::new(),
        };
        let avatar_of = if h.kind == "message" || h.kind == "person" {
            h.mri.as_str()
        } else {
            ""
        };
        HitData {
            id: h.id(),
            kind: h.kind,
            section,
            title: title.clone(),
            subtitle,
            snippet,
            time: chat_time(h.time_ms),
            initials: initials(&title),
            color: color_for(if h.mri.is_empty() { &h.conv } else { &h.mri }),
            avatar: self.avatar(avatar_of),
            person: avatar_of.to_string(),
            rtl: ::render::base_direction(&h.text) == ::render::Dir::Rtl,
        }
    }

    // ---- 7.1 Search page -----------------------------------------------------------

    fn filters_active(f: &State) -> bool {
        f.in_conv.is_some() || f.person.is_some() || f.date != Date::Any
    }

    fn push_filters(&self) {
        let f = self.find.lock().unwrap();
        self.ui.search_filters(
            f.in_conv
                .as_ref()
                .map(|(_, t)| t.clone())
                .unwrap_or_default(),
            f.person
                .as_ref()
                .map(|(_, n)| n.clone())
                .unwrap_or_default(),
            f.date.label(),
            f.editor.clone(),
        );
    }

    /// Sends the stored results to the Search page.
    fn push_search(self: &Arc<Self>, searching: bool) {
        let (rows, terms, searched, more) = {
            let f = self.find.lock().unwrap();
            (
                rows_for(f.tab, Self::filters_active(&f), &f.hits),
                terms_of(&f.query),
                f.searched,
                (f.more && matches!(f.tab, Tab::Messages | Tab::All))
                    || global::tab_has_more(f.tab, &f.global_more),
            )
        };
        let rows: Vec<HitData> = rows
            .iter()
            .map(|(h, section)| self.row_data(h, section.clone(), &terms))
            .collect();
        self.ui.search_results(rows, searching, searched, more);
    }

    /// Starts a search for `q` (Enter on the Search page or in the top bar).
    async fn search_start(self: &Arc<Self>, q: String) {
        {
            let mut f = self.find.lock().unwrap();
            f.query = q.clone();
            f.editor.clear();
        }
        self.ui.show_view("search");
        self.push_filters();
        self.run_search().await;
    }

    async fn rerun_search(self: &Arc<Self>) {
        if !self.find.lock().unwrap().query.trim().is_empty() {
            self.run_search().await;
        }
    }

    async fn run_search(self: &Arc<Self>) {
        let (q, seq, tab, in_conv, person, date) = {
            let mut f = self.find.lock().unwrap();
            f.seq += 1;
            f.msg_from = 0;
            f.more = false;
            f.global_more = Paging::default();
            (
                f.query.clone(),
                f.seq,
                f.tab,
                f.in_conv.clone(),
                f.person.clone(),
                f.date,
            )
        };
        let terms = terms_of(&q);
        if terms.is_empty() {
            {
                let mut f = self.find.lock().unwrap();
                f.hits.clear();
                f.searched = false;
            }
            self.push_search(false);
            return;
        }
        let since = date.since_ms(now_ms());
        let filtered = in_conv.is_some() || person.is_some() || date != Date::Any;
        let mut hits = self.local_message_hits(
            &q,
            in_conv.as_ref().map(|(id, _)| id.as_str()),
            person.as_ref().map(|(m, _)| m.as_str()),
            since,
        );
        if !filtered {
            hits.extend(self.local_conversation_hits(&terms));
        }
        let offline = self.offline.load(Ordering::Relaxed);
        {
            let mut f = self.find.lock().unwrap();
            if f.seq != seq {
                return;
            }
            f.hits = hits;
            f.searched = true;
        }
        self.push_search(!offline);
        if offline {
            return;
        }
        let want_messages = matches!(tab, Tab::All | Tab::Messages);
        let remote_filters = SearchFilters {
            in_thread: in_conv.as_ref().map(|(id, _)| id.clone()),
            from_mri: person.as_ref().map(|(m, _)| m.clone()),
            since_day: (date != Date::Any).then(|| day_of(since)),
        };
        // Files, people and meetings come from the global search, whichever tab is open.
        let (msgs, others) = tokio::join!(
            async {
                if want_messages {
                    self.client
                        .search_messages(&self.me, &q, &remote_filters, 0, PAGE)
                        .await
                        .ok()
                } else {
                    None
                }
            },
            async {
                if filtered {
                    None
                } else {
                    Some(self.others_first(&q).await)
                }
            }
        );
        {
            let mut f = self.find.lock().unwrap();
            if f.seq != seq {
                return;
            }
            if let Some(page) = &msgs {
                f.msg_from = PAGE;
                f.more = page.more;
                let hits: Vec<Hit> = page
                    .hits
                    .iter()
                    .filter(|h| since == 0 || h.time_ms >= since)
                    .map(|h| self.hit_from_message(h))
                    .collect();
                merge_messages(&mut f.hits, hits);
            }
            if let Some(o) = others {
                global::store_others(&mut f, o);
            }
        }
        self.push_search(false);
    }

    /// Messages of the cache with a word starting with each word of `q`, from the cache's
    /// full-text index (`Store::search_messages_local`), newest first. No network: offline
    /// these are the results.
    fn local_message_hits(
        &self,
        q: &str,
        in_conv: Option<&str>,
        person: Option<&str>,
        since: i64,
    ) -> Vec<Hit> {
        let filters = LocalFilters {
            conversation: in_conv,
            sender: person,
            since_ms: Some(since),
        };
        let found = self
            .store
            .lock()
            .unwrap()
            .search_messages_local(q, &filters, TAB_MAX)
            .unwrap_or_default();
        found.iter().map(|h| self.hit_from_local(h)).collect()
    }

    fn hit_from_local(&self, h: &LocalHit) -> Hit {
        let name = if h.sender_name.is_empty() {
            self.user_name(&h.sender_mri).unwrap_or_default()
        } else {
            h.sender_name.clone()
        };
        Hit {
            kind: "message",
            conv: h.conversation_id.clone(),
            message: h.message_id.clone(),
            root: h.parent_id.clone().unwrap_or_default(),
            time_ms: h.compose_ms,
            mri: h.sender_mri.clone(),
            name,
            place: self.location_of(&h.conversation_id, "", ""),
            text: h.snippet.replace([store::MARK_START, store::MARK_END], ""),
            marked: h.snippet.clone(),
            url: String::new(),
        }
    }

    /// Channels, group chats and meetings whose name (or team) contains every term.
    fn local_conversation_hits(&self, terms: &[String]) -> Vec<Hit> {
        let convs = self
            .store
            .lock()
            .unwrap()
            .conversations(ConvFilter::All, 5000)
            .unwrap_or_default();
        convs
            .into_iter()
            .filter_map(|c| {
                let kind = match c.kind {
                    ConvKind::Channel => "channel",
                    ConvKind::Group => "chat",
                    ConvKind::Meeting => "meeting",
                    _ => return None,
                };
                let place = match (&c.team_name, c.kind) {
                    (Some(t), ConvKind::Channel) => format!("{t} › {}", c.title),
                    _ => c.title.clone(),
                };
                matches_all(&place, terms).then(|| Hit {
                    kind,
                    conv: c.id,
                    time_ms: c.last_activity_ms,
                    name: place,
                    ..Default::default()
                })
            })
            .collect()
    }

    /// "Show more messages": the next server page.
    async fn more_results(self: &Arc<Self>) {
        let quick = self.find.lock().unwrap().quick;
        if let Some(kind) = quick {
            return self.quick_more(kind).await;
        }
        let tab = self.find.lock().unwrap().tab;
        if let Some(kind) = global::kind_of_tab(tab) {
            return self.global_more(kind).await;
        }
        let (q, from, seq, in_conv, person, date) = {
            let f = self.find.lock().unwrap();
            (
                f.query.clone(),
                f.msg_from,
                f.seq,
                f.in_conv.clone(),
                f.person.clone(),
                f.date,
            )
        };
        let since = date.since_ms(now_ms());
        let filters = SearchFilters {
            in_thread: in_conv.map(|(id, _)| id),
            from_mri: person.map(|(m, _)| m),
            since_day: (date != Date::Any).then(|| day_of(since)),
        };
        self.push_search(true);
        let page = self
            .client
            .search_messages(&self.me, &q, &filters, from, PAGE)
            .await;
        {
            let mut f = self.find.lock().unwrap();
            // A newer search, or a second quick "Show more" that already took this page.
            if f.seq != seq || f.msg_from != from {
                return;
            }
            match page {
                Ok(page) => {
                    f.msg_from += PAGE;
                    f.more = page.more;
                    let hits: Vec<Hit> =
                        page.hits.iter().map(|h| self.hit_from_message(h)).collect();
                    merge_messages(&mut f.hits, hits);
                }
                Err(err) => {
                    f.more = false;
                    tracing::warn!(%err, "more search results failed");
                }
            }
        }
        self.push_search(false);
    }

    // ---- filters -----------------------------------------------------------------

    async fn editor_open(self: &Arc<Self>, name: String) {
        {
            let mut f = self.find.lock().unwrap();
            f.editor = name.clone();
            f.picks.clear();
        }
        self.ui.search_picks(vec![]);
        if name == "in" {
            self.pick_query(String::new()).await;
        }
    }

    /// Suggestions for the open "In" or "Person" editor.
    async fn pick_query(self: &Arc<Self>, q: String) {
        let (editor, seq) = {
            let mut f = self.find.lock().unwrap();
            f.pick_seq += 1;
            (f.editor.clone(), f.pick_seq)
        };
        let terms = terms_of(&q);
        let picks: Vec<(String, String, String)> = match editor.as_str() {
            "in" => {
                let convs = self
                    .store
                    .lock()
                    .unwrap()
                    .conversations(ConvFilter::All, 2000)
                    .unwrap_or_default();
                convs
                    .into_iter()
                    .filter(|c| c.kind != ConvKind::Notes || terms.is_empty())
                    .map(|c| {
                        let place = match (&c.team_name, c.kind) {
                            (Some(t), ConvKind::Channel) => format!("{t} › {}", c.title),
                            _ => c.title.clone(),
                        };
                        (c.id, place, kind_label(c.kind).to_string())
                    })
                    .filter(|(_, place, _)| terms.is_empty() || matches_all(place, &terms))
                    .take(8)
                    .collect()
            }
            "person" if !q.trim().is_empty() => {
                let found = if self.offline.load(Ordering::Relaxed) {
                    vec![]
                } else {
                    self.client
                        .search_people(&self.me, q.trim())
                        .await
                        .unwrap_or_default()
                };
                found
                    .into_iter()
                    .take(8)
                    .map(|p| {
                        (
                            p.mri,
                            p.display_name,
                            p.job_title.or(p.email).unwrap_or_default(),
                        )
                    })
                    .collect()
            }
            _ => vec![],
        };
        // Only the newest answer for the editor that is still open counts.
        let mut f = self.find.lock().unwrap();
        if f.editor != editor || f.pick_seq != seq {
            return;
        }
        f.picks = picks
            .iter()
            .map(|(id, t, _)| (id.clone(), t.clone()))
            .collect();
        drop(f);
        self.ui.search_picks(
            picks
                .into_iter()
                .map(|(id, title, subtitle)| PickData {
                    id,
                    title,
                    subtitle,
                })
                .collect(),
        );
    }

    async fn pick(self: &Arc<Self>, id: &str) {
        {
            let mut f = self.find.lock().unwrap();
            let Some((id, title)) = f.picks.iter().find(|(i, _)| i == id).cloned() else {
                return;
            };
            match f.editor.as_str() {
                "in" => f.in_conv = Some((id, title)),
                "person" => f.person = Some((id, title)),
                _ => {}
            }
            f.editor.clear();
            f.picks.clear();
        }
        self.ui.search_picks(vec![]);
        self.push_filters();
        self.rerun_search().await;
    }

    // ---- opening a hit -----------------------------------------------------------

    async fn open_hit(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let hit = {
            let f = self.find.lock().unwrap();
            f.hits
                .iter()
                .chain(f.quick_hits.iter())
                .chain(f.top.hits.iter())
                .find(|h| h.id() == id)
                .cloned()
        };
        let Some(h) = hit else { return Ok(()) };
        match h.kind {
            "message" => {
                let root = (!h.root.is_empty()).then_some(h.root.as_str());
                self.go_to_message(&h.conv, &h.message, h.time_ms, root)
                    .await?
            }
            "thread" => {
                self.ui.show_view("chat");
                if let Err(err) = self.open_conversation(&h.conv).await {
                    tracing::warn!(%err, "opening a followed thread");
                }
                self.clone()
                    .handle(super::Cmd::OpenThread(h.root.clone()))
                    .await;
            }
            "file" => {
                self.clone().handle(super::Cmd::OpenUrl(h.url)).await;
            }
            "person" => {
                self.people_cmd(super::PeopleCmd::Profile(h.mri)).await?;
            }
            // A calendar event (it has no chat): the calendar on its day.
            "meeting" if h.conv.is_empty() => self.open_event(h.time_ms).await,
            _ => {
                self.ui.show_view("chat");
                if let Err(err) = self.open_conversation(&h.conv).await {
                    tracing::warn!(%err, "opening a search result");
                }
            }
        }
        Ok(())
    }

    /// Opens a chat or channel in Chat (a channel with its `Team › channel` title).
    async fn open_conversation(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
        let title = self
            .conv_row(conv)
            .filter(|c| c.kind == ConvKind::Channel)
            .map(|_| self.location_of(conv, "", ""));
        // `open` does nothing for the chat already open, which would leave a quick view (Saved,
        // Mentions, Followed threads) covering the message it lands on: it closes here.
        self.close_quick();
        self.open(conv.to_string(), title).await
    }

    fn close_quick(&self) {
        self.find.lock().unwrap().quick = None;
        self.ui.quick_open(false);
    }

    /// Opens the chat of a message and lands on it. `root` is the thread root of a channel reply.
    pub(super) async fn go_to_message(
        self: &Arc<Self>,
        conv: &str,
        id: &str,
        time_ms: i64,
        root: Option<&str>,
    ) -> anyhow::Result<()> {
        self.ui.show_view("chat");
        // The chat opens from the cache even when its history cannot be fetched.
        if let Err(err) = self.open_conversation(conv).await {
            tracing::warn!(%err, "opening the chat of a search result");
        }
        self.land_on(id, time_ms, root, true).await
    }

    /// Scrolls the open chat to a message. A channel reply opens its thread first. `fade` false
    /// keeps the highlight (Find in chat).
    async fn land_on(
        self: &Arc<Self>,
        id: &str,
        time_ms: i64,
        root: Option<&str>,
        fade: bool,
    ) -> anyhow::Result<()> {
        match root {
            Some(root) => {
                let open = self.view.lock().unwrap().thread_root.clone();
                if open.as_deref() != Some(root) {
                    self.clone()
                        .handle(super::Cmd::OpenThread(root.to_string()))
                        .await;
                }
                // A reply's id from the search is only known to about 10 ms: take the nearest.
                let target = {
                    let st = self.store.lock().unwrap();
                    let replies = st.thread_replies(root, 1000).unwrap_or_default();
                    replies
                        .iter()
                        .find(|r| r.id == id)
                        .or_else(|| {
                            replies
                                .iter()
                                .filter(|r| (r.compose_ms - time_ms).abs() < 1500)
                                .min_by_key(|r| (r.compose_ms - time_ms).abs())
                        })
                        .map(|r| r.id.clone())
                };
                match target {
                    Some(t) => self.flash_row(&t),
                    None => self
                        .ui
                        .banner("That reply is no longer in the thread".into()),
                }
                Ok(())
            }
            None => {
                if self.view.lock().unwrap().thread_root.is_some() {
                    self.clone().handle(super::Cmd::CloseThread).await;
                }
                self.jump_to_with(id, fade).await
            }
        }
    }

    // ---- 7.2 Find in chat ----------------------------------------------------------

    fn chat_find_close(self: &Arc<Self>) {
        self.find.lock().unwrap().find = ChatFind::default();
        self.clear_flash();
        self.ui.chat_find(false, 0, 0);
    }

    async fn chat_find_query(self: &Arc<Self>, q: String) {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return;
        };
        let (seq, terms) = {
            let mut f = self.find.lock().unwrap();
            f.find.open = true;
            f.find.query = q.clone();
            f.find.seq += 1;
            (f.find.seq, terms_of(&q))
        };
        if terms.is_empty() {
            self.find.lock().unwrap().find.matches.clear();
            self.clear_flash();
            self.ui.chat_find(true, 0, 0);
            return;
        }
        let local: Vec<Match> = {
            let rows = self
                .store
                .lock()
                .unwrap()
                .messages_containing(Some(&conv), longest(&terms).unwrap_or_default(), CACHE_SCAN)
                .unwrap_or_default();
            rows.iter()
                .filter(|m| !m.message_type.starts_with("Control/"))
                .filter(|m| matches_all(&plain_of(m), &terms))
                .map(|m| Match {
                    id: m.id.clone(),
                    root: m.parent_id.clone(),
                    time_ms: m.compose_ms,
                })
                .collect()
        };
        self.set_matches(seq, local, true).await;
        if self.offline.load(Ordering::Relaxed) {
            return;
        }
        // Older matches than the cache hold come from the server search of this chat.
        let filters = SearchFilters {
            in_thread: Some(conv),
            ..Default::default()
        };
        let Ok(page) = self
            .client
            .search_messages(&self.me, &q, &filters, 0, FIND_MAX)
            .await
        else {
            return;
        };
        let remote: Vec<Match> = page
            .hits
            .iter()
            .map(|h| Match {
                id: h.message_id.clone(),
                root: (!h.root_id.is_empty()).then(|| h.root_id.clone()),
                time_ms: h.time_ms,
            })
            .collect();
        let merged = {
            let f = self.find.lock().unwrap();
            if f.find.seq != seq {
                return;
            }
            let mut all = f.find.matches.clone();
            for m in remote {
                let dup = all.iter().any(|x| {
                    x.id == m.id
                        || (m.root.is_some()
                            && x.root == m.root
                            && (x.time_ms - m.time_ms).abs() < 1500)
                });
                if !dup {
                    all.push(m);
                }
            }
            all
        };
        self.set_matches(seq, merged, false).await;
    }

    /// Stores the matches (oldest first). With `jump` the newest becomes the current one;
    /// otherwise the current match stays where it is.
    async fn set_matches(self: &Arc<Self>, seq: u64, mut matches: Vec<Match>, jump: bool) {
        matches.sort_by_key(|m| m.time_ms);
        let target = {
            let mut f = self.find.lock().unwrap();
            if f.find.seq != seq {
                return;
            }
            let current = f.find.matches.get(f.find.index).map(|m| m.id.clone());
            f.find.index = match (jump, current) {
                (false, Some(id)) => matches.iter().position(|m| m.id == id).unwrap_or(0),
                _ => matches.len().saturating_sub(1),
            };
            f.find.matches = matches;
            let n = f.find.matches.len();
            self.ui
                .chat_find(true, n, if n == 0 { 0 } else { f.find.index + 1 });
            let reached = jump || n == 1;
            f.find
                .matches
                .get(f.find.index)
                .cloned()
                .filter(|_| reached)
        };
        match target {
            Some(m) => {
                let _ = self
                    .land_on(&m.id, m.time_ms, m.root.as_deref(), false)
                    .await;
            }
            None if jump => self.clear_flash(),
            None => {}
        }
    }

    /// Steps to the next (`newer`) or previous match.
    async fn chat_find_step(self: &Arc<Self>, newer: bool) {
        let target = {
            let mut f = self.find.lock().unwrap();
            let n = f.find.matches.len();
            if n == 0 {
                return;
            }
            f.find.index = if newer {
                (f.find.index + 1) % n
            } else {
                (f.find.index + n - 1) % n
            };
            self.ui.chat_find(true, n, f.find.index + 1);
            f.find.matches[f.find.index].clone()
        };
        let _ = self
            .land_on(&target.id, target.time_ms, target.root.as_deref(), false)
            .await;
    }

    // ---- 7.4 Quick views -----------------------------------------------------------

    async fn quick_open(self: &Arc<Self>, kind: &str) {
        let kind: &'static str = match kind {
            "followed" => "followed",
            "saved" => "saved",
            "discover" => "discover",
            _ => "mentions",
        };
        let seq = {
            let mut f = self.find.lock().unwrap();
            f.quick = Some(kind);
            f.quick_hits.clear();
            f.quick_from = 0;
            f.quick_more = false;
            f.quick_seq += 1;
            f.quick_seq
        };
        self.ui.quick_open(true);
        self.push_quick(true);
        match kind {
            "mentions" => self.load_mentions(seq).await,
            "saved" => self.load_saved(seq),
            "discover" => self.load_discover(seq),
            _ => self.load_followed(seq).await,
        }
    }

    /// The Saved view (plan chat 9.8): the local saved list, newest saved first. The list is
    /// kept in the store (see `toggle_save`), so it needs no network.
    fn load_saved(self: &Arc<Self>, seq: u64) {
        let saved = self
            .store
            .lock()
            .unwrap()
            .saved_messages()
            .unwrap_or_default();
        let hits: Vec<Hit> = saved
            .iter()
            .map(|m| {
                // The cached message gives the real time and sender; the saved row is enough
                // when it has been pruned.
                let row = self
                    .store
                    .lock()
                    .unwrap()
                    .message(&m.message_id)
                    .ok()
                    .flatten();
                let mri = row
                    .as_ref()
                    .map(|r| r.sender_mri.clone())
                    .unwrap_or_default();
                let name = if m.sender_name.is_empty() {
                    self.user_name(&mri).unwrap_or_default()
                } else {
                    m.sender_name.clone()
                };
                Hit {
                    kind: "message",
                    conv: m.conversation_id.clone(),
                    message: m.message_id.clone(),
                    root: if m.root_id == m.message_id {
                        String::new()
                    } else {
                        m.root_id.clone()
                    },
                    time_ms: row.as_ref().map_or(m.saved_ms, |r| r.compose_ms),
                    mri,
                    name,
                    place: self.location_of(&m.conversation_id, "", ""),
                    text: m.preview.clone(),
                    marked: String::new(),
                    url: String::new(),
                }
            })
            .collect();
        {
            let mut f = self.find.lock().unwrap();
            if f.quick_seq != seq {
                return;
            }
            f.quick_hits = hits;
        }
        self.push_quick(false);
    }

    /// "Unsave" on a row of the Saved view: the message leaves the list (and the server flag is
    /// cleared, best effort like in `toggle_save`).
    async fn unsave_row(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let hit = self
            .find
            .lock()
            .unwrap()
            .quick_hits
            .iter()
            .find(|h| h.id() == id)
            .cloned();
        let Some(h) = hit else { return Ok(()) };
        self.store
            .lock()
            .unwrap()
            .unsave_message(&h.conv, &h.message)?;
        self.find
            .lock()
            .unwrap()
            .quick_hits
            .retain(|x| x.id() != id);
        self.push_quick(false);
        self.mark(|d| d.messages = true);
        let root = if h.root.is_empty() {
            &h.message
        } else {
            &h.root
        };
        if let Err(err) = self
            .client
            .set_saved(&h.conv, root, &h.message, false)
            .await
        {
            tracing::warn!(%err, "unsaving the message on the server failed; removed locally");
        }
        Ok(())
    }

    fn push_quick(self: &Arc<Self>, loading: bool) {
        let (kind, hits, more) = {
            let f = self.find.lock().unwrap();
            (
                f.quick.unwrap_or("mentions"),
                f.quick_hits.clone(),
                f.quick_more,
            )
        };
        let rows = hits
            .iter()
            .map(|h| self.row_data(h, String::new(), &[]))
            .collect();
        self.ui.quick_rows(kind, rows, loading, more);
    }

    /// Every message that mentions me: the cache first, then the server search.
    async fn load_mentions(self: &Arc<Self>, seq: u64) {
        let me = self.me.clone();
        let local: Vec<Hit> = {
            let rows = self
                .store
                .lock()
                .unwrap()
                .messages_containing(None, "schema.skype.com/Mention", CACHE_SCAN)
                .unwrap_or_default();
            rows.iter()
                .filter(|m| {
                    serde_json::from_str(&m.reactions_json)
                        .is_ok_and(|p| super::mentions::mentions_me(&p, &me))
                })
                .map(|m| self.hit_from_row(m))
                .collect()
        };
        let offline = self.offline.load(Ordering::Relaxed);
        {
            let mut f = self.find.lock().unwrap();
            if f.quick_seq != seq {
                return;
            }
            f.quick_hits = local;
        }
        self.push_quick(!offline);
        if offline {
            return;
        }
        self.mentions_page(seq, 0).await;
    }

    async fn mentions_page(self: &Arc<Self>, seq: u64, from: u32) {
        let Some(q) = mentions_query(&self.me) else {
            self.push_quick(false);
            return;
        };
        let page = self
            .client
            .search_messages(&self.me, &q, &SearchFilters::default(), from, PAGE)
            .await;
        {
            let mut f = self.find.lock().unwrap();
            // A newer view, or a second quick "Show more" that already took this page.
            if f.quick_seq != seq || f.quick_from != from {
                return;
            }
            match page {
                Ok(page) => {
                    f.quick_from = from + PAGE;
                    f.quick_more = page.more;
                    let hits: Vec<Hit> = page
                        .hits
                        .iter()
                        .filter(|h| h.is_mention)
                        .map(|h| self.hit_from_message(h))
                        .collect();
                    merge_messages(&mut f.quick_hits, hits);
                }
                Err(err) => {
                    f.quick_more = false;
                    drop(f);
                    tracing::warn!(%err, "mentions search failed");
                    self.ui.banner(format!("Could not load Mentions: {err}"));
                    self.push_quick(false);
                    return;
                }
            }
        }
        self.push_quick(false);
    }

    async fn quick_more(self: &Arc<Self>, kind: &str) {
        if kind != "mentions" {
            return;
        }
        let (seq, from) = {
            let f = self.find.lock().unwrap();
            (f.quick_seq, f.quick_from)
        };
        self.push_quick(true);
        self.mentions_page(seq, from).await;
    }

    /// The threads I follow, with their root posts, newest activity first.
    async fn load_followed(self: &Arc<Self>, seq: u64) {
        if !self.offline.load(Ordering::Relaxed) {
            match self.client.followed_threads(100).await {
                Ok(list) => {
                    let mut f = self.find.lock().unwrap();
                    f.followed = list;
                    f.followed_loaded = true;
                }
                Err(err) => {
                    tracing::warn!(%err, "followed threads failed");
                    self.ui
                        .banner(format!("Could not load Followed threads: {err}"));
                }
            }
        }
        let threads = self.followed_now();
        if !self.offline.load(Ordering::Relaxed) {
            // The root posts and their replies, into the store.
            let mut loads = tokio::task::JoinSet::new();
            for t in threads.iter().take(FOLLOWED_MAX) {
                let (e, conv, root) = (self.clone(), t.conversation_id.clone(), t.root_id.clone());
                loads.spawn(async move { e.store_thread(&conv, &root).await });
            }
            while loads.join_next().await.is_some() {}
        }
        let mut hits: Vec<Hit> = threads
            .iter()
            .take(FOLLOWED_MAX)
            .map(|t| self.thread_hit(t))
            .collect();
        hits.sort_by_key(|h| std::cmp::Reverse(h.time_ms));
        {
            let mut f = self.find.lock().unwrap();
            if f.quick_seq != seq {
                return;
            }
            f.quick_hits = hits;
        }
        self.push_quick(false);
    }

    /// The followed threads with this session's follow and unfollow choices applied.
    fn followed_now(&self) -> Vec<FollowedThread> {
        let f = self.find.lock().unwrap();
        let mut list: Vec<FollowedThread> = f
            .followed
            .iter()
            .filter(|t| {
                f.follow_override
                    .get(&(t.conversation_id.clone(), t.root_id.clone()))
                    .copied()
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        for ((conv, root), on) in &f.follow_override {
            if *on
                && !list
                    .iter()
                    .any(|t| &t.conversation_id == conv && &t.root_id == root)
            {
                list.push(FollowedThread {
                    conversation_id: conv.clone(),
                    root_id: root.clone(),
                    topic: String::new(),
                    since_ms: now_ms(),
                    implicit: false,
                });
            }
        }
        list
    }

    /// Fetches a thread (its root post and the replies) into the store.
    async fn store_thread(self: Arc<Self>, conv: &str, root: &str) {
        let target = format!("{conv};messageid={root}");
        let Ok(page) = self.client.messages(&target, None, 100).await else {
            return;
        };
        let st = self.store.lock().unwrap();
        for m in page
            .messages
            .iter()
            .filter(|m| !m.message_type.starts_with("Control/"))
        {
            let mut row = super::util::row_from_message(m, Some(root));
            row.conversation_id = conv.to_string();
            if m.id == root {
                row.parent_id = None;
            }
            let _ = st.upsert_message(&row);
        }
    }

    fn thread_hit(&self, t: &FollowedThread) -> Hit {
        let (root, replies) = {
            let st = self.store.lock().unwrap();
            (
                st.message(&t.root_id).ok().flatten(),
                st.thread_replies(&t.root_id, 500).unwrap_or_default(),
            )
        };
        let last = replies
            .last()
            .map(|r| r.compose_ms)
            .or(root.as_ref().map(|r| r.compose_ms))
            .unwrap_or(t.since_ms);
        let author = root
            .as_ref()
            .map(|r| r.sender_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_default();
        let text = root.as_ref().map(plain_of).unwrap_or_default();
        let replies_label = match replies.len() {
            0 => "No replies yet".to_string(),
            1 => "1 reply".to_string(),
            n => format!("{n} replies"),
        };
        Hit {
            kind: "thread",
            conv: t.conversation_id.clone(),
            root: t.root_id.clone(),
            time_ms: last,
            name: if author.is_empty() {
                replies_label
            } else {
                format!("{author} · {replies_label}")
            },
            place: self.location_of(
                &t.conversation_id,
                t.topic.rsplit(" > ").next().unwrap_or(""),
                t.topic
                    .split(" > ")
                    .next()
                    .filter(|_| t.topic.contains(" > "))
                    .unwrap_or(""),
            ),
            text,
            ..Default::default()
        }
    }

    async fn unfollow_row(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let hit = self
            .find
            .lock()
            .unwrap()
            .quick_hits
            .iter()
            .find(|h| h.id() == id)
            .cloned();
        if let Some(h) = hit {
            // The row leaves the list at once (a following command sees it gone), and comes back
            // where it was when the service refuses.
            let at = {
                let mut f = self.find.lock().unwrap();
                let at = f.quick_hits.iter().position(|x| x.id() == id);
                f.quick_hits.retain(|x| x.id() != id);
                at
            };
            self.push_quick(false);
            if let Err(err) = self.apply_follow(&h.conv, &h.root, false).await {
                if let Some(at) = at {
                    let mut f = self.find.lock().unwrap();
                    let at = at.min(f.quick_hits.len());
                    f.quick_hits.insert(at, h);
                }
                self.push_quick(false);
                return Err(err);
            }
        }
        Ok(())
    }

    /// Follows or unfollows the thread of the post `id` of the open channel.
    async fn set_follow(self: &Arc<Self>, id: &str, follow: bool) -> anyhow::Result<()> {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return Ok(());
        };
        self.apply_follow(&conv, id, follow).await?;
        self.ui.menu_following(follow);
        self.ui.banner(
            if follow {
                "You are following this thread"
            } else {
                "You stopped following this thread"
            }
            .into(),
        );
        Ok(())
    }

    async fn apply_follow(
        self: &Arc<Self>,
        conv: &str,
        root: &str,
        follow: bool,
    ) -> anyhow::Result<()> {
        // Here first, so a menu or the Followed view opened right after shows the new state; put
        // back when the service refuses (unless another follow changed it meanwhile).
        let key = (conv.to_string(), root.to_string());
        let before = self
            .find
            .lock()
            .unwrap()
            .follow_override
            .insert(key.clone(), follow);
        if !self.offline.load(Ordering::Relaxed)
            && let Err(err) = self.client.set_thread_follow(conv, root, follow).await
        {
            let mut f = self.find.lock().unwrap();
            if f.follow_override.get(&key) == Some(&follow) {
                match before {
                    Some(b) => f.follow_override.insert(key, b),
                    None => f.follow_override.remove(&key),
                };
            }
            return Err(err.into());
        }
        Ok(())
    }

    /// Tells the open message menu whether the thread of post `id` is followed.
    async fn menu_follow(self: &Arc<Self>, id: &str) {
        let Some(conv) = self.view.lock().unwrap().current.clone() else {
            return;
        };
        let loaded = self.find.lock().unwrap().followed_loaded;
        if !loaded
            && !self.offline.load(Ordering::Relaxed)
            && let Ok(list) = self.client.followed_threads(100).await
        {
            let mut f = self.find.lock().unwrap();
            f.followed = list;
            f.followed_loaded = true;
        }
        let on = self
            .followed_now()
            .iter()
            .any(|t| t.conversation_id == conv && t.root_id == id);
        self.ui.menu_following(on);
    }

    /// Demo and tests: replaces the followed threads the feed would give.
    #[cfg(test)]
    pub(crate) fn seed_followed(&self, threads: Vec<FollowedThread>) {
        let mut f = self.find.lock().unwrap();
        f.followed = threads;
        f.followed_loaded = true;
    }
}

fn kind_label(k: ConvKind) -> &'static str {
    match k {
        ConvKind::Channel => "Channel",
        ConvKind::Group => "Group chat",
        ConvKind::OneOnOne => "Chat",
        ConvKind::Meeting => "Meeting",
        ConvKind::Notes => "Notes",
    }
}

/// Adds server message hits to the list: cache hits stay, and a hit for the same message is
/// not added twice. The messages end up newest first.
fn merge_messages(hits: &mut Vec<Hit>, new: Vec<Hit>) {
    for n in new {
        let dup = hits.iter().any(|h| {
            h.kind == "message"
                && h.conv == n.conv
                && (h.message == n.message
                    || (!n.root.is_empty()
                        && h.root == n.root
                        && (h.time_ms - n.time_ms).abs() < 1500))
        });
        if !dup {
            hits.push(n);
        }
    }
    // Stable: messages by time, the other categories keep their order.
    hits.sort_by_key(|h| {
        if h.kind == "message" {
            std::cmp::Reverse(h.time_ms)
        } else {
            std::cmp::Reverse(i64::MIN)
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(kind: &'static str, conv: &str, message: &str, ms: i64) -> Hit {
        Hit {
            kind,
            conv: conv.into(),
            message: message.into(),
            time_ms: ms,
            ..Default::default()
        }
    }

    /// An offline engine (as in `in_order`'s tests) whose cache holds `msgs` as
    /// `(id, conversation, sender MRI, age in days, html)`.
    fn engine_with(msgs: &[(&str, &str, &str, i64, &str)]) -> (Arc<Engine>, crate::AppWindow) {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::AppWindow::new().unwrap();
        let ui = crate::ui_api::UiHandle::new(slint::ComponentHandle::as_weak(&window));
        let store = store::Store::open_in_memory().unwrap();
        let now = now_ms();
        for (id, conv, mri, days, html) in msgs {
            store
                .upsert_message(&MessageRow {
                    id: (*id).into(),
                    conversation_id: (*conv).into(),
                    parent_id: None,
                    sender_mri: (*mri).into(),
                    sender_name: "Test Sender".into(),
                    compose_ms: now - days * 86_400_000 - id.len() as i64,
                    edited_ms: None,
                    deleted: false,
                    message_type: "RichText/Html".into(),
                    content_html: (*html).into(),
                    client_message_id: None,
                    reactions_json: "[]".into(),
                })
                .unwrap();
        }
        let e = Engine::new(
            teams_proto::Client::offline("test").unwrap(),
            store,
            session::Config::default(),
            ui,
            "8:me".into(),
            "Me".into(),
        );
        e.offline.store(true, Ordering::Relaxed);
        (e, window)
    }

    fn message_ids(e: &Engine) -> Vec<String> {
        let f = e.find.lock().unwrap();
        f.hits
            .iter()
            .filter(|h| h.kind == "message")
            .map(|h| h.message.clone())
            .collect()
    }

    #[test]
    fn an_offline_search_shows_the_cached_matches_with_the_filters() {
        let (e, _window) = engine_with(&[
            (
                "1",
                "19:a",
                "8:orgid:x",
                0,
                "<p>The <b>budget</b> is final</p>",
            ),
            ("2", "19:b", "8:orgid:y", 3, "<p>budgets for Q3</p>"),
            ("3", "19:a", "8:orgid:y", 40, "<p>old budget draft</p>"),
            ("4", "19:a", "8:orgid:x", 0, "<p>ميزانيةُ المشروع</p>"),
            ("5", "19:a", "8:orgid:x", 0, "<p>lunch</p>"),
        ]);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(
            e.clone()
                .handle(super::super::Cmd::Find(FindCmd::Search("budg".into()))),
        );
        assert_eq!(
            message_ids(&e),
            ["1", "2", "3"],
            "prefix matches, newest first"
        );
        {
            let f = e.find.lock().unwrap();
            assert!(f.searched, "offline is not an error");
            let first = f.hits.iter().find(|h| h.message == "1").unwrap();
            assert_eq!(first.conv, "19:a");
            assert_eq!(first.text, "The budget is final");
            assert_eq!(
                first.marked,
                format!(
                    "The {}budget{} is final",
                    store::MARK_START,
                    store::MARK_END
                )
            );
        }

        // Arabic without tashkeel and with the plain teh marbuta form.
        rt.block_on(
            e.clone()
                .handle(super::super::Cmd::Find(FindCmd::Search("ميزانيه".into()))),
        );
        assert_eq!(message_ids(&e), ["4"]);

        // The In, From and date filters narrow the cached matches.
        e.find.lock().unwrap().query = "budget".into();
        e.find.lock().unwrap().in_conv = Some(("19:a".into(), "A".into()));
        rt.block_on(e.run_search());
        assert_eq!(message_ids(&e), ["1", "3"]);
        e.find.lock().unwrap().person = Some(("8:orgid:y".into(), "Y".into()));
        rt.block_on(e.run_search());
        assert_eq!(message_ids(&e), ["3"]);
        e.find.lock().unwrap().in_conv = None;
        e.find.lock().unwrap().date = Date::Week;
        rt.block_on(e.run_search());
        assert_eq!(message_ids(&e), ["2"]);

        // A server answer for the same message does not add a second row.
        let mut f = e.find.lock().unwrap();
        let server = Hit {
            marked: String::new(),
            ..f.hits[0].clone()
        };
        let before = f.hits.len();
        merge_messages(&mut f.hits, vec![server]);
        assert_eq!(f.hits.len(), before);
        assert!(!f.hits[0].marked.is_empty(), "the cache's marked row stays");
    }

    #[test]
    fn commands_parse() {
        assert_eq!(
            FindCmd::parse("search", "hello"),
            Some(FindCmd::Search("hello".into()))
        );
        assert_eq!(
            FindCmd::parse("details", "open"),
            Some(FindCmd::Details(true))
        );
        assert_eq!(
            FindCmd::parse("details", "close"),
            Some(FindCmd::Details(false))
        );
        assert_eq!(FindCmd::parse("find-next", ""), Some(FindCmd::FindNext));
        assert_eq!(FindCmd::parse("nope", ""), None);
    }

    #[test]
    fn all_tab_shows_the_top_of_each_category_with_headers() {
        let mut hits: Vec<Hit> = (0..8)
            .map(|i| hit("message", "c", &i.to_string(), i))
            .collect();
        hits.extend((0..5).map(|i| hit("file", "", &i.to_string(), i)));
        hits.push(hit("person", "", "", 0));
        let rows = rows_for(Tab::All, false, &hits);
        let kinds: Vec<_> = rows.iter().map(|(h, _)| h.kind).collect();
        assert_eq!(
            kinds.iter().filter(|k| **k == "message").count(),
            ALL_MESSAGES
        );
        assert_eq!(kinds.iter().filter(|k| **k == "file").count(), ALL_OTHER);
        let sections: Vec<_> = rows
            .iter()
            .map(|(_, s)| s.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(sections, ["Messages", "Files", "People"]);
    }

    #[test]
    fn a_category_tab_shows_only_that_category_without_headers() {
        let hits = vec![hit("message", "c", "1", 1), hit("file", "", "2", 2)];
        let rows = rows_for(Tab::Files, false, &hits);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.kind, "file");
        assert!(rows[0].1.is_empty());
    }

    #[test]
    fn filters_leave_only_messages_in_the_all_tab() {
        let hits = vec![hit("message", "c", "1", 1), hit("person", "", "", 0)];
        let rows = rows_for(Tab::All, true, &hits);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.kind, "message");
    }

    #[test]
    fn server_hits_do_not_repeat_cache_hits_and_sort_newest_first() {
        let mut hits = vec![hit("message", "c", "100", 100), hit("person", "", "", 0)];
        merge_messages(
            &mut hits,
            vec![
                hit("message", "c", "100", 100),
                hit("message", "c", "300", 300),
                // A channel reply known only by its time, 7 ms off the cached one.
                Hit {
                    root: "50".into(),
                    ..hit("message", "c", "207", 207)
                },
                Hit {
                    root: "50".into(),
                    ..hit("message", "c", "200", 200)
                },
            ],
        );
        let ids: Vec<_> = hits
            .iter()
            .filter(|h| h.kind == "message")
            .map(|h| h.message.as_str())
            .collect();
        assert_eq!(ids, ["300", "207", "100"]);
        assert!(hits.iter().any(|h| h.kind == "person"));
    }

    #[test]
    fn date_filter_maps_to_a_start_time_and_a_day() {
        let now = 1_795_000_000_000;
        assert_eq!(Date::Any.since_ms(now), 0);
        assert_eq!(Date::Day.since_ms(now), now - 86_400_000);
        assert_eq!(Date::parse("7d"), Date::Week);
        assert_eq!(Date::parse("30d").label(), "Past month");
        assert_eq!(day_of(0), "1970-01-01");
        assert_eq!(day_of(1_795_000_000_000), "2026-11-18");
    }
}
