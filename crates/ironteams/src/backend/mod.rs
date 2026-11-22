//! The app engine: sign-in, sync, live events, commands from the UI and rendering.
//!
//! The SQLite store is the source of truth. Network results and live events are written to the
//! store, then marked "dirty"; a render loop rebuilds only the visible view data and pushes it to
//! the UI. This keeps memory small: only the current conversation window is materialized.

use crate::data::Pixels;
use crate::ui_api::UiHandle;
use drafts::reply_preview;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::{MessageRow, Store};
use teams_proto::Client;
use teams_proto::delivery::Importance;

mod actions;
mod activity;
pub(crate) mod appstore;
mod attach;
mod avatar_placeholder;
mod banner;
#[cfg(target_os = "linux")]
mod banner_reply;
mod cal;
pub(crate) mod call;
#[cfg(feature = "calls")]
pub(crate) mod call_direct;
pub(crate) mod call_feedback;
pub(crate) mod call_presence;
#[cfg(feature = "calls")]
pub(crate) mod calls_page;
mod cards;
mod channel_tabs;
mod channels;
mod clip_capture;
pub(crate) mod composer_apps;
mod dirty_stats;
mod drafts;
pub(crate) mod drive;
pub(crate) mod engage;
pub(crate) mod expiry;
mod find;
mod gifs;
mod in_order;
mod lists;
mod live;
mod loop_paragraph;
mod manage;
mod media;
pub(crate) mod meeting_cards;
mod mentions;
mod msg_memo;
mod my_presence;
mod outbox;
pub(crate) mod outsider;
mod people;
mod people_card;
#[cfg(test)]
mod people_live;
pub(crate) mod people_presence;
mod photo;
mod remind;
mod render;
pub(crate) mod ringtone;
mod scheduled;
mod send;
pub(crate) mod settings;
mod shared;
pub(crate) mod sounds;
mod stickers;
pub(crate) mod storyline;
mod sync;
mod team_photo;
pub(crate) mod typing;
mod unread;
pub(crate) mod util;
mod video_clips;
mod voice_messages;

pub use activity::ActivityCmd;
#[cfg(test)]
pub(crate) use activity::{State as ActivityState, row_data as activity_row_data};
pub use appstore::AppsCmd;
#[cfg(test)]
pub use cal::{
    AttendeeRow, Ctx, DEFAULT_PREFS, Prefs, ProposalRow, SuggestRow, build, build_split,
    filter_opts, parse_view, peek_data, room_rows, sched_rows,
};
pub use cal::{
    CalCmd, CalData, DetailData, FormFields, FormOpen, Panels, PeekData, PermRow, RoomsPush,
    SchedPush, SchedReq, SlotReq, local_tz, parse_hm, people_lenient, share_roles, slot_for_pick,
    slot_minutes,
};
#[cfg(feature = "calls")]
pub use call_direct::CallCmd as DirectCallCmd;
#[cfg(feature = "calls")]
pub use calls_page::{CallsCmd, CallsData};
pub use channels::ChannelCmd;
pub use composer_apps::ComposerAppsCmd;
pub use drive::DriveCmd;
pub use engage::EngageCmd;
pub use find::FindCmd;
pub use gifs::{GifCmd, GifRow};
pub use lists::ListsCmd;
#[cfg(test)]
pub use lists::MANAGE_APPS_NOTE;
pub use manage::{
    CommunityData, DeletedData, DialogData, DiscoverData, PersonData as TeamsPersonData, PostData,
    SettingData, TagData, TeamRowData, TeamsCmd, TeamsData,
};
pub use media::MediaCmd;
#[cfg(test)]
pub(crate) use media::decode_for_test;
pub use people::PeopleCmd;
pub use remind::{RemindCmd, ReminderData, SnoozedData};
pub use scheduled::ScheduledCmd;
#[cfg(test)]
pub(crate) use video_clips::RecordedClip;
pub use video_clips::{ClipCamera, ClipCmd, ClipDevices};
pub use voice_messages::VoiceCmd;

