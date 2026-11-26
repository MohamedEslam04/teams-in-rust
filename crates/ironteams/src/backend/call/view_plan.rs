//! Who the call window shows, and how large: the visible participants and each one's on-screen
//! role, derived from the window's view state alone (no Slint types). A meeting's camera
//! subscriptions follow it (`remote_video.rs`): a tile on the stage asks for 720p, a gallery tile
//! for 360p, a filmstrip tile for 180p, and a person no tile shows for nothing at all.
//!
//! The rules mirror `ui/organisms/call_stage.slint` (`Stage`): keep them in step.

use super::Person;

/// A visible person's place on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileRole {
    /// The large tile: the speaker view's speaker (or the pinned person), a gallery page of one,
    /// or a small gallery page in full screen.
    Stage,
    /// A tile of the gallery's grid.
    Gallery,
    /// A small tile: the speaker view's strip, or the filmstrip beside a shared screen.
    Strip,
}

/// The picture a role's tile asks for: width, height, frames a second (the brief's ladder; what a
/// server accepts of each is UNVERIFIED live).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileSize {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl TileRole {
    pub fn size(self) -> TileSize {
        let (width, height, fps) = match self {
            TileRole::Stage => (1280, 720, 30),
            TileRole::Gallery => (640, 360, 30),
            TileRole::Strip => (320, 180, 15),
        };
        TileSize { width, height, fps }
    }

    pub fn name(self) -> &'static str {
        match self {
            TileRole::Stage => "stage",
            TileRole::Gallery => "gallery",
            TileRole::Strip => "strip",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "stage" => TileRole::Stage,
            "gallery" => TileRole::Gallery,
            "strip" => TileRole::Strip,
            _ => return None,
        })
    }

    /// Earlier roles get the receive lines first when there are more tiles than lines.
    fn rank(self) -> u8 {
        match self {
            TileRole::Stage => 0,
            TileRole::Gallery => 1,
            TileRole::Strip => 2,
        }
    }
}

/// The window's view state (`CallStore`): what decides who is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewState<'a> {
    /// `view-mode`: "gallery" | "speaker" | "content".
    pub mode: &'a str,
    /// `pinned`: an MRI, "" for nobody.
    pub pinned: &'a str,
    /// `filmstrip-pos`: "right" | "bottom" | "hidden" | "auto" (anything else is "right").
    pub filmstrip: &'a str,
    /// `gallery-page`, from 0 (clamped here as the stage clamps it).
    pub page: i32,
    /// `gallery-per-page`.
    pub per_page: i32,
    /// `fullscreen`: the immersive call window.
    pub fullscreen: bool,
    /// Someone else's share is up (`CallStore.content-shared()`): never our own.
    pub others_share: bool,
}

/// In full screen a gallery page of at most this many tiles (two columns) draws each tile about
/// half the screen wide: they ask for the stage's size.
const FULLSCREEN_STAGE_TILES: usize = 4;

