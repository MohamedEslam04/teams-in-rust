//! A cheap fingerprint of a message row's view data.
//!
//! The message list is rebuilt from the backend on every render (an avatar decoded, a reaction
//! came in, someone's presence changed...). Turning every row into Slint types again (markdown
//! parse, nested models) and writing it to the model notifies every binding of every row, so a
//! long list stutters while it scrolls. Comparing fingerprints tells which rows really changed.
//!
//! `MsgData` is destructured without `..`: adding a field to it fails to compile here until the
//! fingerprint covers it, so a changed field can never be skipped silently.

use crate::data::{
    ChatData, FileData, ImgData, LinkData, LoopData, MeetingCardData, MsgData, Pixels, ReactData,
    VideoClipData, VoiceData,
};
use crate::lists_data::{CardCellData, CardRowData, PostData, ReplierData};
use std::hash::{Hash, Hasher};

/// Pixel buffers are shared, not copied: the same buffer is the same picture. A new decode is a
/// new buffer (the old one stays alive in the row until it is replaced), so its address differs.
fn pixels(h: &mut impl Hasher, p: &Option<Pixels>) {
    match p {
        None => 0u8.hash(h),
        Some(p) => {
            1u8.hash(h);
            p.width().hash(h);
            p.height().hash(h);
            (p.as_slice().as_ptr() as usize).hash(h);
        }
    }
}

fn float(h: &mut impl Hasher, f: f32) {
    f.to_bits().hash(h);
}

fn image(h: &mut impl Hasher, i: &ImgData) {
    let ImgData {
        url,
        x,
        y,
        w,
        h: ht,
        overflow,
        pixels: px,
    } = i;
    url.hash(h);
    for f in [x, y, w, ht] {
        float(h, *f);
    }
    overflow.hash(h);
    pixels(h, px);
}

fn file(h: &mut impl Hasher, f: &FileData) {
    let FileData {
        name,
        url,
        ext,
        place,
        detail,
    } = f;
    (name, url, ext, place, detail).hash(h);
}

fn reaction(h: &mut impl Hasher, r: &ReactData) {
    let ReactData {
        key,
        emoji,
        name,
        count,
        mine,
        names,
        people,
    } = r;
    (key, emoji, name, count, mine, names).hash(h);
    for p in people {
        let crate::data::ReactorData {
            name,
            initials,
            color,
            avatar,
        } = p;
        (name, initials, color).hash(h);
        pixels(h, avatar);
    }
}

fn link(h: &mut impl Hasher, l: &LinkData) {
    let LinkData {
        url,
        title,
        description,
        domain,
        thumb,
    } = l;
    (url, title, description, domain).hash(h);
    pixels(h, thumb);
}

fn loop_card(h: &mut impl Hasher, l: &LoopData) {
    let LoopData { url, title } = l;
    (url, title).hash(h);
}

fn voice(h: &mut impl Hasher, v: &VoiceData) {
    let VoiceData {
        key,
        url,
        duration_ms,
        peaks,
        unplayed,
        failed,
    } = v;
    (key, url, duration_ms, unplayed, failed).hash(h);
    for p in peaks {
        float(h, *p);
    }
}

fn video(h: &mut impl Hasher, v: &VideoClipData) {
    let VideoClipData {
        key,
        url,
        duration_ms,
        thumb,
    } = v;
    (key, url, duration_ms).hash(h);
    pixels(h, thumb);
}

fn meeting(h: &mut impl Hasher, m: &MeetingCardData) {
    let MeetingCardData {
        kind,
        title,
        detail,
        people,
        more,
        state,
        url,
        recap_chat,
        join_chat,
    } = m;
    (kind, title, detail, more, state, url, recap_chat, join_chat).hash(h);
    for p in people {
        (&p.name, &p.initials, p.color).hash(h);
    }
}