/// Commands sent by the UI.
#[derive(Debug, Clone)]
pub enum Cmd {
    RetryLogin,
    SelectChat(String),
    SelectChannel(String, String),
    ToggleTeam(usize),
    ToggleFavorite(String),
    SetFilter(String),
    Search(String),
    Send(String, String),
    /// A reply typed in a notification or a message card: the text for a chat (or a channel
    /// thread), sent without opening it.
    QuickReply(String, String),
    TypingTick,
    LoadOlder,
    React(String, String),
    Reply(String),
    CancelReply,
    Edit(String),
    Delete(String),
    OpenUrl(String),
    OpenThread(String),
    CloseThread,
    AttachImage,
    AttachFile,
    CopyLink(String),
    ToggleSave(String),
    /// The composer's "Mark as important" state, sent right before the message it applies to.
    SetImportant(bool),
    /// "Set delivery options" › Urgent, sent right before the message it applies to.
    SetUrgent(bool),
    /// The send button's "Schedule message" and the scheduled messages of the open chat.
    Scheduled(ScheduledCmd),
    Pin(String),
    /// Message menu "Mark as unread": the chat turns unread from this message on.
    MarkUnread(String),
    /// Forward dialog opened for one message, or for the multi-select ticks when `None`.
    ForwardBegin(Option<String>),
    ForwardSearch(String),
    ForwardToggle(String),
    ForwardSend(String),
    ForwardCancel,
    SelectBegin(String),
    ToggleSelect(String),
    SelectCancel,
    /// The list reports whether it sits at the newest message.
    AtBottom(bool),
    /// Failed row: send it again (the row id).
    RetrySend(String),
    /// Failed row: forget it (the row id).
    DiscardSend(String),
    /// Chat list "Mark as unread": the chat turns unread from its newest message on.
    MarkChatUnread(String),
    MarkAllRead,
    Calendar(CalCmd),
    Activity(ActivityCmd),
    Find(FindCmd),
    People(PeopleCmd),
    Teams(TeamsCmd),
    Settings(settings::SettingsCmd),
    Drive(DriveCmd),
    /// Viva Engage: a community's read-only feed (plan S3.2).
    Engage(EngageCmd),
    Gif(GifCmd),
    /// The chat list: sections, item menu, options, channel menu (plan 8.1 to 8.3, 8.5).
    Lists(ListsCmd),
    /// The channel Posts tab (plan 8.4).
    Channels(ChannelCmd),
    /// The image viewer, Translate and link previews (plan 9.5 to 9.7).
    Media(MediaCmd),
    Apps(AppsCmd),
    /// Reminders on messages and snoozed chats (local to this device).
    Remind(RemindCmd),
    /// Voice messages: the players in chat rows and the recorder (`voice_messages`).
    Voice(VoiceCmd),
    /// "Record video clip" and the clip cards in chat rows (`video_clips`).
    Clip(ClipCmd),
    /// A Loop paragraph from the composer, sent as a formatted message (its HTML).
    SendLoop(String),
    /// The call window (pre-join, meeting controls, call ended).
    Call(call::CallCmd),
    /// The composer's "+" apps picker (plan S3.6).
    ComposerApps(ComposerAppsCmd),
    SetEnterSends(bool),
    /// The composer text changed.
    DraftEdited(String),
    /// The window is closing: write what is pending.
    Flush,
    /// An emoji was inserted from the panel or the `:name` popup.
    EmojiUsed(String),
    SetSkinTone(u8),
    /// Ctrl+V: attach the clipboard image, if there is one.
    PasteImage,
    /// A file dropped on the window.
    AddFile(std::path::PathBuf),
    RemoveAttachment(usize),
    /// 1:1 calls: the incoming toast and the outgoing ringing screen.
    #[cfg(feature = "calls")]
    DirectCall(DirectCallCmd),
    /// The Calls page: history, speed dial, voicemail (plans/calls.md K1.1 to K1.3).
    #[cfg(feature = "calls")]
    CallsPage(CallsCmd),
}

