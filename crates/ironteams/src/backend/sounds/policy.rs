//! When a sound plays. Pure: the engine reads the settings, the presence and the chat, and asks
//! here. The banner decision (`quiet::should_banner`: pause, quiet hours, Focus mode, the
//! chat's own routing) comes in as one flag, so a sound never plays for a message that shows no
//! banner.

/// How the user shows right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Available,
    Busy,
    DoNotDisturb,
}

/// The sound switches of Settings › Notifications, as Teams keeps them. What Teams never said
/// is on, except the two that let sounds through while busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switches {
    /// "Sound for new messages" (`alertSound`).
    pub messages: bool,
    /// The global sound switch (`GlobalSoundAndPreview`).
    pub global: bool,
    /// "Sound for incoming calls and meetings starting".
    pub calls_and_meetings: bool,
    /// "Play sounds when I'm busy or in a meeting".
    pub when_busy: bool,
    /// "Play sounds for urgent and priority messages".
    pub urgent_and_priority: bool,
}

impl Default for Switches {
    fn default() -> Self {
        Switches {
            messages: true,
            global: true,
            calls_and_meetings: true,
            when_busy: false,
            urgent_and_priority: false,
        }
    }
}

/// What is known about a message that is about to notify.
#[derive(Debug, Clone, Copy, Default)]
pub struct Message {
    /// The banner decision let it through (not paused, not in quiet hours, not held back by
    /// Focus mode or the chat's routing).
    pub banner: bool,
    /// The chat is muted or snoozed.
    pub muted_chat: bool,
    /// The chat is open and the window has the focus.
    pub open_and_focused: bool,
    /// Marked important or urgent.
    pub urgent: bool,
    /// From someone with priority access.
    pub priority: bool,
    pub status: Status,
    /// In a call or a meeting.
    pub occupied: bool,
}

/// Busy, in Do not disturb or in a call or meeting: sounds need their own switch then.
fn may_sound(status: Status, occupied: bool, important: bool, s: &Switches) -> bool {
    let quiet = occupied || status != Status::Available;
    !quiet || s.when_busy || (s.urgent_and_priority && important)
}

/// Whether a new message plays its sound.
pub fn message_sound(m: &Message, s: &Switches) -> bool {
    m.banner
        && !m.muted_chat
        && !m.open_and_focused
        && s.global
        && s.messages
        && may_sound(m.status, m.occupied, m.urgent || m.priority, s)
}

/// Whether a calendar reminder or a "meeting started" notice plays its sound. `banner`: the
/// notice is allowed (not paused, not in quiet hours).
pub fn reminder_sound(banner: bool, status: Status, occupied: bool, s: &Switches) -> bool {
    banner && s.global && s.calls_and_meetings && may_sound(status, occupied, false, s)
}

/// Collapses a burst of events into one sound: the first plays, and so does the first after a
/// pause of `gap_ms`. Every event, played or not, restarts the pause, so a steady flow of
/// messages sounds once, when it starts.
#[derive(Debug)]
pub struct Burst {
    last_ms: Option<i64>,
    gap_ms: i64,
}

impl Burst {
    pub const fn new(gap_ms: i64) -> Burst {
        Burst {
            last_ms: None,
            gap_ms,
        }
    }

