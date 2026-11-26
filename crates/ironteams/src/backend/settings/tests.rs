use super::*;
use appearance::{PortalScheme, Theme};
use serde_json::json;

#[test]
fn local_prefs_round_trip_through_their_files() {
    let dir = tempfile_dir("roundtrip");
    let mut l = Local::load(&dir);
    assert_eq!(l.appearance, AppearancePrefs::default());
    l.appearance.theme = Theme::Dark;
    l.general.auto_start = true;
    l.a11y.larger_text = true;
    l.devices.microphone = Some("alsa:pipewire".into());
    l.files.open_office_in = OpenFilesIn::Desktop;
    l.calls.ask_feedback = call_feedback::Ask::Never;
    l.grants.set("app-1", DevicePermission::Media, true);
    l.save_appearance().unwrap();
    l.save_general().unwrap();
    l.save_a11y().unwrap();
    l.save_devices().unwrap();
    l.save_files().unwrap();
    l.save_calls().unwrap();
    l.save_grants().unwrap();
    assert_eq!(Local::load(&dir), l);
    let _ = std::fs::remove_dir_all(&dir);
}

fn tempfile_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ironteams-settings-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn palettes_follow_theme_contrast_and_accent() {
    assert_eq!(palette(ResolvedTheme::Dark, false, None), DARK);
    assert_eq!(palette(ResolvedTheme::Light, false, None), LIGHT);
    assert_eq!(palette(ResolvedTheme::Dark, true, None), CONTRAST_DARK);
    // High contrast keeps its highlights even with an accent.
    assert_eq!(
        palette(ResolvedTheme::Light, true, Some(Rgb::new(1, 2, 3))),
        CONTRAST_LIGHT
    );
    let p = palette(ResolvedTheme::Dark, false, Some(Rgb::new(0x0f, 0x6c, 0xbd)));
    assert_eq!(p.accent_strong, 0x0f6cbd);
    assert_ne!(p.accent, DARK.accent);
    let l = palette(
        ResolvedTheme::Light,
        false,
        Some(Rgb::new(0x0f, 0x6c, 0xbd)),
    );
    assert_eq!(l.accent, 0x0f6cbd);
}

#[test]
fn a_code_block_stands_out_from_the_bubble_it_sits_in() {
    for p in [DARK, LIGHT] {
        assert_ne!(p.code_bg, p.bubble_other);
        assert_ne!(p.code_bg, p.bubble_mine);
    }
}

#[test]
fn look_resolves_system_theme_contrast_motion_and_text_scale() {
    let mut l = Local::load(&tempfile_dir("look"));
    let dark_portal = PortalSettings {
        color_scheme: PortalScheme::Dark,
        animations_enabled: Some(false),
        ..Default::default()
    };
    let lk = look(&l, Some(&dark_portal));
    assert!(lk.dark && lk.reduced_motion && !lk.high_contrast);
    assert!(look(&l, None).dark, "no portal: dark, the built-in look");
    l.appearance.theme = Theme::Dark;
    l.appearance.set_font_scale(1.2);
    l.a11y.larger_text = true;
    l.a11y.high_contrast = AutoSwitch::On;
    let lk = look(&l, None);
    assert!(lk.dark && lk.high_contrast);
    assert!((lk.font_scale - 1.5).abs() < 1e-6);
    assert_eq!(lk.palette, CONTRAST_DARK);
}

#[test]
fn font_steps_land_on_round_values_and_clamp() {
    assert_eq!(step_font_scale(1.0, 1), 1.05);
    assert_eq!(step_font_scale(1.0, -1), 0.95);
    assert_eq!(step_font_scale(1.03, 1), 1.1, "snaps to 1.05, then steps");
    assert_eq!(step_font_scale(2.0, 1), 2.0);
    assert_eq!(step_font_scale(0.75, -1), 0.75);
}