#[derive(Default)]
struct View {
    current: Option<String>,
    current_title: String,
    thread_root: Option<String>,
    filter: String,
    search: String,
    limit: usize,
    reply_to: Option<MessageRow>,
    editing: Option<String>,
    /// Sent messages (client message id, optimistic row) waiting for their echo.
    pending: Vec<(String, MessageRow)>,
    typing: typing::Typing,
    /// Messages that arrived live while their chat was open, and when (they fade in once).
    fresh: HashMap<String, Instant>,
    last_typing_sent: Option<Instant>,
    collapsed_teams: HashSet<String>,
    teams_order: Vec<String>,
    /// Multi-select ticks, in the order they were made (at most `actions::MAX_FORWARD`).
    selected: Vec<String>,
    /// Open forward dialog: what is forwarded, the last search and the picked chats.
    forward: Option<ForwardState>,
    /// Chat marked unread while open; the list shows it until another chat is opened.
    keep_unread: Option<String>,
    /// Read horizon when the open chat was opened, kept so the "New messages" divider stays
    /// where it was while the chat is read.
    unread_marker: Option<i64>,
    /// Where the open chat opens (from the config when it was opened).
    open_at: session::OpenChatAt,
    /// True from opening a chat until its first full render, which may scroll to the divider.
    opening: bool,
    /// Set once the newest page of the opened chat is in the store.
    loaded: bool,
    /// The open chat refused its history because the user is not in it (`open`): it shows a
    /// calm note instead of messages and a composer, and opening it again asks once more.
    not_member: bool,
    /// The list is at (or about to follow) the newest message.
    at_bottom: bool,
    /// The first message shown before an older page was added, for the list to keep in place.
    older_anchor: Option<String>,
    /// New messages that arrived while scrolled up; shown on the jump button.
    new_count: usize,
    /// The delivery option of the next message: "Mark as important" or "Set delivery options"
    /// (set by the composer just before a send).
    importance: teams_proto::delivery::Importance,
    /// The scheduled messages of the account, as last read (or made here).
    scheduled: Vec<teams_proto::scheduled::ScheduledMessage>,
    /// `scheduled` was read from the account once.
    scheduled_loaded: bool,
    /// Latest composer text of the current chat as the engine knows it.
    draft_text: String,
    /// `draft_text` (or the reply target) changed since it was last written to the store.
    draft_dirty: bool,
    draft_gen: u64,
    /// Attachments waiting to be sent, by chat.
    attachments: HashMap<String, Vec<attach::Attachment>>,
}

#[derive(Default)]
struct ForwardState {
    ids: Vec<String>,
    query: String,
    picked: Vec<String>,
}

#[derive(Default)]
struct Dirty {
    chats: bool,
    messages: bool,
    teams: bool,
    header: bool,
    scroll: bool,
    activity: bool,
}

impl Dirty {
    /// Sets every part `other` sets.
    fn merge(&mut self, other: &Dirty) {
        self.chats |= other.chats;
        self.messages |= other.messages;
        self.teams |= other.teams;
        self.header |= other.header;
        self.scroll |= other.scroll;
        self.activity |= other.activity;
    }

    /// The parts set, as bits in the order of `dirty_stats::PARTS`.
    fn mask(&self) -> u8 {
        [
            self.chats,
            self.messages,
            self.teams,
            self.header,
            self.scroll,
            self.activity,
        ]
        .iter()
        .enumerate()
        .fold(0, |m, (i, on)| if *on { m | 1 << i } else { m })
    }
}

/// Shared engine state.
pub struct Engine {
    client: Client,
    store: Mutex<Store>,
    cfg: Mutex<session::Config>,
    cal: cal::CalState,
    ui: UiHandle,
    me: String,
    my_name: String,
    view: Mutex<View>,
    dirty: Mutex<Dirty>,
    /// The dirty marks of the last minute, by the line that made them (debug log only).
    mark_stats: Mutex<dirty_stats::MarkStats>,
    /// Signalled on every dirty mark and typing change: the render loop sleeps until then.
    render_wake: tokio::sync::Notify,
    /// The parsed text of the message rows last built (`build_msgs`).
    msg_memo: Mutex<msg_memo::MsgMemo>,
    activity: Mutex<activity::State>,
    people: Mutex<people::State>,
    find: Mutex<find::State>,
    manage: Mutex<manage::State>,
    drive: Mutex<drive::Drive>,
    engage: Mutex<engage::State>,
    lists: Mutex<lists::State>,
    remind: Mutex<remind::State>,
    voice: Mutex<voice_messages::State>,
    clips: Mutex<video_clips::State>,
    channels: Mutex<channels::State>,
    media: Mutex<media::State>,
    apps: Mutex<appstore::Apps>,
    /// The active call or meeting and the calling signalling.
    pub(crate) calls: call::CallManager,
    /// 1:1 calls: the incoming toast and the outgoing ringing screen.
    #[cfg(feature = "calls")]
    direct_calls: call_direct::Calls,
    /// The Calls page's history, search and voicemail.
    #[cfg(feature = "calls")]
    calls_page: Mutex<calls_page::State>,
    composer_apps: Mutex<composer_apps::State>,
    avatars: Mutex<HashMap<String, Option<Pixels>>>,
    /// The pictures the service sends for people without a photo (shown as initials).
    avatar_placeholders: Mutex<avatar_placeholder::Placeholders>,
    images: Mutex<HashMap<String, Option<Pixels>>>,
    /// Reactor MRIs whose profile fetch was already started (tooltip names).
    name_requests: Mutex<HashSet<String>>,
    pub focused: Arc<AtomicBool>,
    /// Demo mode: no network, so network errors are not shown.
    pub offline: AtomicBool,
    /// Tests: the error the next newest-page read of an opened chat answers with.
    #[cfg(test)]
    history_error: Mutex<Option<teams_proto::Error>>,
    /// The live connection is up; while it is not, the outbox waits.
    connected: AtomicBool,
    /// One outbox flush at a time keeps messages in order.
    flush_lock: tokio::sync::Mutex<()>,
    /// A delayed outbox retry is already scheduled.
    retry_pending: AtomicBool,
    /// Last client message id handed out.
    last_cmid: AtomicI64,
    /// The call commands' lane: they run one at a time, in the order they were sent (`in_order`).
    calls_lane: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<Cmd>>,
    /// One lane per store (Find, People, Calendar, ...): each command's ordered part runs in the
    /// order the store's commands were sent (`in_order::store_lane`).
    store_lanes: Mutex<HashMap<&'static str, tokio::sync::mpsc::UnboundedSender<Cmd>>>,
    /// Teams settings writes, one at a time in the order they were made (tokio's mutex is
    /// first come, first served), and the number of the latest settings read.
    settings_writes: tokio::sync::Mutex<()>,
    settings_reads: std::sync::atomic::AtomicU64,
}

