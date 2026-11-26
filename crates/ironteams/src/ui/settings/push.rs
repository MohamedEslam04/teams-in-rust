use super::*;

/// The portal state at start, so the first frame already has the right theme.
pub(super) fn read_portal() -> Option<PortalSettings> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_millis(400), appearance::portal::read())
            .await
            .ok()?
            .inspect_err(|e| tracing::debug!(%e, "settings: no portal"))
            .ok()
            .flatten()
    })
}

/// Follows portal changes (theme, accent, contrast, animations) on a thread of its own.
pub(super) fn watch_portal(weak: slint::Weak<AppWindow>) {
    std::thread::Builder::new()
        .name("ironteams-portal".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                use futures_util::StreamExt;
                let Ok(Some(mut stream)) = appearance::portal::watch().await else {
                    return;
                };
                while let Some(change) = stream.next().await {
                    let _ = weak.upgrade_in_event_loop(move |w| portal_changed(&w, change));
                }
            });
        })
        .ok();
}

pub(super) fn portal_changed(w: &AppWindow, change: PortalChange) {
    let Some(shared) = ctl() else { return };
    let mut c = shared.borrow_mut();
    c.portal.get_or_insert_with(Default::default).apply(change);
    apply_look(w, &c);
}

/// Sets the `Theme` global from the preferences and the portal.
pub(super) fn apply_look(w: &AppWindow, c: &Ctl) {
    let lk: Look = st::look(&c.local, c.portal.as_ref());
    let t = w.global::<Theme>();
    let p = lk.palette;
    t.set_rail(rgb(p.rail));
    t.set_sidebar(rgb(p.sidebar));
    t.set_main(rgb(p.main));
    t.set_topbar(rgb(p.topbar));
    t.set_surface(rgb(p.surface));
    t.set_surface_hover(rgb(p.surface_hover));
    t.set_selected(rgb(p.selected));
    t.set_border(rgb(p.border));
    t.set_accent(rgb(p.accent));
    t.set_accent_strong(rgb(p.accent_strong));
    t.set_fg(rgb(p.fg));
    t.set_fg_muted(rgb(p.fg_muted));
    t.set_fg_subtle(rgb(p.fg_subtle));
    t.set_bubble_mine(rgb(p.bubble_mine));
    t.set_bubble_other(rgb(p.bubble_other));
    t.set_mention(rgb(p.mention));
    t.set_code_bg(rgb(p.code_bg));
    // Rows drawn from now on show mentions in this colour (`markdown::spans_md`).
    crate::markdown::set_mention_color(p.mention);
    t.set_danger(rgb(p.danger));
    t.set_unread_badge(rgb(p.unread_badge));
    t.set_dark(lk.dark);
    t.set_font_scale(lk.font_scale);
    t.set_compact(lk.compact);
    t.set_high_contrast(lk.high_contrast);
    t.set_reduced_motion(lk.reduced_motion);

    let g = w.global::<SettingsStore>();
    g.set_theme_shown(s(if lk.dark { "dark" } else { "light" }));
    g.set_accent_shown(s(match (c.local.appearance.accent, lk.accent) {
        (Some(a), _) => a.to_hex(),
        (None, Some(a)) => format!("desktop accent {}", a.to_hex()),
        (None, None) => "IronTeams purple".into(),
    }));
    g.set_a11y_shown(s(format!(
        "high contrast {}, reduced motion {}, text {}%",
        if lk.high_contrast { "on" } else { "off" },
        if lk.reduced_motion { "on" } else { "off" },
        (lk.font_scale * 100.0).round()
    )));
}

/// Hands the general preferences to what reads them outside Settings: the date and time helpers
/// and the tray's close-to-tray decision, plus the interface language (translation and direction).
pub(super) fn apply_general(g: &appearance::GeneralPrefs) {
    appearance::formats::set_from(g);
    crate::language::apply(g);
    crate::tray_wiring::set_keep_running(g.keep_running_on_close);
    st::set_link_previews(g.link_previews);
}