#[test]
fn notify_rows_group_and_read_delivery() {
    let rows = notify_rows(None);
    assert_eq!(rows.len(), Kind::ALL.len());
    assert!(rows.iter().all(|r| r.value.is_empty()));
    assert_eq!(rows[0].group, "Chat");
    assert_eq!(rows[1].group, "");
    assert_eq!(rows[4].group, "Teams and channels");
    let s = NotificationSettings::from_properties(&json!({
        "userPersonalSettings": "{\"toastSettings\":{\"Chat\":false},\"activityGenerationSettings\":{\"Chat\":true}}"
    }));
    assert_eq!(notify_rows(Some(&s))[0].value, "feed");
}

#[test]
fn privacy_rows_are_disabled_until_read() {
    assert!(privacy_rows(None).iter().all(|r| !r.enabled && !r.on));
    let p = PrivacySettings {
        read_receipts: true,
        ..Default::default()
    };
    let rows = privacy_rows(Some(&p));
    assert!(rows.iter().all(|r| r.enabled));
    assert_eq!(rows[0].key, "read-receipts");
    assert!(rows[0].on);
}

#[test]
fn dry_runs_build_the_documented_requests() {
    let r = dry_run_privacy("https://chat.invalid", PrivacyFlag::ReadReceipts, false);
    assert_eq!(
        r.dry_run(),
        "PUT https://chat.invalid/v1/users/ME/properties?name=readReceiptsEnabled\n{\"readReceiptsEnabled\":\"false\"}"
    );
    let cur = NotificationSettings::from_properties(&json!({}));
    let n = dry_run_notify(
        "https://chat.invalid",
        &cur,
        Kind::ChatMessage,
        Delivery::Off,
    );
    assert!(n.url.ends_with("?name=userPersonalSettings"));
    assert!(
        cur.delivery(Kind::ChatMessage).is_none(),
        "current is untouched"
    );
    let u = dry_run_remove("https://mt.invalid", ContactList::Blocked, "8:orgid:x");
    assert_eq!(
        u.dry_run(),
        "POST https://mt.invalid/beta/userSettings/blocklist/manage\n{\"remove\":[\"8:orgid:x\"]}"
    );
}

#[test]
fn find_matches_labels_pages_and_keywords() {
    assert!(find("  ").is_empty());
    let dark: Vec<_> = find("dark").iter().map(|f| f.label).collect();
    assert_eq!(dark, ["Theme"]);
    assert!(find("CAPTIONS").iter().all(|f| f.page == "accessibility"));
    assert!(find("block calls").iter().any(|f| f.page == "privacy"));
    assert!(find("zzz").is_empty());
}

#[test]
fn the_message_style_is_found_on_the_appearance_page() {
    for q in ["message style", "bubble", "flat", "slack"] {
        assert!(
            find(q)
                .iter()
                .any(|f| f.page == "appearance" && f.label == "Message style"),
            "{q}"
        );
    }
}

#[test]
fn what_a_message_shows_is_found_on_the_appearance_page() {
    for (q, label) in [
        ("avatar", "Show avatars"),
        ("photo", "Show avatars"),
        ("timestamp", "Show time stamps"),
        ("reactions", "Show reactions"),
    ] {
        assert!(
            find(q)
                .iter()
                .any(|f| f.page == "appearance" && f.label == label),
            "{q}"
        );
    }
}

#[test]
fn the_graphics_setting_is_found_on_the_general_page() {
    for q in ["graphics", "gpu", "cpu", "memory"] {
        assert!(
            find(q)
                .iter()
                .any(|f| f.page == "general" && f.label == "Graphics"),
            "{q}"
        );
    }
}