const PAGE: usize = 60;
/// Emoji kept in the panel's recent row.
const RECENT_EMOJI: usize = 16;

/// Today in the chosen time zone (`chat_time` counts days in it).
fn local_day() -> Option<chrono::NaiveDate> {
    appearance::formats::local_now().map(|t| t.date())
}

impl Engine {
    pub fn new(
        client: Client,
        store: Store,
        cfg: session::Config,
        ui: UiHandle,
        me: String,
        my_name: String,
    ) -> Arc<Engine> {
        let avatar_placeholders = avatar_placeholder::Placeholders::load(
            store
                .get_kv(avatar_placeholder::KV_KEY)
                .ok()
                .flatten()
                .as_deref(),
        );
        let engine = Arc::new(Engine {
            calls: call::CallManager::new(ui.clone(), client.clone(), my_name.clone()),
            client,
            store: Mutex::new(store),
            cal: cal::CalState::new(&cfg),
            cfg: Mutex::new(cfg),
            ui,
            me,
            my_name,
            view: Mutex::new(View {
                collapsed_teams: crate::window_state::collapsed_teams(),
                filter: "all".into(),
                limit: PAGE,
                at_bottom: true,
                ..Default::default()
            }),
            dirty: Mutex::new(Dirty {
                chats: true,
                teams: true,
                ..Default::default()
            }),
            mark_stats: Mutex::new(dirty_stats::MarkStats::new(Instant::now())),
            render_wake: tokio::sync::Notify::new(),
            msg_memo: Mutex::default(),
            activity: Mutex::new(activity::State::default()),
            people: Mutex::new(people::State::default()),
            find: Mutex::new(find::State::default()),
            manage: Mutex::new(manage::State::default()),
            drive: Mutex::new(drive::Drive::default()),
            engage: Mutex::new(engage::State::default()),
            lists: Mutex::new(lists::State::default()),
            remind: Mutex::new(remind::State::default()),
            voice: Mutex::default(),
            clips: Mutex::default(),
            channels: Mutex::new(channels::State::default()),
            media: Mutex::new(media::State::default()),
            apps: Mutex::new(appstore::Apps::default()),
            #[cfg(feature = "calls")]
            direct_calls: call_direct::Calls::default(),
            #[cfg(feature = "calls")]
            calls_page: Mutex::default(),
            composer_apps: Mutex::new(composer_apps::State::default()),
            avatars: Mutex::new(HashMap::new()),
            avatar_placeholders: Mutex::new(avatar_placeholders),
            images: Mutex::new(HashMap::new()),
            name_requests: Mutex::new(HashSet::new()),
            focused: Arc::new(AtomicBool::new(true)),
            offline: AtomicBool::new(false),
            #[cfg(test)]
            history_error: Mutex::new(None),
            connected: AtomicBool::new(true),
            flush_lock: tokio::sync::Mutex::new(()),
            retry_pending: AtomicBool::new(false),
            last_cmid: AtomicI64::new(0),
            calls_lane: std::sync::OnceLock::new(),
            store_lanes: Mutex::default(),
            settings_writes: tokio::sync::Mutex::new(()),
            settings_reads: std::sync::atomic::AtomicU64::new(0),
        });
        // The call window's photos come from the chat avatar cache. A miss downloads in the
        // background, which needs the runtime; without one only the cache is read.
        let weak = Arc::downgrade(&engine);
        engine.calls.set_photos(move |mri| {
            let e = weak.upgrade()?;
            if tokio::runtime::Handle::try_current().is_ok() {
                e.avatar(mri)
            } else {
                e.avatars.lock().unwrap().get(mri).cloned().flatten()
            }
        });
        engine
    }