pub(super) fn toggles(rows: Vec<st::ToggleRow>) -> ModelRc<SToggle> {
    model(
        rows.into_iter()
            .map(|r| SToggle {
                key: r.key.into(),
                label: r.label.into(),
                hint: r.hint.into(),
                on: r.on,
                enabled: r.enabled,
            })
            .collect(),
    )
}

/// Pushes every local preference into `SettingsStore`.
pub(super) fn push_local(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let l = &c.local;
    let a = &l.appearance;
    g.set_theme(s(match a.theme {
        ThemeChoice::Light => "light",
        ThemeChoice::Dark => "dark",
        ThemeChoice::System => "system",
    }));
    g.set_accent(s(a.accent.map(Rgb::to_hex).unwrap_or_default()));
    g.set_density(s(match a.density {
        Density::Comfy => "comfy",
        Density::Compact => "compact",
    }));
    g.set_font_scale(a.font_scale);
    g.set_skin_tone(w.get_skin_tone());

    let gp = &l.general;
    let system = appearance::system_language()
        .map(|t| appearance::general::best_match(&t).native_name)
        .unwrap_or("English");
    let mut langs = vec![SOption {
        v: s(""),
        l: s(format!("System ({system})")),
    }];
    langs.extend(appearance::available_languages().iter().map(|l| SOption {
        v: s(l.tag),
        l: s(l.native_name),
    }));
    g.set_languages(model(langs));
    g.set_language(s(gp.language.clone().unwrap_or_default()));
    g.set_language_shown(s(appearance::effective_language(gp).english_name));
    g.set_date_format(s(st::date_slug(gp.date_format)));
    g.set_time_format(s(st::time_slug(gp.time_format)));
    w.global::<Theme>()
        .set_h24(gp.time_format == appearance::TimeFormat::H24);
    g.set_time_zone(s(gp.time_zone.clone().unwrap_or_default()));
    g.set_system_time_zone(s(iana_time_zone::get_timezone().unwrap_or_default()));
    g.set_startup(toggles(st::startup_rows(gp)));
    let started = crate::renderer::running();
    g.set_renderer(s(gp.renderer.slug()));
    g.set_renderer_running(s(started.running.slug()));
    g.set_renderer_fallback(started.fell_back());
    // What the next start will use differs from what this start asked for.
    g.set_renderer_restart(crate::renderer::wanted(gp.renderer) != started.wanted);
    g.set_self_update(c.env.hooks.updates.is_some());
    g.set_beta_updates(gp.beta_updates);
    g.set_banner_photo(gp.banner_photo);
    g.set_notification_style(s(match gp.notification_style {
        appearance::NotificationStyle::Cards => "cards",
        appearance::NotificationStyle::System => "system",
    }));
    g.set_banner_actions(gp.banner_actions);
    g.set_check_spelling(gp.check_spelling);
    if gp.check_spelling {
        crate::spell::ensure_loaded(w);
    }
    show_focus(w, crate::support::quiet::focus_until(gp));
    let q = &gp.quiet_hours;
    g.set_quiet_on(q.enabled);
    g.set_quiet_days(model(q.days.to_vec()));
    g.set_quiet_start(s(appearance::formats::clock(i32::from(q.start_min))));
    g.set_quiet_end(s(appearance::formats::clock(i32::from(q.end_min))));
    g.set_focus_duration(s(focus_slug(gp.focus_duration)));
    g.set_keywords(model(
        gp.keyword_alerts.iter().map(|k| s(k.as_str())).collect(),
    ));
    g.set_link_previews(gp.link_previews);
    g.set_wheel_speed(appearance::general::clamp_wheel_speed(gp.wheel_speed));
    g.set_open_chat_at(s(match c.open_chat_at {
        OpenChatAt::FirstUnread => "last-read",
        OpenChatAt::Newest => "newest",
    }));

    let x = &l.a11y;
    g.set_high_contrast(s(st::switch_slug(x.high_contrast)));
    g.set_reduced_motion(s(st::switch_slug(x.reduced_motion)));
    g.set_larger_text(x.larger_text);
    g.set_a11y_flags(toggles(st::a11y_rows(x)));

    g.set_mic(s(l.devices.microphone.clone().unwrap_or_default()));
    g.set_speaker(s(l.devices.speaker.clone().unwrap_or_default()));
    g.set_camera(s(l.devices.camera.clone().unwrap_or_default()));
    g.set_noise_suppression(l.devices.noise_suppression);
    g.set_echo_cancellation(l.devices.echo_cancellation);
    g.set_speaker_volume(f32::from(l.devices.speaker_volume));
    g.set_open_files(s(l.files.open_office_in.slug()));
    g.set_call_feedback(s(l.calls.ask_feedback.slug()));
    g.set_ringtone(s(l.calls.ringtone.slug()));
    g.set_ringtone_file(s(file_label(l.calls.ringtone_file.as_deref())));
    g.set_message_sound(s(l.sounds.message.slug()));
    g.set_message_sound_file(s(file_label(l.sounds.message_file.as_deref())));
    g.set_urgent_sound(s(l.sounds.urgent.slug()));
    g.set_download_teams_sounds(l.sounds.download_teams_sounds);
    // Teams' own sounds, from the one table.
    let originals = |slot| {
        model(
            sounds::teams_assets::of_slot(slot)
                .map(|o| SOption {
                    v: s(o.slug),
                    l: s(o.label),
                })
                .collect::<Vec<_>>(),
        )
    };
    g.set_ringtone_originals(originals(Slot::Ring));
    g.set_message_originals(originals(Slot::Message));
    g.set_urgent_originals(originals(Slot::Urgent));
    push_devices(w, c);
    push_perms(w, c);
}