fn cell(h: &mut impl Hasher, c: &CardCellData) {
    let CardCellData {
        text,
        stretch,
        size,
        bold,
        tone,
        pixels: px,
    } = c;
    (text, bold, tone).hash(h);
    float(h, *stretch);
    float(h, *size);
    pixels(h, px);
}

fn card_row(h: &mut impl Hasher, r: &CardRowData) {
    let CardRowData {
        kind,
        text,
        label,
        size,
        bold,
        subtle,
        tone,
        band,
        cells,
        buttons,
        pixels: px,
        image_w,
        image_h,
    } = r;
    (kind, text, label, bold, subtle, tone, band, buttons).hash(h);
    float(h, *size);
    float(h, *image_w);
    float(h, *image_h);
    for c in cells {
        cell(h, c);
    }
    pixels(h, px);
}

/// The fingerprint of a chat list row.
pub(crate) fn of_chat(c: &ChatData) -> u64 {
    let ChatData {
        id,
        title,
        preview,
        time,
        unread,
        initials,
        color,
        avatar,
        section,
        rtl_title,
        rtl_preview,
        favorite,
        draft,
        mention,
        group,
        muted,
        channel,
        presence,
        person,
        private,
        kind,
    } = c;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (
        id, title, preview, time, initials, section, presence, person, kind,
    )
        .hash(&mut h);
    (
        unread,
        color,
        rtl_title,
        rtl_preview,
        favorite,
        draft,
        mention,
        group,
        muted,
        channel,
        private,
    )
        .hash(&mut h);
    pixels(&mut h, avatar);
    h.finish()
}

/// The fingerprint of a channel's post card: the post, its day divider and its replies summary.
pub(crate) fn of_post(p: &PostData) -> u64 {
    let PostData {
        post,
        day,
        reply_count,
        repliers,
        following,
    } = p;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    of(post).hash(&mut h);
    day.hash(&mut h);
    reply_count.hash(&mut h);
    following.hash(&mut h);
    for r in repliers {
        let ReplierData {
            name,
            initials,
            color,
            avatar,
        } = r;
        name.hash(&mut h);
        initials.hash(&mut h);
        color.hash(&mut h);
        pixels(&mut h, avatar);
    }
    h.finish()
}

/// The fingerprint of one row: equal data gives an equal value.
pub(crate) fn of(m: &MsgData) -> u64 {
    let MsgData {
        id,
        sender,
        initials,
        color,
        avatar,
        mine,
        time,
        day,
        show_header,
        edited,
        deleted,
        system,
        subject,
        quote_author,
        quote_text,
        quote_id,
        markdown,
        segments,
        plain,
        rtl,
        images,
        files,
        reactions,
        reply_count,
        pending,
        saved,
        selected,
        failed,
        new_above,
        sender_id,
        presence,
        mentions_me,
        receipt,
        receipt_names,
        receipt_readers,
        receipt_side,
        flash,
        fresh,
        card,
        headline,
        importance,
        grid_w,
        grid_h,
        translate,
        translate_from,
        links,
        pinned,
        urls,
        loops,
        joins,
        voices,
        videos,
        meeting: meet,
    } = m;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let h = &mut h;
    (
        id,
        sender,
        initials,
        color,
        mine,
        time,
        day,
        show_header,
        edited,
        deleted,
        system,
    )
        .hash(h);
    (
        subject,
        quote_author,
        quote_text,
        quote_id,
        markdown,
        segments,
        plain,
        rtl,
        reply_count,
    )
        .hash(h);
    (
        pending,
        saved,
        selected,
        failed,
        new_above,
        sender_id,
        presence,
        mentions_me,
    )
        .hash(h);
    (
        receipt,
        receipt_names,
        receipt_readers,
        receipt_side,
        flash,
        fresh,
        headline,
        importance,
    )
        .hash(h);
    (translate, translate_from, pinned, urls, joins).hash(h);
    float(h, *grid_w);
    float(h, *grid_h);
    pixels(h, avatar);
    for i in images {
        image(h, i);
    }
    for f in files {
        file(h, f);
    }
    for r in reactions {
        reaction(h, r);
    }
    for r in card {
        card_row(h, r);
    }
    for l in links {
        link(h, l);
    }
    for l in loops {
        loop_card(h, l);
    }
    for v in voices {
        voice(h, v);
    }
    for v in videos {
        video(h, v);
    }
    for m in meet {
        meeting(h, m);
    }
    h.finish()
}

