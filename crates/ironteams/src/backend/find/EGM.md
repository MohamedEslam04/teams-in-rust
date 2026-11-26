# backend/find

## Overview
Search features: global search (Files/People/Meetings), top search box suggestions, find-in-chat, Saved/Mentions/Followed quick views and Discover. `model.rs` defines `FindCmd`; `dispatch.rs` routes every command.

Files:
- `discover.rs`: Discover quick view: ranks last-7-days channel posts from the local store by reactions plus replies, plus reaction_count/engagement helpers.
- `dispatch.rs`: Engine::find_cmd routes every FindCmd to its handler; find_chat_opened resets find/quick state; location/row helpers (conv_row, location_of, hit_from_message/row, row_data); Search page push/start/rerun.
- `global.rs`: Global search (Files/People/Meetings tabs): paging, hit conversion, legacy per-kind fallback, Show-more, event-row open to calendar.
- `goto.rs`: Top search box local side: cached-conversation suggestions and /goto, /chat, /call resolution and navigation.
- `model.rs`: Data model of finding: FindCmd enum and name/arg parser, Tab, Date filter, Hit, Match, ChatFind and the find State.
- `navigate.rs`: Search filter editors and picks, opening a hit, landing on a message, Find in chat (local plus server matches, stepping), quick view opening.
- `pure.rs`: Pure helpers: category row layout for the find view, plain text of a message, longest term, conversation kind labels, and merging server message hits into the hit list.
- `quick_lists.rs`: Engine methods for the quick views (Saved, Mentions, Followed threads): load from cache then server, paging, unsave/unfollow, follow overrides and thread fetch into the store.
- `search.rs`: Engine search flow: run_search (local cache hits then server messages and global others), local message/conversation hits, and 'Show more' paging.
- `top_messages.rs`: Messages group of the top search box dropdown: generation-guarded TopMessages state, cache hits at once then debounced server search, pushed to the UI.
- `unreads.rs`: Unreads quick view: unread messages from others past each unread conversation's read horizon, grouped by conversation (header row `unread-group` with the count, newest conversation first, messages oldest first), per-conversation and all "Mark as read", refresh when the chat list changes.