#[test]
fn the_chats_page_is_listed_after_general_and_its_settings_are_found() {
    assert_eq!(page_title("chats"), "Chats and channels");
    let keys: Vec<_> = PAGES.iter().map(|(k, _)| *k).collect();
    assert_eq!(&keys[..2], ["general", "chats"]);
    let on_chats = |q: &str| {
        find(q)
            .into_iter()
            .filter(|f| f.page == "chats")
            .map(|f| f.label)
            .collect::<Vec<_>>()
    };
    assert_eq!(on_chats("link preview"), ["Link previews"]);
    assert_eq!(
        on_chats("when i open a chat"),
        ["When I open a chat take me to"]
    );
    assert_eq!(on_chats("enter"), ["Press Enter to send"]);
    assert!(on_chats("read receipts").contains(&"Read receipts"));
}

#[test]
fn the_storage_page_is_listed_and_its_settings_are_found() {
    assert_eq!(page_title("storage"), "Storage");
    assert!(PAGES.iter().any(|(k, _)| *k == "storage"));
    let on_storage = |q: &str| {
        find(q)
            .into_iter()
            .filter(|f| f.page == "storage")
            .map(|f| f.label)
            .collect::<Vec<_>>()
    };
    assert_eq!(on_storage("media cache limit"), ["Media cache limit"]);
    assert_eq!(on_storage("backup"), ["Delete old update backups"]);
    assert!(on_storage("free up space").contains(&"Clear all cached data"));
    assert!(on_storage("disk").len() >= 4);
}

#[test]
fn the_keyboard_shortcuts_page_is_listed_and_its_settings_are_found() {
    assert_eq!(page_title("shortcuts"), "Keyboard shortcuts");
    assert!(PAGES.iter().any(|(k, _)| *k == "shortcuts"));
    let on_shortcuts = |q: &str| {
        find(q)
            .into_iter()
            .filter(|f| f.page == "shortcuts")
            .map(|f| f.label)
            .collect::<Vec<_>>()
    };
    assert_eq!(on_shortcuts("rebind"), ["Change a keyboard shortcut"]);
    assert!(on_shortcuts("hotkey").contains(&"Reset all shortcuts"));
    // The page's title is part of what a search matches, so both settings come up.
    assert_eq!(on_shortcuts("keyboard").len(), 2);
}

#[test]
fn raw_alsa_entries_hide_unless_asked_or_chosen() {
    let d = |id: &str, def: bool| devices::AudioDevice {
        id: id.into(),
        name: id.into(),
        is_default: def,
        channels: 2,
        sample_rates: vec![48_000],
    };
    let list = [
        d("alsa:default", true),
        d("alsa:pipewire", false),
        d("alsa:hw:CARD=0,DEV=0", false),
        d("alsa:front:CARD=PCH,DEV=0", false),
    ];
    let ids = |v: Vec<DeviceRow>| v.into_iter().map(|r| r.0).collect::<Vec<_>>();
    assert_eq!(
        ids(audio_rows(&list, false, None)),
        ["alsa:default", "alsa:pipewire"]
    );
    assert_eq!(
        audio_rows(&list, false, None)[0].1,
        "alsa:default (default)"
    );
    assert_eq!(audio_rows(&list, true, None).len(), 4);
    assert_eq!(
        audio_rows(&list, false, Some("alsa:hw:CARD=0,DEV=0")).len(),
        3
    );
}

#[test]
fn org_rows_mark_the_home_org_active_for_organizations() {
    let mut cfg = session::Config::default();
    cfg.normalize();
    let t = |id: &str, home: bool| Tenant {
        id: id.into(),
        name: format!("Org {id}"),
        domain: None,
        is_home: home,
        invitation_redeemed: true,
        user_type: None,
    };
    let live = [t("home-id", true), t("guest-id", false)];
    let rows = org_rows(&cfg, Some(&live));
    assert!(rows[0].active && !rows[1].active);
    cfg.switch_tenant("guest-id").unwrap();
    let rows = org_rows(&cfg, Some(&live));
    assert!(!rows[0].active && rows[1].active);
    assert!(org_rows(&cfg, None).is_empty(), "nothing cached yet");
}