    /// Marks parts of the view for the next render. With debug logging on, the mark is counted
    /// by its caller's line (`dirty_stats`).
    #[track_caller]
    fn mark(&self, f: impl FnOnce(&mut Dirty)) {
        // The closure only ever sets parts, so running it on a blank set tells which ones.
        let mut set = Dirty::default();
        f(&mut set);
        self.dirty.lock().unwrap().merge(&set);
        self.wake_render();
        if tracing::enabled!(tracing::Level::DEBUG) {
            let mut stats = self.mark_stats.lock().unwrap();
            stats.record(std::panic::Location::caller(), set.mask());
            if let Some(summary) = stats.take_due(Instant::now()) {
                tracing::debug!("dirty marks, last minute: {summary}");
            }
        }
    }

    /// Wakes the render loop for a render after its short debounce (a permit is kept when the
    /// loop is busy, so no wake is lost).
    fn wake_render(&self) {
        self.render_wake.notify_one();
    }

    /// The delivery option for the next message: `level` when `on`, else Standard.
    fn set_importance(&self, on: bool, level: Importance) {
        self.view.lock().unwrap().importance = if on { level } else { Importance::Standard };
    }

    /// Starts only the render loop (demo mode: no network).
    pub fn start_offline(self: &Arc<Self>) {
        self.offline.store(true, Ordering::Relaxed);
        self.start_calendar_offline();
        self.push_prefs();
        self.start_people();
        self.start_apps();
        #[cfg(feature = "calls")]
        self.start_calls_demo();
        let e = self.clone();
        tokio::spawn(async move { e.render_loop().await });
    }

    /// Starts background loops: render, live events, periodic token persistence.
    pub fn start(self: &Arc<Self>) {
        self.push_prefs();
        self.start_people();
        self.start_apps();
        self.start_manage();
        self.calls.start();
        #[cfg(feature = "calls")]
        self.start_calls();
        self.start_notify_prefs();
        self.start_my_photo();
        self.start_my_presence();
        self.start_people_presence();
        self.start_reminders();
        let e = self.clone();
        tokio::spawn(async move { e.render_loop().await });
        let e = self.clone();
        tokio::spawn(async move { e.live_loop().await });
        self.start_calendar();
        let e = self.clone();
        tokio::spawn(async move {
            let mut day = local_day();
            loop {
                // Shorter than the 300 s refresh margin, so tokens never lapse while idle.
                tokio::time::sleep(Duration::from_secs(240)).await;
                if e.client.ensure_fresh().await.is_ok() {
                    let cfg = e.cfg.lock().unwrap().clone();
                    let _ = session::persist(&cfg, &e.client).await;
                }
                // The chat list and the Activity feed name the day of a row ("Yesterday", a
                // weekday): they are drawn again when the day turns, not on every tick.
                let today = local_day();
                if today != day {
                    day = today;
                    e.mark(|d| d.chats = true);
                }
            }
        });
        let e = self.clone();
        tokio::spawn(async move {
            if let Err(err) = e.activity_refresh().await {
                tracing::warn!(%err, "activity feed load failed");
            }
        });
        let e = self.clone();
        tokio::spawn(async move {
            // Messages left in the outbox by the last run go out first.
            e.flush_outbox(true).await;
            match e.sync().await {
                Ok(()) => e.ui.synced(),
                Err(err) => {
                    tracing::error!(%err, "initial sync failed");
                    e.ui.banner(format!("Sync failed: {err}"));
                }
            }
        });
    }