    /// An event at `now_ms`: true when it should make the sound.
    pub fn hit(&mut self, now_ms: i64) -> bool {
        let quiet_for = self.last_ms.map(|l| now_ms - l);
        self.last_ms = Some(now_ms);
        quiet_for.is_none_or(|q| q >= self.gap_ms || q < 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Message {
        Message {
            banner: true,
            ..Message::default()
        }
    }

    #[test]
    fn a_plain_new_message_plays() {
        assert!(message_sound(&fresh(), &Switches::default()));
    }

    #[test]
    fn nothing_plays_when_the_banner_is_held_back() {
        // Paused, quiet hours, Focus mode and routing all come in as "no banner".
        let m = Message {
            banner: false,
            ..fresh()
        };
        assert!(!message_sound(&m, &Switches::default()));
        let s = Switches {
            when_busy: true,
            urgent_and_priority: true,
            ..Switches::default()
        };
        assert!(!message_sound(&m, &s), "no switch overrides it");
    }

    #[test]
    fn a_muted_chat_and_the_open_focused_chat_stay_silent() {
        let s = Switches::default();
        let muted = Message {
            muted_chat: true,
            urgent: true,
            ..fresh()
        };
        assert!(!message_sound(&muted, &s));
        let open = Message {
            open_and_focused: true,
            ..fresh()
        };
        assert!(!message_sound(&open, &s));
    }

    #[test]
    fn the_message_and_global_switches_turn_it_off() {
        let m = fresh();
        let off = |f: fn(&mut Switches)| {
            let mut s = Switches::default();
            f(&mut s);
            s
        };
        assert!(!message_sound(&m, &off(|s| s.messages = false)));
        assert!(!message_sound(&m, &off(|s| s.global = false)));
        // The calls and meetings switch is not about messages.
        assert!(message_sound(&m, &off(|s| s.calls_and_meetings = false)));
    }

    #[test]
    fn busy_do_not_disturb_and_calls_need_the_busy_switch() {
        let s = Switches::default();
        for m in [
            Message {
                status: Status::Busy,
                ..fresh()
            },
            Message {
                status: Status::DoNotDisturb,
                ..fresh()
            },
            Message {
                occupied: true,
                ..fresh()
            },
        ] {
            assert!(!message_sound(&m, &s), "{m:?}");
            let allowed = Switches {
                when_busy: true,
                ..s
            };
            assert!(message_sound(&m, &allowed), "{m:?} with the busy switch");
        }
    }

    #[test]
    fn urgent_and_priority_messages_get_through_when_that_switch_is_on() {
        let on = Switches {
            urgent_and_priority: true,
            ..Switches::default()
        };
        let busy = Message {
            status: Status::DoNotDisturb,
            ..fresh()
        };
        assert!(!message_sound(&busy, &on), "an ordinary message");
        let urgent = Message {
            urgent: true,
            ..busy
        };
        let priority = Message {
            priority: true,
            ..busy
        };
        assert!(message_sound(&urgent, &on));
        assert!(message_sound(&priority, &on));
        assert!(!message_sound(&urgent, &Switches::default()), "switch off");
    }

    #[test]
    fn reminders_follow_the_calls_and_meetings_switch() {
        let s = Switches::default();
        assert!(reminder_sound(true, Status::Available, false, &s));
        assert!(
            !reminder_sound(false, Status::Available, false, &s),
            "quiet hours"
        );
        let off = Switches {
            calls_and_meetings: false,
            ..s
        };
        assert!(!reminder_sound(true, Status::Available, false, &off));
        let global_off = Switches { global: false, ..s };
        assert!(!reminder_sound(true, Status::Available, false, &global_off));
        // The message switch is not about reminders.
        let no_messages = Switches {
            messages: false,
            ..s
        };
        assert!(reminder_sound(true, Status::Available, false, &no_messages));
        // Busy or in a call: only with the busy switch.
        assert!(!reminder_sound(true, Status::Busy, false, &s));
        assert!(!reminder_sound(true, Status::Available, true, &s));
        let busy_ok = Switches {
            when_busy: true,
            ..s
        };
        assert!(reminder_sound(true, Status::Busy, true, &busy_ok));
    }

    #[test]
    fn a_burst_sounds_once() {
        let mut b = Burst::new(1500);
        assert!(b.hit(10_000), "the first");
        assert!(!b.hit(10_300));
        assert!(!b.hit(11_200));
        assert!(
            !b.hit(12_600),
            "1.4 s after the last one: still the same burst"
        );
        assert!(b.hit(14_100), "1.5 s of quiet: a new burst");
        assert!(b.hit(20_000));
    }

    #[test]
    fn a_clock_that_went_back_does_not_silence_the_next_sound() {
        let mut b = Burst::new(1500);
        assert!(b.hit(50_000));
        assert!(b.hit(40_000));
    }
}