#[test]
fn permission_rows_merge_teams_asks_with_saved_grants() {
    let mut g = Grants::default();
    g.set("app-b", DevicePermission::Geolocation, true);
    let asks = [AppAsk {
        id: "app-a".into(),
        name: "Alpha".into(),
        requested: vec![DevicePermission::Media],
    }];
    let rows = permission_rows(&asks, &g, &g.to_json());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, "Alpha");
    assert_eq!(rows[0].2, [(DevicePermission::Media, false)]);
    assert_eq!(rows[1].0, "app-b");
    assert_eq!(rows[1].2, [(DevicePermission::Geolocation, true)]);
}

// ---- which events notify (chat plan 9.1) ----

fn mention(kind: MentionKind, mri: &str) -> Mention {
    Mention {
        item_id: 0,
        mri: mri.into(),
        display_name: "x".into(),
        kind,
    }
}

const CHAT: &str = "19:abc@thread.v2";
const CHANNEL: &str = "19:abc@thread.tacv2";

#[test]
fn incoming_messages_map_to_the_notification_types() {
    let me = "8:orgid:me";
    let kind = |c, reply, m: &[Mention]| incoming_kind(c, reply, m, me);
    assert_eq!(kind(CHAT, false, &[]), Kind::ChatMessage);
    assert_eq!(kind("19:a_b@unq.gbl.spaces", false, &[]), Kind::ChatMessage);
    assert_eq!(
        kind(CHAT, false, &[mention(MentionKind::Person, me)]),
        Kind::ChatMention
    );
    assert_eq!(
        kind(
            CHAT,
            false,
            &[mention(MentionKind::Person, "8:orgid:other")]
        ),
        Kind::ChatMessage,
        "someone else's mention is a plain chat message"
    );
    assert_eq!(
        kind(CHAT, false, &[mention(MentionKind::Everyone, CHAT)]),
        Kind::ChatEveryoneMention
    );
    assert_eq!(kind(CHANNEL, false, &[]), Kind::ChannelPost);
    assert_eq!(kind(CHANNEL, true, &[]), Kind::ChannelReply);
    assert_eq!(
        kind(CHANNEL, true, &[mention(MentionKind::Person, me)]),
        Kind::PersonMention,
        "a mention of me beats the reply type"
    );
    assert_eq!(
        kind(CHANNEL, false, &[mention(MentionKind::Channel, CHANNEL)]),
        Kind::ChannelMention
    );
    assert_eq!(
        kind(CHANNEL, false, &[mention(MentionKind::Team, "19:t")]),
        Kind::TeamMention
    );
    assert_eq!(
        kind(CHANNEL, false, &[mention(MentionKind::Tag, "tag:1")]),
        Kind::TagMention
    );
}

fn prefs(json: serde_json::Value) -> NotificationSettings {
    NotificationSettings::from_properties(&json!({ "userPersonalSettings": json.to_string() }))
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-11-19T12:00:00Z")
        .unwrap()
        .into()
}

fn shows(b: Banner) -> bool {
    matches!(b, Banner::Show { .. })
}

#[test]
fn without_settings_every_event_notifies_as_before() {
    for k in Kind::ALL {
        assert_eq!(
            banner_decision(None, k, false, now()),
            Banner::Show { preview: true }
        );
    }
    // A type Teams has no value for also notifies.
    let n = prefs(json!({"toastSettings": {"Chat": false}}));
    assert!(shows(banner_decision(Some(&n), Kind::Reply, false, now())));
}