    /// Runs a command in the background. Quick local ones run inline, so they stay ordered with the
    /// command that follows them (a draft edit must land before the chat switch that flushes it).
    /// The call commands take their own lane, in order, and each store's commands take the
    /// store's lane (`in_order`).
    pub fn dispatch(self: &Arc<Self>, cmd: Cmd) {
        match cmd {
            Cmd::DraftEdited(text) => self.draft_edited(text),
            Cmd::Flush => {
                self.flush_draft();
                // IronTeams is closing: this device stops counting for presence.
                self.stop_my_presence();
            }
            // In order with the Send that follows it.
            Cmd::SetImportant(on) => self.set_importance(on, Importance::Important),
            Cmd::SetUrgent(on) => self.set_importance(on, Importance::Urgent),
            // The call window, the 1:1 ringing flows and the Calls page share the call state.
            // A chat switch changes the view before it waits: every command after it sees the
            // new chat (`in_order::start_now`).
            cmd @ (Cmd::SelectChat(_) | Cmd::SelectChannel(..)) => {
                in_order::start_now(Box::pin(self.clone().handle(cmd)));
            }
            cmd @ Cmd::Call(_) => self.in_order(cmd),
            #[cfg(feature = "calls")]
            cmd @ (Cmd::DirectCall(_) | Cmd::CallsPage(_)) => self.in_order(cmd),
            cmd => match in_order::store_lane(&cmd) {
                Some(lane) => self.in_store_lane(lane, cmd),
                None => {
                    tokio::spawn(self.clone().handle(cmd));
                }
            },
        }
    }

    pub async fn handle(self: Arc<Self>, cmd: Cmd) {
        if let Err(err) = self.clone().handle_inner(cmd).await {
            self.command_failed(err);
        }
    }

    fn command_failed(&self, err: anyhow::Error) {
        tracing::warn!(%err, "command failed");
        if !self.offline.load(Ordering::Relaxed) {
            self.ui
                .banner(crate::support::error_text::shown_error(&err));
        }
    }