## Interface
- `[model.rs:FindCmd]` pub enum of finding UI commands (Search, Go, Quick, Shared*, ChannelTab*, FindQuery, ...)
- `[pure.rs:rows_for]` pub(super) fn rows_for(tab: Tab, filtered: bool, hits: &[Hit]) -> Vec<(Hit, String)> ; rows per category with section label on first row; All tab caps (ALL_MESSAGES/ALL_OTHER), category tab caps TAB_MAX, filtered All shows only messages
- `[model.rs:FindCmd::parse]` pub fn parse(name:&str, arg:&str)->Option<FindCmd>; None for unknown names; Details(open) is arg=="open"
- `[pure.rs:plain_of]` pub(super) fn plain_of(m: &MessageRow) -> String ; message HTML to plain text via render
- `[model.rs:Tab]` pub(super) enum All|Messages|Files|People|Channels|Chats|Meetings with parse(&str) and kind()->Option<&'static str>
- `[pure.rs:longest]` pub(super) fn longest(terms: &[String]) -> Option<&str> ; longest term by char count
- `[model.rs:Date]` pub(super) enum Any|Day|Week|Month with parse (24h/7d/30d), label(), since_ms(now)
- `[pure.rs:kind_label]` pub(super) fn kind_label(k: ConvKind) -> &'static str
- `[model.rs:day_of]` pub(super) fn(ms:i64)->String YYYY-MM-DD (UTC) for the sent>= server filter
- `[pure.rs:merge_messages]` pub(super) fn merge_messages(hits: &mut Vec<Hit>, new: Vec<Hit>) ; dedups, sorts messages newest first, other kinds keep order
- `[model.rs:Hit]` pub(in backend) struct {kind,conv,message,root,time_ms,mri,name,place,text,marked,url}; id() joins kind|conv|message|root|mri|url
- `[search.rs:Engine::run_search]` pub(super) async fn run_search(self: &Arc<Self>) ; full search: local hits then server messages + others
- `[model.rs:State]` pub(in backend) struct of search page, quick view, find-in-chat and shared state (fields pub(super); `shared` pub(in backend))
- `[search.rs:Engine::local_message_hits]` pub(super) fn local_message_hits(&self, q, in_conv: Option<&str>, person: Option<&str>, since: i64) -> Vec<Hit> ; FTS on the store, max TAB_MAX
- `[model.rs:ChatFind]` pub(super) struct {open,query,matches,index,seq}; Match {id,root,time_ms}
- `[search.rs:Engine::hit_from_local]` pub(super) fn hit_from_local(&self, h: &LocalHit) -> Hit
- `[dispatch.rs:Engine::find_cmd]` pub(in backend) async fn(self:&Arc<Self>, FindCmd)->anyhow::Result<()> main dispatcher
- `[search.rs:Engine::local_conversation_hits]` pub(super) fn local_conversation_hits(&self, terms: &[String]) -> Vec<Hit> ; channels/groups/meetings whose name contains all terms
- `[dispatch.rs:Engine::find_chat_opened]` pub(in backend) fn(&Arc<Self>, conv:&str) resets find bar/quick view/chat tab, calls shared_chat_opened
- `[search.rs:Engine::more_results]` pub(super) async fn more_results(self: &Arc<Self>) ; routes to quick_more, global_more, or next server message page
- `[dispatch.rs:Engine::location_of]` pub(in backend) fn(&self, conv, topic, team)->String; 'Team › channel', chat title, 'Notes' or fallback
- `[top_messages.rs:TOP_MAX]` pub(super) const TOP_MAX: usize = 6
- `[dispatch.rs:Engine::conv_row]` pub(super) fn(&self, conv)->Option<ConversationRow> from store
- `[top_messages.rs:TopMessages]` pub(super) struct (Default) with pub(super) hits: Vec<Hit> and private generation
- `[dispatch.rs:Engine::user_name]` pub(super) fn(&self, mri)->Option<String> non-empty display name from store
- `[top_messages.rs:TopMessages::start]` pub(super) fn start(&mut self, local: Vec<Hit>) -> u64 ; bumps generation, replaces hits, truncates to TOP_MAX
- `[dispatch.rs:Engine::hit_from_message/hit_from_row]` pub(super) build Hit from MessageHit / MessageRow
- `[top_messages.rs:TopMessages::is_current]` pub(super) fn is_current(&self, generation: u64) -> bool
- `[dispatch.rs:Engine::row_data]` pub(super) fn(&Arc<Self>, &Hit, section, terms)->HitData for the UI row (snippet highlight, initials, color, avatar, rtl)
- `[top_messages.rs:TopMessages::add_server]` pub(super) fn add_server(&mut self, generation: u64, server: Vec<Hit>) -> bool ; merge + truncate, false if stale
- `[dispatch.rs:Engine::filters_active/push_filters/push_search]` pub(super) Search page filter state and result push to UI
- `[top_messages.rs:Engine::suggest_messages]` pub(super) fn suggest_messages(self: &Arc<Self>, query: String) ; cache hits now, spawns delayed server search
- `[dispatch.rs:Engine::search_start/rerun_search]` pub(super) async; start a search on query / rerun only if query non-blank
- `[quick_lists.rs:Engine::load_saved]` pub(super) fn load_saved(self: &Arc<Self>, seq: u64) ; Saved view from store
- `[navigate.rs:Engine::editor_open/pick_query/pick]` pub(super) async filter editors (In / Person) and applying a pick
- `[quick_lists.rs:Engine::unsave_row]` pub(super) async fn unsave_row(self: &Arc<Self>, id: &str) -> anyhow::Result<()>
- `[navigate.rs:Engine::open_hit]` pub(super) async fn(&Arc<Self>, id)->Result; opens message/thread/file/person/event/chat hit by id
- `[quick_lists.rs:Engine::push_quick]` pub(super) fn push_quick(self: &Arc<Self>, loading: bool) ; sends quick_hits to ui.quick_rows
- `[navigate.rs:Engine::open_conversation]` pub(super) async; opens chat/channel and closes quick view
- `[quick_lists.rs:Engine::load_mentions]` pub(super) async fn load_mentions(self: &Arc<Self>, seq: u64) ; cache scan for Mention rows then server page 0
- `[navigate.rs:Engine::close_quick]` pub(super) fn; clears quick state and UI quick_open(false)
- `[quick_lists.rs:Engine::mentions_page]` pub(super) async fn mentions_page(self: &Arc<Self>, seq: u64, from: u32)
- `[navigate.rs:Engine::go_to_message]` pub(in backend) async fn(conv,id,time_ms,root:Option<&str>)->Result; opens chat and lands on message
- `[quick_lists.rs:Engine::quick_more]` pub(super) async fn quick_more(self: &Arc<Self>, kind: &str) ; only 'mentions' pages
- `[navigate.rs:Engine::land_on]` pub(super) async; scrolls to message, opening thread first for channel replies, fade flag
- `[quick_lists.rs:Engine::load_followed]` pub(super) async fn load_followed(self: &Arc<Self>, seq: u64) ; fetch followed threads, store them, build thread hits
- `[navigate.rs:Engine::chat_find_close/chat_find_query/set_matches/chat_find_step]` pub(super) Find in chat
- `[quick_lists.rs:Engine::followed_now]` pub(super) fn followed_now(&self) -> Vec<FollowedThread> ; feed list with follow_override applied
- `[navigate.rs:Engine::quick_open]` pub(super) async fn(&Arc<Self>, kind:&str); followed|saved|discover else mentions
- `[quick_lists.rs:Engine::store_thread]` pub(super) async fn store_thread(self: Arc<Self>, conv: &str, root: &str) ; fetch thread messages into store
- `[goto.rs:Engine::suggest_start]` pub(super) fn(&Arc<Self>, query) sends topbar suggestions from cached conversations
- `[quick_lists.rs:Engine::thread_hit]` pub(super) fn thread_hit(&self, t: &FollowedThread) -> Hit
- `[goto.rs:Engine::go_to]` pub(super) async fn(&Arc<Self>, arg:&str)->Result; arg = command U+001F name
- `[quick_lists.rs:Engine::unfollow_row]` pub(super) async fn unfollow_row(self: &Arc<Self>, id: &str) -> anyhow::Result<()> ; optimistic removal, rollback on error
- `[global.rs:Engine::others_first]` pub(super) async fn(&Arc<Self>, q)->Others; global search for Files/People/Events with legacy fallback
- `[quick_lists.rs:Engine::set_follow]` pub(super) async fn set_follow(self: &Arc<Self>, id: &str, follow: bool) -> anyhow::Result<()> ; for open channel
- `[global.rs:Engine::global_more]` pub(super) async fn(&Arc<Self>, SearchKind) next page for a tab
- `[quick_lists.rs:Engine::apply_follow]` pub(super) async fn apply_follow(self: &Arc<Self>, conv: &str, root: &str, follow: bool) -> anyhow::Result<()>
- `[global.rs:store_others/is_global/add_new/tab_has_more/kind_of_tab/hits_of]` pub(super) helpers over State/Hit/Paging
- `[quick_lists.rs:Engine::menu_follow]` pub(super) async fn menu_follow(self: &Arc<Self>, id: &str) ; tells menu whether thread is followed

