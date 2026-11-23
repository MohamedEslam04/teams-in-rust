//! UI-thread side of the channel Posts tab and of Adaptive Cards: fills the `ChannelStore` global
//! and turns card rows into Slint rows (plan 8.4, 8.8).

use super::{UiHandle, img, model, model_sync, msg, rgb, row_fingerprint, styled};
use crate::lists_data::{CardCellData, CardRowData, PostData, PostThreadData, ReplierData};
use crate::{AppWindow, CardButton, CardCell, CardRow, ChannelStore, Msg, PostItem, Replier};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// The Posts tab's model and the print of each card in it (`model_sync`).
    static POSTS: RefCell<Option<model_sync::Synced<PostItem>>> =
        const { RefCell::new(None) };
    /// The thread panel's replies, their prints, and the print of the post above them.
    static THREAD: RefCell<Option<(model_sync::Synced<Msg>, u64)>> =
        const { RefCell::new(None) };
}

fn cell(c: CardCellData) -> CardCell {
    let (image, has_img) = img(&c.pixels);
    CardCell {
        plain: c.text.clone().into(),
        text: styled(&c.text),
        stretch: c.stretch,
        size: c.size,
        bold: c.bold,
        tone: c.tone.into(),
        img: image,
        has_img,
    }
}

/// Slint rows for a card (empty when the message has none).
pub(super) fn card_rows(rows: Vec<CardRowData>) -> ModelRc<CardRow> {
    model(
        rows.into_iter()
            .map(|r| {
                let (image, has_img) = img(&r.pixels);
                CardRow {
                    kind: r.kind.into(),
                    plain: r.text.clone().into(),
                    text: styled(&r.text),
                    label: r.label.into(),
                    size: r.size,
                    bold: r.bold,
                    subtle: r.subtle,
                    tone: r.tone.into(),
                    band: r.band.into(),
                    cells: model(r.cells.into_iter().map(cell).collect()),
                    buttons: model(
                        r.buttons
                            .into_iter()
                            .map(|(title, url)| CardButton {
                                title: title.into(),
                                url: url.into(),
                            })
                            .collect(),
                    ),
                    img: image,
                    has_img,
                    img_w: r.image_w,
                    img_h: r.image_h,
                }
            })
            .collect(),
    )
}

fn replier(r: ReplierData) -> Replier {
    let (avatar, has_avatar) = img(&r.avatar);
    Replier {
        name: r.name.into(),
        initials: r.initials.into(),
        color: rgb(r.color),
        avatar,
        has_avatar,
    }
}

fn post(p: PostData) -> PostItem {
    PostItem {
        post: msg(p.post),
        day: p.day.into(),
        reply_count: p.reply_count,
        repliers: model(p.repliers.into_iter().map(replier).collect()),
        following: p.following,
    }
}

impl UiHandle {
    fn channel_store(&self, f: impl FnOnce(ChannelStore<'_>) + Send + 'static) {
        self.run(move |w: AppWindow| f(w.global::<ChannelStore>()));
    }

    /// The post cards of the open channel. `anchor`: older posts were added above, and the card
    /// at that index (the one that was first) stays where it was.
    ///
    /// The cards are patched into the model the list already has (`model_sync`): a new model each
    /// time made the list build every card again and jump, for every avatar or reply count.
    pub fn posts(&self, posts: Vec<PostData>, newest_top: bool, anchor: Option<usize>) {
        self.channel_store(move |s| {
            let rows = posts
                .into_iter()
                .map(|p| ((p.post.id.clone(), row_fingerprint::of_post(&p)), p))
                .collect();
            POSTS.with(|cell| {
                let mut cell = cell.borrow_mut();
                let (m, prints) =
                    cell.get_or_insert_with(|| (Rc::new(VecModel::default()), vec![]));
                // The model shown is another (a test, or the store's default): start from it empty.
                if !model_sync::is_shown(&s.get_posts(), m) {
                    m.set_vec(vec![]);
                    prints.clear();
                    s.set_posts(ModelRc::from(m.clone()));
                }
                model_sync::patch(m, prints, rows, post);
            });
            s.set_newest_top(newest_top);
            s.set_active(true);
            if let Some(i) = anchor {
                s.set_anchor_index(i as i32);
                s.set_anchor_tick(s.get_anchor_tick() + 1);
            }
        });
    }

    /// The thread open in the panel (the panel shows it while its id is the open one). Every
    /// render of the channel sends it: the post is written only when it changed and the replies
    /// are patched (`model_sync`), so the panel's rows are not built again under the reader.
    pub fn post_thread(&self, t: PostThreadData) {
        self.channel_store(move |s| {
            let print = row_fingerprint::of(&t.post);
            let rows = t
                .replies
                .into_iter()
                .map(|r| ((r.id.clone(), row_fingerprint::of(&r)), r))
                .collect();
            THREAD.with(|cell| {
                let mut cell = cell.borrow_mut();
                let ((m, prints), post_print) =
                    cell.get_or_insert_with(|| ((Rc::new(VecModel::default()), vec![]), u64::MAX));
                if *post_print != print || s.get_thread_post().id != t.post.id.as_str() {
                    *post_print = print;
                    s.set_thread_post(msg(t.post));
                }
                if !model_sync::is_shown(&s.get_thread_replies(), m) {
                    m.set_vec(vec![]);
                    prints.clear();
                    s.set_thread_replies(ModelRc::from(m.clone()));
                }
                model_sync::patch(m, prints, rows, msg);
            });
        });
    }

    /// The open conversation shows no post cards (a chat, or a thread).
    pub fn posts_inactive(&self) {
        self.channel_store(|s| s.set_active(false));
    }
}