pub(super) fn push_devices(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let all = g.get_all_devices();
    let rows = |v: Vec<st::DeviceRow>| {
        model(
            v.into_iter()
                .map(|(id, name)| SDevice {
                    id: id.into(),
                    name: name.into(),
                })
                .collect(),
        )
    };
    match &c.lists {
        Some(l) => {
            let d = &c.local.devices;
            // A saved device that is not in the list (an id from an older version, an unplugged
            // one) is what calls use: the default. Show that one checked.
            let id =
                |d: Option<&devices::AudioDevice>| s(d.map(|d| d.id.clone()).unwrap_or_default());
            g.set_mic(id(d.find_microphone(&l.inputs)));
            g.set_speaker(id(d.find_speaker(&l.outputs)));
            g.set_mics(rows(st::audio_rows(
                &l.inputs,
                all,
                d.microphone.as_deref(),
            )));
            g.set_speakers(rows(st::audio_rows(&l.outputs, all, d.speaker.as_deref())));
            g.set_cameras(rows(st::camera_rows(&l.cameras)));
            g.set_devices_status(s(l.error.clone().unwrap_or_default()));
        }
        None => g.set_devices_status(s(if c.listing {
            "Looking for devices…"
        } else {
            ""
        })),
    }
}

pub(super) fn push_perms(w: &AppWindow, c: &Ctl) {
    let asks = c
        .server
        .as_ref()
        .map(|d| d.apps.clone())
        .unwrap_or_default();
    let saved = c.local.grants.to_json();
    let rows = st::permission_rows(&asks, &c.local.grants, &saved);
    w.global::<SettingsStore>().set_app_perms(model(
        rows.into_iter()
            .map(|(app, name, perms)| SAppPerm {
                app: app.into(),
                name: name.into(),
                perms: toggles(
                    perms
                        .into_iter()
                        .map(|(p, on)| st::ToggleRow {
                            key: p.as_str().into(),
                            label: st::permission_label(p).into(),
                            hint: String::new(),
                            on,
                            enabled: true,
                        })
                        .collect(),
                ),
            })
            .collect(),
    ));
}