## Invariants
- `[discover.rs:load_discover]` Discarded when f.quick_seq != seq (stale quick view); results stored then push_quick(false)
- `[search.rs:run_search]` f.seq is incremented under the find lock at start; after each await/long step, results are discarded if f.seq != seq (stale search guard)
- `[discover.rs:DAYS/CHANNELS/PER_CHANNEL/SHOWN]` 7 days, 200 channels, 50 posts per channel, 50 rows shown; Control/ and ThreadActivity/ and deleted messages skipped
- `[search.rs:run_search]` empty terms_of(q) clears hits, sets searched=false and returns without network
- `[discover.rs:rank]` Sort by reactions+replies desc, then compose_ms desc, then id asc; 'follow' reactions not counted
- `[search.rs:run_search]` filtered (in_conv/person/date set) skips local conversation hits and the global 'others' search
- `[goto.rs:ALL]` Suggestions/goto scan up to 5000 cached conversations only; no server calls
- `[search.rs:run_search]` offline: only local cache hits are shown, no server calls
- `[goto.rs:go_to]` Argument format is command, U+001F, name; unknown command is a silent no-op; no match shows a banner
- `[search.rs:more_results]` discards the page if f.seq != seq or f.msg_from != from, so a second concurrent Show more cannot take the same page
- `[goto.rs:call_person]` Calls dispatch only under feature `calls`; otherwise a banner
- `[quick_lists.rs:mentions_page]` discards if f.quick_seq != seq or f.quick_from != from; only server hits with is_mention are kept
- `[global.rs:apply_page/global_more]` Page applied only if f.seq == seq; a duplicate Show-more is dropped when global_more.get(kind) changed during the await; skipped when offline or no next page
- `[quick_lists.rs:load_saved/load_mentions/load_followed]` results dropped when f.quick_seq != seq (newer view)
- `[global.rs:is_global/store_others]` A new first answer replaces file, person and event rows (meeting without conv) only; cached meeting chats and messages stay
- `[top_messages.rs:TopMessages]` generation counter incremented by start; add_server only applies when generation matches, so late answers never overwrite newer hits
- `[global.rs:FIRST_PAGE/OTHER_KINDS]` First query asks 10 hits each of Files, People, Events; messages come from the older path
- `[top_messages.rs:TOP_MAX/SERVER_MIN_CHARS/SERVER_PAUSE]` hits capped at 6; server asked only for queries >= 2 chars, online, after 200 ms extra pause and still current
- `[navigate.rs:pick_query]` Result applied only if the editor is unchanged and pick_seq matches (newest wins); 'in' scans 2000 convs, takes 8; Notes hidden when terms exist
- `[pure.rs:merge_messages]` duplicate = same conv and (same message id, or same non-empty root within 1500 ms); stable sort puts messages newest first, non-messages keep order
- `[navigate.rs:chat_find_query/set_matches]` f.find.seq guards stale results; matches sorted oldest first; remote matches deduped by id or same root within 1500 ms; replies resolved to nearest within 1500 ms
- `[quick_lists.rs:apply_follow]` follow_override is set before the server call; rolled back only if the override still equals the value this call set
- `[dispatch.rs:find_cmd/ChannelTabOpen]` Only https:// URLs are opened for channel tabs; Mutex locks on self.find are released before awaiting/UI calls (scoped blocks)
- `[quick_lists.rs:unfollow_row]` row removed immediately, reinserted at original (clamped) index if apply_follow fails; mutex guards are dropped before awaits

