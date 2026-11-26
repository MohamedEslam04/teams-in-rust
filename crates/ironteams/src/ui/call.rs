//! UI-thread side of the call window: fills the `CallStore` global (`ui/stores/call_store.slint`).

use super::{UiHandle, model};
use crate::backend::call::{
    Absent, CallData, Floating, Found, Person, REACTIONS, is_meeting_chat, matches_query,
};
use crate::backend::call_feedback::{self, Ask};
use crate::backend::outsider::Outsider;
use crate::data::{Pixels, initials};
use crate::support::meter::{Probe, Reader};
use crate::{
    AppWindow, CallAbsent, CallCaption, CallFound, CallPerson, CallReaction, CallStore,
    SettingsStore, VideoFxStore,
};
use slint::{ComponentHandle, Image, SharedString};
use std::cell::RefCell;
use std::collections::HashMap;

fn int(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn texts(v: Vec<String>) -> slint::ModelRc<SharedString> {
    model(v.into_iter().map(SharedString::from).collect())
}

/// The layout to switch to when someone else's share starts (`now`) or ends: the shared screen
/// when it starts, back to the gallery when it ends while it was shown. `None`: keep `mode`.
fn view_after_share(mode: &str, was: bool, now: bool) -> Option<&'static str> {
    match (was, now) {
        (false, true) if mode != "content" => Some("content"),
        (true, false) if mode == "content" => Some("gallery"),
        _ => None,
    }
}

/// Switches the layout when a share started or ended since `was`, as Teams does.
fn follow_share(s: &CallStore, was: bool) {
    let now = shared(s);
    if let Some(mode) = view_after_share(&s.get_view_mode(), was, now) {
        s.set_view_mode(mode.into());
    }
}

/// Someone else's share is up: never our own (see `CallStore.content-shared`).
fn shared(s: &CallStore) -> bool {
    !s.get_sharing() && (!s.get_presenter().is_empty() || s.get_has_content())
}

