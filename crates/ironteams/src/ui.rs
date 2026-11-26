//! UI-thread side: turns view data into Slint models.

mod cal;
pub use appstore::wire as install_apps;
pub use cal::wire as install_calendar;
pub use clip::wire as install_clip;
pub use composer_apps::wire as install_composer_apps;
pub use voice::wire as install_voice;

use crate::data::{
    AttachmentData, ChatData, HeaderData, InlineImageData, MeetingCardData, MsgData, Pixels,
    RecipientData, Scroll, TeamData,
};
use crate::markdown::{PostKind, Segment};
use crate::reactions;
use crate::support::inline_images;
use crate::ui_api::UiHandle;
use crate::{
    AppWindow, BodyPart, ChannelRow, ChatRow, ComposerEmoji, EmojiItem, FileItem, ImageItem,
    InlineImage, InlineImageStore, LinkCard, LoopCard, MeetingCard, MeetingPerson, Msg,
    PendingAttachment, PostBlock, QuoteItem, ReactionItem, Reactor, ReaderItem, Recipient, TeamRow,
    VideoClipItem, VoiceItem,
};
use slint::{Color, ComponentHandle, Image, Model, ModelRc, SharedString, StyledText, VecModel};
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
mod chat_note;
mod clip;
mod composer_apps;
pub(crate) mod drive;
mod engage;
mod find;
mod gifs;
mod lists;
mod media;
pub(crate) mod message_toast;
mod model_sync;
pub(crate) mod motion;
mod ongoing_call;
mod palette;
mod people;
pub(crate) mod popout;
mod remind;
pub(crate) mod row_fingerprint;
pub(crate) mod settings;
mod split;
mod teams;
pub(crate) mod topbar;
mod voice;

thread_local! {
    static MESSAGES: RefCell<Option<Rc<VecModel<Msg>>>> = const { RefCell::new(None) };
    /// The fingerprint of each row of `MESSAGES` (see `row_fingerprint`), in the same order.
    static MESSAGE_PRINTS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// The chat list's sections (favorites, chats, channels) and the print of each row.
    static CHAT_LISTS: RefCell<Vec<model_sync::Synced<ChatRow>>> =
        const { RefCell::new(Vec::new()) };
}

