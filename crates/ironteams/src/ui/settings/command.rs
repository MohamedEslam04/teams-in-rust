use super::*;

/// One command from the page. Runs on the UI thread.
pub(super) fn command(shared: &Shared, w: &AppWindow, name: &str, arg: &str) {
    let g = w.global::<SettingsStore>();
    let parts: Vec<&str> = arg.split('\u{1f}').collect();
    match name {
        "open" | "page" => {
            notice(w, "");
            {
                let mut c = shared.borrow_mut();
                if name == "open" {
                    push_local(w, &c);
                    push_accounts(w, &c);
                    request_load(&mut c, false);
                }
                if arg != "devices" && c.mic.is_some() {
                    stop_mic(w, &mut c);
                }
            }
            if arg == "devices" {
                list_devices(w, shared);
            }
            if arg == "storage" {
                open_storage(w, shared);
            }
        }
        "close" => {
            let mut c = shared.borrow_mut();
            stop_mic(w, &mut c);
        }
        "profile" => {
            let mut c = shared.borrow_mut();
            push_profile(w, &c);
            // The menu's Accounts section, with the unread counts saved since.
            push_accounts(w, &c);
            request_load(&mut c, false);
        }
        "query" => {
            g.set_found(model(
                st::find(arg)
                    .into_iter()
                    .map(|f| SFind {
                        page: f.page.into(),
                        page_title: f.page_title.into(),
                        label: f.label.into(),
                    })
                    .collect(),
            ));
        }
        // --- Appearance ---
        "theme" | "accent" | "density" | "font-step" | "font-scale" => {
            let mut c = shared.borrow_mut();
            let a = &mut c.local.appearance;
            match name {
                "theme" => {
                    a.theme = match arg {
                        "light" => ThemeChoice::Light,
                        "dark" => ThemeChoice::Dark,
                        _ => ThemeChoice::System,
                    }
                }
                "accent" => a.accent = Rgb::from_hex(arg),
                "density" => {
                    a.density = if arg == "compact" {
                        Density::Compact
                    } else {
                        Density::Comfy
                    }
                }
                "font-step" => {
                    let v = st::step_font_scale(a.font_scale, arg.parse().unwrap_or(0));
                    a.set_font_scale(v);
                }
                _ => a.set_font_scale(arg.parse().unwrap_or(1.0)),
            }
            let r = c.local.save_appearance();
            apply_look(w, &c);
            push_local(w, &c);
            saved(w, r);
        }
        "skin-tone" => {
            let t = arg.parse::<i32>().unwrap_or(0).clamp(0, 5);
            w.set_skin_tone(t);
            g.set_skin_tone(t);
            // The engine keeps the skin tone with the other chat preferences.
            w.invoke_skin_tone_changed(t);
            notice(w, "Saved.");
        }
        // --- General ---
        "language" | "date-format" | "time-format" | "time-zone" | "startup" => {
            let mut c = shared.borrow_mut();
            let gp = &mut c.local.general;
            let mut autostart = None;
            match name {
                "language" => gp.language = appearance::general::normalize_tag(arg),
                "date-format" => gp.date_format = st::date_from_slug(arg).unwrap_or_default(),
                "time-format" => gp.time_format = st::time_from_slug(arg).unwrap_or_default(),
                "time-zone" => {
                    let z = arg.trim();
                    gp.time_zone = (!z.is_empty()).then(|| z.to_string());
                }
                _ => {
                    let on = bit(parts.get(1).copied().unwrap_or(""));
                    let key = parts[0];
                    st::set_startup(gp, key, on);
                    if key == "auto-start" {
                        autostart = Some(on);
                    }
                }
            }
            let formats_changed = matches!(name, "date-format" | "time-format");
            let mut r = c.local.save_general();
            if c.env.apply_globals {
                apply_general(&c.local.general);
            }
            if formats_changed && let Some(tx) = &c.tx {
                // Rows already drawn show the old format until the engine draws them again.
                let _ = tx.send(Cmd::Settings(SettingsCmd::Redraw));
            }
            if let Some(on) = autostart
                && r.is_ok()
            {
                r = (c.env.hooks.autostart.set)(on).map_err(|e| anyhow::anyhow!(e));
            }
            push_local(w, &c);
            saved(w, r);
        }
        // Graphics: saved now, used by the next start (the renderer cannot change while running).
        "renderer" => {
            let mut c = shared.borrow_mut();
            c.local.general.renderer = appearance::Renderer::parse(arg).unwrap_or_default();
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        // --- App bar: `rail-move` (key, up|down, rail), `rail-show` (key, 0|1), `rail-auto` ---
        // "rail" after the direction: a move from the rail's menu jumps over the hidden entries.
        "rail-move" | "rail-show" | "rail-auto" => {
            let mut c = shared.borrow_mut();
            let key = parts[0];
            let arg1 = parts.get(1).copied().unwrap_or("");
            match name {
                "rail-move" => {
                    let from_rail = parts.get(2) == Some(&"rail");
                    c.local.rail.move_item(key, arg1 == "up", from_rail);
                }
                "rail-show" => c.local.rail.set_hidden(key, !bit(arg1)),
                _ => c.local.rail.auto_collapse = bit(arg),
            }
            let r = c.local.save_rail();
            push_rail(w, &c.local.rail);
            saved(w, r);
        }
        // --- Accessibility ---
        "high-contrast" | "reduced-motion" | "larger-text" | "a11y" => {
            let mut c = shared.borrow_mut();
            let x = &mut c.local.a11y;
            match name {
                "high-contrast" => x.high_contrast = st::switch_from_slug(arg).unwrap_or_default(),
                "reduced-motion" => {
                    x.reduced_motion = st::switch_from_slug(arg).unwrap_or_default()
                }
                "larger-text" => x.larger_text = bit(arg),
                _ => {
                    st::set_a11y_flag(x, parts[0], bit(parts.get(1).copied().unwrap_or("")));
                }
            }
            let r = c.local.save_a11y();
            apply_look(w, &c);
            push_local(w, &c);
            saved(w, r);
        }
        // --- Notifications (Teams: dry run) ---
        "notify-sound" | "meeting-chat" => {
            let mut c = shared.borrow_mut();
            let Some((base, current)) = with_server_note(&c) else {
                notice(
                    w,
                    "Your Teams settings were not read, so nothing can be changed.",
                );
                push_server(w, &c);
                return;
            };
            let req = if name == "notify-sound" {
                let Some(field) = SoundField::from_slug(parts[0]) else {
                    return;
                };
                st::dry_run_sound(
                    &base,
                    &current,
                    field,
                    bit(parts.get(1).copied().unwrap_or("")),
                )
            } else {
                let Some(mode) = MeetingChatMode::from_slug(arg) else {
                    return;
                };
                st::dry_run_meeting_chat(&base, &current, mode)
            };
            write_or_plan(w, &mut c, req);
        }
        "notify" | "pause" => {
            let mut c = shared.borrow_mut();
            let Some((base, current)) = with_server_note(&c) else {
                notice(
                    w,
                    "Your Teams settings were not read, so nothing can be changed.",
                );
                push_server(w, &c);
                return;
            };
            let req = if name == "notify" {
                let kind = Kind::from_slug(parts[0]);
                let delivery = parts.get(1).and_then(|d| Delivery::from_slug(d));
                let (Some(kind), Some(delivery)) = (kind, delivery) else {
                    return;
                };
                st::dry_run_notify(&base, &current, kind, delivery)
            } else {
                // `off`, `1h`, `2h`, `tomorrow` or `custom` U+001F text. The first switch only
                // knew 0 and 1: 1 meant a day.
                let (choice, custom) = match parts[0] {
                    "0" | "" => ("off", ""),
                    "1" => ("tomorrow", ""),
                    c => (c, parts.get(1).copied().unwrap_or("")),
                };
                let until = if choice == "off" {
                    None
                } else {
                    match expiry::expiry_iso(choice, custom) {
                        Ok(t) => t,
                        Err(why) => {
                            notice(w, why);
                            push_server(w, &c);
                            return;
                        }
                    }
                };
                g.set_pause_custom_open(false);
                st::dry_run_pause(&base, &current, until)
            };
            write_or_plan(w, &mut c, req);
        }
        "test-notification" => {
            (shared.borrow().env.hooks.test_notification)();
            notice(w, "Sent a test notification to your desktop.");
        }
        // --- Privacy (Teams; a dry run without an engine) ---
        "privacy" | "unblock" | "priority-remove" => {
            let mut c = shared.borrow_mut();
            let Some(d) = c.server.as_ref().filter(|d| d.privacy.is_some()) else {
                notice(
                    w,
                    "Your Teams settings were not read, so nothing can be changed.",
                );
                push_server(w, &c);
                return;
            };
            let req = match name {
                "privacy" => {
                    let Some(flag) = PrivacyFlag::from_slug(parts[0]) else {
                        return;
                    };
                    let on = bit(parts.get(1).copied().unwrap_or(""));
                    st::dry_run_privacy(&d.chat_service, flag, on)
                }
                "unblock" => st::dry_run_remove(&d.middle_tier, ContactList::Blocked, arg),
                _ => st::dry_run_remove(&d.middle_tier, ContactList::Priority, arg),
            };
            write_or_plan(w, &mut c, req);
        }
        // --- Devices (local) ---
        "speaker" | "mic" | "camera" => {
            let mut c = shared.borrow_mut();
            let v = (!arg.is_empty()).then(|| arg.to_string());
            match name {
                "speaker" => c.local.devices.speaker = v,
                "mic" => c.local.devices.microphone = v,
                _ => c.local.devices.camera = v,
            }
            let r = c.local.save_devices();
            if name == "mic" && c.mic.is_some() {
                stop_mic(w, &mut c);
            }
            push_local(w, &c);
            saved(w, r);
        }
        "noise-suppression" | "echo-cancellation" => {
            let mut c = shared.borrow_mut();
            if name == "noise-suppression" {
                c.local.devices.noise_suppression = bit(arg);
            } else {
                c.local.devices.echo_cancellation = bit(arg);
            }
            let r = c.local.save_devices();
            push_local(w, &c);
            saved(w, r);
        }
        // The call speaker's slider: `-live` while it moves (the call's playback follows, nothing
        // is written), the plain one when it is let go (kept in the devices file).
        "speaker-volume" | "speaker-volume-live" => {
            let percent = arg.parse::<u8>().unwrap_or(100).min(100);
            #[cfg(feature = "calls")]
            calling::audio::set_output_volume(percent);
            if name == "speaker-volume" {
                let mut c = shared.borrow_mut();
                c.local.devices.speaker_volume = percent;
                if let Err(e) = c.local.save_devices() {
                    tracing::warn!(error = %e, "the speaker volume was not saved");
                }
            }
        }
        "all-devices" => {
            g.set_all_devices(bit(arg));
            push_devices(w, &shared.borrow());
        }
        "test-sound" => {
            let c = shared.borrow();
            let out = c.local.devices.speaker.clone();
            let play = c.env.hooks.test_sound.clone();
            if c.env.background {
                notice(w, "Playing a test sound…");
                let weak = w.as_weak();
                std::thread::spawn(move || {
                    let r = play(out);
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        if let Err(e) = r {
                            notice(&w, format!("Could not play the test sound: {e}"));
                        }
                    });
                });
            } else {
                match play(out) {
                    Ok(()) => notice(w, "Played a test sound."),
                    Err(e) => notice(w, format!("Could not play the test sound: {e}")),
                }
            }
        }
        "mic-test" => {
            let mut c = shared.borrow_mut();
            if bit(arg) {
                start_mic(w, &mut c);
            } else {
                stop_mic(w, &mut c);
            }
        }
        "call-feedback" => {
            let mut c = shared.borrow_mut();
            c.local.calls.ask_feedback = Ask::from_slug(arg).unwrap_or_default();
            let r = c.local.save_calls();
            push_local(w, &c);
            saved(w, r);
        }
        "ringtone" | "message-sound" | "urgent-sound" => {
            let choice = RingtoneChoice::from_slug(arg).unwrap_or_default();
            let needs_file = {
                let mut c = shared.borrow_mut();
                let (r, has_file) = match name {
                    "ringtone" => {
                        c.local.calls.ringtone = choice;
                        (c.local.save_calls(), c.local.calls.ringtone_file.is_some())
                    }
                    "urgent-sound" => {
                        c.local.sounds.urgent = choice;
                        (c.local.save_sounds(), true)
                    }
                    _ => {
                        c.local.sounds.message = choice;
                        (c.local.save_sounds(), c.local.sounds.message_file.is_some())
                    }
                };
                push_local(w, &c);
                saved(w, r);
                choice == RingtoneChoice::Custom && !has_file
            };
            // A Teams sound that is not on disk yet is fetched now; until then its synthesized
            // equivalent plays.
            sounds::fetch_picked(
                choice,
                match name {
                    "ringtone" => Slot::Ring,
                    "urgent-sound" => Slot::Urgent,
                    _ => Slot::Message,
                },
            );
            // Custom with no file yet: ask for one right away.
            if needs_file {
                let which = if name == "ringtone" {
                    SoundFor::Ringtone
                } else {
                    SoundFor::Message
                };
                choose_sound_file(shared, w, which);
            }
        }
        "teams-sounds" => {
            let on = bit(arg);
            let r = {
                let mut c = shared.borrow_mut();
                c.local.sounds.download_teams_sounds = on;
                let r = c.local.save_sounds();
                push_local(w, &c);
                r
            };
            saved(w, r);
            if on {
                // Fetch what is missing now rather than at the next start.
                sounds::fetch_teams_sounds();
            }
        }
        "ringtone-file" => choose_sound_file(shared, w, SoundFor::Ringtone),
        "message-sound-file" => choose_sound_file(shared, w, SoundFor::Message),
        "ringtone-preview" | "message-sound-preview" | "urgent-sound-preview" => {
            let c = shared.borrow();
            let choice = RingtoneChoice::from_slug(arg).unwrap_or_default();
            let (kind, file) = match name {
                "ringtone-preview" => (SoundKind::Ringtone, c.local.calls.ringtone_file.clone()),
                "urgent-sound-preview" => (SoundKind::Urgent, None),
                _ => (SoundKind::Message, c.local.sounds.message_file.clone()),
            };
            (c.env.hooks.play_sound)(kind, choice, file);
        }
        // --- Files and app permissions (local) ---
        "open-files" => {
            let mut c = shared.borrow_mut();
            c.local.files.open_office_in = OpenFilesIn::from_slug(arg).unwrap_or_default();
            let r = c.local.save_files();
            push_local(w, &c);
            saved(w, r);
        }
        // --- Storage (the engine measures and clears; it answers with a notice and the sizes) ---
        "media-cache-limit" => {
            let Some(mb) = limit_from_slug(arg) else {
                return;
            };
            let c = shared.borrow();
            let r = c.env.paths.update_config(|cfg| {
                cfg.media_cache_mb = mb;
                Ok(())
            });
            if r.is_ok()
                && let Some(tx) = &c.tx
            {
                let _ = tx.send(Cmd::Settings(SettingsCmd::ApplyMediaLimit));
            }
            saved(w, r.map_err(|e| anyhow::anyhow!(e.to_string())));
        }
        "clear-media-cache" | "clear-all-cache" | "delete-update-backups" => {
            let c = shared.borrow();
            let Some(tx) = &c.tx else {
                notice(w, "Not signed in, so there is nothing to clear.");
                return;
            };
            let (cmd, text) = match name {
                "clear-media-cache" => (Cmd::ClearMediaCache, "Clearing the media cache…"),
                "clear-all-cache" => (Cmd::ClearAllCache, "Clearing the cached data…"),
                _ => (
                    Cmd::Settings(SettingsCmd::DeleteUpdateBackups),
                    "Deleting the old update backups…",
                ),
            };
            let _ = tx.send(cmd);
            notice(w, text);
        }
        "perm" => {
            let mut c = shared.borrow_mut();
            let (Some(app), Some(p)) = (
                parts.first(),
                parts.get(1).and_then(|p| DevicePermission::parse(p)),
            ) else {
                return;
            };
            let on = bit(parts.get(2).copied().unwrap_or(""));
            let r = c.local.set_grant(app, p, on);
            push_perms(w, &c);
            saved(w, r);
        }
        // --- Accounts ---
        // Relaunches on the choice, after asking when something would be cut short (`switch`).
        "switch-account" => switch::start(shared, w, Some(arg), None),
        "switch-org" => switch::start(shared, w, None, Some(arg)),
        "switch-confirm" => switch::confirm(shared, w),
        "switch-cancel" => switch::cancel(shared, w),
        // --- Updates (always on; only the beta channel is a choice; local) ---
        "notification-style" => {
            let mut c = shared.borrow_mut();
            c.local.general.notification_style = match arg {
                "system" => appearance::NotificationStyle::System,
                _ => appearance::NotificationStyle::Cards,
            };
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "banner-photo" | "banner-actions" => {
            let mut c = shared.borrow_mut();
            if name == "banner-photo" {
                c.local.general.banner_photo = bit(arg);
            } else {
                c.local.general.banner_actions = bit(arg);
            }
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        // --- Quiet hours, Focus mode and keyword alerts (local; see backend/quiet.rs) ---
        "quiet-on" | "quiet-day" | "quiet-start" | "quiet-end" => {
            let mut c = shared.borrow_mut();
            let q = &mut c.local.general.quiet_hours;
            match name {
                "quiet-on" => q.enabled = bit(arg),
                "quiet-day" => {
                    if let Some(d) = arg.parse::<usize>().ok().and_then(|i| q.days.get_mut(i)) {
                        *d = !*d;
                    }
                }
                _ => match crate::send_options::parse_time(arg).map(|m| m as u16) {
                    Some(m) if name == "quiet-start" => q.start_min = m,
                    Some(m) => q.end_min = m,
                    None => {
                        push_local(w, &c);
                        notice(w, "Write the time like 10:00 PM or 22:00.");
                        return;
                    }
                },
            }
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "focus" => {
            let mut c = shared.borrow_mut();
            let now = chrono::Local::now();
            let until = bit(arg).then(|| {
                crate::support::quiet::focus_end(
                    c.local.general.focus_duration,
                    now.naive_local(),
                    now.timestamp_millis(),
                )
            });
            crate::support::quiet::set_focus(until);
            c.local.general.focus_until_ms = until;
            let r = c.local.save_general();
            push_local(w, &c);
            crate::tray_wiring::apply_focus(w, until);
            saved(w, r);
        }
        "focus-duration" => {
            use appearance::general::FocusDuration as D;
            let Some(d) = [D::Hour1, D::Hour2, D::Tomorrow, D::Manual]
                .into_iter()
                .find(|d| focus_slug(*d) == arg)
            else {
                return;
            };
            let mut c = shared.borrow_mut();
            c.local.general.focus_duration = d;
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "keyword-add" | "keyword-remove" => {
            let mut c = shared.borrow_mut();
            let words = &mut c.local.general.keyword_alerts;
            let word = arg.split_whitespace().collect::<Vec<_>>().join(" ");
            if name == "keyword-add" {
                let known = words.iter().any(|k| {
                    crate::support::quiet::normalise(k) == crate::support::quiet::normalise(&word)
                });
                if word.is_empty() || known {
                    return;
                }
                words.push(word);
            } else {
                words.retain(|k| *k != word);
            }
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "check-spelling" => {
            let mut c = shared.borrow_mut();
            c.local.general.check_spelling = bit(arg);
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "check-now" => {
            let mut c = shared.borrow_mut();
            if c.env.hooks.updates.is_some() {
                start_update(w, &mut c, None);
            } else {
                check_updates(w, &mut c, true);
            }
        }
        "beta-updates" => {
            let mut c = shared.borrow_mut();
            c.local.general.beta_updates = bit(arg);
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "restart-update" => restart_to_update(w, &mut shared.borrow_mut()),
        "release-notes" => {
            let c = shared.borrow();
            if let Some(url) = &c.update_url {
                (c.env.hooks.open_url)(url);
            }
        }
        "add-account" => {
            let c = shared.borrow();
            let label = arg.trim();
            // The new account becomes the active one; IronTeams has no tokens for it, so the next
            // start runs the device-code sign-in (the code is shown in the login screen).
            let r = c.env.paths.update_config(|cfg| {
                cfg.add_account(label)?;
                cfg.switch_account(label)
            });
            match r {
                Ok(()) => {
                    session::allow_account_tokens(label);
                    g.set_restart_needed(true);
                    g.set_accounts_status(s(format!(
                        "Added \"{label}\". Restart IronTeams to sign in to it."
                    )));
                }
                Err(e) => g.set_accounts_status(s(format!("Could not add the account: {e}"))),
            }
            push_accounts(w, &c);
        }
        "remove-account" => {
            let c = shared.borrow();
            // An account of a kind this version does not know is not listed and never removed.
            let Some(acct) = config(&c)
                .and_then(|cfg| cfg.account_by_label(arg).filter(|a| a.is_usable()).cloned())
            else {
                g.set_accounts_status(s("That account is not in the list."));
                push_accounts(w, &c);
                return;
            };
            // Tokens first: the running engine saves its token every few minutes, and the
            // forget marks the account so a save that is already running skips it.
            let was_active = config(&c).is_some_and(|cfg| cfg.account == arg);
            let r = session::forget_account_tokens(&*c.env.tokens, &acct)
                .and_then(|()| c.env.paths.update_config(|cfg| cfg.remove_account(arg)));
            match r {
                Ok(()) => {
                    remove_account_cache(&c.env.paths, &acct);
                    // The engine runs on the account that was active at start: when that one is
                    // gone, IronTeams has to start again on another.
                    if was_active {
                        let now = config(&c).map(|cfg| cfg.account).unwrap_or_default();
                        g.set_restart_needed(true);
                        g.set_accounts_status(s(format!(
                            "Removed \"{arg}\" and its saved sign-in. Restart IronTeams: it uses \"{now}\" next."
                        )));
                    } else {
                        g.set_accounts_status(s(format!(
                            "Removed \"{arg}\" and its saved sign-in."
                        )));
                    }
                }
                Err(e) => g.set_accounts_status(s(format!("Could not remove the account: {e}"))),
            }
            push_accounts(w, &c);
        }
        // --- Profile menu: my status and work location (sent to Teams) ---
        "status" | "work-location" | "status-note" | "photo-change" | "photo-remove" => {
            let c = shared.borrow();
            let Some(tx) = &c.tx else {
                notice(w, "Not signed in, so your profile can't change.");
                return;
            };
            let cmd = match name {
                // An empty status resets it; a forced one ends after "Reset status after".
                "status" => {
                    let until = if arg.is_empty() {
                        Ok(None)
                    } else {
                        expiry::expiry_iso(&g.get_status_duration(), &g.get_status_custom())
                    };
                    match until {
                        Ok(until) => SettingsCmd::SetStatus(arg.to_string(), until),
                        Err(why) => {
                            notice(w, why);
                            return;
                        }
                    }
                }
                "status-note" => {
                    let until = if arg.trim().is_empty() {
                        Ok(None)
                    } else {
                        expiry::expiry_iso(&g.get_note_clear_after(), "")
                    };
                    match until {
                        Ok(until) => SettingsCmd::SetStatusNote(
                            arg.to_string(),
                            until,
                            g.get_note_show_when_messaged(),
                        ),
                        Err(why) => {
                            notice(w, why);
                            return;
                        }
                    }
                }
                "photo-change" => SettingsCmd::ChangePhoto,
                "photo-remove" => SettingsCmd::RemovePhoto,
                _ => SettingsCmd::SetWorkLocation(arg.to_string()),
            };
            let picked = match &cmd {
                SettingsCmd::SetStatus(a, until) if !a.is_empty() => Some((
                    a.clone(),
                    until.as_deref().and_then(teams_proto::time::parse_iso_ms),
                )),
                SettingsCmd::SetStatus(..) => Some((String::new(), None)),
                _ => None,
            };
            let _ = tx.send(Cmd::Settings(cmd));
            drop(c);
            // The menu shows the picked status at once (the service's answer and the read after
            // it take seconds); a refusal puts the old one back (`settings_status_failed`).
            if let Some((status, until)) = picked {
                let mut c = shared.borrow_mut();
                c.status_before = Some((
                    g.get_me_presence().to_string(),
                    g.get_me_activity().to_string(),
                    g.get_me_presence_until().to_string(),
                ));
                c.status_until = until.map(|ms| (status.clone(), ms));
                // A reset goes back to the automatic status, which only the service knows.
                if !status.is_empty() {
                    g.set_me_presence_until(s(status_until_line(&c, &status)));
                    show_my_status(&g, &status, people_presence::forced_activity(&status));
                } else {
                    g.set_me_presence_until(s(""));
                }
            }
            // The photo answers with its own notice; the status options close.
            if !name.starts_with("photo") {
                g.set_status_open(false);
            }
            // A picked status closes the menu too; a saved message, its editor.
            if name == "status" {
                g.set_profile_open(false);
            }
            if name == "status-note" {
                g.set_message_open(false);
            }
        }
        // --- Chats (local, and where a chat opens: kept by the engine) ---
        "open-chat-at" => {
            let mut c = shared.borrow_mut();
            let at = if arg == "newest" {
                OpenChatAt::Newest
            } else {
                OpenChatAt::FirstUnread
            };
            c.open_chat_at = at;
            if let Some(tx) = &c.tx {
                let _ = tx.send(Cmd::Settings(SettingsCmd::SetOpenChatAt(at)));
            }
            push_local(w, &c);
            notice(w, "Saved.");
        }
        "wheel-speed" => {
            let mut c = shared.borrow_mut();
            c.local.general.wheel_speed = appearance::general::clamp_wheel_speed(
                arg.parse()
                    .unwrap_or(appearance::general::WHEEL_SPEED_DEFAULT),
            );
            let r = c.local.save_general();
            push_local(w, &c);
            saved(w, r);
        }
        "link-previews" => {
            let mut c = shared.borrow_mut();
            c.local.general.link_previews = bit(arg);
            let r = c.local.save_general();
            if c.env.apply_globals {
                apply_general(&c.local.general);
            }
            if let Some(tx) = &c.tx {
                // Messages already drawn keep their cards until the engine draws them again.
                let _ = tx.send(Cmd::Settings(SettingsCmd::Redraw));
            }
            push_local(w, &c);
            saved(w, r);
        }
        // The top bar menu's "Check for updates": opens About and checks now.
        "check-updates-menu" => {
            let self_update = shared.borrow().env.hooks.updates.is_some();
            g.invoke_show(s("about"));
            if self_update {
                start_update(w, &mut shared.borrow_mut(), None);
            } else {
                check_updates(w, &mut shared.borrow_mut(), true);
            }
        }
        "refresh-license" => {
            let c = shared.borrow();
            let Some(tx) = &c.tx else {
                notice(w, "Not signed in, so there is no license to refresh.");
                return;
            };
            let _ = tx.send(Cmd::Settings(SettingsCmd::RefreshLicense));
            notice(w, "Refreshing your license…");
        }
        "restart" => {
            let mut c = shared.borrow_mut();
            // With an update downloaded, the restart installs it.
            if c.updater.ready().is_some() {
                restart_to_update(w, &mut c);
            } else {
                (c.env.hooks.restart)();
            }
        }
        "sign-out" => {
            let c = shared.borrow();
            let Some(acct) = config(&c).and_then(|cfg| cfg.active_account().cloned()) else {
                notice(w, "Could not read the account to sign out of.");
                return;
            };
            g.set_profile_open(false);
            let tokens = c.env.tokens.clone();
            if c.env.background {
                let weak = w.as_weak();
                std::thread::spawn(move || {
                    let r = session::forget_account_tokens(&*tokens, &acct);
                    let _ = weak.upgrade_in_event_loop(move |w| signed_out(&w, r));
                });
            } else {
                let r = session::forget_account_tokens(&*tokens, &acct);
                drop(c);
                signed_out(w, r);
            }
        }
        "view-account" => {
            (shared.borrow().env.hooks.open_url)("https://myaccount.microsoft.com/");
            g.set_profile_open(false);
        }
        "report-problem" => {
            let c = shared.borrow();
            let report = c.env.hooks.report_problem.clone();
            if c.env.background {
                notice(w, "Writing the problem report…");
                let weak = w.as_weak();
                std::thread::spawn(move || {
                    let r = report();
                    let _ = weak.upgrade_in_event_loop(move |w| report_done(&w, r));
                });
            } else {
                report_done(w, report());
            }
        }
        other => tracing::debug!(command = other, "settings: unknown command"),
    }
}