fn set(w: &AppWindow, d: CallData, photos: HashMap<String, Pixels>) {
    let s = w.global::<CallStore>();
    let was_shared = shared(&s);
    if d.phase.is_empty() || d.phase == "prejoin" {
        // A new call opens full size, with no panel or menu left from the last one.
        s.set_minimized(false);
        s.set_menu(SharedString::new());
        s.set_view_mode("gallery".into());
        s.set_gallery_page(0);
        s.set_pinned(SharedString::new());
        if d.phase.is_empty() {
            s.set_panel(SharedString::new());
        }
    }
    if !d.error.is_empty() {
        // Why a join was refused shows on the full window.
        s.set_minimized(false);
    }
    // The quality survey is decided once, when the call ends; it goes with the ended screen.
    if d.phase != "ended" {
        s.set_feedback_open(false);
    } else if s.get_phase() != "ended" {
        let ask = Ask::from_slug(&w.global::<SettingsStore>().get_call_feedback());
        let demo = crate::external::suppressed();
        s.set_feedback_open(
            d.feedback_eligible
                && call_feedback::sampled(ask.unwrap_or_default(), call_feedback::roll(), demo),
        );
    }
    if s.get_phase() == "prejoin" && d.phase != "prejoin" {
        // The effects panel was the pre-join screen's; the call's More panel opens on its own.
        w.global::<VideoFxStore>().set_open(false);
    }
    if d.phase == "ended" && s.get_phase() != "ended" {
        // The call just ended: its floating card has nothing left to show, so the ended screen
        // takes its place (Rejoin, Close). Only on the change, so it can be shrunk again.
        s.set_minimized(false);
    }
    s.set_phase(d.phase.into());
    s.set_kind(d.kind.into());
    s.set_title(d.title.into());
    s.set_my_initials(initials(&d.my_name).into());
    s.set_my_name(d.my_name.into());
    s.set_timer(d.timer.into());
    s.set_muted(d.muted);
    s.set_hand(d.hand);
    s.set_can_share(d.can_share);
    s.set_sharing(d.sharing);
    s.set_can_camera(d.can_camera);
    s.set_camera(d.camera);
    if !d.camera {
        s.set_has_self_view(false);
    }
    if !matches!(d.phase, "connected" | "reconnecting") {
        s.set_has_remote_video(false);
        s.set_has_content(false);
        // Immersive full screen only makes sense in a live call: leave it when the call ends,
        // drops to the lobby, or a new one opens.
        s.set_fullscreen(false);
    }
    s.set_presenter(d.presenter.into());
    follow_share(&s, was_shared);
    s.set_remote_control(d.remote_control);
    s.set_control_mode(d.control_mode.into());
    s.set_control_name(d.control_name.into());
    s.set_prejoin_mic(d.prejoin_mic);
    s.set_prejoin_audio(d.prejoin_audio);
    s.set_prejoin_camera(d.prejoin_camera);
    s.set_recording(d.recording);
    s.set_transcribing(d.transcribing);
    s.set_captions(d.captions);
    s.set_caption_lines(model(
        d.caption_lines
            .into_iter()
            .map(|c| CallCaption {
                speaker: c.speaker.into(),
                text: c.text.into(),
            })
            .collect(),
    ));
    s.set_notice(d.notice.into());
    s.set_error(d.error.into());
    s.set_ended(d.ended.into());
    s.set_chat_id(d.chat_id.into());
    s.set_can_rejoin(d.can_rejoin);
    s.set_held_name(d.held_name.into());
    s.set_in_call(int(d.in_call));
    s.set_gallery_page(clamp_page(
        s.get_gallery_page(),
        d.in_call,
        s.get_gallery_per_page(),
    ));
    s.set_in_lobby(int(d.in_lobby));
    s.set_sound_off(d.sound_off);
    s.set_other_device(d.other_device);
    s.set_can_admit(d.can_admit);
    let images = photo_images(&photos);
    // Every tile and People row shows its person's live reaction.
    let live = live_reactions(&d.reactions);
    let call_person =
        |p, images: &HashMap<String, Image>, video| reacting(call_person(p, images, video), &live);
    let meeting = d.kind == "meeting";
    let (mini, more) = mini_people(&d.people);
    let mini_owner = video_owner(&mini, meeting);
    s.set_mini_people(model(
        mini.into_iter()
            .enumerate()
            .map(|(i, p)| call_person(p, &images, Some(i) == mini_owner))
            .collect(),
    ));
    s.set_mini_more(int(more));
    // Somebody who left is no longer pinned.
    if !pin_holds(&s.get_pinned(), &d.people) {
        s.set_pinned(SharedString::new());
    }
    let last = Last {
        people: d.people.clone(),
        images: images.clone(),
        live: live.clone(),
        meeting,
    };
    fill_speakers(&s, &last);
    LAST.with(|l| *l.borrow_mut() = Some(last));
    let owner = video_owner(&d.people, meeting);
    // The People panel hides the rows its search box does not match; the stage keeps everyone.
    let filter = d.people_filter;
    s.set_people(model(
        d.people
            .into_iter()
            .enumerate()
            .map(|(i, p)| CallPerson {
                listed: matches_query(&p.name, &filter),
                ..call_person(p, &images, Some(i) == owner)
            })
            .collect(),
    ));
    s.set_invited(model(
        d.invited
            .into_iter()
            .map(|a| call_absent(a, &images, &filter))
            .collect(),
    ));
    s.set_chat_people(model(
        d.chat_people
            .into_iter()
            .map(|a| call_absent(a, &images, &filter))
            .collect(),
    ));
    s.set_can_share_invite(d.can_share_invite);
    MIC_ID.with(|m| m.borrow_mut().clone_from(&d.mic_id));
    s.set_mics(texts(d.mics));
    s.set_mic(int(d.mic));
    s.set_speakers(texts(d.speakers));
    s.set_speaker(int(d.speaker));
    let fx = w.global::<VideoFxStore>();
    fx.set_cameras(texts(d.cameras));
    fx.set_camera(int(d.camera_device));
    s.set_held(d.held);
    s.set_found(model(d.found.into_iter().map(call_found).collect()));
    s.set_invite_status(d.invite_status.into());
    REACTIONS_MODEL.with(|m| sync_reactions(m, &d.reactions));
}

/// A reaction's emoji and the name read out.
fn reaction_look(name: &str) -> (&'static str, &'static str) {
    REACTIONS
        .iter()
        .find(|r| r.0 == name)
        .map_or(("", ""), |r| (r.1, r.2))
}

/// Where a reaction rises across the stage, 0..1: spread by its id (golden-ratio steps), so
/// reactions in a row do not stack and the same one keeps its lane.
fn lane(id: u64) -> f32 {
    (id as f64 * 0.618_034).fract() as f32
}

