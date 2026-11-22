//! The sounds Settings › Calls and Settings › Notifications offer, for an incoming call and for a
//! new message. The choice is local to IronTeams (Teams keeps its own per device). The Default
//! sounds and the message sounds are made in `sounds`; the older ringtones (Chime, Soft, Pulse)
//! are generated in `call_direct`, which needs the calling stack.

use serde::{Deserialize, Serialize};

/// The sound of an incoming call or a new message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RingtoneChoice {
    /// IronTeams's own sounds: soft, marimba-like notes (`sounds`). Shown as "Default".
    #[default]
    #[serde(rename = "default")]
    Teams,
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
    pub const ALL: [RingtoneChoice; 5] = [
        RingtoneChoice::Teams,
        RingtoneChoice::Chime,
        RingtoneChoice::Soft,
        RingtoneChoice::Pulse,
        RingtoneChoice::Custom,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            RingtoneChoice::Teams => "default",
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
