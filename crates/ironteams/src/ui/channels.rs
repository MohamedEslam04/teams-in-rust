//! UI-thread side of the channel Posts tab and of Adaptive Cards: fills the `ChannelStore` global
//! and turns card rows into Slint rows (plan 8.4, 8.8).

use super::{UiHandle, img, model, msg, rgb, styled};
use crate::lists_data::{CardCellData, CardRowData, PostData, PostThreadData, ReplierData};
use crate::{AppWindow, CardButton, CardCell, CardRow, ChannelStore, PostItem, Replier};
use slint::{ComponentHandle, ModelRc};

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
    pub fn posts(&self, posts: Vec<PostData>, newest_top: bool, anchor: Option<usize>) {
        self.channel_store(move |s| {
            s.set_posts(model(posts.into_iter().map(post).collect()));
            s.set_newest_top(newest_top);
            s.set_active(true);
            if let Some(i) = anchor {
                s.set_anchor_index(i as i32);
                s.set_anchor_tick(s.get_anchor_tick() + 1);
            }
        });
    }

    /// The thread open in the panel (the panel shows it while its id is the open one).
    pub fn post_thread(&self, t: PostThreadData) {
        self.channel_store(move |s| {
            s.set_thread_post(msg(t.post));
            s.set_thread_replies(model(t.replies.into_iter().map(msg).collect()));
        });
    }

    /// The open conversation shows no post cards (a chat, or a thread).
    pub fn posts_inactive(&self) {
        self.channel_store(|s| s.set_active(false));
    }
}