#[test]
fn each_types_own_switch_decides() {
    let n = prefs(json!({
        "toastSettings": {"Chat": true, "Mention_Person": true, "Like": false,
                          "Visible_ChannelNewMessage_v2": false},
        "activityGenerationSettings": {"Chat": true, "Mention_Person": true, "Like": true,
                                       "Visible_ChannelNewMessage_v2": false},
    }));
    let d = |k| banner_decision(Some(&n), k, false, now());
    assert!(shows(d(Kind::ChatMessage)), "banner and feed");
    assert!(shows(d(Kind::PersonMention)));
    assert_eq!(d(Kind::Like), Banner::Hidden(Hidden::Delivery), "feed only");
    assert_eq!(
        d(Kind::ChannelPost),
        Banner::Hidden(Hidden::Delivery),
        "off"
    );
}

#[test]
fn mute_and_pause_beat_every_type() {
    let n = prefs(json!({
        "toastSettings": {"Chat": true, "temporarilyPauseUntil": "2026-11-19T13:00:00.000Z"},
        "activityGenerationSettings": {"Chat": true},
    }));
    assert_eq!(
        banner_decision(Some(&n), Kind::ChatMessage, false, now()),
        Banner::Hidden(Hidden::Paused)
    );
    // The pause is over one hour later.
    let later = now() + chrono::Duration::hours(2);
    assert!(shows(banner_decision(
        Some(&n),
        Kind::ChatMessage,
        false,
        later
    )));
    // The tray's mute wins, with or without settings.
    assert_eq!(
        banner_decision(Some(&n), Kind::ChatMessage, true, later),
        Banner::Hidden(Hidden::Muted)
    );
    assert_eq!(
        banner_decision(None, Kind::ChatMessage, true, now()),
        Banner::Hidden(Hidden::Muted)
    );
    // An unreadable end time is not a pause.
    let bad = prefs(json!({"toastSettings": {"temporarilyPauseUntil": "soon"}}));
    assert!(shows(banner_decision(
        Some(&bad),
        Kind::ChatMessage,
        false,
        now()
    )));
}

#[test]
fn the_preview_switch_only_changes_the_text() {
    let off = prefs(json!({"toastSettings": {"ShowPreviewInToasts": false}}));
    assert_eq!(
        banner_decision(Some(&off), Kind::ChatMessage, false, now()),
        Banner::Show { preview: false }
    );
    let on = prefs(json!({"toastSettings": {"ShowPreviewInToasts": true}}));
    assert_eq!(
        banner_decision(Some(&on), Kind::ChatMessage, false, now()),
        Banner::Show { preview: true }
    );
    assert_eq!(banner_body(true, "hello".into()), "hello");
    assert_eq!(banner_body(false, "hello".into()), "New message");
}