/// The latest live reaction of each sender, by MRI ("" for us).
fn live_reactions(reactions: &[Floating]) -> HashMap<String, &'static str> {
    reactions.iter().map(|r| (r.mri.clone(), r.name)).collect()
}

/// A tile or People row shows its person's live reaction.
fn reacting(mut cp: CallPerson, live: &HashMap<String, &'static str>) -> CallPerson {
    // Ours has no MRI; someone else's without one cannot be matched.
    let key = if cp.me {
        String::new()
    } else {
        cp.id.to_string()
    };
    let name = live.get(&key).filter(|_| cp.me || !key.is_empty());
    if let Some(name) = name {
        let (emoji, label) = reaction_look(name);
        cp.reaction = emoji.into();
        cp.reaction_name = label.into();
    }
    cp
}

fn reaction_id(r: &Floating) -> i32 {
    i32::try_from(r.id).unwrap_or(i32::MAX)
}

fn call_reaction(r: &Floating) -> CallReaction {
    let (emoji, name) = reaction_look(r.name);
    CallReaction {
        id: reaction_id(r),
        emoji: emoji.into(),
        name: name.into(),
        who: r.who.clone().into(),
        mine: r.mine,
        lane: lane(r.id),
    }
}

thread_local! {
    /// The floating reactions, one model for the window: rows are removed and added, never
    /// replaced, so each reaction's rise goes on while the call state is pushed again.
    static REACTIONS_MODEL: std::rc::Rc<slint::VecModel<CallReaction>> = std::rc::Rc::default();
}

/// Makes `m` the floating reactions in `now`: the gone ones leave, the new ones come last.
fn sync_reactions(m: &slint::VecModel<CallReaction>, now: &[Floating]) {
    use slint::Model;
    let ids: Vec<i32> = now.iter().map(reaction_id).collect();
    let mut i = 0;
    while i < m.row_count() {
        match m.row_data(i) {
            Some(r) if !ids.contains(&r.id) => {
                m.remove(i);
            }
            _ => i += 1,
        }
    }
    let shown: Vec<i32> = m.iter().map(|r| r.id).collect();
    for r in now.iter().filter(|r| !shown.contains(&reaction_id(r))) {
        m.push(call_reaction(r));
    }
}

fn call_found(f: Found) -> CallFound {
    CallFound {
        id: f.mri.into(),
        initials: initials(&f.name).into(),
        name: f.name.into(),
        subtitle: f.subtitle.into(),
    }
}

thread_local! {
    /// The photos shown last, as the images made from them: the call state is replaced every
    /// second, and the same pixels should not be uploaded again each time.
    static PHOTO_IMAGES: RefCell<HashMap<String, (Pixels, Image)>> = RefCell::default();
}

/// The images for these photos, reusing the ones made from the same pixels before.
fn photo_images(photos: &HashMap<String, Pixels>) -> HashMap<String, Image> {
    PHOTO_IMAGES.with_borrow_mut(|cache| {
        // Only the people still in the call stay cached.
        cache.retain(|mri, _| photos.contains_key(mri));
        for (mri, px) in photos {
            let same = |old: &Pixels| std::ptr::eq(old.as_bytes().as_ptr(), px.as_bytes().as_ptr());
            if !cache.get(mri).is_some_and(|(old, _)| same(old)) {
                cache.insert(mri.clone(), (px.clone(), Image::from_rgba8(px.clone())));
            }
        }
        cache
            .iter()
            .map(|(mri, (_, img))| (mri.clone(), img.clone()))
            .collect()
    })
}

/// A People panel row of someone who is not in the call.
fn call_absent(a: Absent, images: &HashMap<String, Image>, filter: &str) -> CallAbsent {
    let photo = images.get(&a.mri);
    CallAbsent {
        id: a.mri.into(),
        has_photo: photo.is_some(),
        photo: photo.cloned().unwrap_or_default(),
        initials: initials(&a.name).into(),
        listed: matches_query(&a.name, filter),
        name: a.name.into(),
        status: a.status.into(),
    }
}

