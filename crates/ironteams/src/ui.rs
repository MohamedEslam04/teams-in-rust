//! UI-thread side: turns view data into Slint models.

mod cal;
pub use appstore::wire as install_apps;
#[cfg(test)]
pub use cal::read_form as read_form_for_test;
pub use cal::wire as install_calendar;
pub use clip::{ClipView, wire as install_clip};
pub use composer_apps::wire as install_composer_apps;
pub use voice::{PlayerView, RecorderView, wire as install_voice};

use crate::data::{
    AttachmentData, ChatData, HeaderData, MeetingCardData, MsgData, Pixels, RecipientData, Scroll,
    TeamData,
};
use crate::reactions;
use crate::{
    AppWindow, ChannelRow, ChatRow, ComposerEmoji, EmojiItem, FileItem, ImageItem, LinkCard,
    LoopCard, MeetingCard, MeetingPerson, Msg, PendingAttachment, ReactionItem, Reactor,
    ReaderItem, Recipient, TeamRow, VideoClipItem, VoiceItem,
};
use slint::{
    Color, ComponentHandle, Image, Model, ModelRc, SharedString, StyledText, VecModel, Weak,
};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) mod activity;
pub(crate) mod appstore;
pub(crate) mod call;
#[cfg(feature = "calls")]
pub(crate) mod call_direct;
#[cfg(feature = "calls")]
pub(crate) mod calls_page;
mod channels;
mod clip;
mod composer_apps;
pub(crate) mod drive;
mod engage;
mod find;
mod gifs;
mod lists;
mod media;
pub(crate) mod message_toast;
pub(crate) mod motion;
mod palette;
mod people;
pub(crate) mod popout;
mod remind;
pub(crate) mod settings;
mod teams;
pub(crate) mod topbar;
mod voice;

thread_local! {
    static MESSAGES: RefCell<Option<Rc<VecModel<Msg>>>> = const { RefCell::new(None) };
}

fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn img(p: &Option<Pixels>) -> (Image, bool) {
    match p {
        Some(px) => (Image::from_rgba8(px.clone()), true),
        None => (Image::default(), false),
    }
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn chat_row(c: ChatData) -> ChatRow {
    let (avatar, has_avatar) = img(&c.avatar);
    ChatRow {
        id: c.id.into(),
        title: c.title.into(),
        preview: c.preview.into(),
        time: c.time.into(),
        unread: c.unread,
        initials: c.initials.into(),
        color: rgb(c.color),
        avatar,
        has_avatar,
        section: c.section.into(),
        rtl_title: c.rtl_title,
        rtl_preview: c.rtl_preview,
        favorite: c.favorite,
        draft: c.draft,
        mention: c.mention,
        group: c.group,
        muted: c.muted,
        channel: c.channel,
        presence: c.presence.into(),
        person: c.person.into(),
        private: c.private,
        kind: c.kind.into(),
    }
}

fn styled(markdown: &str) -> StyledText {
    StyledText::from_markdown(markdown).unwrap_or_else(|_| StyledText::from_plain_text(markdown))
}

/// Everyone who has seen my newest message: a circle of initials and the full name each. The
/// line under the message draws the first three (`Msg.readers`); the "Seen by" list shows them
/// all (`Msg.reader_list`).
fn readers(names: &[String]) -> Vec<ReaderItem> {
    names
        .iter()
        .filter(|n| !n.trim().is_empty())
        .map(|n| ReaderItem {
            name: n.as_str().into(),
            initials: crate::data::initials(n).into(),
            color: rgb(crate::data::color_for(n)),
        })
        .collect()
}

fn msg(m: MsgData) -> Msg {
    let (avatar, has_avatar) = img(&m.avatar);
    let readers = readers(&m.receipt_readers);
    Msg {
        id: m.id.into(),
        sender: m.sender.into(),
        initials: m.initials.into(),
        color: rgb(m.color),
        avatar,
        has_avatar,
        mine: m.mine,
        time: m.time.into(),
        day: m.day.into(),
        show_header: m.show_header,
        edited: m.edited,
        deleted: m.deleted,
        system: m.system,
        subject: m.subject.into(),
        quote_author: m.quote_author.into(),
        quote_text: m.quote_text.into(),
        body: styled(&m.markdown),
        plain: m.plain.into(),
        rtl: m.rtl,
        images: model(
            m.images
                .into_iter()
                .map(|i| {
                    let (image, loaded) = img(&i.pixels);
                    ImageItem {
                        url: i.url.into(),
                        img: image,
                        loaded,
                        x: i.x,
                        y: i.y,
                        w: i.w,
                        h: i.h,
                        overflow: i.overflow as i32,
                    }
                })
                .collect(),
        ),
        files: model(
            m.files
                .into_iter()
                .map(|f| FileItem {
                    name: f.name.into(),
                    url: f.url.into(),
                    ext: f.ext.into(),
                    place: f.place.into(),
                    detail: f.detail.into(),
                })
                .collect(),
        ),
        reactions: model(
            m.reactions
                .into_iter()
                .map(|r| ReactionItem {
                    key: r.key.into(),
                    emoji: r.emoji.into(),
                    name: r.name.into(),
                    count: r.count,
                    mine: r.mine,
                    names: r.names.into(),
                    people: model(
                        r.people
                            .into_iter()
                            .map(|p| {
                                let (img, has_img) = img(&p.avatar);
                                Reactor {
                                    name: p.name.into(),
                                    initials: p.initials.into(),
                                    color: rgb(p.color),
                                    img,
                                    has_img,
                                }
                            })
                            .collect(),
                    ),
                })
                .collect(),
        ),
        reply_count: m.reply_count,
        pending: m.pending,
        saved: m.saved,
        selected: m.selected,
        failed: m.failed,
        new_above: m.new_above,
        sender_id: m.sender_id.into(),
        presence: m.presence.into(),
        mentions_me: m.mentions_me,
        receipt: m.receipt.into(),
        receipt_names: m.receipt_names.into(),
        flash: m.flash,
        fresh: m.fresh,
        readers: model(readers.iter().take(3).cloned().collect()),
        reader_list: model(readers),
        card: channels::card_rows(m.card),
        headline: m.headline.into(),
        importance: m.importance.into(),
        grid_w: m.grid_w,
        grid_h: m.grid_h,
        translate: m.translate.into(),
        translate_from: m.translate_from.into(),
        links: model(
            m.links
                .into_iter()
                .map(|l| {
                    let (thumb, has_thumb) = img(&l.thumb);
                    LinkCard {
                        url: l.url.into(),
                        title: l.title.into(),
                        description: l.description.into(),
                        domain: l.domain.into(),
                        thumb,
                        has_thumb,
                    }
                })
                .collect(),
        ),
        pinned: m.pinned,
        urls: model(m.urls.into_iter().map(SharedString::from).collect()),
        loops: model(
            m.loops
                .into_iter()
                .map(|l| LoopCard {
                    url: l.url.into(),
                    title: l.title.into(),
                })
                .collect(),
        ),
        joins: model(m.joins.into_iter().map(SharedString::from).collect()),
        voices: model(
            m.voices
                .into_iter()
                .map(|v| VoiceItem {
                    key: v.key.into(),
                    url: v.url.into(),
                    duration_ms: v.duration_ms,
                    peaks: model(v.peaks),
                    unplayed: v.unplayed,
                    failed: v.failed,
                })
                .collect(),
        ),
        videos: model(
            m.videos
                .into_iter()
                .map(|c| {
                    let (thumb, has_thumb) = img(&c.thumb);
                    VideoClipItem {
                        key: c.key.into(),
                        url: c.url.into(),
                        duration_ms: c.duration_ms,
                        thumb,
                        has_thumb,
                    }
                })
                .collect(),
        ),
        meeting: model(m.meeting.into_iter().map(meeting_card).collect()),
    }
}

fn meeting_card(c: MeetingCardData) -> MeetingCard {
    MeetingCard {
        kind: c.kind.into(),
        title: c.title.into(),
        detail: c.detail.into(),
        people: model(
            c.people
                .into_iter()
                .map(|p| MeetingPerson {
                    name: p.name.into(),
                    initials: p.initials.into(),
                    color: rgb(p.color),
                })
                .collect(),
        ),
        more: c.more,
        state: c.state.into(),
        url: c.url.into(),
        recap_chat: c.recap_chat.into(),
        join_chat: c.join_chat.into(),
    }
}

fn recipient(r: RecipientData) -> Recipient {
    Recipient {
        id: r.id.into(),
        title: r.title.into(),
        initials: r.initials.into(),
        color: rgb(r.color),
        picked: r.picked,
    }
}

fn picker_model(query: &str) -> ModelRc<EmojiItem> {
    model(
        reactions::picker_items(query)
            .into_iter()
            .map(|i| EmojiItem {
                key: i.key.into(),
                emoji: i.emoji.into(),
                name: i.name.into(),
            })
            .collect(),
    )
}

/// Wires what the UI thread answers by itself, without the engine: the "More reactions" search.
pub fn install_local(window: &AppWindow) {
    crate::loop_compose::install_local(window);
    palette::wire(window);
    message_toast::install_local(window);
    window.set_picker_items(picker_model(""));
    let weak = window.as_weak();
    window.on_picker_search(move |q| {
        if let Some(w) = weak.upgrade() {
            w.set_picker_items(picker_model(&q));
        }
    });
}

/// Thread-safe handle the backend uses to update the window.
#[derive(Clone)]
pub struct UiHandle {
    weak: Weak<AppWindow>,
}

impl UiHandle {
    pub fn new(weak: Weak<AppWindow>) -> Self {
        UiHandle { weak }
    }

    fn run(&self, f: impl FnOnce(AppWindow) + Send + 'static) {
        // Tests drive the window on their own thread without an event loop.
        #[cfg(test)]
        if let Some(w) = self.weak.upgrade() {
            return f(w);
        }
        let _ = self.weak.upgrade_in_event_loop(f);
    }

    pub fn login_checking(&self) {
        self.run(|w| w.set_login_state("checking".into()));
    }

    pub fn login_code(&self, code: String, url: String) {
        self.run(move |w| {
            w.set_user_code(code.into());
            w.set_verify_url(url.into());
            w.set_login_state("code".into());
        });
    }

    pub fn login_error(&self, err: String) {
        self.run(move |w| {
            w.set_login_error(err.into());
            w.set_login_state("error".into());
        });
    }

    pub fn ready(&self, my_initials: String) {
        self.run(move |w| {
            w.set_me_initials(my_initials.into());
            w.set_login_state("ready".into());
        });
    }

    /// Shows a transient message for 8 seconds.
    pub fn banner(&self, text: String) {
        self.transient(text, false);
    }

    /// Shows a transient notice (not an error) for 8 seconds.
    pub fn info(&self, text: String) {
        self.transient(text, true);
    }

    fn transient(&self, text: String, info: bool) {
        let text = crate::backend::util::plain_banner(&text);
        self.run(move |w| {
            let shown: SharedString = text.into();
            w.set_banner_action(SharedString::new());
            w.set_banner_info(info);
            w.set_banner(shown.clone());
            let weak = w.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_secs(8), move || {
                if let Some(w) = weak.upgrade()
                    && w.get_banner() == shown
                {
                    w.set_banner(SharedString::new());
                }
            });
        });
    }

    pub fn connected(&self, ok: bool) {
        self.run(move |w| w.set_connected(ok));
    }

    /// The first sync after start-up has finished ("While you were away" can show).
    pub fn synced(&self) {
        self.run(|w| w.set_synced(true));
    }

    /// Who is typing in the open chat, and in which chats (dots on their rows).
    pub fn typing(&self, names: Vec<String>, chats: Vec<String>) {
        self.run(move |w| motion::set_typing(&w, &names, &chats));
    }

    pub fn loading_older(&self, on: bool) {
        self.run(move |w| w.set_loading_older(on));
    }

    /// The open chat or channel is reading its newest page.
    pub fn loading_latest(&self, on: bool) {
        self.run(move |w| w.set_loading_latest(on));
    }

    /// The open chat is one the user is not in: `text` replaces its messages and composer (empty
    /// again when the chat is opened anew).
    pub fn unavailable(&self, id: String, text: String) {
        self.run(move |w| {
            w.set_unavailable_id(id.into());
            w.set_unavailable_text(text.into());
        });
    }

    pub fn focus(&self) {
        self.run(|w| w.set_focus_tick(w.get_focus_tick() + 1));
    }

    /// The chips above the composer.
    pub fn attachments(&self, chips: Vec<AttachmentData>) {
        self.run(move |w| {
            w.set_pending_attachments(model(
                chips
                    .into_iter()
                    .map(|c| {
                        let (preview, has_preview) = img(&c.thumb);
                        PendingAttachment {
                            name: c.name.into(),
                            detail: c.detail.into(),
                            is_image: c.is_image,
                            preview,
                            has_preview,
                        }
                    })
                    .collect(),
            ));
        });
    }

    /// Replaces the composer text, with the cursor at its end.
    pub fn set_draft(&self, text: String) {
        self.run(move |w| {
            w.set_draft(text.into());
            w.set_cursor_tick(w.get_cursor_tick() + 1);
        });
    }

    /// The open chat's scheduled messages ("Sending on …" bubbles).
    pub fn scheduled(&self, items: Vec<crate::backend::ScheduledData>) {
        self.run(move |w| {
            w.global::<crate::SendStore>().set_scheduled(model(
                items
                    .into_iter()
                    .map(|s| crate::ScheduledItem {
                        id: s.id.into(),
                        label: s.label.into(),
                        text: s.text.into(),
                    })
                    .collect(),
            ));
        });
    }

    /// The composer's "Schedule message" choice: the banner above the box shows it and Send
    /// schedules. `None` clears it.
    pub fn schedule_choice(&self, at_ms: Option<i64>) {
        self.run(move |w| {
            let s = w.global::<crate::SendStore>();
            s.set_schedule_at(at_ms.map(|m| m.to_string()).unwrap_or_default().into());
            s.set_schedule_label(
                at_ms
                    .map(crate::send_options::when_label)
                    .unwrap_or_default()
                    .into(),
            );
        });
    }

    /// Saved composer preferences: Enter behavior, emoji skin tone and the recent emoji row.
    pub fn prefs(&self, enter_sends: bool, skin_tone: u8, recent: Vec<String>) {
        self.run(move |w| {
            w.set_enter_sends(enter_sends);
            w.set_skin_tone(skin_tone.into());
            w.set_recent_emoji(model(
                recent
                    .into_iter()
                    .map(|ch| ComposerEmoji {
                        ch: ch.into(),
                        name: SharedString::new(),
                    })
                    .collect(),
            ));
        });
    }

    pub fn clear_subject(&self) {
        self.run(|w| w.set_post_subject(SharedString::new()));
    }

    pub fn reply_bar(&self, author: String, preview: String, editing: bool) {
        self.run(move |w| {
            w.set_reply_author(author.into());
            w.set_reply_preview(preview.into());
            if !editing {
                w.set_editing_id(SharedString::new());
            } else {
                w.set_editing_id("1".into());
            }
        });
    }

    /// Search results and picked recipients of the forward dialog.
    pub fn forward_results(&self, results: Vec<RecipientData>, picked: Vec<RecipientData>) {
        self.run(move |w| {
            w.set_forward_results(model(results.into_iter().map(recipient).collect()));
            w.set_forward_picked(model(picked.into_iter().map(recipient).collect()));
        });
    }

    /// The message was forwarded: close the dialog and leave multi-select mode.
    pub fn forward_done(&self) {
        self.run(|w| {
            w.set_forward_open(false);
            w.set_select_mode(false);
            w.set_select_count(0);
        });
    }

    /// Multi-select mode and how many messages are ticked.
    pub fn selection(&self, mode: bool, count: i32) {
        self.run(move |w| {
            w.set_select_mode(mode);
            w.set_select_count(count);
        });
    }

    /// The fixed sections' rows. `main` ends with the rows of the Meetings section (`section`
    /// "meetings"), which the list draws under its own header.
    pub fn chats(&self, fav: Vec<ChatData>, main: Vec<ChatData>, chans: Vec<ChatData>) {
        self.run(move |w| {
            let meetings: Vec<_> = main.iter().filter(|c| c.section == "meetings").collect();
            let list = w.global::<crate::ListStore>();
            list.set_meetings_count(meetings.len() as i32);
            list.set_meetings_unread(meetings.iter().filter(|c| c.unread).count() as i32);
            w.set_chats_favorites(model(fav.into_iter().map(chat_row).collect()));
            w.set_chats_main(model(main.into_iter().map(chat_row).collect()));
            w.set_chats_channels(model(chans.into_iter().map(chat_row).collect()));
        });
    }

    pub fn teams(&self, teams: Vec<TeamData>) {
        self.run(move |w| {
            w.set_teams(model(
                teams
                    .into_iter()
                    .map(|t| {
                        let (avatar, has_avatar) = img(&t.avatar);
                        TeamRow {
                            id: t.id.into(),
                            name: t.name.into(),
                            initials: t.initials.into(),
                            color: rgb(t.color),
                            avatar,
                            has_avatar,
                            expanded: t.expanded,
                            channels: model(
                                t.channels
                                    .into_iter()
                                    .map(|c| ChannelRow {
                                        id: c.id.into(),
                                        name: c.name.into(),
                                        unread: c.unread,
                                        hidden: c.hidden,
                                        private: c.private,
                                    })
                                    .collect(),
                            ),
                        }
                    })
                    .collect(),
            ));
        });
    }

    pub fn header(&self, h: HeaderData) {
        self.run(move |w| {
            let (avatar, has) = img(&h.avatar);
            // The Teams page opens the last channel again; the Chat page the last conversation
            // that was opened outside the Teams page.
            if h.is_channel && h.thread_root.is_empty() {
                w.set_last_channel_id(h.id.as_str().into());
                w.set_last_channel_title(h.title.as_str().into());
            }
            if !h.id.is_empty() && h.thread_root.is_empty() && w.get_view() != "teams" {
                w.set_last_chat_id(h.id.as_str().into());
            }
            // Another conversation never shows the messages (or posts) of the previous one
            // under its header while its own are still being drawn.
            if w.get_current_id() != h.id.as_str() {
                MESSAGES.with(|cell| {
                    if let Some(m) = cell.borrow().as_ref() {
                        m.set_vec(vec![]);
                    }
                });
                let posts = w.global::<crate::ChannelStore>();
                posts.set_posts(ModelRc::default());
                // The thread panel belongs to the channel it was opened in.
                posts.set_thread_id("".into());
                w.set_current_meeting_live(false);
            }
            w.set_current_id(h.id.into());
            w.set_current_title(h.title.into());
            w.set_current_initials(h.initials.into());
            w.set_current_color(rgb(h.color));
            w.set_current_avatar(avatar);
            w.set_current_has_avatar(has);
            w.set_current_is_channel(h.is_channel);
            w.set_current_kind(h.kind.into());
            w.set_current_muted(h.muted);
            w.set_current_person(h.person.into());
            w.set_current_presence(h.presence.into());
            w.set_thread_root(h.thread_root.into());
        });
    }

    /// The open chat's meeting is going on (the header's Join is filled).
    pub fn meeting_live(&self, live: bool) {
        self.run(move |w| w.set_current_meeting_live(live));
    }

    /// Replaces the message list, updating rows in place when the ids are unchanged so the
    /// scroll position is kept.
    pub fn messages(&self, msgs: Vec<MsgData>, scroll: Scroll) {
        self.run(move |w| {
            MESSAGES.with(|cell| {
                let mut cell = cell.borrow_mut();
                let m = cell.get_or_insert_with(|| {
                    let m = Rc::new(VecModel::default());
                    w.set_messages(ModelRc::from(m.clone()));
                    m
                });
                // The rows both lists share (the list window slid by a new message, an older
                // page came in above, a pending message became the sent one) are updated in
                // place and only the rows around them are removed or added: a rebuilt row
                // forgets the pointer resting on it until the pointer moves again, so its photo
                // would not open the hover card.
                let old = m.row_count();
                let id = |i: usize| m.row_data(i).map(|r| r.id).unwrap_or_default();
                // Where the shared run starts in the old list (a) and in the new one (b).
                let (a, b) = match msgs
                    .first()
                    .and_then(|f| (0..old).find(|&i| id(i) == f.id.as_str()))
                {
                    Some(a) => (a, 0),
                    None => (
                        0,
                        (0..msgs.len())
                            .find(|&j| old > 0 && id(0) == msgs[j].id.as_str())
                            .unwrap_or(msgs.len()),
                    ),
                };
                // My newest message, still pending, that the server's copy replaced is the same
                // row: it is updated in place, so its clock turns into the check with a fade.
                let settled = |k: usize| {
                    k + 1 == old - a
                        && b + k + 1 == msgs.len()
                        && m.row_data(a + k).is_some_and(|o| o.pending && o.mine)
                        && msgs[b + k].mine
                        && !msgs[b + k].pending
                };
                let run = (0..(old - a).min(msgs.len().saturating_sub(b)))
                    .take_while(|&k| id(a + k) == msgs[b + k].id.as_str() || settled(k))
                    .count();
                if run == 0 {
                    m.set_vec(msgs.into_iter().map(msg).collect::<Vec<_>>());
                } else {
                    // Keep only the run, then add the new rows before and after it.
                    for _ in 0..a {
                        m.remove(0);
                    }
                    for _ in 0..old - a - run {
                        m.remove(run);
                    }
                    for (j, x) in msgs.into_iter().enumerate() {
                        if j < b {
                            m.insert(j, msg(x));
                        } else if j < b + run {
                            m.set_row_data(j, msg(x));
                        } else {
                            m.push(msg(x));
                        }
                    }
                }
            });
            match scroll {
                Scroll::Keep => {}
                // The list corrects its own estimate from the divider row's real position.
                Scroll::Divider(i) => {
                    w.set_divider_index(i as i32);
                    w.set_divider_tick(w.get_divider_tick() + 1);
                }
                Scroll::Anchor(i) => {
                    w.set_anchor_index(i as i32);
                    w.set_anchor_tick(w.get_anchor_tick() + 1);
                }
                Scroll::Bottom => {
                    // Two ticks: one now, one after layout settles with images.
                    w.set_scroll_tick(w.get_scroll_tick() + 1);
                    let weak = w.as_weak();
                    slint::Timer::single_shot(std::time::Duration::from_millis(150), move || {
                        if let Some(w) = weak.upgrade() {
                            w.set_scroll_tick(w.get_scroll_tick() + 1);
                        }
                    });
                }
            }
        });
    }

    /// Brings the window to the front (a notification was clicked); `compose` also puts the
    /// cursor in the message box.
    pub fn raise(&self, compose: bool) {
        self.run(move |w| {
            crate::tray_wiring::show_and_raise(&w);
            if compose {
                w.set_focus_tick(w.get_focus_tick() + 1);
            }
        });
    }

    /// Count shown on the "Jump to newest" button.
    pub fn new_count(&self, n: usize) {
        self.run(move |w| w.set_new_count(n as i32));
    }
}

#[cfg(test)]
mod tests {
    use super::readers;

    #[test]
    fn every_reader_reaches_the_model_with_a_full_name() {
        let names: Vec<String> = [
            "Reader One",
            "Reader Two",
            "Reader Three",
            "Reader Four",
            "Reader Five",
        ]
        .iter()
        .map(|n| n.to_string())
        .collect();
        let r = readers(&names);
        let got: Vec<String> = r.iter().map(|x| x.name.to_string()).collect();
        assert_eq!(got, names, "every one of the five is there");
        let initials: Vec<String> = r.iter().map(|x| x.initials.to_string()).collect();
        assert_eq!(initials, ["RO", "RT", "RT", "RF", "RF"]);
        assert!(readers(&[]).is_empty(), "a 1:1 chat names nobody");
    }
}