/// Who the speaker view shows, in-call people only: the pinned person first, else the latest
/// speaker (somebody else rather than us until anyone speaks), then everyone else in the call's
/// order.
pub fn speaker_order(people: &[Person], pinned: &str) -> Vec<Person> {
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

/// The other people on screen (never us: our own camera is never subscribed) and each one's
/// role, the receive lines' priority order: the stage first, then the gallery, then the strip;
/// within a role the latest speakers first, else the order on screen. `people` is the call's
/// order (`CallData::people`: in the call first, the lobby after).
///
/// - Someone else's share on the stage (`view-mode` content): the filmstrip's people
///   ([`speaker_order`]) are strip tiles; a hidden filmstrip shows nobody, but the pinned person
///   keeps a strip-sized line (decision D3), ready for when it shows again.
/// - Speaker view, or anybody pinned: the first of [`speaker_order`] on the stage, the rest in
///   the strip.
/// - Otherwise the gallery: the page shown, as gallery tiles; a page of one is the stage, and so
///   is a page of up to four in full screen.
pub fn visible_roles(v: &ViewState, people: &[Person]) -> Vec<(String, TileRole)> {
    let content = v.mode == "content" && v.others_share;
    let order = speaker_order(people, v.pinned);
    let pinned = |mri: &str| !v.pinned.is_empty() && mri == v.pinned;
    let mut shown: Vec<(&Person, TileRole)> = Vec::new();
    if content {
        let hidden = v.filmstrip == "hidden";
        shown.extend(
            order
                .iter()
                .filter(|p| !hidden || pinned(&p.mri))
                .map(|p| (p, TileRole::Strip)),
        );
    } else if v.mode == "speaker" || !v.pinned.is_empty() {
        shown.extend(order.iter().enumerate().map(|(i, p)| {
            let role = if i == 0 {
                TileRole::Stage
            } else {
                TileRole::Strip
            };
            (p, role)
        }));
    } else {
        let in_call: Vec<&Person> = people.iter().filter(|p| !p.lobby).collect();
        let per_page = usize::try_from(v.per_page).unwrap_or(1).max(1);
        let pages = in_call.len().div_ceil(per_page).max(1);
        let page = usize::try_from(v.page).unwrap_or(0).min(pages - 1);
        let on_page: Vec<&Person> = in_call
            .into_iter()
            .skip(page * per_page)
            .take(per_page)
            .collect();
        let large = on_page.len() == 1 || (v.fullscreen && on_page.len() <= FULLSCREEN_STAGE_TILES);
        let role = if large {
            TileRole::Stage
        } else {
            TileRole::Gallery
        };
        shown.extend(on_page.into_iter().map(|p| (p, role)));
    }
    shown.retain(|(p, _)| !p.me && !p.mri.is_empty());
    // Stable: equal roles and speakers keep their order on screen. The pinned person leads
    // their role (the filmstrip beside a share).
    shown.sort_by_key(|(p, role)| (role.rank(), !pinned(&p.mri), std::cmp::Reverse(p.spoke)));
    shown
        .into_iter()
        .map(|(p, role)| (p.mri.clone(), role))
        .collect()
}

/// The `video-view` command's argument: one `role mri` line per visible person, in priority order.
pub fn encode(roles: &[(String, TileRole)]) -> String {
    roles
        .iter()
        .map(|(mri, role)| format!("{} {mri}", role.name()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads [`encode`]'s text back; `None` when a line is not `role mri`. An empty text is nobody on
/// screen.
pub fn decode(text: &str) -> Option<Vec<(String, TileRole)>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (role, mri) = l.trim().split_once(' ')?;
            let mri = mri.trim();
            if mri.is_empty() {
                return None;
            }
            Some((mri.to_owned(), TileRole::named(role)?))
        })
        .collect()
}

/// Coalesces the window's view into the updates worth sending: a plan goes out once it has held
/// for two looks in a row (the window looks every 150 ms) and differs from the one sent last, so
/// a burst of changes (page flips, a pin and a mode change) sends one update, the settled one.
#[derive(Debug, Default)]
pub struct ViewSend {
    seen: Option<Vec<(String, TileRole)>>,
    sent: Option<Vec<(String, TileRole)>>,
}

impl ViewSend {
    /// One look at the plan now (`None`: no live call, which forgets what was sent so the next
    /// call starts afresh); the plan to send, if any.
    pub fn look(
        &mut self,
        now: Option<Vec<(String, TileRole)>>,
    ) -> Option<Vec<(String, TileRole)>> {
        let Some(now) = now else {
            self.seen = None;
            self.sent = None;
            return None;
        };
        let settled = self.seen.as_ref() == Some(&now);
        self.seen = Some(now.clone());
        (settled && self.sent.as_ref() != Some(&now)).then(|| {
            self.sent = Some(now.clone());
            now
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(mri: &str, me: bool, lobby: bool, spoke: u64) -> Person {
        Person {
            mri: mri.into(),
            name: mri.into(),
            me,
            lobby,
            spoke,
            ..Default::default()
        }
    }

    fn view() -> ViewState<'static> {
        ViewState {
            mode: "gallery",
            pinned: "",
            filmstrip: "auto",
            page: 0,
            per_page: 16,
            fullscreen: false,
            others_share: false,
        }
    }

    fn people() -> Vec<Person> {
        vec![
            p("me", true, false, 0),
            p("ada", false, false, 3),
            p("bo", false, false, 7),
            p("cy", false, true, 0),
            p("di", false, false, 0),
        ]
    }

    fn roles(v: &ViewState, people: &[Person]) -> Vec<(&'static str, String)> {
        visible_roles(v, people)
            .into_iter()
            .map(|(mri, r)| (r.name(), mri))
            .collect()
    }

    fn r(role: &'static str, mri: &str) -> (&'static str, String) {
        (role, mri.to_owned())
    }

    #[test]
    fn the_gallery_shows_everyone_in_the_call_but_us_latest_speakers_first() {
        assert_eq!(
            roles(&view(), &people()),
            [r("gallery", "bo"), r("gallery", "ada"), r("gallery", "di")],
            "not us, not the lobby"
        );
    }

    #[test]
    fn a_gallery_page_shows_only_its_people() {
        let many: Vec<Person> = (0..20)
            .map(|i| p(&format!("p{i:02}"), i == 0, false, 0))
            .collect();
        let first = roles(&view(), &many);
        assert_eq!(first.len(), 15, "16 tiles, one of them ours");
        let second = roles(&ViewState { page: 1, ..view() }, &many);
        assert_eq!(
            second,
            (16..20)
                .map(|i| r("gallery", &format!("p{i}")))
                .collect::<Vec<_>>()
        );
        // Past the last page: the last page, as the stage clamps it.
        let past = ViewState { page: 9, ..view() };
        assert_eq!(roles(&past, &many), second);
    }

    #[test]
    fn a_gallery_page_of_one_is_the_stage_and_so_is_a_small_one_in_full_screen() {
        let two = [p("me", true, false, 0), p("ada", false, false, 0)];
        assert_eq!(roles(&view(), &two), [r("gallery", "ada")]);
        let alone_on_page = [p("ada", false, false, 0)];
        assert_eq!(roles(&view(), &alone_on_page), [r("stage", "ada")]);
        let full = ViewState {
            fullscreen: true,
            ..view()
        };
        assert_eq!(roles(&full, &two), [r("stage", "ada")]);
        let five: Vec<Person> = (0..5)
            .map(|i| p(&format!("p{i}"), false, false, 0))
            .collect();
        assert!(
            visible_roles(&full, &five)
                .iter()
                .all(|(_, role)| *role == TileRole::Gallery),
            "five tiles are a grid even in full screen"
        );
    }

    #[test]
    fn the_speaker_view_puts_the_speaker_on_the_stage_and_the_rest_in_the_strip() {
        let speaker = ViewState {
            mode: "speaker",
            ..view()
        };
        assert_eq!(
            roles(&speaker, &people()),
            [r("stage", "bo"), r("strip", "ada"), r("strip", "di")]
        );
    }

    #[test]
    fn a_pin_puts_its_person_on_the_stage_whatever_the_mode() {
        for mode in ["gallery", "speaker", "content"] {
            let pinned = ViewState {
                mode,
                pinned: "di",
                ..view()
            };
            assert_eq!(roles(&pinned, &people())[0], r("stage", "di"), "{mode}");
        }
    }

    #[test]
    fn beside_someone_else_s_share_everyone_is_in_the_filmstrip() {
        let content = ViewState {
            mode: "content",
            others_share: true,
            ..view()
        };
        assert_eq!(
            roles(&content, &people()),
            [r("strip", "bo"), r("strip", "ada"), r("strip", "di")]
        );
        for pos in ["right", "bottom", "auto", "sideways"] {
            let at = ViewState {
                filmstrip: pos,
                ..content
            };
            assert_eq!(roles(&at, &people()).len(), 3, "{pos}");
        }
        // A pin leads the strip; the share keeps the stage.
        let pinned = ViewState {
            pinned: "di",
            ..content
        };
        assert_eq!(roles(&pinned, &people())[0], r("strip", "di"));
    }

    #[test]
    fn a_hidden_filmstrip_shows_nobody_but_keeps_the_pinned_person() {
        let hidden = ViewState {
            mode: "content",
            others_share: true,
            filmstrip: "hidden",
            ..view()
        };
        assert!(visible_roles(&hidden, &people()).is_empty());
        let pinned = ViewState {
            pinned: "ada",
            ..hidden
        };
        assert_eq!(roles(&pinned, &people()), [r("strip", "ada")]);
    }

    #[test]
    fn content_while_nobody_else_shares_is_the_gallery() {
        // Nobody shares, or the share is ours: the stage shows the gallery.
        let content = ViewState {
            mode: "content",
            ..view()
        };
        assert_eq!(roles(&content, &people()), roles(&view(), &people()));
    }

    #[test]
    fn people_without_an_mri_are_never_asked_for() {
        let unknown = [p("", false, false, 0), p("ada", false, false, 0)];
        assert_eq!(roles(&view(), &unknown), [r("gallery", "ada")]);
    }

    #[test]
    fn the_command_text_reads_back() {
        let plan = vec![
            ("8:orgid:a".to_owned(), TileRole::Stage),
            ("8:orgid:b".to_owned(), TileRole::Strip),
            ("8:teamsvisitor:c".to_owned(), TileRole::Gallery),
        ];
        assert_eq!(decode(&encode(&plan)), Some(plan));
        assert_eq!(decode(""), Some(vec![]), "nobody on screen");
        assert_eq!(decode("huge 8:orgid:a"), None);
        assert_eq!(decode("stage"), None);
        assert_eq!(
            super::super::CallCmd::parse("video-view", "strip 8:orgid:b"),
            Some(super::super::CallCmd::VideoView(vec![(
                "8:orgid:b".into(),
                TileRole::Strip
            )]))
        );
        assert_eq!(super::super::CallCmd::parse("video-view", "tiny x"), None);
    }

    #[test]
    fn a_plan_goes_out_once_it_has_settled_and_only_when_it_changed() {
        let a = vec![("8:a".to_owned(), TileRole::Stage)];
        let b = vec![("8:b".to_owned(), TileRole::Stage)];
        let mut s = ViewSend::default();
        assert_eq!(s.look(Some(a.clone())), None, "not settled yet");
        assert_eq!(s.look(Some(a.clone())), Some(a.clone()));
        assert_eq!(s.look(Some(a.clone())), None, "already sent");
        // A burst: b, a, b... only the plan that holds goes out.
        assert_eq!(s.look(Some(b.clone())), None);
        assert_eq!(s.look(Some(a.clone())), None, "back to what was sent");
        assert_eq!(s.look(Some(b.clone())), None);
        assert_eq!(s.look(Some(b.clone())), Some(b.clone()));
        // The call ended: the next one sends its plan again.
        assert_eq!(s.look(None), None);
        assert_eq!(s.look(Some(b.clone())), None);
        assert_eq!(s.look(Some(b.clone())), Some(b));
    }
}
