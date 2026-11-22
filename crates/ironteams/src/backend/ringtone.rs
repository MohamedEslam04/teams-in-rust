//! The sounds Settings › Calls and Settings › Notifications offer, for an incoming call and for a
//! new message. The choice is local to IronTeams (Teams keeps its own per device). The Teams
//! sounds (downloaded, with IronTeams's own as the fallback), IronTeams's own sounds and the
//! message sounds are made in `sounds`; the older ringtones (Chime, Soft, Pulse) are generated in
//! `call_direct`, which needs the calling stack.

use serde::{Deserialize, Serialize};

/// The sound of an incoming call or a new message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RingtoneChoice {
    /// Microsoft Teams' original sounds, downloaded at run time (`sounds::teams_assets`), or
    /// IronTeams's own equivalent until they are there. Shown as "Teams (default)". Its slug is
    /// still "default", which is what a file saved before the originals existed says, so those
    /// users move to the originals with no migration.
    #[default]
    #[serde(rename = "default")]
    Teams,
    /// IronTeams's own sounds: soft, marimba-like notes (`sounds::synth`).
    #[serde(rename = "ironteams")]
    IronTeams,
    /// The two-note chime.
    Chime,
    /// One long, gentle note.
    Soft,
    /// Three quick beeps.
    Pulse,
    /// A sound file the user picked (WAV, MP3 or M4A).
    Custom,
}

impl RingtoneChoice {
    pub const ALL: [RingtoneChoice; 6] = [
        RingtoneChoice::Teams,
        RingtoneChoice::IronTeams,
        RingtoneChoice::Chime,
        RingtoneChoice::Soft,
        RingtoneChoice::Pulse,
        RingtoneChoice::Custom,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            RingtoneChoice::Teams => "default",
            RingtoneChoice::IronTeams => "ironteams",
            RingtoneChoice::Chime => "chime",
            RingtoneChoice::Soft => "soft",
            RingtoneChoice::Pulse => "pulse",
            RingtoneChoice::Custom => "custom",
        }
    }

    pub fn from_slug(s: &str) -> Option<RingtoneChoice> {
        Self::ALL.into_iter().find(|c| c.slug() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_comes_first_and_is_the_default() {
        assert_eq!(RingtoneChoice::ALL[0], RingtoneChoice::Teams);
        assert_eq!(RingtoneChoice::default(), RingtoneChoice::Teams);
        assert_eq!(RingtoneChoice::Teams.slug(), "default");
    }

    #[test]
    fn every_choice_round_trips_through_its_slug_and_json() {
        for c in RingtoneChoice::ALL {
            assert_eq!(RingtoneChoice::from_slug(c.slug()), Some(c));
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", c.slug()),
                "the file says what the slug says"
            );
            assert_eq!(serde_json::from_str::<RingtoneChoice>(&json).unwrap(), c);
        }
        assert_eq!(RingtoneChoice::from_slug("siren"), None);
    }

    #[test]
    fn the_saved_default_moves_to_the_teams_originals_and_the_other_choices_stay() {
        // "default" was IronTeams's own sound; it now names the Teams originals.
        assert_eq!(
            serde_json::from_str::<RingtoneChoice>("\"default\"").unwrap(),
            RingtoneChoice::Teams
        );
        assert_eq!(
            RingtoneChoice::from_slug("default"),
            Some(RingtoneChoice::Teams)
        );
        // IronTeams's own set is a choice of its own now.
        assert_eq!(RingtoneChoice::IronTeams.slug(), "ironteams");
        assert_eq!(
            RingtoneChoice::from_slug("ironteams"),
            Some(RingtoneChoice::IronTeams)
        );
        assert_ne!(RingtoneChoice::IronTeams, RingtoneChoice::default());
        for (saved, want) in [
            ("\"chime\"", RingtoneChoice::Chime),
            ("\"soft\"", RingtoneChoice::Soft),
            ("\"pulse\"", RingtoneChoice::Pulse),
            ("\"custom\"", RingtoneChoice::Custom),
        ] {
            assert_eq!(serde_json::from_str::<RingtoneChoice>(saved).unwrap(), want);
        }
    }

    #[test]
    fn a_file_saved_before_the_default_existed_still_reads() {
        for (saved, want) in [
            ("\"chime\"", RingtoneChoice::Chime),
            ("\"soft\"", RingtoneChoice::Soft),
            ("\"pulse\"", RingtoneChoice::Pulse),
        ] {
            assert_eq!(serde_json::from_str::<RingtoneChoice>(saved).unwrap(), want);
        }
    }
}