fn call_person(p: Person, images: &HashMap<String, Image>, video: bool) -> CallPerson {
    let photo = images.get(&p.mri);
    CallPerson {
        listed: true,
        id: p.mri.clone().into(),
        has_photo: photo.is_some(),
        photo: photo.cloned().unwrap_or_default(),
        video,
        gallery_slot: p.video_slot.map_or(0, |i| int(i + 1)),
        initials: initials(&p.name).into(),
        name: p.name.into(),
        muted: p.muted,
        hand: p.hand,
        reaction: SharedString::new(),
        reaction_name: SharedString::new(),
        lobby: p.lobby,
        me: p.me,
        role: p.role.into(),
        speaking: p.speaking,
        badge: p.outsider.map_or("", Outsider::label).into(),
    }
}

/// Whose tile shows the call's one remote picture (`VideoSlot::Remote`: a line no gallery slot or
/// share names). In a 1:1 call the other side's: the person heard now, else the first other
/// person. In a `meeting` only when its sender is known, the one other person in the call: with
/// more, it is the media server's unasked stream and nothing maps it to a person yet, so no tile
/// shows it rather than one person's camera under another's name (live 2026-11-25, five people:
/// one picture, in whoever spoke). Subscribed pictures (gallery slots) show in their own tiles.
fn video_owner(people: &[Person], meeting: bool) -> Option<usize> {
    let others = || people.iter().enumerate().filter(|(_, p)| !p.me && !p.lobby);
    if meeting {
        let mut it = others();
        return match (it.next(), it.next()) {
            (Some((i, _)), None) => Some(i),
            _ => None,
        };
    }
    others()
        .find(|(_, p)| p.speaking)
        .or_else(|| others().next())
        .map(|(i, _)| i)
}

/// What the speaker view is built from, kept so pinning (a window-side choice) can rebuild it
/// without waiting for the next roster.
struct Last {
    people: Vec<Person>,
    images: HashMap<String, Image>,
    live: HashMap<String, &'static str>,
    meeting: bool,
}

thread_local! {
    static LAST: RefCell<Option<Last>> = const { RefCell::new(None) };
}

/// Fills the speaker view's people: the pinned person first, else the latest speaker.
fn fill_speakers(s: &CallStore, last: &Last) {
    let speakers = speaker_order(&last.people, &s.get_pinned());
    let owner = video_owner(&speakers, last.meeting);
    s.set_speaker_people(model(
        speakers
            .into_iter()
            .enumerate()
            .map(|(i, p)| reacting(call_person(p, &last.images, Some(i) == owner), &last.live))
            .collect(),
    ));
}

/// Pins `mri` for us ("" unpins) and rebuilds the speaker view around it.
fn pin(s: &CallStore, mri: &str) {
    s.set_pinned(mri.into());
    LAST.with(|l| {
        if let Some(last) = l.borrow().as_ref() {
            fill_speakers(s, last);
        }
    });
}

/// The gallery page to show: `page`, at most the last of the pages `in_call` people fill.
fn clamp_page(page: i32, in_call: usize, per_page: i32) -> i32 {
    let per_page = usize::try_from(per_page).unwrap_or(1).max(1);
    let last = int(in_call.div_ceil(per_page).max(1) - 1);
    page.clamp(0, last)
}

/// The pinned person (an MRI, "" for nobody) is still in the call.
fn pin_holds(pinned: &str, people: &[Person]) -> bool {
    pinned.is_empty() || people.iter().any(|p| !p.lobby && p.mri == pinned)
}

/// The floating call shows at most this many tiles.
const MINI_TILES: usize = 4;

/// Who the floating call shows: the others in the call, latest speakers first (the call's order
/// otherwise), or us when alone; and how many more there are beyond [`MINI_TILES`].
fn mini_people(people: &[Person]) -> (Vec<Person>, usize) {
    let mut others: Vec<&Person> = people.iter().filter(|p| !p.lobby && !p.me).collect();
    if others.is_empty() {
        others = people.iter().filter(|p| !p.lobby).collect();
    }
    // Stable: people who never spoke keep the roster's order.
    others.sort_by_key(|p| std::cmp::Reverse(p.spoke));
    let more = others.len().saturating_sub(MINI_TILES);
    (others.into_iter().take(MINI_TILES).cloned().collect(), more)
}

