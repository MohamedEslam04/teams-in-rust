//! Sounds: a new message, a reminder or a meeting starting, a call joined or left, the ringtone of
//! an incoming call and the ringback of our own.
//!
//! The default sounds are made here from nothing (`synth`: additive synthesis, soft marimba-like
//! notes, no asset and no one else's recording). The user can keep the older ringtones, or pick a
//! sound file (WAV, MP3 or M4A) for the message sound and the ringtone.
//!
//! Everything plays through `devices::open_output_stream`, the same cpal output the Settings test
//! sound uses, on the speaker chosen in Settings › Devices (the system default when none is) and
//! at the system volume. A stream lives on a thread of its own and closes when the sound is over.
//! With no output device nothing plays and the reason is logged. Tests and demo mode never play.
//!
//! Whether a sound plays is `policy` (settings, presence, the chat) on top of the banner decision
//! in `quiet`; a burst of messages makes one sound.

mod cursor;
mod policy;
mod synth;
mod wav;

use crate::support::quiet::{self, Decision, Incoming};
use super::ringtone::RingtoneChoice;
use super::{Engine, call_presence, people_presence, settings};
use cursor::Cursor;
use policy::{Burst, Status, Switches};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use synth::{Plain, Sound};
use teams_proto::user_settings::{NotificationSettings, SoundField};

#[cfg(feature = "calls")]
use std::sync::Arc;

pub const SOUNDS_FILE: &str = "sounds.json";

/// The sound preferences kept in `sounds.json` (the ringtone is in `calls.json`, with the rest of
/// the call settings).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundPrefs {
    /// The sound of a new message. A file saved without one gets the Default.
    pub message: RingtoneChoice,
    /// The sound file of the "Custom" message sound.
    pub message_file: Option<String>,
}

/// The saved sound preferences, read each time (a change in Settings applies to the next sound).
fn sound_prefs() -> SoundPrefs {
    session::Paths::new()
        .map(|p| settings::read_json(&p.config_dir.join(SOUNDS_FILE)))
        .unwrap_or_default()
}

/// What a Settings preview plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Message,
    Ringtone,
}

/// What to play: one of the built-in sounds, or a file (with the built-in sound to fall back to).
#[derive(Debug, Clone, PartialEq)]
enum Tone {
    Sound(Sound),
    Plain(Plain),
    File(PathBuf, Sound),
}

/// How long a sound may be, and whether it repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Played once (a message, a cue, a reminder).
    Once,
    /// Round and round (the ringtone, the ringback).
    Ring,
    /// A Settings preview: a few seconds at most.
    Preview,
}

impl Role {
    /// The longest a file is kept, in seconds, and the rest added after it when it repeats.
    fn file_limits(self) -> (f64, f64) {
        match self {
            Role::Once => (10.0, 0.0),
            Role::Ring => (30.0, 1.0),
            Role::Preview => (6.0, 0.0),
        }
    }
}

/// A loud file is turned down to this peak (-6 dBFS); a quiet one is left alone.
const FILE_CEILING: f32 = 0.5;
/// The largest file read for a sound.
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// A sound that plays once is cut off after this, whatever the device does.
const MAX_ONCE: Duration = Duration::from_secs(40);
/// A ringtone stops by itself after this, whatever happens to the call.
#[cfg(feature = "calls")]
const RING_MAX: Duration = Duration::from_secs(90);

fn file_tone(file: Option<&str>, fallback: Sound) -> Tone {
    match file.filter(|f| !f.is_empty()) {
        Some(f) => Tone::File(PathBuf::from(f), fallback),
        None => Tone::Sound(fallback),
    }
}

/// The message sound of a choice.
fn message_tone(choice: RingtoneChoice, file: Option<&str>) -> Tone {
    match choice {
        RingtoneChoice::Teams => Tone::Sound(Sound::Message),
        RingtoneChoice::Chime => Tone::Plain(Plain::Chime),
        RingtoneChoice::Soft => Tone::Plain(Plain::Soft),
        RingtoneChoice::Pulse => Tone::Plain(Plain::Pulse),
        RingtoneChoice::Custom => file_tone(file, Sound::Message),
    }
}