/// Pushes what was read from Teams (or that nothing was).
pub(super) fn push_server(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let d = c.server.as_ref();
    let n = d.and_then(|d| d.notifications.as_ref());
    g.set_notify(model(
        st::notify_rows(n)
            .into_iter()
            .map(|r| SNotify {
                key: r.key.into(),
                label: r.label.into(),
                group: r.group.into(),
                value: r.value.into(),
            })
            .collect(),
    ));
    g.set_paused(n.is_some_and(|n| n.paused_until().is_some()));
    g.set_pause_until(s(n
        .and_then(|n| n.paused_until())
        .and_then(teams_proto::time::parse_iso_ms)
        .and_then(appearance::formats::local)
        .map(|t| {
            format!(
                "Paused until {} {}",
                t.format(appearance::formats::date_pattern(false)),
                appearance::formats::time_of(&t)
            )
        })
        .unwrap_or_default()));
    g.set_notify_sounds(toggles(st::sound_rows(n)));
    g.set_meeting_chat(s(st::meeting_chat_slug(n)));
    let status = match d {
        None => "Reading your Teams settings…".to_string(),
        Some(d) if d.offline => "Offline: your Teams settings were not read.".to_string(),
        Some(d) => d.errors.join(" "),
    };
    g.set_notify_status(s(if n.is_none() || d.is_some_and(|d| d.offline) {
        status.clone()
    } else {
        String::new()
    }));
    let p = d.and_then(|d| d.privacy.as_ref());
    g.set_privacy(toggles(st::privacy_rows(p)));
    let people = |v: &[(String, String)]| {
        model(
            v.iter()
                .map(|(m, n)| SPerson {
                    mri: m.into(),
                    name: n.into(),
                })
                .collect(),
        )
    };
    g.set_blocked(people(d.map(|d| d.blocked.as_slice()).unwrap_or(&[])));
    g.set_priority(people(d.map(|d| d.priority.as_slice()).unwrap_or(&[])));
    g.set_privacy_status(s(
        if p.is_none() || d.is_some_and(|d| !d.errors.is_empty()) {
            status
        } else {
            String::new()
        },
    ));
    g.set_dry_runs(model(c.dry_runs.iter().map(s).collect()));
    push_profile(w, c);
    push_perms(w, c);
}

pub(super) fn config(c: &Ctl) -> Option<session::Config> {
    c.env
        .paths
        .load_config()
        .inspect_err(|e| tracing::warn!(%e, "settings: config"))
        .ok()
}

/// "Until 3:45 PM" while `presence` is the status picked here with an end that is still ahead.
pub(super) fn status_until_line(c: &Ctl, presence: &str) -> String {
    match &c.status_until {
        Some((picked, end)) if picked == presence && *end > teams_proto::time::now_ms() => {
            expiry::until_label(*end)
        }
        _ => String::new(),
    }
}

/// My availability (the dot's colour) and what I am doing, with the words Teams shows for them
/// ("In a call").
pub(in super::super) fn show_my_status(g: &SettingsStore<'_>, presence: &str, activity: &str) {
    g.set_me_status_label(s(people_presence::status_label(presence, activity)));
    g.set_me_activity(s(activity));
    g.set_me_presence(s(presence));
}

pub(super) fn push_profile(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let cfg = config(c);
    let acct = cfg.as_ref().and_then(|c| c.active_account().cloned());
    let d = c.server.as_ref();
    let name = d
        .map(|d| d.me_name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| acct.as_ref().and_then(|a| a.display_name.clone()))
        .unwrap_or_default();
    let email = d
        .and_then(|d| d.me_email.clone())
        .or_else(|| acct.as_ref().and_then(|a| a.upn.clone()))
        .unwrap_or_default();
    g.set_me_name(s(name));
    g.set_me_email(s(email));
    g.set_me_status(s(d.and_then(|d| d.me_status.clone()).unwrap_or_default()));
    // "Clears at 14:30" under the message: only for a message that has an end still ahead.
    g.set_me_status_ends(s(d
        .filter(|d| d.me_status.is_some())
        .and_then(|d| d.me_status_ends)
        .filter(|end| *end > teams_proto::time::now_ms())
        .map(expiry::at_label)
        .unwrap_or_default()));
    let presence = d.and_then(|d| d.me_presence.clone()).unwrap_or_default();
    let activity = d.and_then(|d| d.me_activity.clone()).unwrap_or_default();
    g.set_me_presence_until(s(status_until_line(c, &presence)));
    show_my_status(&g, &presence, &activity);
    g.set_me_work_location(s(d
        .and_then(|d| d.me_work_location.as_deref())
        .map(
            |w| match ::calendar::WorkLocation::from_value(&serde_json::json!(w)) {
                ::calendar::WorkLocation::Office => "office",
                ::calendar::WorkLocation::Remote => "remote",
                ::calendar::WorkLocation::Unset => "",
            },
        )
        .unwrap_or_default()));
    g.set_me_initials(w.get_me_initials());
}