## Side Effects
- `[dispatch.rs:find_cmd]` Mutates the self.find mutex state and drives UI (show_view, push_*), dispatching Cmd::OpenUrl/OpenThread/CloseThread
- `[search.rs:run_search]` network: client.search_messages and others_first via tokio::join!; SQLite reads via store
- `[global.rs:others_first]` Network: client.global_search; fallback client.search_people and onedrive::search; tracing warn on failure
- `[search.rs:more_results]` network: client.search_messages next page; tracing::warn on error
- `[global.rs:global_more]` Network: paged client.global_search
- `[top_messages.rs:suggest_messages]` tokio::spawn delayed task with tokio::time::sleep and client.search_messages; drives ui.topbar_searching and ui.topbar_messages
- `[navigate.rs:pick_query]` Network: client.search_people for the Person editor unless offline
- `[quick_lists.rs:unsave_row]` store.unsave_message write, mark(|d| d.messages = true), network client.set_saved(false) best effort
- `[navigate.rs:chat_find_query]` Network: client.search_messages scoped to the chat unless offline
- `[quick_lists.rs:mentions_page]` network search; on error ui.banner and tracing::warn
- `[discover.rs:load_discover/goto.rs]` Reads the SQLite store through the self.store mutex (conversations, messages, reply counts)
- `[quick_lists.rs:load_followed]` network client.followed_threads; JoinSet of up to FOLLOWED_MAX store_thread tasks; ui.banner on error
- `[navigate.rs:open_hit/go_to_message]` Opens chats (self.open), threads, files by URL, people profile, calendar goto; shows banners
- `[quick_lists.rs:store_thread]` network client.messages(conv;messageid=root, 100) and store.upsert_message writes
- `[global.rs:open_event]` Dispatches Cmd::Calendar(CalCmd::Goto) after switching the view to calendar
- `[quick_lists.rs:apply_follow]` network client.set_thread_follow; mutates find.follow_override global engine state
- `[dispatch.rs:find_cmd]` Delegates to follow, saved, shared files and channel/chat tab handlers (network, uploads, downloads, clipboard copy of link) implemented elsewhere
- `[quick_lists.rs:set_follow]` ui.menu_following and ui.banner
- `[quick_lists.rs:menu_follow]` network client.followed_threads when not yet loaded; sets followed/followed_loaded; ui.menu_following

## Verification
- Run: `cargo test -p ironteams`