/// Who the speaker view shows, in-call people only: the pinned person first, else the latest
/// speaker (somebody else rather than us until anyone speaks), then everyone else in the call's
/// order.
fn speaker_order(people: &[Person], pinned: &str) -> Vec<Person> {
    let mut v: Vec<&Person> = people.iter().filter(|p| !p.lobby).collect();
    let at = (!pinned.is_empty())
        .then(|| v.iter().position(|p| p.mri == pinned))
        .flatten();
    let lead = at.or_else(|| {
        v.iter()
            .enumerate()
            .max_by_key(|(i, p)| (p.spoke, !p.me, std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
    });
    if let Some(i) = lead {
        let p = v.remove(i);
        v.insert(0, p);
    }
    v.into_iter().cloned().collect()
}

/// Wires what the call window answers by itself: whether a conversation is a meeting chat (the
/// header's Join button).
pub fn wire(window: &AppWindow) {
    let s = window.global::<CallStore>();
    s.on_is_meeting_chat(|id| is_meeting_chat(&id));
    let weak = window.as_weak();
    s.on_pin(move |mri| {
        if let Some(w) = weak.upgrade() {
            pin(&w.global::<CallStore>(), &mri);
        }
    });
    #[cfg(feature = "calls")]
    s.set_camera_available(crate::backend::call::camera::wanted());
    #[cfg(feature = "calls")]
    s.set_slot_videos(GALLERY.with(|g| slint::ModelRc::from(g.clone())));
    s.set_reactions(REACTIONS_MODEL.with(|m| slint::ModelRc::from(m.clone())));
    start_watch(window);
}

/// How often the open menus are looked at ([`watch`]).
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_millis(150);

/// Reads what the chosen microphone hears, 0 to 1 (peak), or `None` until it has a reading.
pub(crate) type MeterPoll = Reader;
/// Opens a level meter on a microphone (`None`: the system default). Blocking (it asks the sound
/// server): it runs on the meter's worker thread, never on the UI thread (see `support::meter`).
pub(crate) type MeterOpen = fn(Option<&str>) -> Option<MeterPoll>;

fn open_meter(mic: Option<&str>) -> Option<MeterPoll> {
    let stream = devices::mic_level(mic).ok()?;
    Some(Box::new(move || stream.latest().map(|l| l.peak)))
}

struct Watch {
    /// The camera menu was open at the last look (its self-preview runs).
    camera_menu: bool,
    /// The mic menu's meter: its worker listens to the microphone, off the UI thread.
    meter: Probe,
    /// The microphone whose meter could not be opened (not tried again until the menu closes).
    failed: Option<Option<String>>,
    open: MeterOpen,
    timer: Option<slint::Timer>,
}

thread_local! {
    static WATCH: RefCell<Watch> = RefCell::new(Watch {
        camera_menu: false,
        meter: Probe::default(),
        failed: None,
        open: open_meter,
        timer: None,
    });
    /// The microphone the call uses now (its id; `None`: the system default).
    static MIC_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The fill of the mic menu's level meter, 0 to 1, for a microphone's peak: square-rooted, so
/// quiet speech still lights a few bars.
pub(crate) fn meter_level(peak: f32) -> f32 {
    peak.clamp(0.0, 1.0).sqrt()
}

/// Looks at the call window's open menus every [`WATCH_EVERY`].
fn start_watch(window: &AppWindow) {
    let weak = window.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, WATCH_EVERY, move || {
        if let Some(w) = weak.upgrade() {
            watch(&w);
        }
    });
    WATCH.with(|x| x.borrow_mut().timer = Some(timer));
}

/// What the open menus need running: the camera menu shows our camera (the self-preview starts
/// when it opens and ends when it closes), the mic menu its level meter (the microphone is
/// listened to only while it is open).
pub(crate) fn watch(w: &AppWindow) {
    let s = w.global::<CallStore>();
    let menu = s.get_menu();

    let camera = menu == "camera";
    let was = WATCH.with(|x| std::mem::replace(&mut x.borrow_mut().camera_menu, camera));
    if camera != was {
        let state = if camera { "on" } else { "off" };
        s.invoke_command("video-preview".into(), state.into());
    }

    if menu != "devices" {
        WATCH.with(|x| {
            let mut x = x.borrow_mut();
            x.meter.stop();
            x.failed = None;
        });
        if s.get_mic_level() != 0.0 {
            s.set_mic_level(0.0);
        }
        return;
    }
    let mic = MIC_ID.with(|m| m.borrow().clone());
    let reading = WATCH.with(|x| {
        let mut x = x.borrow_mut();
        if x.failed.as_ref() == Some(&mic) {
            return None;
        }
        let open = x.open;
        let got = x.meter.poll(mic.as_deref(), move |m| {
            open(m).ok_or_else(|| "the microphone could not be opened".to_string())
        });
        if got.failed.is_some() {
            x.failed = Some(mic.clone());
        }
        got.level
    });
    if let Some(peak) = reading {
        s.set_mic_level(meter_level(peak));
    }
}

/// Tests: the meter reads from `open` instead of a real microphone.
#[cfg(test)]
pub(crate) fn set_meter_source(open: MeterOpen) {
    WATCH.with(|x| x.borrow_mut().open = open);
}

#[cfg(feature = "calls")]
thread_local! {
    /// The meeting gallery's pictures, one row per receive slot, updated in place per frame.
    static GALLERY: std::rc::Rc<slint::VecModel<crate::CallVideo>> = std::rc::Rc::new(
        slint::VecModel::from(vec![
            crate::CallVideo::default();
            crate::backend::call::remote_video::RECEIVE_SLOTS
        ]),
    );
}

impl UiHandle {
    /// Replaces everything the call window shows; `photos` are the profile photos known by MRI.
    pub fn call_state(&self, d: CallData, photos: HashMap<String, Pixels>) {
        self.run(move |w| set(&w, d, photos));
    }

    /// The Video effects panel's choices and its background tiles.
    #[cfg(feature = "calls")]
    pub fn video_fx(&self, v: crate::backend::call::video_fx::FxView) {
        // The thumbnails' pixels are copied here, off the UI thread.
        let tiles: Vec<_> = v
            .choices
            .into_iter()
            .map(|c| {
                let thumb = c.thumb.and_then(|f| {
                    let (w, h) = (u32::try_from(f.width).ok()?, u32::try_from(f.height).ok()?);
                    Some(
                        slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
                            &f.data, w, h,
                        ),
                    )
                });
                (c.id, c.name, thumb, c.loading)
            })
            .collect();
        let p = v.prefs;
        let teams_note = v.teams_note;
        self.run(move |w| {
            let fx = w.global::<VideoFxStore>();
            fx.set_backgrounds(model(
                tiles
                    .into_iter()
                    .map(|(id, name, thumb, loading)| crate::FxChoice {
                        id: id.into(),
                        name: name.into(),
                        loading,
                        has_thumb: thumb.is_some(),
                        thumb: thumb.map(Image::from_rgb8).unwrap_or_default(),
                    })
                    .collect(),
            ));
            fx.set_teams_note(teams_note.into());
            fx.set_background(p.background.into());
            fx.set_filter(p.filter.into());
            fx.set_brighten(p.brighten);
            fx.set_soft_focus(p.soft_focus);
            fx.set_mirror(p.mirror);
        });
    }

    /// Why the pre-join preview has no picture yet ("" once it has).
    #[cfg(feature = "calls")]
    pub fn preview_status(&self, text: String) {
        self.run(move |w| w.global::<VideoFxStore>().set_preview_status(text.into()));
    }

    /// Draws our self-view or the other side's video (`None` clears it).
    #[cfg(feature = "calls")]
    pub fn call_video(
        &self,
        slot: crate::backend::call::VideoSlot,
        frame: Option<calling::video::RgbFrame>,
    ) {
        use crate::backend::call::VideoSlot;
        // The pixels are copied here, off the UI thread; an `Image` itself is not `Send`.
        let pixels = frame.and_then(|f| {
            let (w, h) = (u32::try_from(f.width).ok()?, u32::try_from(f.height).ok()?);
            Some(slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(&f.data, w, h))
        });
        self.run(move |w| {
            let s = w.global::<CallStore>();
            let shown = pixels.is_some();
            let image = pixels.map(slint::Image::from_rgb8).unwrap_or_default();
            match slot {
                VideoSlot::SelfView => {
                    s.set_self_view(image);
                    s.set_has_self_view(shown);
                }
                VideoSlot::Preview => {
                    let fx = w.global::<VideoFxStore>();
                    fx.set_preview(image);
                    fx.set_has_preview(shown);
                }
                VideoSlot::Remote => {
                    s.set_remote_video(image);
                    s.set_has_remote_video(shown);
                }
                VideoSlot::Content => {
                    let was = shared(&s);
                    s.set_content_video(image);
                    s.set_has_content(shown);
                    follow_share(&s, was);
                }
                VideoSlot::Gallery(i) => GALLERY.with(|g| {
                    use slint::Model;
                    if i < g.row_count() {
                        g.set_row_data(
                            i,
                            crate::CallVideo {
                                frame: image,
                                shown,
                            },
                        );
                    }
                }),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, me: bool, lobby: bool, spoke: u64) -> Person {
        Person {
            name: name.into(),
            me,
            lobby,
            spoke,
            ..Default::default()
        }
    }

    fn names(v: &[Person]) -> Vec<&str> {
        v.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn the_floating_call_shows_the_latest_speakers_first_and_counts_the_rest() {
        let people = [
            p("Me", true, false, 9),
            p("Ada", false, false, 0),
            p("Bo", false, false, 3),
            p("Cy", false, true, 0),
            p("Di", false, false, 5),
            p("Ed", false, false, 0),
            p("Fa", false, false, 0),
        ];
        let (mini, more) = mini_people(&people);
        assert_eq!(names(&mini), ["Di", "Bo", "Ada", "Ed"]);
        assert_eq!(more, 1, "Fa; the lobby and we are not counted");
    }

    #[test]
    fn the_speaker_view_puts_the_latest_speaker_first_and_keeps_the_rest_in_order() {
        let people = [
            p("Me", true, false, 0),
            p("Ada", false, false, 3),
            p("Cy", false, true, 99),
            p("Bo", false, false, 7),
            p("Di", false, false, 0),
        ];
        assert_eq!(
            names(&speaker_order(&people, "")),
            ["Bo", "Me", "Ada", "Di"]
        );
    }

    #[test]
    fn the_speaker_view_leads_with_somebody_else_until_someone_speaks() {
        let people = [
            p("Me", true, false, 0),
            p("Ada", false, false, 0),
            p("Bo", false, false, 0),
        ];
        assert_eq!(names(&speaker_order(&people, "")), ["Ada", "Me", "Bo"]);
        assert_eq!(names(&speaker_order(&people[..1], "")), ["Me"]);
        assert!(speaker_order(&[], "").is_empty());
    }

    #[test]
    fn we_lead_the_speaker_view_when_we_spoke_last() {
        let people = [p("Ada", false, false, 2), p("Me", true, false, 5)];
        assert_eq!(names(&speaker_order(&people, "")), ["Me", "Ada"]);
    }

    #[test]
    fn a_pinned_person_leads_the_speaker_view_whoever_speaks() {
        let mut people = [
            p("Me", true, false, 0),
            p("Ada", false, false, 9),
            p("Bo", false, false, 0),
            p("Cy", false, true, 0),
        ];
        people[2].mri = "8:orgid:bo".into();
        people[3].mri = "8:orgid:cy".into();
        assert_eq!(
            names(&speaker_order(&people, "8:orgid:bo")),
            ["Bo", "Me", "Ada"]
        );
        // Nobody pinned, an unknown MRI or one waiting in the lobby: the latest speaker leads.
        for pinned in ["", "8:orgid:nobody", "8:orgid:cy"] {
            assert_eq!(names(&speaker_order(&people, pinned))[0], "Ada");
        }
    }

    #[test]
    fn a_pin_holds_while_its_person_is_in_the_call() {
        let mut people = [p("Ada", false, false, 0), p("Bo", false, true, 0)];
        people[0].mri = "8:orgid:ada".into();
        people[1].mri = "8:orgid:bo".into();
        assert!(pin_holds("", &people));
        assert!(pin_holds("8:orgid:ada", &people));
        assert!(
            !pin_holds("8:orgid:bo", &people),
            "the lobby is not the call"
        );
        assert!(!pin_holds("8:orgid:gone", &people));
    }

    #[test]
    fn the_gallery_page_never_passes_the_last_page() {
        // 17 people at 16 a page: two pages.
        assert_eq!(clamp_page(0, 17, 16), 0);
        assert_eq!(clamp_page(1, 17, 16), 1);
        assert_eq!(clamp_page(2, 17, 16), 1);
        // The 17th left: back to one page.
        assert_eq!(clamp_page(1, 16, 16), 0);
        assert_eq!(clamp_page(1, 0, 16), 0);
        assert_eq!(clamp_page(-1, 40, 16), 0);
        assert_eq!(clamp_page(5, 40, 16), 2);
    }

    #[test]
    fn the_other_side_s_video_shows_in_the_speaker_s_tile_else_the_first_other_one() {
        let mut people = [
            p("Me", true, false, 0),
            p("Cy", false, true, 0),
            p("Ada", false, false, 0),
            p("Bo", false, false, 0),
        ];
        assert_eq!(
            video_owner(&people, false),
            Some(2),
            "not us, not the lobby"
        );
        people[3].speaking = true;
        assert_eq!(video_owner(&people, false), Some(3));
        assert_eq!(video_owner(&people[..2], false), None);
    }

    /// Live 2026-11-25 (a meeting of five): the one picture no gallery slot names showed in the
    /// tile of whoever spoke, under the wrong name. Nothing tells its sender in a meeting, so it
    /// shows only when there is one other person to be its sender.
    #[test]
    fn a_meeting_shows_the_unasked_picture_only_in_a_known_sender_s_tile() {
        let mut people = [
            p("Me", true, false, 0),
            p("Cy", false, true, 0),
            p("Ada", false, false, 0),
            p("Bo", false, false, 0),
        ];
        people[3].speaking = true;
        assert_eq!(video_owner(&people, true), None, "Ada or Bo: unknown");
        assert_eq!(
            video_owner(&people[..3], true),
            Some(2),
            "Ada alone (Cy waits in the lobby)"
        );
        assert_eq!(video_owner(&people[..2], true), None, "nobody else");
    }

    #[test]
    fn a_share_that_starts_takes_the_stage_and_one_that_ends_gives_it_back() {
        assert_eq!(view_after_share("gallery", false, true), Some("content"));
        assert_eq!(view_after_share("content", true, false), Some("gallery"));
        assert_eq!(
            view_after_share("gallery", true, true),
            None,
            "still sharing"
        );
        assert_eq!(
            view_after_share("gallery", true, false),
            None,
            "the user had left the shared screen"
        );
        assert_eq!(view_after_share("content", false, false), None);
    }

    fn floating(id: u64, name: &'static str, mri: &str) -> Floating {
        Floating {
            id,
            name,
            mri: mri.into(),
            who: if mri.is_empty() { "You" } else { "Ada" }.into(),
            mine: mri.is_empty(),
            at: std::time::Instant::now(),
        }
    }

    #[test]
    fn the_reactions_model_keeps_its_rows_and_only_adds_and_removes() {
        use slint::Model;
        let m = slint::VecModel::<CallReaction>::default();
        sync_reactions(&m, &[floating(1, "like", ""), floating(2, "heart", "8:a")]);
        let ids = |m: &slint::VecModel<CallReaction>| m.iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(&m), [1, 2]);
        let first = m.row_data(1).unwrap();
        assert_eq!((first.emoji.as_str(), first.name.as_str()), ("❤️", "Heart"));
        assert_eq!(first.who, "Ada");
        // 1 expired, 3 came: 2 stays where it was.
        sync_reactions(
            &m,
            &[floating(2, "heart", "8:a"), floating(3, "laugh", "8:a")],
        );
        assert_eq!(ids(&m), [2, 3]);
        sync_reactions(&m, &[]);
        assert_eq!(m.row_count(), 0);
    }

    #[test]
    fn reactions_rise_in_lanes_inside_the_stage() {
        for id in 1..50 {
            assert!((0.0..1.0).contains(&lane(id)), "{id}");
        }
        assert!((lane(1) - lane(2)).abs() > 0.2, "neighbours are apart");
    }

    #[test]
    fn a_tile_shows_the_live_reaction_of_its_person() {
        let live = live_reactions(&[floating(1, "like", ""), floating(2, "applause", "8:a")]);
        let person = |id: &str, me| CallPerson {
            id: id.into(),
            me,
            ..Default::default()
        };
        let ours = reacting(person("8:me", true), &live);
        assert_eq!(
            (ours.reaction.as_str(), ours.reaction_name.as_str()),
            ("👍", "Like")
        );
        let ada = reacting(person("8:a", false), &live);
        assert_eq!(ada.reaction_name, "Applause");
        assert_eq!(reacting(person("8:b", false), &live).reaction, "");
        assert_eq!(
            reacting(person("", false), &live).reaction,
            "",
            "an unknown person is not us"
        );
    }

    #[test]
    fn alone_the_floating_call_shows_us() {
        let (mini, more) = mini_people(&[p("Me", true, false, 0), p("Cy", false, true, 0)]);
        assert_eq!(names(&mini), ["Me"]);
        assert_eq!(more, 0);
    }
}