/// Drops what the previous conversation left on screen: its messages, posts, thread panel and
/// live meeting.
fn forget_previous_chat(w: &AppWindow) {
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

/// The chat the user chose in the search box, shown before the engine has switched to it: its
/// header from the suggestion, a loading state in place of the messages, and no composer (and an
/// empty draft) until the engine's header for it comes, so nothing typed meanwhile reaches the
/// previous chat. `kind` is the header's ("chat" for a person, "group" for a group chat).
pub(crate) fn show_opening(
    w: &AppWindow,
    id: &str,
    title: &str,
    initials: &str,
    color: u32,
    kind: &str,
    person: &str,
) {
    forget_previous_chat(w);
    w.set_opening_id(id.into());
    w.set_current_id(id.into());
    w.set_current_title(title.into());
    w.set_current_initials(initials.into());
    w.set_current_color(rgb(color));
    w.set_current_avatar(Image::default());
    w.set_current_has_avatar(false);
    w.set_current_is_channel(false);
    w.set_current_kind(kind.into());
    w.set_current_muted(false);
    w.set_current_person(person.into());
    w.set_current_presence("".into());
    w.set_thread_root("".into());
    w.set_draft("".into());
    w.set_loading_latest(true);
}

fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A theme token colour, `0xRRGGBBAA`.
fn rgba(c: u32) -> Color {
    Color::from_argb_u8(c as u8, (c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8)
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

/// Makes section `i` of the chat list (favorites, chats, channels) show `rows`, patching the model
/// it shows (`shown`, put there with `set` when it is not ours).
fn patch_chats(
    i: usize,
    shown: ModelRc<ChatRow>,
    set: impl FnOnce(ModelRc<ChatRow>),
    rows: Vec<ChatData>,
) {
    let rows = rows
        .into_iter()
        .map(|c| ((c.id.clone(), row_fingerprint::of_chat(&c)), c))
        .collect();
    CHAT_LISTS.with(|cell| {
        let mut lists = cell.borrow_mut();
        while lists.len() <= i {
            lists.push((Rc::new(VecModel::default()), vec![]));
        }
        let (m, prints) = &mut lists[i];
        if !model_sync::is_shown(&shown, m) {
            m.set_vec(vec![]);
            prints.clear();
            set(ModelRc::from(m.clone()));
        }
        model_sync::patch(m, prints, rows, chat_row);
    });
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
        muted_until: c.muted_until.into(),
        channel: c.channel,
        presence: c.presence.into(),
        person: c.person.into(),
        private: c.private,
        kind: c.kind.into(),
        notify: c.notify.into(),
    }
}

/// The row to keep in place when older messages land above it: the row that held the top of the
/// view (`top`, tracked by the list as the user scrolls) and `true` to keep the user's offset in
/// it, else the row that was first (`fallback`, `false`).
fn anchor_row(top: &str, valid: bool, ids: &[String], fallback: usize) -> (usize, bool) {
    if valid
        && !top.is_empty()
        && let Some(i) = ids.iter().position(|id| id == top)
    {
        return (i, true);
    }
    (fallback, false)
}

fn styled(markdown: &str) -> StyledText {
    StyledText::from_markdown(markdown).unwrap_or_else(|_| StyledText::from_plain_text(markdown))
}

/// A run of text or a code block of a message that has code blocks (`Msg.parts`).
fn body_part(s: &Segment) -> BodyPart {
    match s {
        Segment::Text { md, rtl } => BodyPart {
            body: styled(md),
            code: SharedString::default(),
            is_code: false,
            rtl: *rtl,
        },
        Segment::Code(code) => BodyPart {
            body: StyledText::default(),
            code: code.as_str().into(),
            is_code: true,
            rtl: false,
        },
    }
}

/// The blocks of a channel post's body (`Msg.blocks`).
fn post_blocks(blocks: &[crate::markdown::PostBlock]) -> ModelRc<PostBlock> {
    model(
        blocks
            .iter()
            .map(|b| {
                let (kind, code, marker, depth, level) = match &b.kind {
                    PostKind::Text => ("text", "", "", 0, 0),
                    PostKind::Heading(level) => ("heading", "", "", 0, *level as i32),
                    PostKind::Item { marker, depth } => {
                        ("item", "", marker.as_str(), *depth as i32, 0)
                    }
                    PostKind::Code(code) => ("code", code.as_str(), "", 0, 0),
                };
                PostBlock {
                    kind: kind.into(),
                    body: if b.md.is_empty() {
                        StyledText::default()
                    } else {
                        styled(&b.md)
                    },
                    code: code.into(),
                    marker: marker.into(),
                    depth,
                    level,
                    rtl: b.rtl,
                }
            })
            .collect(),
    )
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
    let subject_rtl = render::text_direction(&m.subject) == Some(render::Dir::Rtl);
    // A message that is only a theme share line gets a chip with a Preview button.
    let (theme_name, theme_nav, theme_content, theme_accent, theme_line) =
        match crate::markdown::theme_chip(&m.plain) {
            Some(t) => {
                let chip_colour = |c: appearance::Rgb| Color::from_rgb_u8(c.r, c.g, c.b);
                (
                    SharedString::from(t.name),
                    chip_colour(t.nav),
                    chip_colour(t.content),
                    chip_colour(t.accent),
                    SharedString::from(t.line),
                )
            }
            None => Default::default(),
        };
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
        subject_rtl,
        quotes: model(
            m.quotes
                .into_iter()
                .map(|q| QuoteItem {
                    author: q.author.into(),
                    text: q.text.into(),
                    id: q.id.into(),
                    time: q.time.into(),
                    rtl: q.rtl,
                })
                .collect(),
        ),
        body: styled(&m.markdown),
        parts: model(m.segments.iter().map(body_part).collect()),
        // Filled for channel posts only (`channels::post`): a chat bubble draws `body` or `parts`.
        blocks: ModelRc::default(),
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
        receipt_compact: m.receipt_compact,
        receipt_side: m.receipt_side,
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
        theme_name,
        theme_nav,
        theme_content,
        theme_accent,
        theme_line,
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

/// The "More reactions" rows for `query`, in the saved skin tone `tone`.
fn picker_model(query: &str, tone: i32) -> ModelRc<EmojiItem> {
    emoji_model(reactions::picker_items(
        query,
        u8::try_from(tone).unwrap_or(0),
    ))
}

/// Reaction cells as the UI's `EmojiItem` rows.
fn emoji_model(items: Vec<reactions::PickerItem>) -> ModelRc<EmojiItem> {
    model(
        items
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
    crate::support::watchdog::start_ui();
    crate::loop_compose::install_local(window);
    palette::wire(window);
    find::wire(window);
    message_toast::install_local(window);
    window.set_picker_items(picker_model("", window.get_skin_tone()));
    let weak = window.as_weak();
    window.on_picker_search(move |q| {
        if let Some(w) = weak.upgrade() {
            w.set_picker_items(picker_model(&q, w.get_skin_tone()));
        }
    });
}

impl UiHandle {
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

    /// The sign-in error screen. `report` is what its "Copy error details" button copies; empty
    /// hides the button.
    pub fn login_error(&self, err: String, report: String) {
        self.run(move |w| {
            w.set_login_error(err.into());
            w.set_login_report(report.into());
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
        let text = crate::support::error_text::plain_banner(&text);
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

    /// How many messages a jump (to the first message, to a date) has read so far: the loading
    /// line counts them (0 shows the plain line).
    pub fn loading_progress(&self, read: usize) {
        self.run(move |w| w.set_loading_progress(read.min(i32::MAX as usize) as i32));
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

    /// The images placed between the composer's lines, by slot.
    pub fn inline_images(&self, images: Vec<InlineImageData>) {
        self.run(move |w| {
            w.global::<InlineImageStore>().set_images(model(
                images
                    .into_iter()
                    .map(|i| InlineImage {
                        preview: img(&i.thumb).0,
                        live: i.live,
                    })
                    .collect(),
            ));
        });
    }

    /// Puts the line of the image in `slot` into the composer at its cursor, and the cursor after
    /// it (docs/designs/composer-inline-images.md). The engine hears of the new text as of any
    /// edit, once this update is done (it may be running inside the engine's own command).
    pub fn insert_inline_image(&self, slot: u32) {
        self.run(move |w| {
            let store = w.global::<InlineImageStore>();
            let caret = usize::try_from(store.get_caret()).unwrap_or(0);
            let (text, caret) = inline_images::insert(&w.get_draft(), caret, slot);
            let caret = i32::try_from(caret).unwrap_or(i32::MAX);
            w.set_draft(text.into());
            // Read by the next image at once, before the composer reports its cursor.
            store.set_caret(caret);
            store.set_place(caret);
            store.set_place_tick(store.get_place_tick() + 1);
            let weak = w.as_weak();
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                if let Some(w) = weak.upgrade() {
                    w.invoke_draft_edited(w.get_draft());
                }
            });
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
    pub fn scheduled(&self, items: Vec<crate::ui_api::ScheduledData>) {
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
            let recent: Vec<ComposerEmoji> = recent
                .into_iter()
                .map(|ch| ComposerEmoji {
                    ch: ch.into(),
                    name: SharedString::new(),
                })
                .collect();
            w.set_enter_sends(enter_sends);
            w.set_skin_tone(skin_tone.into());
            w.set_recent_emoji(model(recent.clone()));
            // The editor of a sent message is a composer too, and follows the same settings.
            let edit = w.global::<crate::EditStore>();
            edit.set_enter_sends(enter_sends);
            edit.set_skin_tone(skin_tone.into());
            edit.set_recent_emoji(model(recent));
        });
    }

    /// The editor of a sent message opens in its row (`id`) with `text`, or closes (`id` empty).
    pub fn inline_edit(&self, id: String, text: String) {
        self.run(move |w| {
            let edit = w.global::<crate::EditStore>();
            edit.set_draft(text.into());
            if id.is_empty() {
                edit.set_focused(false);
            }
            edit.set_id(id.into());
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
            // Patched, not replaced (`model_sync`): every presence change or avatar built every
            // row again, which dropped the hover under the pointer and flickered.
            patch_chats(
                0,
                w.get_chats_favorites(),
                |m| w.set_chats_favorites(m),
                fav,
            );
            patch_chats(1, w.get_chats_main(), |m| w.set_chats_main(m), main);
            patch_chats(
                2,
                w.get_chats_channels(),
                |m| w.set_chats_channels(m),
                chans,
            );
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
            let opening = w.get_opening_id();
            // The chat chosen in the search box is on screen already: a header for any other
            // chat is one the engine drew before it saw the switch.
            if !opening.is_empty() && opening != h.id.as_str() {
                return;
            }
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
            if w.get_current_id() != h.id.as_str() || !opening.is_empty() {
                forget_previous_chat(&w);
            }
            // The engine has switched: the composer comes back.
            w.set_opening_id("".into());
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
            // Rows drawn for the previous chat before the engine saw the switch chosen in the
            // search box do not belong under the chat shown now; its own come after its header.
            if !w.get_opening_id().is_empty() {
                return;
            }
            let started = std::time::Instant::now();
            // Where the user is in the list now, before the rows change.
            let top_id = w.get_msg_top_id().to_string();
            let top_off = w.get_msg_top_off();
            // The row was recorded where the view is now (not before a jump the rows missed).
            let top_valid = (w.get_msg_top_cy() - w.get_msg_view_y()).abs() < 1.0;
            let ids: Vec<String> = if matches!(scroll, Scroll::Anchor(_)) {
                msgs.iter().map(|m| m.id.clone()).collect()
            } else {
                Vec::new()
            };
            let (rows, written) = MESSAGES.with(|cell| {
                let mut cell = cell.borrow_mut();
                let mut written = 0usize;
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
                let new_prints: Vec<u64> = msgs.iter().map(row_fingerprint::of).collect();
                // The model was emptied elsewhere (another conversation opened): nothing is known.
                let mut prints = MESSAGE_PRINTS.with(|p| std::mem::take(&mut *p.borrow_mut()));
                if prints.len() != old {
                    prints = vec![u64::MAX; old];
                }
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
                    written = new_prints.len();
                    m.set_vec(msgs.into_iter().map(msg).collect::<Vec<_>>());
                    prints = new_prints;
                } else {
                    // Keep only the run, then add the new rows before and after it.
                    for _ in 0..a {
                        m.remove(0);
                        prints.remove(0);
                    }
                    for _ in 0..old - a - run {
                        m.remove(run);
                        prints.remove(run);
                    }
                    for (j, x) in msgs.into_iter().enumerate() {
                        if j < b {
                            m.insert(j, msg(x));
                            written += 1;
                            prints.insert(j, new_prints[j]);
                        } else if j < b + run {
                            // A row that did not change is not written: writing it notifies
                            // every binding of the row and parses its text again.
                            if prints[j] != new_prints[j] {
                                m.set_row_data(j, msg(x));
                                written += 1;
                                prints[j] = new_prints[j];
                            }
                        } else {
                            m.push(msg(x));
                            written += 1;
                            prints.push(new_prints[j]);
                        }
                    }
                }
                let rows = prints.len();
                MESSAGE_PRINTS.with(|p| *p.borrow_mut() = prints);
                (rows, written)
            });
            // `rows` shown, `written` of them converted and written to the model (the rest were
            // unchanged): see docs/perf.md.
            tracing::debug!(
                rows,
                written,
                us = started.elapsed().as_micros() as u64,
                "ui_messages"
            );
            match scroll {
                Scroll::Keep => {}
                // The list corrects its own estimate from the divider row's real position.
                Scroll::Divider(i) => {
                    w.set_divider_index(i as i32);
                    w.set_divider_tick(w.get_divider_tick() + 1);
                }
                Scroll::Anchor(i) => {
                    // The row the user had at the top of the view stays there; when it is not
                    // in the list, the row that was first does.
                    let (row, keep_offset) = anchor_row(&top_id, top_valid, &ids, i);
                    let off = if keep_offset { top_off } else { 0.0 };
                    w.set_anchor_index(row as i32);
                    w.set_anchor_off(off);
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

    /// The row of the "New messages" divider in the open list (`None`: no divider) and the count
    /// of messages from others from there on: the "↑ New messages" button shows them while the
    /// divider is above the view.
    pub fn unread_mark(&self, row: Option<usize>, count: usize) {
        self.run(move |w| {
            w.set_unread_index(row.map_or(-1, |i| i as i32));
            w.set_unread_count(count as i32);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{msg, readers};
    use crate::ui::row_fingerprint::sample_row;

    #[test]
    fn a_theme_line_gets_a_chip_and_a_malformed_line_none() {
        const LINE: &str = "ironteams-theme:1:Night shift:#1d2b45,#e6ecf5,#2f4a75,#ffffff,#1c2430,#2f6fd6,#1f57b8,#d93f3f";
        let chip = msg(sample_row("1", LINE));
        assert_eq!(chip.theme_name, "Night shift");
        assert_eq!(chip.theme_line, LINE);
        assert_eq!(chip.theme_nav, slint::Color::from_rgb_u8(0x1d, 0x2b, 0x45));
        assert_eq!(
            chip.theme_accent,
            slint::Color::from_rgb_u8(0x2f, 0x6f, 0xd6)
        );
        let inside = format!("look: {LINE}");
        for text in [
            "plain words",
            inside.as_str(),
            "ironteams-theme:1:Night shift:#1d2b45",
        ] {
            assert_eq!(msg(sample_row("2", text)).theme_name, "", "{text}");
        }
    }

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

#[cfg(test)]
mod anchor_tests {
    use super::anchor_row;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_row_at_the_top_of_the_view_is_kept_with_its_offset() {
        let rows = ids(&["o1", "o2", "a", "b", "c"]);
        assert_eq!(anchor_row("b", true, &rows, 2), (3, true));
    }

    #[test]
    fn without_a_tracked_row_the_first_row_is_kept() {
        let rows = ids(&["o1", "o2", "a", "b"]);
        assert_eq!(anchor_row("", true, &rows, 2), (2, false));
        assert_eq!(anchor_row("gone", true, &rows, 2), (2, false));
    }

    #[test]
    fn a_row_recorded_before_a_jump_is_not_trusted() {
        let rows = ids(&["o1", "a", "b"]);
        assert_eq!(anchor_row("b", false, &rows, 1), (1, false));
    }
}