pub(super) fn push_accounts(w: &AppWindow, c: &Ctl) {
    let g = w.global::<SettingsStore>();
    let Some(cfg) = config(c) else {
        g.set_accounts_status(s("Could not read the accounts."));
        return;
    };
    g.set_accounts(model(
        st::account_rows(&cfg)
            .into_iter()
            .map(|a| SAccount {
                initials: crate::data::initials(if a.name.is_empty() { &a.label } else { &a.name })
                    .into(),
                label: a.label.into(),
                name: a.name.into(),
                upn: a.upn.into(),
                active: a.active,
                unread: i32::try_from(a.unread).unwrap_or(i32::MAX),
            })
            .collect(),
    ));
    let live = c.server.as_ref().and_then(|d| d.tenants.as_deref());
    g.set_orgs(model(
        st::org_rows(&cfg, live)
            .into_iter()
            .map(|o| SOrg {
                tenant: o.tenant.into(),
                name: o.name.into(),
                home: o.home,
                active: o.active,
            })
            .collect(),
    ));
}

impl UiHandle {
    /// A message from the engine for the Settings window and the profile menu.
    pub fn settings_notice(&self, text: String) {
        self.run(move |w| notice(&w, text));
    }

    /// My profile photo (`None`: none), for the top bar and the profile menu.
    pub fn settings_photo(&self, photo: Option<crate::data::Pixels>) {
        self.run(move |w| {
            let (image, has) = super::super::img(&photo);
            let g = w.global::<SettingsStore>();
            g.set_me_photo(image);
            g.set_me_has_photo(has);
        });
    }

    /// "Refresh license and restart" finished in the engine: restart, or say why not.
    pub fn settings_license(&self, r: Result<(), String>) {
        self.run(move |w| license_done(&w, r));
    }

    /// How much disk each kind of data takes, read by the engine for the Storage page.
    pub fn settings_storage(&self, sizes: crate::backend::CacheSizes) {
        use crate::backend::drive::state::size_label;
        self.run(move |w| {
            let g = w.global::<SettingsStore>();
            g.set_storage_db(s(size_label(sizes.db_bytes)));
            // This account's share, and the total when other accounts have media too.
            let media = if sizes.media_total_bytes > sizes.media_bytes {
                format!(
                    "{} (all accounts: {})",
                    size_label(sizes.media_bytes),
                    size_label(sizes.media_total_bytes)
                )
            } else {
                size_label(sizes.media_bytes)
            };
            g.set_storage_media(s(media));
            g.set_storage_temp(s(size_label(sizes.temp_bytes)));
            g.set_storage_backups(s(size_label(sizes.backups_bytes)));
            let total = [
                sizes.db_bytes,
                sizes.media_bytes.max(sizes.media_total_bytes),
                sizes.temp_bytes,
                sizes.backups_bytes,
            ]
            .iter()
            .fold(0u64, |a, b| a.saturating_add(*b));
            g.set_storage_total(s(size_label(total)));
            g.set_storage_loaded(true);
        });
    }

    /// The Teams side of Settings, read by the engine.
    pub fn settings_server(&self, data: ServerData) {
        self.run(move |w| {
            let Some(shared) = ctl() else { return };
            let mut c = shared.borrow_mut();
            c.server = Some(data);
            push_server(&w, &c);
            push_accounts(&w, &c);
        });
    }

    /// Teams refused a status picked in the profile menu: the menu shows what it showed before
    /// the pick again, and the red banner says why.
    pub fn settings_status_failed(&self, why: String) {
        self.run(|w| {
            let Some(shared) = ctl() else { return };
            let mut c = shared.borrow_mut();
            c.status_until = None;
            if let Some((presence, activity, until)) = c.status_before.take() {
                let g = w.global::<SettingsStore>();
                show_my_status(&g, &presence, &activity);
                g.set_me_presence_until(s(until));
            }
        });
        self.banner(why);
    }
}
