//! One account in the call on two devices: "Add this device" joins with the mic and the sound
//! off, the window says we are on another device too, and the roster lists us once.

use super::*;

fn me(elsewhere: bool) -> Person {
    Person {
        name: "Me".into(),
        me: true,
        elsewhere,
        ..Default::default()
    }
}

fn ada() -> Person {
    Person {
        name: "Ada".into(),
        ..Default::default()
    }
}

#[test]
fn add_this_device_joins_with_the_mic_and_the_sound_off() {
    let mut m = ready();
    assert!(m.mic_on && !m.sound_off);
    assert!(m.add_device());
    let (_, opts) = m.joining().unwrap();
    assert!(!opts.mic_on, "the other device is heard, not this one");
    assert!(opts.sound_off, "no echo from two speakers in one room");
    assert!(m.muted);
    let d = m.data(Instant::now());
    assert!(d.muted && d.sound_off);
}

#[test]
fn add_this_device_is_only_on_the_pre_join_screen() {
    let mut m = ready();
    m.joining();
    assert!(!m.add_device());
    assert!(!m.sound_off);
}

#[test]
fn a_plain_join_keeps_the_sound_on() {
    let mut m = ready();
    let (_, opts) = m.joining().unwrap();
    assert!(!opts.sound_off);
    assert!(!m.data(Instant::now()).sound_off);
}

#[test]
fn the_sound_turns_back_on_only_in_a_call() {
    let mut m = ready();
    assert_eq!(m.toggle_sound(), None, "nothing to hear before joining");
    m.add_device();
    m.joining();
    assert_eq!(m.toggle_sound(), Some(false), "on again");
    assert!(!m.data(Instant::now()).sound_off);
    assert_eq!(m.toggle_sound(), Some(true), "and off");
    assert!(m.data(Instant::now()).sound_off);
}

#[test]
fn a_new_pre_join_screen_starts_with_the_sound_on() {
    let t0 = Instant::now();
    let mut m = ready();
    m.add_device();
    m.joining();
    m.leave(t0);
    m.prejoin(
        meeting(),
        JoinTarget::Link("l".into()),
        "19:c".into(),
        devices(),
    )
    .unwrap();
    assert!(m.mic_on && !m.sound_off);
}

#[test]
fn the_window_says_when_we_are_on_another_device_too() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    m.apply(Event::Roster(vec![me(false), ada()]), t0);
    assert!(!m.data(t0).other_device);

    m.apply(Event::Roster(vec![me(true), ada()]), t0);
    let d = m.data(t0);
    assert!(d.other_device);
    assert_eq!(d.in_call, 2, "we are counted once");
    assert_eq!(d.people.iter().filter(|p| p.me).count(), 1);

    // The other device left.
    m.apply(Event::Roster(vec![me(false), ada()]), t0);
    assert!(!m.data(t0).other_device);
}

#[test]
fn someone_else_on_two_devices_is_not_us_on_another_device() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    let ada_twice = Person {
        elsewhere: true,
        ..ada()
    };
    m.apply(Event::Roster(vec![me(false), ada_twice]), t0);
    assert!(!m.data(t0).other_device);
}

#[test]
fn the_banner_goes_with_the_call() {
    let t0 = Instant::now();
    let mut m = ready();
    m.joining();
    m.apply(Event::Connected, t0);
    m.apply(Event::Roster(vec![me(true), ada()]), t0);
    m.leave(t0);
    assert!(!m.data(t0).other_device);
}

#[test]
fn the_commands_parse() {
    assert_eq!(CallCmd::parse("join-device", ""), Some(CallCmd::JoinDevice));
    assert_eq!(CallCmd::parse("sound", ""), Some(CallCmd::ToggleSound));
}

#[test]
fn dont_use_audio_joins_with_the_mic_and_the_sound_off() {
    let mut m = ready();
    assert!(m.data(Instant::now()).prejoin_audio, "Computer audio is the default");
    m.prejoin_audio(false);
    let d = m.data(Instant::now());
    assert!(!d.prejoin_audio && !d.prejoin_mic);
    let (_, opts) = m.joining().unwrap();
    assert!(!opts.mic_on && opts.sound_off);
}

#[test]
fn computer_audio_again_brings_the_sound_back_and_leaves_the_mic_off() {
    let mut m = ready();
    m.prejoin_audio(false);
    m.prejoin_audio(true);
    let d = m.data(Instant::now());
    assert!(d.prejoin_audio);
    assert!(!d.prejoin_mic, "the mic stays as Don't use audio left it; its switch is back");
    let (_, opts) = m.joining().unwrap();
    assert!(!opts.sound_off);
}

#[test]
fn the_audio_choice_is_only_on_the_pre_join_screen() {
    let mut m = ready();
    m.joining();
    m.prejoin_audio(false);
    assert!(!m.sound_off, "a running call's sound has its own toggle");
}

#[test]
fn the_audio_choice_parses() {
    assert_eq!(
        CallCmd::parse("prejoin-audio", "none"),
        Some(CallCmd::PrejoinAudio(false))
    );
    assert_eq!(
        CallCmd::parse("prejoin-audio", "computer"),
        Some(CallCmd::PrejoinAudio(true))
    );
}
