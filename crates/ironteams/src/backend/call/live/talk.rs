use super::*;

/// Who is heard: the participants whose audio `sourceId`s the latest packets name, and the order
/// people last spoke in (the floating call shows the latest speakers first).
#[derive(Debug, Default)]
pub(super) struct Talk {
    pub(super) now: std::collections::HashSet<String>,
    pub(super) last: std::collections::HashMap<String, u64>,
    pub(super) tick: u64,
}

impl Talk {
    /// Takes the sources heard now; true when who speaks changed.
    pub(super) fn heard(&mut self, roster: &Roster, sources: &[u32]) -> bool {
        let now: std::collections::HashSet<String> = roster
            .participants()
            .iter()
            .filter(|p| {
                p.endpoints
                    .iter()
                    .any(|e| e.audio_sources.iter().any(|s| sources.contains(s)))
            })
            .map(|p| p.mri.clone())
            .collect();
        if now == self.now {
            return false;
        }
        self.tick += 1;
        for m in &now {
            self.last.insert(m.clone(), self.tick);
        }
        self.now = now;
        true
    }
}

/// A call's media options: [`share::media_options`] (no video lines when `video` is off), plus in
/// a meeting the data channel that carries live captions.
pub(super) fn media_options(meeting: bool, video: bool) -> MediaOptions {
    MediaOptions {
        data_channel: meeting,
        ..if video {
            share::media_options()
        } else {
            MediaOptions::default()
        }
    }
}

/// The captions in one whole data-channel message (none for another data id). A caption without
/// the speaker's name takes it from the roster by the speaker's id.
pub(super) fn captions_of(msg: DataMessage, roster: &Roster) -> Vec<Caption> {
    if msg.data_id != CAPTIONS_DATA_ID {
        return Vec::new();
    }
    parse_caption_message(&msg.payload)
        .into_iter()
        .map(|l| Caption {
            speaker: if l.speaker.is_empty() {
                speaker_name(roster, &l.user_id)
            } else {
                l.speaker
            },
            id: l.id,
            text: l.text,
        })
        .collect()
}

/// The roster's name for a caption's `userId`: the MRI itself, or (UNVERIFIED which one the
/// service sends) the id after the MRI's prefix.
pub(super) fn speaker_name(roster: &Roster, user_id: &str) -> String {
    if user_id.is_empty() {
        return String::new();
    }
    roster
        .get(user_id)
        .or_else(|| {
            roster.participants().iter().find(|p| {
                p.mri
                    .rsplit_once(':')
                    .is_some_and(|(_, id)| id.eq_ignore_ascii_case(user_id))
            })
        })
        .map(|p| p.display_name.clone())
        .unwrap_or_default()
}

/// The people of the call. `standing` is what profile lookups said about them (the roster alone
/// seldom marks a guest of our organization: same `8:orgid:` MRI and tenant as a colleague).
pub(super) fn people(
    roster: &Roster,
    me: &str,
    own_tenant: Option<&str>,
    standing: &HashMap<String, Standing>,
    talk: &Talk,
    video: &RemoteVideo,
) -> Vec<Person> {
    roster
        .participants()
        .iter()
        .map(|p| Person {
            mri: p.mri.clone(),
            speaking: talk.now.contains(&p.mri),
            spoke: talk.last.get(&p.mri).copied().unwrap_or(0),
            name: if p.display_name.is_empty() {
                "Unknown".into()
            } else {
                p.display_name.clone()
            },
            muted: p.muted(),
            hand: p.hand.is_some(),
            lobby: p.presence == Presence::Lobby,
            me: p.mri == me,
            role: p.role.map(|r| r.name().to_owned()).unwrap_or_default(),
            video_slot: video.slot_of(&p.mri),
            outsider: if p.mri == me {
                None
            } else {
                let looked_up = standing.get(&p.mri);
                classify_outsider(
                    &p.mri,
                    p.tenant_id
                        .as_deref()
                        .or(looked_up.and_then(|s| s.tenant.as_deref())),
                    own_tenant,
                    p.guest || looked_up.is_some_and(|s| s.guest),
                )
            },
        })
        .collect()
}