/// A message row with every field filled in, for tests and measurements.
#[cfg(test)]
pub(crate) fn sample_row(id: &str, text: &str) -> MsgData {
    MsgData {
        id: id.into(),
        sender: "Ann".into(),
        initials: "A".into(),
        color: 0x112233,
        avatar: None,
        mine: false,
        time: "10:00".into(),
        day: "Today".into(),
        show_header: true,
        edited: false,
        deleted: false,
        system: false,
        subject: String::new(),
        quote_author: String::new(),
        quote_text: String::new(),
        quote_id: String::new(),
        markdown: text.into(),
        segments: vec![],
        plain: text.into(),
        rtl: false,
        images: vec![],
        files: vec![],
        reactions: vec![],
        reply_count: 0,
        pending: false,
        saved: false,
        selected: false,
        failed: false,
        new_above: false,
        sender_id: "8:a".into(),
        presence: String::new(),
        mentions_me: false,
        receipt: String::new(),
        receipt_names: String::new(),
        receipt_readers: vec![],
        receipt_side: false,
        flash: false,
        fresh: false,
        card: vec![],
        headline: String::new(),
        importance: String::new(),
        grid_w: 0.0,
        grid_h: 0.0,
        translate: "",
        translate_from: String::new(),
        links: vec![],
        pinned: false,
        urls: vec![],
        loops: vec![],
        joins: vec![],
        voices: vec![],
        videos: vec![],
        meeting: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ReactData;
    use slint::{Rgba8Pixel, SharedPixelBuffer};

    fn sample() -> MsgData {
        super::sample_row("1", "hello")
    }

    #[test]
    fn a_row_and_its_clone_have_the_same_fingerprint() {
        let a = sample();
        assert_eq!(of(&a), of(&a.clone()));
    }

    #[test]
    fn a_changed_text_flag_or_reaction_changes_the_fingerprint() {
        let base = of(&sample());
        let mut edited = sample();
        edited.markdown = "hello!".into();
        assert_ne!(of(&edited), base);
        let mut pending = sample();
        pending.pending = true;
        assert_ne!(of(&pending), base);
        let mut reacted = sample();
        reacted.reactions.push(ReactData {
            key: "like".into(),
            emoji: "👍".into(),
            name: "Like".into(),
            count: 1,
            mine: false,
            names: "Bob".into(),
            people: vec![],
        });
        assert_ne!(of(&reacted), base);
    }

    #[test]
    fn a_changed_code_block_changes_the_fingerprint() {
        use crate::markdown::Segment;
        let mut a = sample();
        a.segments = vec![Segment::Code("let a = 1;".into())];
        let mut b = sample();
        b.segments = vec![Segment::Code("let a = 2;".into())];
        assert_ne!(of(&a), of(&sample()));
        assert_ne!(of(&a), of(&b));
        assert_eq!(of(&a), of(&a.clone()));
    }

    #[test]
    fn a_decoded_photo_changes_the_fingerprint_but_the_same_buffer_does_not() {
        let base = of(&sample());
        let photo: Pixels = SharedPixelBuffer::<Rgba8Pixel>::new(4, 4);
        let mut a = sample();
        a.avatar = Some(photo.clone());
        let mut b = sample();
        b.avatar = Some(photo);
        assert_ne!(of(&a), base);
        assert_eq!(of(&a), of(&b));
    }
}