/// The ringtone of a choice made here. `None`: one of the older ringtones, which `call_direct`
/// generates.
fn ring_tone(choice: RingtoneChoice, file: Option<&str>) -> Option<Tone> {
    match choice {
        RingtoneChoice::Teams => Some(Tone::Sound(Sound::Ringtone)),
        RingtoneChoice::Custom => Some(file_tone(file, Sound::Ringtone)),
        RingtoneChoice::Chime | RingtoneChoice::Soft | RingtoneChoice::Pulse => None,
    }
}

/// The samples of a tone for a device running at `rate`.
fn samples(tone: &Tone, rate: u32, role: Role) -> Vec<f32> {
    match tone {
        Tone::Sound(s) => synth::synth(*s, rate),
        Tone::Plain(p) => synth::plain(*p, rate),
        Tone::File(path, fallback) => load_file(path, rate, role).unwrap_or_else(|err| {
            tracing::warn!(%err, path = %path.display(), "the sound file can't be played");
            synth::synth(*fallback, rate)
        }),
    }
}

/// A sound file as mono samples at `rate`, cut to the role's length and turned down if loud.
fn load_file(path: &Path, rate: u32, role: Role) -> Result<Vec<f32>, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err("the file is too large for a sound (25 MB at most)".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pcm = decode(&bytes)?;
    let mut out = wav::resample(&pcm.samples, pcm.rate, rate);
    let (max_secs, rest_secs) = role.file_limits();
    wav::fit(&mut out, rate, max_secs, FILE_CEILING);
    let rest = (rest_secs * f64::from(rate)) as usize;
    out.resize(out.len() + rest, 0.0);
    Ok(out)
}

/// WAV is read here; MP3 and M4A (AAC) by the decoder voice messages use. OGG is not supported.
fn decode(bytes: &[u8]) -> Result<wav::Pcm, String> {
    if bytes.starts_with(b"RIFF") {
        return wav::decode_wav(bytes);
    }
    let clip =
        voice::decode(bytes).map_err(|e| format!("{e} (WAV, MP3 and M4A files can be used)"))?;
    Ok(wav::Pcm {
        samples: clip.samples,
        rate: clip.sample_rate,
    })
}

/// Whether `path` can be used as a sound: the reason when it can't.
pub(crate) fn check_file(path: &Path) -> Result<(), String> {
    load_file(path, 48_000, Role::Preview).map(|_| ())
}

// ---------------------------------------------------------------------------------------------
// Playing

/// The speaker chosen in Settings › Devices (`None`: the system default).
fn speaker() -> Option<String> {
    let dir = session::Paths::new().ok()?.config_dir;
    settings::read_json::<devices::DevicePrefs>(&dir.join(settings::DEVICES_FILE)).speaker
}