#[test]
fn call_quality_questions_are_asked_sometimes_until_another_choice_is_saved() {
    assert_eq!(
        CallsPrefs::default().ask_feedback,
        call_feedback::Ask::Sometimes
    );
    let p: CallsPrefs = serde_json::from_str("{}").unwrap();
    assert_eq!(p.ask_feedback, call_feedback::Ask::Sometimes);
    let p: CallsPrefs = serde_json::from_str(r#"{"ask_feedback":"always"}"#).unwrap();
    assert_eq!(p.ask_feedback, call_feedback::Ask::Always);
}

#[test]
fn the_ringtone_defaults_to_the_default_sound_and_survives_a_save() {
    use super::super::ringtone::RingtoneChoice;
    assert_eq!(CallsPrefs::default().ringtone, RingtoneChoice::Teams);
    assert_eq!(CallsPrefs::default().ringtone.slug(), "default");
    // New users, and a file that never saved a ringtone, get the new default.
    let p: CallsPrefs = serde_json::from_str("{}").unwrap();
    assert_eq!(p.ringtone, RingtoneChoice::Teams);
    let p: CallsPrefs = serde_json::from_str(r#"{"ask_feedback":"never"}"#).unwrap();
    assert_eq!(p.ringtone, RingtoneChoice::Teams, "an old file has none");
    assert_eq!(p.ringtone_file, None);
    let p = CallsPrefs {
        ringtone: RingtoneChoice::Pulse,
        ..Default::default()
    };
    let json = serde_json::to_string(&p).unwrap();
    assert!(json.contains("\"pulse\""), "{json}");
    assert_eq!(serde_json::from_str::<CallsPrefs>(&json).unwrap(), p);
}

#[test]
fn a_saved_ringtone_choice_is_kept_whatever_it_is() {
    use super::super::ringtone::RingtoneChoice;
    for (saved, want) in [
        ("chime", RingtoneChoice::Chime),
        ("soft", RingtoneChoice::Soft),
        ("pulse", RingtoneChoice::Pulse),
        ("default", RingtoneChoice::Teams),
        ("ironteams", RingtoneChoice::IronTeams),
        ("custom", RingtoneChoice::Custom),
    ] {
        let json = format!(r#"{{"ringtone":"{saved}"}}"#);
        let p: CallsPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(p.ringtone, want, "{saved}");
    }
    let json = r#"{"ringtone":"custom","ringtone_file":"/a/b.wav"}"#;
    let p: CallsPrefs = serde_json::from_str(json).unwrap();
    assert_eq!(p.ringtone_file.as_deref(), Some("/a/b.wav"));
}

#[test]
fn a_file_that_says_default_moves_to_the_teams_sounds_and_the_other_choices_stay() {
    use super::super::ringtone::RingtoneChoice;
    use super::super::sounds::SoundPrefs;
    // "default" was IronTeams's own sound before the Teams originals: it now names those.
    let p: SoundPrefs = serde_json::from_str(r#"{"message":"default"}"#).unwrap();
    assert_eq!(p.message, RingtoneChoice::Teams);
    let p: CallsPrefs = serde_json::from_str(r#"{"ringtone":"default"}"#).unwrap();
    assert_eq!(p.ringtone, RingtoneChoice::Teams);
    for (saved, want) in [
        ("chime", RingtoneChoice::Chime),
        ("soft", RingtoneChoice::Soft),
        ("pulse", RingtoneChoice::Pulse),
        ("custom", RingtoneChoice::Custom),
        ("ironteams", RingtoneChoice::IronTeams),
    ] {
        let p: SoundPrefs = serde_json::from_str(&format!(r#"{{"message":"{saved}"}}"#)).unwrap();
        assert_eq!(p.message, want, "{saved}");
        let p: CallsPrefs = serde_json::from_str(&format!(r#"{{"ringtone":"{saved}"}}"#)).unwrap();
        assert_eq!(p.ringtone, want, "{saved}");
    }
    // One of Teams' other sounds is kept too, and so is the urgent choice.
    let p: SoundPrefs = serde_json::from_str(r#"{"message":"vibe","urgent":"escalate"}"#).unwrap();
    assert_eq!(p.message.slug(), "vibe");
    assert_eq!(p.urgent.slug(), "escalate");
    let p: CallsPrefs = serde_json::from_str(r#"{"ringtone":"wish-granted"}"#).unwrap();
    assert_eq!(p.ringtone.slug(), "wish-granted");
    // The download switch is on in every file that never said, and a saved off is kept.
    let old: SoundPrefs = serde_json::from_str(r#"{"message":"soft"}"#).unwrap();
    assert!(old.download_teams_sounds);
    let off = SoundPrefs {
        download_teams_sounds: false,
        ..SoundPrefs::default()
    };
    let json = serde_json::to_string(&off).unwrap();
    assert_eq!(serde_json::from_str::<SoundPrefs>(&json).unwrap(), off);
}

#[test]
fn the_message_sound_defaults_to_the_default_and_a_saved_choice_is_kept() {
    use super::super::ringtone::RingtoneChoice;
    use super::super::sounds::SoundPrefs;
    assert_eq!(SoundPrefs::default().message, RingtoneChoice::Teams);
    let p: SoundPrefs = serde_json::from_str("{}").unwrap();
    assert_eq!(p.message, RingtoneChoice::Teams);
    assert_eq!(p.message_file, None);
    let p: SoundPrefs = serde_json::from_str(r#"{"message":"soft"}"#).unwrap();
    assert_eq!(p.message, RingtoneChoice::Soft);
    // Saved through Local, read back by a fresh load.
    let dir = tempfile_dir("sounds");
    let mut l = Local::load(&dir);
    assert_eq!(l.sounds.message, RingtoneChoice::Teams, "no file yet");
    l.sounds.message = RingtoneChoice::Custom;
    l.sounds.message_file = Some("/a/ping.mp3".into());
    l.save_sounds().unwrap();
    assert_eq!(Local::load(&dir).sounds, l.sounds);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- files and app grants (X1.6, X1.7) ----

#[test]
fn office_files_open_in_the_ironteams_preview_until_another_choice_is_saved() {
    assert_eq!(FilesPrefs::default().open_office_in, OpenFilesIn::IronTeams);
    let p: FilesPrefs = serde_json::from_str("{}").unwrap();
    assert_eq!(p.open_office_in, OpenFilesIn::IronTeams);
    let p: FilesPrefs = serde_json::from_str(r#"{"open_office_in":"kelma"}"#).unwrap();
    assert_eq!(p.open_office_in, OpenFilesIn::IronTeams);
    let p: FilesPrefs = serde_json::from_str(r#"{"open_office_in":"desktop"}"#).unwrap();
    assert_eq!(p.open_office_in, OpenFilesIn::Desktop);
}

#[test]
fn settings_write_the_grants_file_the_web_host_reads() {
    assert_eq!(PERMISSIONS_FILE, "app-grants.json");
    let dir = tempfile_dir("grants");
    let mut l = Local::load(&dir);
    l.grants.set("app-1", DevicePermission::Media, true);
    l.grants.set("app-1", DevicePermission::Geolocation, false);
    l.save_grants().unwrap();
    // What the web host does with the file.
    let text = std::fs::read_to_string(dir.join("app-grants.json")).unwrap();
    let g = Grants::from_json(&serde_json::from_str(&text).unwrap());
    assert!(g.allowed("app-1", DevicePermission::Media));
    assert!(!g.allowed("app-1", DevicePermission::Geolocation));
    assert!(!g.allowed("app-2", DevicePermission::Media));
    // And back into Settings.
    assert_eq!(Local::load(&dir).grants, l.grants);
}

#[test]
fn a_grant_change_keeps_what_the_store_saved_since_settings_loaded() {
    let dir = tempfile_dir("grant-rmw");
    let mut l = Local::load(&dir);
    // The Store grants another app after Settings read the (empty) file.
    let mut store = Grants::default();
    store.set("store-app", DevicePermission::Media, true);
    write_json(&dir.join(PERMISSIONS_FILE), &store.to_json()).unwrap();
    l.set_grant("settings-app", DevicePermission::Geolocation, true)
        .unwrap();
    let text = std::fs::read_to_string(dir.join(PERMISSIONS_FILE)).unwrap();
    let saved = Grants::from_json(&serde_json::from_str(&text).unwrap());
    assert_eq!(saved.to_json(), l.grants.to_json());
    assert!(saved.allowed("store-app", DevicePermission::Media));
    assert!(saved.allowed("settings-app", DevicePermission::Geolocation));
}

#[test]
fn grants_from_the_first_settings_build_are_still_read_once() {
    let dir = tempfile_dir("grants-legacy");
    let mut g = Grants::default();
    g.set("old-app", DevicePermission::Media, true);
    write_json(&dir.join("app-permissions.json"), &g.to_json()).unwrap();
    assert_eq!(Local::load(&dir).grants, g);
    // Once the new file exists it wins.
    write_json(&dir.join("app-grants.json"), &Grants::default().to_json()).unwrap();
    assert_eq!(Local::load(&dir).grants, Grants::default());
}
