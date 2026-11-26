use super::*;

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
    /// "Mark as read" on a conversation header of the Unreads view (the header row's id).
    UnreadsRead(String),
    /// "Mark all as read" in the Unreads view.
    UnreadsReadAll,
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
            "unreads-read" => FindCmd::UnreadsRead(a),
            "unreads-read-all" => FindCmd::UnreadsReadAll,
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
pub(super) enum Tab {
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
    pub(super) fn parse(s: &str) -> Tab {
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
    pub(super) fn kind(self) -> Option<&'static str> {
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
pub(super) enum Date {
    #[default]
    Any,
    Day,
    Week,
    Month,
}

impl Date {
    pub(super) fn parse(s: &str) -> Date {
        match s {
            "24h" => Date::Day,
            "7d" => Date::Week,
            "30d" => Date::Month,
            _ => Date::Any,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Date::Any => "",
            Date::Day => "Past 24 hours",
            Date::Week => "Past week",
            Date::Month => "Past month",
        }
    }

    /// The earliest time a message may have been sent, or 0.
    pub(super) fn since_ms(self, now: i64) -> i64 {
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
pub(super) fn day_of(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// One search or quick view result before it becomes a row.
#[derive(Debug, Clone, Default)]
pub(in super::super) struct Hit {
    pub(super) kind: &'static str,
    pub(super) conv: String,
    pub(super) message: String,
    pub(super) root: String,
    pub(super) time_ms: i64,
    pub(super) mri: String,
    /// Who sent it, or the name of a person, file or conversation.
    pub(super) name: String,
    /// Where it happened (`Team › channel`, a chat's title) or a person's job.
    pub(super) place: String,
    /// Plain text to show a snippet of. On an `unread-group` header (a conversation of the
    /// Unreads view) it is the number of unread messages listed under it.
    pub(super) text: String,
    /// A snippet the cache's search index cut and marked (`store::MARK_START`…), shown instead
    /// of highlighting `text`; empty for the other hits.
    pub(super) marked: String,
    pub(super) url: String,
}

impl Hit {
    pub(super) fn id(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.kind, self.conv, self.message, self.root, self.mri, self.url
        )
    }
}

/// A match of Find in chat.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Match {
    pub(super) id: String,
    /// The thread root when the match is a channel reply.
    pub(super) root: Option<String>,
    pub(super) time_ms: i64,
}

#[derive(Default)]
pub(super) struct ChatFind {
    pub(super) open: bool,
    pub(super) query: String,
    pub(super) matches: Vec<Match>,
    /// Index into `matches`, in time order.
    pub(super) index: usize,
    pub(super) seq: u64,
}

#[derive(Default)]
pub(in super::super) struct State {
    // Search page.
    pub(super) query: String,
    pub(super) tab: Tab,
    pub(super) in_conv: Option<(String, String)>,
    pub(super) person: Option<(String, String)>,
    pub(super) date: Date,
    pub(super) editor: String,
    pub(super) picks: Vec<(String, String)>,
    /// Bumped per editor query, so a slow people search older than the latest one is dropped.
    pub(super) pick_seq: u64,
    pub(super) seq: u64,
    pub(super) hits: Vec<Hit>,
    pub(super) msg_from: u32,
    pub(super) more: bool,
    /// Next page of Files, People and Meetings (the global search), see `global`.
    pub(super) global_more: Paging,
    pub(super) searched: bool,
    // Quick views.
    pub(super) quick: Option<&'static str>,
    pub(super) quick_hits: Vec<Hit>,
    pub(super) quick_from: u32,
    pub(super) quick_more: bool,
    pub(super) quick_seq: u64,
    pub(super) followed: Vec<FollowedThread>,
    pub(super) followed_loaded: bool,
    pub(super) follow_override: HashMap<(String, String), bool>,
    // Find in chat.
    pub(super) find: ChatFind,
    /// The Messages group of the top search box's dropdown.
    pub(super) top: top_messages::TopMessages,
    pub(in super::super) shared: super::super::shared::State,
}