/// Plays `tone` once, on a thread of its own. Silent in tests and demo mode.
fn play_once(tone: Tone, role: Role) {
    if crate::external::suppressed() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("ironteams-sound".into())
        .spawn(move || {
            if let Err(err) = play_to_end(&tone, role) {
                tracing::debug!(%err, "sound not played");
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(%err, "sound not started");
    }
}

/// Opens the speaker, plays `tone` once and returns after it has been heard.
fn play_to_end(tone: &Tone, role: Role) -> Result<(), devices::Error> {
    let done = std::sync::Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let _out = devices::open_output_stream(speaker().as_deref(), move |rate, channels| {
        let mut cursor = Cursor::new(samples(tone, rate, role), channels, false);
        move |buf: &mut [f32]| {
            if cursor.fill(buf) {
                flag.store(true, Ordering::Release);
            }
        }
    })?;
    let deadline = Instant::now() + MAX_ONCE;
    while !done.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    // The last buffer is handed over before it is audible; let it drain.
    std::thread::sleep(Duration::from_millis(250));
    Ok(())
}

/// A sound that repeats until this is dropped.
#[cfg(feature = "calls")]
pub(crate) struct Loop {
    stop: Arc<AtomicBool>,
}

#[cfg(feature = "calls")]
impl Drop for Loop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Repeats `tone` on a thread of its own until the returned handle is dropped (or `max` passes).
/// `None` in tests and demo mode, and when the thread can't start.
#[cfg(feature = "calls")]
fn start_loop(tone: Tone, max: Duration) -> Option<Loop> {
    if crate::external::suppressed() {
        return None;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("ironteams-sound-loop".into())
        .spawn(move || {
            if let Err(err) = play_looped(&tone, &flag, max) {
                tracing::debug!(%err, "sound not played");
            }
        });
    match spawned {
        Ok(_) => Some(Loop { stop }),
        Err(err) => {
            tracing::warn!(%err, "sound not started");
            None
        }
    }
}

#[cfg(feature = "calls")]
fn play_looped(tone: &Tone, stop: &AtomicBool, max: Duration) -> Result<(), devices::Error> {
    let _out = devices::open_output_stream(speaker().as_deref(), |rate, channels| {
        let mut cursor = Cursor::new(samples(tone, rate, Role::Ring), channels, true);
        move |buf: &mut [f32]| {
            cursor.fill(buf);
        }
    })?;
    let until = Instant::now() + max;
    while !stop.load(Ordering::Relaxed) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// The ringtone of an incoming call, repeating until the handle is dropped. `None` for the older
/// ringtones (`call_direct` plays those) and when nothing can play.
#[cfg(feature = "calls")]
pub(crate) fn start_ringtone(choice: RingtoneChoice, file: Option<&str>) -> Option<Loop> {
    start_loop(ring_tone(choice, file)?, RING_MAX)
}

/// The soft tone pair our own call plays while it rings.
#[cfg(feature = "calls")]
pub(crate) fn start_ringback() -> Option<Loop> {
    start_loop(Tone::Sound(Sound::Ringback), RING_MAX)
}

/// The cue of a call that was joined or left.
pub(crate) fn call_cue(joined: bool) {
    if !switches().global {
        return;
    }
    let cue = if joined { Sound::Join } else { Sound::Leave };
    play_once(Tone::Sound(cue), Role::Once);
}

/// The Play button of Settings: `choice` (and `file`, for Custom) once.
pub(crate) fn preview(kind: Kind, choice: RingtoneChoice, file: Option<&str>) {
    match kind {
        Kind::Message => play_once(message_tone(choice, file), Role::Preview),
        Kind::Ringtone => match ring_tone(choice, file) {
            Some(tone) => play_once(tone, Role::Preview),
            None => preview_older_ringtone(choice),
        },
    }
}

#[cfg(feature = "calls")]
fn preview_older_ringtone(choice: RingtoneChoice) {
    super::call_direct::preview_ringtone(choice);
}

#[cfg(not(feature = "calls"))]
fn preview_older_ringtone(_choice: RingtoneChoice) {}

// ---------------------------------------------------------------------------------------------
// When to play

/// The sound switches of Settings › Notifications, as last read from Teams.
fn switches() -> Switches {
    switches_from(settings::remembered_notification_settings().as_ref())
}

fn switches_from(n: Option<&NotificationSettings>) -> Switches {
    let Some(n) = n else {
        return Switches::default();
    };
    let is = |f: SoundField, v: &str| n.sound(f).is_some_and(|x| x.eq_ignore_ascii_case(v));
    Switches {
        messages: !is(SoundField::AlertSound, "off"),
        global: !is(SoundField::GlobalSoundAndPreview, "off"),
        calls_and_meetings: !is(SoundField::CallAndMeetingStart, "off"),
        when_busy: is(SoundField::WhenBusyOrInMeetings, "on"),
        urgent_and_priority: is(SoundField::UrgentAndPriority, "on"),
    }
}

/// My availability as the presence service reports it ("BusyIdle" is Busy).
fn status_of(availability: &str) -> Status {
    match availability.strip_suffix("Idle").unwrap_or(availability) {
        "Busy" => Status::Busy,
        "DoNotDisturb" => Status::DoNotDisturb,
        _ => Status::Available,
    }
}

/// In a call or a meeting, or presenting.
fn occupied(activity: &str, in_call: bool) -> bool {
    in_call
        || matches!(
            activity,
            "InACall" | "InAConferenceCall" | "InAMeeting" | "Presenting"
        )
}

/// A banner is shown for this decision.
fn banner_shown(decision: &Decision<'_>) -> bool {
    matches!(decision, Decision::Show { .. })
}

/// One message sound for a burst of messages.
static MESSAGES: Mutex<Burst> = Mutex::new(Burst::new(1500));
/// A reminder and a "meeting started" notice in the same minute share one sound.
static REMINDERS: Mutex<Burst> = Mutex::new(Burst::new(5000));

fn burst_hit(burst: &Mutex<Burst>, now_ms: i64) -> bool {
    burst.lock().unwrap_or_else(|e| e.into_inner()).hit(now_ms)
}

impl Engine {
    /// A message reached the notification step with the banner decision `decision`: plays the
    /// message sound when the settings, the presence and the chat allow it. The open chat in a
    /// focused window never gets here (`live::should_notify`).
    pub(super) fn play_message_sound(
        &self,
        m: &teams_proto::model::Message,
        inc: &Incoming<'_>,
        decision: &Decision<'_>,
    ) {
        if crate::external::suppressed() {
            return;
        }
        let facts = policy::Message {
            banner: banner_shown(decision),
            muted_chat: self.lists_silenced(&m.conversation_id),
            open_and_focused: false,
            urgent: inc.urgent,
            priority: inc.priority_sender,
            status: status_of(&people_presence::known(&self.me)),
            occupied: occupied(
                &people_presence::known_activity(&self.me),
                call_presence::in_call(),
            ),
        };
        if !policy::message_sound(&facts, &switches())
            || !burst_hit(&MESSAGES, teams_proto::time::now_ms())
        {
            return;
        }
        let prefs = sound_prefs();
        play_once(
            message_tone(prefs.message, prefs.message_file.as_deref()),
            Role::Once,
        );
    }

    /// A calendar reminder or a "meeting started" notice is being shown: plays the reminder sound
    /// when "Sound for incoming calls and meetings starting" and the rest allow it.
    pub(super) fn play_reminder_sound(&self) {
        if crate::external::suppressed() {
            return;
        }
        let now = chrono::Utc::now();
        let prefs = quiet::load_prefs();
        let inc = Incoming {
            paused: settings::notifications_paused(now),
            routed: true,
            call: true,
            ..Incoming::default()
        };
        let focus = quiet::focus_active(quiet::focus_until(&prefs), now.timestamp_millis());
        let decision =
            quiet::should_banner(&inc, chrono::Local::now().naive_local(), &prefs, focus);
        let allowed = policy::reminder_sound(
            banner_shown(&decision),
            status_of(&people_presence::known(&self.me)),
            occupied(
                &people_presence::known_activity(&self.me),
                call_presence::in_call(),
            ),
            &switches(),
        );
        if allowed && burst_hit(&REMINDERS, now.timestamp_millis()) {
            play_once(Tone::Sound(Sound::Reminder), Role::Once);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use appearance::general::{GeneralPrefs, QuietHoursPrefs};
    use chrono::NaiveDate;

    #[test]
    fn the_default_and_the_older_choices_map_to_their_sounds() {
        assert_eq!(
            message_tone(RingtoneChoice::Teams, None),
            Tone::Sound(Sound::Message)
        );
        assert_eq!(
            message_tone(RingtoneChoice::Chime, None),
            Tone::Plain(Plain::Chime)
        );
        assert_eq!(
            message_tone(RingtoneChoice::Soft, None),
            Tone::Plain(Plain::Soft)
        );
        assert_eq!(
            message_tone(RingtoneChoice::Pulse, None),
            Tone::Plain(Plain::Pulse)
        );
        assert_eq!(
            ring_tone(RingtoneChoice::Teams, None),
            Some(Tone::Sound(Sound::Ringtone))
        );
        for older in [
            RingtoneChoice::Chime,
            RingtoneChoice::Soft,
            RingtoneChoice::Pulse,
        ] {
            assert_eq!(ring_tone(older, None), None, "{older:?} is call_direct's");
        }
    }

    #[test]
    fn a_custom_choice_plays_its_file_or_the_default_when_it_has_none() {
        let file = Some("/home/me/ping.wav");
        assert_eq!(
            message_tone(RingtoneChoice::Custom, file),
            Tone::File("/home/me/ping.wav".into(), Sound::Message)
        );
        assert_eq!(
            ring_tone(RingtoneChoice::Custom, file),
            Some(Tone::File("/home/me/ping.wav".into(), Sound::Ringtone))
        );
        for none in [None, Some("")] {
            assert_eq!(
                message_tone(RingtoneChoice::Custom, none),
                Tone::Sound(Sound::Message)
            );
            assert_eq!(
                ring_tone(RingtoneChoice::Custom, none),
                Some(Tone::Sound(Sound::Ringtone))
            );
        }
    }

    /// A 16-bit mono WAV of a sine, `secs` long.
    fn sine_wav(rate: u32, secs: f32, amp: f32) -> Vec<u8> {
        let n = (rate as f32 * secs) as usize;
        let data: Vec<u8> = (0..n)
            .flat_map(|i| {
                let x = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin();
                ((x * amp * 32_767.0) as i16).to_le_bytes()
            })
            .collect();
        let mut f = Vec::new();
        f.extend(b"RIFF");
        f.extend(((36 + data.len()) as u32).to_le_bytes());
        f.extend(b"WAVEfmt ");
        f.extend(16u32.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(rate.to_le_bytes());
        f.extend((rate * 2).to_le_bytes());
        f.extend(2u16.to_le_bytes());
        f.extend(16u16.to_le_bytes());
        f.extend(b"data");
        f.extend((data.len() as u32).to_le_bytes());
        f.extend(data);
        f
    }

    #[test]
    fn a_wav_file_becomes_samples_at_the_speakers_rate_and_loud_ones_are_turned_down() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ping.wav");
        std::fs::write(&path, sine_wav(22_050, 1.0, 1.0)).unwrap();
        assert_eq!(check_file(&path), Ok(()));
        let s = load_file(&path, 48_000, Role::Once).unwrap();
        assert!((s.len() as i64 - 48_000).abs() <= 2, "{}", s.len());
        let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak <= FILE_CEILING + 1e-3, "{peak}");
        assert!(peak > 0.4, "not silenced: {peak}");

        // A quiet file is left as it is.
        std::fs::write(&path, sine_wav(48_000, 0.5, 0.1)).unwrap();
        let s = load_file(&path, 48_000, Role::Once).unwrap();
        let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((0.08..0.12).contains(&peak), "{peak}");
    }

    #[test]
    fn long_files_are_cut_by_role_and_a_ringtone_file_rests_before_it_repeats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.wav");
        std::fs::write(&path, sine_wav(8_000, 40.0, 0.3)).unwrap();
        let secs = |role| load_file(&path, 8_000, role).unwrap().len() as f64 / 8_000.0;
        assert_eq!(secs(Role::Once), 10.0);
        assert_eq!(secs(Role::Preview), 6.0);
        assert_eq!(secs(Role::Ring), 31.0, "30 s and a second of rest");
        let ring = load_file(&path, 8_000, Role::Ring).unwrap();
        assert!(ring[ring.len() - 8_000..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_file_that_cannot_be_played_says_why_and_falls_back_to_the_default_sound() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("notes.mp3");
        std::fs::write(&junk, b"this is not audio").unwrap();
        let why = check_file(&junk).unwrap_err();
        assert!(why.contains("WAV, MP3 and M4A"), "{why}");
        assert!(check_file(&dir.path().join("missing.wav")).is_err());
        assert!(check_file(Path::new("/")).is_err(), "a folder");
        let tone = Tone::File(junk, Sound::Message);
        assert_eq!(
            samples(&tone, 48_000, Role::Once),
            synth::synth(Sound::Message, 48_000),
            "plays the default instead of nothing"
        );
    }

    #[test]
    fn the_sound_switches_read_as_teams_keeps_them() {
        assert_eq!(switches_from(None), Switches::default(), "not read yet");
        let mut n = NotificationSettings::default();
        assert_eq!(switches_from(Some(&n)), Switches::default(), "nothing said");
        n.set_sound(SoundField::AlertSound, "Off");
        n.set_sound(SoundField::GlobalSoundAndPreview, "Off");
        n.set_sound(SoundField::CallAndMeetingStart, "Off");
        n.set_sound(SoundField::WhenBusyOrInMeetings, "On");
        n.set_sound(SoundField::UrgentAndPriority, "on");
        assert_eq!(
            switches_from(Some(&n)),
            Switches {
                messages: false,
                global: false,
                calls_and_meetings: false,
                when_busy: true,
                urgent_and_priority: true,
            }
        );
        // Teams' own word for the ring switch is "Ring": that is on.
        n.set_sound(SoundField::CallAndMeetingStart, "Ring");
        assert!(switches_from(Some(&n)).calls_and_meetings);
    }

    #[test]
    fn presence_words_map_to_a_status_and_an_occupied_flag() {
        assert_eq!(status_of("Available"), Status::Available);
        assert_eq!(status_of("AvailableIdle"), Status::Available);
        assert_eq!(status_of("Busy"), Status::Busy);
        assert_eq!(status_of("BusyIdle"), Status::Busy);
        assert_eq!(status_of("DoNotDisturb"), Status::DoNotDisturb);
        assert_eq!(status_of(""), Status::Available, "unknown is not silenced");
        assert_eq!(status_of("Away"), Status::Available);
        assert!(occupied("InACall", false));
        assert!(occupied("InAMeeting", false));
        assert!(occupied("Presenting", false));
        assert!(occupied("", true));
        assert!(!occupied("", false));
        assert!(!occupied("Away", false));
    }

    /// The banner decision for a plain message at `hour`:00 on a Monday, with quiet hours from
    /// 22:00 to 07:00, and whether the message then sounds (everything else allowing it).
    fn sounds_at(hour: u32, focus: bool, inc: Incoming<'_>) -> bool {
        let prefs = GeneralPrefs {
            quiet_hours: QuietHoursPrefs {
                enabled: true,
                ..QuietHoursPrefs::default()
            },
            ..GeneralPrefs::default()
        };
        let now = NaiveDate::from_ymd_opt(2026, 11, 23)
            .and_then(|d| d.and_hms_opt(hour, 0, 0))
            .unwrap();
        let decision = quiet::should_banner(&inc, now, &prefs, focus);
        let facts = policy::Message {
            banner: banner_shown(&decision),
            ..policy::Message::default()
        };
        policy::message_sound(&facts, &Switches::default())
    }

    #[test]
    fn quiet_hours_pause_and_focus_mode_silence_the_message_sound_through_the_banner_decision() {
        let plain = Incoming {
            routed: true,
            ..Incoming::default()
        };
        assert!(sounds_at(12, false, plain), "midday");
        assert!(!sounds_at(23, false, plain), "quiet hours");
        let paused = Incoming {
            paused: true,
            ..plain
        };
        assert!(!sounds_at(12, false, paused), "paused");
        assert!(
            !sounds_at(12, true, plain),
            "Focus mode holds an ordinary message back"
        );
        let mention = Incoming {
            mentions_me: true,
            ..plain
        };
        assert!(sounds_at(12, true, mention), "Focus lets a mention through");
        assert!(!sounds_at(23, true, mention), "but not in quiet hours");
        let muted = Incoming::default(); // not routed: a muted chat or a switched-off type
        assert!(!sounds_at(12, false, muted));
    }

    #[test]
    fn the_burst_gates_hold_one_sound_per_burst_and_do_not_share_state() {
        let messages = Mutex::new(Burst::new(1500));
        assert!(burst_hit(&messages, 1_000));
        assert!(!burst_hit(&messages, 1_400));
        assert!(!burst_hit(&messages, 2_700));
        assert!(burst_hit(&messages, 4_200));
        let reminders = Mutex::new(Burst::new(5000));
        assert!(burst_hit(&reminders, 1_400), "its own clock");
    }

    #[test]
    fn tests_never_make_a_sound() {
        // Both end at the first line, before any thread or device.
        play_once(Tone::Sound(Sound::Message), Role::Once);
        preview(Kind::Message, RingtoneChoice::Teams, None);
        preview(Kind::Ringtone, RingtoneChoice::Chime, None);
        call_cue(true);
    }

    #[cfg(feature = "calls")]
    #[test]
    fn tests_never_ring() {
        assert!(start_ringback().is_none());
        assert!(start_ringtone(RingtoneChoice::Teams, None).is_none());
        assert!(start_ringtone(RingtoneChoice::Custom, Some("/x.wav")).is_none());
    }
}