    async fn handle_inner(self: Arc<Self>, cmd: Cmd) -> anyhow::Result<()> {
        match cmd {
            Cmd::RetryLogin => {}
            Cmd::SelectChat(id) => self.open(id, None).await?,
            Cmd::SelectChannel(id, title) => self.open(id, Some(title)).await?,
            Cmd::ToggleTeam(i) => {
                let mut v = self.view.lock().unwrap();
                if let Some(id) = v.teams_order.get(i).cloned()
                    && !v.collapsed_teams.remove(&id)
                {
                    v.collapsed_teams.insert(id);
                }
                crate::window_state::remember_collapsed_teams(&v.collapsed_teams);
                drop(v);
                self.mark(|d| d.teams = true);
            }
            Cmd::ToggleFavorite(id) => {
                let st = self.store.lock().unwrap();
                let fav = st.conversation(&id)?.map(|c| c.favorite).unwrap_or(false);
                st.set_favorite(&id, !fav)?;
                drop(st);
                self.lists_clear_assign(&id);
                self.mark(|d| d.chats = true);
            }
            Cmd::SetFilter(f) => {
                self.view.lock().unwrap().filter = f;
                self.mark(|d| d.chats = true);
            }
            Cmd::Search(q) => {
                self.view.lock().unwrap().search = q;
                self.mark(|d| d.chats = true);
            }
            Cmd::Send(text, subject) => self.send(text, subject).await?,
            Cmd::QuickReply(target, text) => self.quick_reply(&target, &text).await?,
            Cmd::TypingTick => {
                let target = {
                    let mut v = self.view.lock().unwrap();
                    let due = v
                        .last_typing_sent
                        .is_none_or(|t| t.elapsed() > Duration::from_secs(15));
                    let target = v.current.clone().filter(|_| due);
                    if target.is_some() {
                        v.last_typing_sent = Some(Instant::now());
                    }
                    target
                };
                if let Some(c) = target {
                    let _ = self.client.send_typing(&c).await;
                }
            }
            Cmd::LoadOlder => self.load_older().await?,
            Cmd::React(id, key) => self.react(&id, &key).await?,
            Cmd::Reply(id) => {
                let row = self.store.lock().unwrap().message(&id)?;
                if let Some(r) = row {
                    self.ui
                        .reply_bar(r.sender_name.clone(), reply_preview(&r), false);
                    let mut v = self.view.lock().unwrap();
                    v.editing = None;
                    v.reply_to = Some(r);
                    drop(v);
                    self.schedule_draft_save();
                }
                self.ui.focus();
            }
            Cmd::Edit(id) => {
                let row = self.store.lock().unwrap().message(&id)?;
                if let Some(r) = row.filter(|r| r.sender_mri == self.me) {
                    let text = ::render::plain_text(&::render::parse_teams_html(&r.content_html));
                    let text = self.restore_edit_mentions(&r, text);
                    self.ui
                        .reply_bar(String::new(), text.chars().take(120).collect(), true);
                    self.ui.set_draft(text);
                    let mut v = self.view.lock().unwrap();
                    v.reply_to = None;
                    v.editing = Some(r.id);
                }
                self.ui.focus();
            }
            Cmd::CancelReply => {
                let mut v = self.view.lock().unwrap();
                v.reply_to = None;
                if v.editing.take().is_some() {
                    // Back to the draft that was in the composer before the edit.
                    self.ui.set_draft(v.draft_text.clone());
                }
                drop(v);
                self.ui.reply_bar(String::new(), String::new(), false);
                self.schedule_draft_save();
            }
            Cmd::Delete(id) => {
                let conv = self.current_target();
                if let Some(conv) = conv {
                    self.client.delete_message(&conv, &id).await?;
                    self.store.lock().unwrap().delete_message_local(&id)?;
                    self.mark(|d| d.messages = true);
                }
            }
            Cmd::OpenUrl(url) => {
                if let Some(media) = url.strip_prefix("kelma-media:") {
                    self.play_media(media).await?;
                } else if let Some(chip) = url.strip_prefix("kelma-file:") {
                    self.open_file_chip(chip).await;
                } else if url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("mailto:")
                {
                    let _ = crate::external::open(&url);
                }
            }
            Cmd::OpenThread(root) => {
                {
                    let mut v = self.view.lock().unwrap();
                    v.thread_root = Some(root.clone());
                    v.limit = PAGE;
                    v.at_bottom = true;
                }
                self.mark(|d| {
                    d.header = true;
                    d.messages = true;
                    d.scroll = true;
                });
                let conv = self.view.lock().unwrap().current.clone();
                if let Some(c) = conv {
                    let page = self
                        .client
                        .messages(&format!("{c};messageid={root}"), None, 100)
                        .await?;
                    self.store_messages(
                        &page
                            .messages
                            .into_iter()
                            .filter(|m| m.id != root)
                            .collect::<Vec<_>>(),
                        Some(&root),
                    );
                    self.mark(|d| {
                        d.messages = true;
                        d.scroll = true;
                    });
                }
            }
            Cmd::CloseThread => {
                {
                    let mut v = self.view.lock().unwrap();
                    v.thread_root = None;
                    v.at_bottom = true;
                }
                self.mark(|d| {
                    d.header = true;
                    d.messages = true;
                    d.scroll = true;
                });
            }
            Cmd::AttachImage => self.attach_image().await?,
            Cmd::AttachFile => self.attach_file().await?,
            Cmd::CopyLink(id) => self.copy_link(&id)?,
            Cmd::ToggleSave(id) => self.toggle_save(&id).await?,
            Cmd::SetImportant(on) => self.set_importance(on, Importance::Important),
            Cmd::SetUrgent(on) => self.set_importance(on, Importance::Urgent),
            Cmd::Scheduled(c) => self.scheduled_cmd(c).await?,
            Cmd::Pin(id) => self.pin(&id).await?,
            Cmd::MarkUnread(id) => self.mark_unread_from(&id).await?,
            Cmd::ForwardBegin(id) => self.forward_begin(id)?,
            Cmd::ForwardSearch(q) => self.forward_search(q)?,
            Cmd::ForwardToggle(id) => self.forward_toggle(&id)?,
            Cmd::ForwardSend(note) => self.forward_send(&note).await?,
            Cmd::ForwardCancel => self.view.lock().unwrap().forward = None,
            Cmd::SelectBegin(id) => self.select_begin(&id),
            Cmd::ToggleSelect(id) => self.toggle_select(&id),
            Cmd::SelectCancel => self.select_cancel(),
            Cmd::AtBottom(at_bottom) => {
                let mut v = self.view.lock().unwrap();
                v.at_bottom = at_bottom;
                if at_bottom && v.new_count > 0 {
                    v.new_count = 0;
                    drop(v);
                    self.ui.new_count(0);
                }
            }
            Cmd::RetrySend(id) => self.retry_send(&id).await?,
            Cmd::DiscardSend(id) => self.discard_send(&id)?,
            Cmd::MarkChatUnread(id) => self.mark_chat_unread(&id).await?,
            Cmd::MarkAllRead => self.mark_all_read().await?,
            Cmd::Calendar(c) => self.calendar_cmd(c).await?,
            Cmd::Activity(c) => self.activity_cmd(c).await?,
            // Boxed: a search result opens a chat through `handle` again.
            Cmd::Find(c) => Box::pin(self.find_cmd(c)).await?,
            Cmd::People(c) => self.people_cmd(c).await?,
            Cmd::Teams(c) => self.manage_cmd(c).await?,
            // Selection and view changes: no network, so they finish within the lane's first step.
            Cmd::Drive(c) if c.is_instant() => self.drive_instant(c),
            Cmd::Drive(c) => self.drive_cmd(c).await?,
            Cmd::Settings(c) => self.settings_cmd(c).await?,
            Cmd::Engage(c) => self.engage_cmd(c).await?,
            Cmd::Gif(c) => self.gif_cmd(c).await?,
            Cmd::Lists(c) => self.lists_cmd(c).await?,
            Cmd::Channels(c) => self.channel_cmd(c).await?,
            Cmd::Media(c) => self.media_cmd(c).await?,
            Cmd::Apps(c) => self.apps_cmd(c).await?,
            // Boxed: the notification's "Open" opens a chat through `handle` again.
            Cmd::Remind(c) => Box::pin(self.remind_cmd(c)).await?,
            Cmd::Voice(c) => Box::pin(self.voice_cmd(c)).await?,
            Cmd::Clip(c) => Box::pin(self.clip_cmd(c)).await?,
            Cmd::SendLoop(html) => self.send_loop(html).await?,
            Cmd::Call(c) => in_order::finish(self.call_cmd(c)?).await?,
            #[cfg(feature = "calls")]
            Cmd::DirectCall(c) => self.direct_call_cmd(c)?,
            #[cfg(feature = "calls")]
            Cmd::CallsPage(c) => in_order::finish(self.calls_page_cmd(c)?).await?,
            Cmd::ComposerApps(c) => self.composer_apps_cmd(c).await?,
            Cmd::PasteImage => self.paste_image().await?,
            Cmd::AddFile(path) => self.add_file(path).await?,
            Cmd::RemoveAttachment(i) => self.remove_attachment(i),
            Cmd::DraftEdited(text) => self.draft_edited(text),
            Cmd::Flush => self.flush_draft(),
            Cmd::SetEnterSends(on) => {
                self.cfg.lock().unwrap().enter_sends = on;
                self.save_cfg();
            }
            Cmd::EmojiUsed(ch) => {
                {
                    let mut cfg = self.cfg.lock().unwrap();
                    cfg.recent_emoji.retain(|e| e != &ch);
                    cfg.recent_emoji.insert(0, ch);
                    cfg.recent_emoji.truncate(RECENT_EMOJI);
                }
                self.save_cfg();
                self.push_prefs();
            }
            Cmd::SetSkinTone(tone) => {
                self.cfg.lock().unwrap().skin_tone = tone;
                self.save_cfg();
                self.push_prefs();
            }
        }
        Ok(())
    }

