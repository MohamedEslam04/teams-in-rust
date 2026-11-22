//! The sounds Settings › Calls and Settings › Notifications offer, for an incoming call, a new
//! message and an urgent message. The choice is local to IronTeams (Teams keeps its own per
//! device). Microsoft Teams' sounds (downloaded, with IronTeams's own as the fallback), IronTeams's
//! own sounds and the message sounds are made in `sounds`; the older ringtones (Chime, Soft,
//! Pulse) are generated in `call_direct`, which needs the calling stack.

use super::sounds::teams_assets::{self, Original, Slot};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The sound of an incoming call, a new message or an urgent message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RingtoneChoice {
    /// Microsoft Teams' default for what it is used for (the ringtone "Default", Tap for
    /// messages, Prioritize for urgent ones), downloaded at run time, or IronTeams's own
    /// equivalent until it is there. Its slug is "default", which is also what a file saved
    /// before the Teams sounds existed says, so those users move to them with no migration.
    #[default]
    Teams,
    /// Another of Teams' sounds (never a slot's default). One of another slot's (a ringtone
    /// where a message sound goes) plays that slot's default.
    Original(&'static Original),
    /// IronTeams's own sounds: soft, marimba-like notes (`sounds::synth`).
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
    /// Every choice, in the order Settings shows them: Teams' own first, then IronTeams's and the
    /// older ones, then the file.
    #[cfg(test)]
    pub fn all() -> Vec<RingtoneChoice> {
        let originals = teams_assets::ORIGINALS
            .iter()
            .filter(|o| !o.is_default())
            .map(RingtoneChoice::Original);
        [RingtoneChoice::Teams]
            .into_iter()
            .chain(originals)
            .chain([
                RingtoneChoice::IronTeams,
                RingtoneChoice::Chime,
                RingtoneChoice::Soft,
                RingtoneChoice::Pulse,
                RingtoneChoice::Custom,
            ])
            .collect()
    }

    pub fn slug(self) -> &'static str {
        match self {
            RingtoneChoice::Teams => "default",
            RingtoneChoice::Original(o) => o.slug,
            RingtoneChoice::IronTeams => "ironteams",
            RingtoneChoice::Chime => "chime",
            RingtoneChoice::Soft => "soft",
            RingtoneChoice::Pulse => "pulse",
            RingtoneChoice::Custom => "custom",
        }
    }

    pub fn from_slug(s: &str) -> Option<RingtoneChoice> {
        match s {
            "default" => Some(RingtoneChoice::Teams),
            "ironteams" => Some(RingtoneChoice::IronTeams),
            "chime" => Some(RingtoneChoice::Chime),
            "soft" => Some(RingtoneChoice::Soft),
            "pulse" => Some(RingtoneChoice::Pulse),
            "custom" => Some(RingtoneChoice::Custom),
            other => teams_assets::by_slug(other).map(RingtoneChoice::Original),
        }
    }

    /// The Teams sound this plays in `slot`: `None` for IronTeams's own sounds, the older ones
    /// and a file. A Teams sound made for another slot gives the slot's default.
    pub fn original(self, slot: Slot) -> Option<&'static Original> {
        match self {
            RingtoneChoice::Teams => Some(teams_assets::default_of(slot)),
            RingtoneChoice::Original(o) if o.slot == slot => Some(o),
            RingtoneChoice::Original(_) => Some(teams_assets::default_of(slot)),
            _ => None,
        }
    }
}

/// Saved as its slug.
impl Serialize for RingtoneChoice {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.slug())
    }
}

impl<'de> Deserialize<'de> for RingtoneChoice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let slug = String::deserialize(d)?;
        RingtoneChoice::from_slug(&slug)
            .ok_or_else(|| D::Error::custom(format!("unknown sound {slug:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_comes_first_and_is_the_default() {
        assert_eq!(RingtoneChoice::all()[0], RingtoneChoice::Teams);
        assert_eq!(RingtoneChoice::default(), RingtoneChoice::Teams);
        assert_eq!(RingtoneChoice::Teams.slug(), "default");
    }

    #[test]
    fn every_choice_round_trips_through_its_slug_and_json() {
        let all = RingtoneChoice::all();
        // The default, the older ones, IronTeams's and the file, and 25 more of Teams'.
        assert_eq!(all.len(), 1 + 25 + 5);
        for c in all {
            assert_eq!(RingtoneChoice::from_slug(c.slug()), Some(c), "{c:?}");
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", c.slug()),
                "the file says what the slug says"
            );
            assert_eq!(serde_json::from_str::<RingtoneChoice>(&json).unwrap(), c);
        }
        assert_eq!(RingtoneChoice::from_slug("siren"), None);
        assert!(serde_json::from_str::<RingtoneChoice>("\"siren\"").is_err());
        assert!(serde_json::from_str::<RingtoneChoice>("3").is_err());
    }

    #[test]
    fn the_saved_default_moves_to_the_teams_sounds_and_the_other_choices_stay() {
        // "default" was IronTeams's own sound; it now names Teams' default.
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
    fn a_teams_sound_is_named_by_its_slug_and_plays_in_its_own_slot() {
        let beep = RingtoneChoice::from_slug("beep").unwrap();
        assert!(matches!(beep, RingtoneChoice::Original(o) if o.file == "beep.mp3"));
        assert_eq!(beep.original(Slot::Ring).unwrap().file, "beep.mp3");
        // A ringtone chosen where a message sound goes (a hand-edited file) plays the default.
        assert!(beep.original(Slot::Message).unwrap().is_default());
        assert_eq!(beep.original(Slot::Message).unwrap().slot, Slot::Message);
        assert!(beep.original(Slot::Urgent).unwrap().is_default());
        let alarm = RingtoneChoice::from_slug("alarm").unwrap();
        assert_eq!(alarm.original(Slot::Urgent).unwrap().label, "Alarm");
        for slot in [Slot::Ring, Slot::Message, Slot::Urgent] {
            let d = RingtoneChoice::Teams.original(slot).unwrap();
            assert!(d.is_default() && d.slot == slot);
        }
        for own in [
            RingtoneChoice::IronTeams,
            RingtoneChoice::Chime,
            RingtoneChoice::Soft,
            RingtoneChoice::Pulse,
            RingtoneChoice::Custom,
        ] {
            assert_eq!(own.original(Slot::Message), None, "{own:?}");
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