    /// Tells the UI the saved composer preferences.
    fn push_prefs(&self) {
        let cfg = self.cfg.lock().unwrap();
        self.ui
            .prefs(cfg.enter_sends, cfg.skin_tone, cfg.recent_emoji.clone());
    }

    #[cfg(test)]
    pub(crate) fn config(&self) -> session::Config {
        self.cfg.lock().unwrap().clone()
    }

    /// Writes the config file. Demo mode has no config of its own, so it never touches the real one.
    fn save_cfg(&self) {
        if self.offline.load(Ordering::Relaxed) {
            return;
        }
        let cfg = self.cfg.lock().unwrap().clone();
        let saved = session::Paths::new().and_then(|p| p.save_app_prefs(&cfg));
        if let Err(err) = saved {
            tracing::warn!(%err, "saving config failed");
        }
    }

    /// Conversation id messages are sent to (channel thread aware).
    fn current_target(&self) -> Option<String> {
        let v = self.view.lock().unwrap();
        let c = v.current.clone()?;
        Some(match &v.thread_root {
            Some(r) => format!("{c};messageid={r}"),
            None => c,
        })
    }

    fn current_target_locked(&self, v: &View) -> Option<String> {
        let c = v.current.clone()?;
        Some(match &v.thread_root {
            Some(r) => format!("{c};messageid={r}"),
            None => c,
        })
    }
}
