//! IronTeams: an unofficial native Microsoft Teams client written in Rust.
// A release build on Windows is a GUI program: no console window next to it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod backend;
mod catch_up;
mod composer;
mod composer_format;
mod data;
mod demo;
mod external;
mod find_data;
mod language;
mod lists_data;
mod loop_compose;
mod markdown;
mod media_data;
mod mention;
#[cfg(not(target_os = "linux"))]
mod native_tray;
mod reactions;
mod send_options;
mod shortcuts;
mod snippets;
mod spell;
mod suggest;
mod topbar;
mod tray_wiring;
mod ui;
#[cfg(test)]
mod ui_tests;
mod update_check;
mod updater;
mod window_state;

slint::include_modules!();

use backend::{ActivityCmd, Cmd, Engine};
use slint::ComponentHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use teams_proto::Client;
use ui::UiHandle;

/// Signs in (stored session or device code), then runs the engine until the app quits.
async fn app_main(
    ui: UiHandle,
    mut cmds: tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    focus: Arc<AtomicBool>,
) {
    if compat::var_os("IRONTEAMS_DEMO").is_some() {
        let engine = demo_engine(&ui).expect("demo setup");
        engine.start_offline();
        if let Ok(id) = compat::var("IRONTEAMS_DEMO_OPEN") {
            tokio::spawn(engine.clone().handle(Cmd::SelectChat(id)));
        }
        while let Some(cmd) = cmds.recv().await {
            engine.dispatch(cmd);
        }
        return;
    }
    let engine = loop {
        match sign_in(&ui).await {
            Ok(e) => break e,
            Err(err) => {
                tracing::error!(%err, "sign-in failed");
                ui.login_error(format!("Sign-in failed: {err}"));
                while let Some(c) = cmds.recv().await {
                    if matches!(c, Cmd::RetryLogin) {
                        break;
                    }
                }
                ui.login_checking();
            }
        }
    };
    let shared_focus = engine.focused.clone();
    tokio::spawn(async move {
        loop {
            shared_focus.store(focus.load(Ordering::Relaxed), Ordering::Relaxed);
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    });
    engine.start();
    // Development aid: open a conversation on startup.
    if let Ok(target) = compat::var("IRONTEAMS_OPEN") {
        let e = engine.clone();
        tokio::spawn(async move {
            let (conv, root) = match target.split_once(";messageid=") {
                Some((c, r)) => (c.to_string(), Some(r.to_string())),
                None => (target, None),
            };
            e.clone().handle(Cmd::SelectChat(conv)).await;
            if let Some(r) = root {
                e.handle(Cmd::OpenThread(r)).await;
            }
        });
    }
    while let Some(cmd) = cmds.recv().await {
        engine.dispatch(cmd);
    }
}

fn demo_engine(ui: &UiHandle) -> anyhow::Result<Arc<Engine>> {
    let store = store::Store::open_in_memory()?;
    demo::seed(&store, teams_proto::time::now_ms())?;
    let client = Client::offline("demo")?;
    let now = teams_proto::time::now_ms();
    let activity = demo::activity(&store, now);
    ui.ready("ME".into());
    let engine = Engine::new(
        client,
        store,
        session::Config::default(),
        ui.clone(),
        demo::ME.into(),
        "Mohamed Eslam".into(),
    );
    engine.seed_activity(activity);
    engine
        .calls
        .demo(compat::var_os("IRONTEAMS_CALL_DEMO").is_some());
    Ok(engine)
}

async fn sign_in(ui: &UiHandle) -> anyhow::Result<Arc<Engine>> {
    let paths = session::Paths::new()?;
    let mut cfg = paths.load_config()?;
    let client = match session::stored_session(&cfg)? {
        Some(sess) => match Client::resume(sess).await {
            Ok(c) => c,
            Err(teams_proto::Error::Auth(msg)) => {
                tracing::warn!(%msg, "stored session rejected, signing in again");
                device_code(ui, &cfg).await?
            }
            Err(e) => return Err(e.into()),
        },
        None => device_code(ui, &cfg).await?,
    };
    session::persist(&cfg, &client).await?;
    let me = match cfg.self_mri.clone() {
        Some(m) => m,
        None => client.self_mri().await?,
    };
    let my_name = match cfg.display_name.clone() {
        Some(n) => n,
        None => client
            .profiles(std::slice::from_ref(&me))
            .await
            .ok()
            .and_then(|p| p.into_iter().next())
            .map(|p| p.display_name)
            .unwrap_or_else(|| "Me".into()),
    };
    cfg.self_mri = Some(me.clone());
    cfg.display_name = Some(my_name.clone());
    paths.save_config(&cfg)?;
    let store = store::Store::open(&paths.db_path_for(&cfg))?;
    ui.ready(data::initials(&my_name));
    Ok(Engine::new(client, store, cfg, ui.clone(), me, my_name))
}

async fn device_code(ui: &UiHandle, cfg: &session::Config) -> anyhow::Result<Client> {
    let ui2 = ui.clone();
    let client = Client::sign_in_device_code(&cfg.tenant, &cfg.epid, move |dc| {
        ui2.login_code(dc.user_code.clone(), dc.verification_url.clone());
    })
    .await?;
    ui.login_checking();
    Ok(client)
}

/// Twemoji (COLRv0) — Slint's renderer draws COLRv0/CBDT color glyphs but not COLRv1, which is
/// what most distributions ship as their emoji font. CC-BY 4.0, see ui/fonts/README.md.
/// IronTeams's desktop-file id (the Flatpak's app id), which is also its Wayland app id and X11 window
/// class, so the compositor and the task manager match the window to the installed desktop file.
/// The desktop portal gets it too: the ScreenCast and RemoteDesktop dialogs then show IronTeams's
/// name instead of the terminal's. The portal takes reverse-DNS ids only.
const APP_ID: &str = tray::APP_ID;

const EMOJI_FONT: &[u8] = include_bytes!("../ui/fonts/TwemojiMozilla.ttf");

/// The window saw a right button go up: the message under the pointer opens its "…" menu
/// (`MessagePointer` in `message_row.slint`; its text takes the press itself).
fn right_click_released(w: &AppWindow) {
    let p = w.global::<MessagePointer>();
    p.set_right_clicks(p.get_right_clicks().wrapping_add(1));
}

/// A mouse button is going down at `at`, before the window handles it: the message menu and
/// the profile menu close when it is outside them, so the press reaches what is under them
/// (`pointer-pressed` in `app.slint`) instead of only closing the menu.
fn pointer_pressed(w: &AppWindow, at: slint::LogicalPosition) {
    w.invoke_pointer_pressed(at.x, at.y);
}

/// Finds a system font file for a fontconfig pattern.
fn fc_match(pattern: &str) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .ok()?;
    let path = std::path::PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    path.is_file().then_some(path)
}

/// Builds the font chain: the UI font in each weight, then the bundled color emoji font. Linux
/// only (fontconfig): Windows and macOS keep the system UI font and its color emoji fallback.
fn configure_fonts() {
    if std::env::var_os("SLINT_FONT_PATH").is_some() || !cfg!(target_os = "linux") {
        return;
    }
    let Ok(paths) = session::Paths::new() else {
        return;
    };
    let dir = paths.cache_dir.join("fonts");
    let emoji = dir.join("TwemojiMozilla.ttf");
    let fresh = std::fs::metadata(&emoji)
        .map(|m| m.len() == EMOJI_FONT.len() as u64)
        .unwrap_or(false);
    if !fresh
        && (std::fs::create_dir_all(&dir).is_err() || std::fs::write(&emoji, EMOJI_FONT).is_err())
    {
        return;
    }
    // Static files per weight: weights of a variable font loaded this way are not applied.
    // Arabic and other scripts keep using the system fallback.
    let mut chain: Vec<std::path::PathBuf> =
        ["sans-serif", "sans-serif:semibold", "sans-serif:bold"]
            .into_iter()
            .filter_map(fc_match)
            .collect();
    chain.dedup();
    chain.push(emoji);
    if let Ok(joined) = std::env::join_paths(chain) {
        // SAFETY: called at the start of main, before any other thread exists.
        unsafe { std::env::set_var("SLINT_FONT_PATH", joined) };
    }
}

/// `ironteams --report-problem [--log FILE] [--out PATH]`: writes the local, redacted problem
/// bundle (nothing is uploaded), prints where it is and shows a desktop notification. It runs
/// before any window exists so it works even when the UI is what is broken.
fn report_problem(args: &[String]) -> anyhow::Result<()> {
    let paths = session::Paths::new()?;
    let out = diag::report_from_args(&paths, env!("CARGO_PKG_VERSION"), args)?;
    println!("Wrote {}", out.display());
    let _ = notify_rust::Notification::new()
        .appname("IronTeams")
        .summary("Problem report saved")
        .body(&format!(
            "{}\nStored on this computer only. Tokens and ids are redacted.",
            out.display()
        ))
        .icon("dialog-information")
        .show();
    Ok(())
}

fn main() -> anyhow::Result<()> {
    // First of all: Velopack's install, update and uninstall hooks run (and exit) here. A
    // downloaded update is never applied at the start; only on "Restart to update" or on quit.
    velopack::VelopackApp::build()
        .set_auto_apply_on_startup(false)
        .run();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--report-problem") {
        return report_problem(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("--self-check") {
        return updater::self_check::run(args.iter().any(|a| a == "--packaged"));
    }
    if args.iter().any(|a| a == "--call-demo") {
        // SAFETY: called at the start of main, before any other thread exists.
        unsafe { std::env::set_var("IRONTEAMS_DEMO", "1") };
        unsafe { std::env::set_var("IRONTEAMS_CALL_DEMO", "1") };
    }
    configure_fonts();
    tracing_subscriber::fmt()
        .with_env_filter(
            compat::var("IRONTEAMS_LOG")
                .ok()
                .and_then(|v| tracing_subscriber::EnvFilter::try_new(v).ok())
                .unwrap_or_else(|| "warn".into()),
        )
        .init();
    // Before anything opens the store, the session or the preferences: the first run after the
    // rename from kelma moves the old directories.
    compat::migrate_legacy_dirs();
    // Before anything opens the store or the config: a new version backs up the old data, and
    // the crash-loop guard counts this start. Demo mode keeps its hands off the real data.
    let data_dir = session::Paths::new()
        .ok()
        .filter(|_| compat::var_os("IRONTEAMS_DEMO").is_none())
        .map(|paths| {
            let version = env!("CARGO_PKG_VERSION");
            match updater::backup::on_start(&paths, version) {
                Ok(updater::backup::Outcome::Nothing) => {}
                Ok(done) => tracing::info!(?done, "data of the previous version saved"),
                Err(e) => tracing::warn!(error = %e, "could not back up the data"),
            }
            updater::guard::start(&paths.data_dir, version);
            paths.data_dir
        });

    // Software rendering keeps memory low (docs/adr/0001-ui-toolkit.md).
    // IRONTEAMS_RENDERER=femtovg switches to the GPU renderer.
    let renderer = compat::var("IRONTEAMS_RENDERER").unwrap_or_else(|_| "software".into());
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer)
        .select()?;
    // The desktop file is named after the app id (inside Flatpak too), so the window uses it.
    let _ = slint::set_xdg_app_id(std::env::var("FLATPAK_ID").unwrap_or_else(|_| APP_ID.into()));
    // The ScreenCast, RemoteDesktop and Camera dialogs name IronTeams, not the terminal it ran from.
    #[cfg(feature = "calls")]
    calling::video::portal::set_host_app_id(APP_ID);

    let window = AppWindow::new()?;
    composer::wire(&window);
    if let Ok(view) = compat::var("IRONTEAMS_VIEW") {
        window.set_view(view.into());
    }
    let ui = UiHandle::new(window.as_weak());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    let focus = Arc::new(AtomicBool::new(true));

    macro_rules! cmd {
        ($cb:ident, || $e:expr) => {{
            let tx = tx.clone();
            window.$cb(move || {
                let _ = tx.send($e);
            });
        }};
        ($cb:ident, |$($a:ident),*| $e:expr) => {{
            let tx = tx.clone();
            window.$cb(move |$($a),*| {
                let _ = tx.send($e);
            });
        }};
    }
    cmd!(on_retry_login, || Cmd::RetryLogin);
    cmd!(on_select_chat, |id| Cmd::SelectChat(id.into()));
    cmd!(on_select_channel, |id, title| Cmd::SelectChannel(
        id.into(),
        title.into()
    ));
    cmd!(on_toggle_team, |i| Cmd::ToggleTeam(i.max(0) as usize));
    cmd!(on_toggle_favorite, |id| Cmd::ToggleFavorite(id.into()));
    cmd!(on_set_filter, |f| Cmd::SetFilter(f.into()));
    cmd!(on_search, |q| Cmd::Search(q.into()));
    cmd!(on_typing_tick, || Cmd::TypingTick);
    cmd!(on_load_older, || Cmd::LoadOlder);
    cmd!(on_react, |id, key| Cmd::React(id.into(), key.into()));
    cmd!(on_reply, |id| Cmd::Reply(id.into()));
    cmd!(on_cancel_reply, || Cmd::CancelReply);
    cmd!(on_edit, |id| Cmd::Edit(id.into()));
    cmd!(on_delete, |id| Cmd::Delete(id.into()));
    cmd!(on_open_url, |u| Cmd::OpenUrl(u.into()));
    cmd!(on_open_thread, |id| Cmd::OpenThread(id.into()));
    cmd!(on_close_thread, || Cmd::CloseThread);
    cmd!(on_attach_image, || Cmd::AttachImage);
    cmd!(on_attach_file, || Cmd::AttachFile);
    cmd!(on_copy_link, |id| Cmd::CopyLink(id.into()));
    cmd!(on_toggle_save, |id| Cmd::ToggleSave(id.into()));
    cmd!(on_pin, |id| Cmd::Pin(id.into()));
    cmd!(on_mark_unread, |id| Cmd::MarkUnread(id.into()));
    cmd!(on_forward_begin, |id| Cmd::ForwardBegin(Some(id.into())));
    cmd!(on_forward_begin_selected, || Cmd::ForwardBegin(None));
    cmd!(on_forward_search, |q| Cmd::ForwardSearch(q.into()));
    cmd!(on_forward_toggle, |id| Cmd::ForwardToggle(id.into()));
    cmd!(on_forward_send, |note| Cmd::ForwardSend(note.into()));
    cmd!(on_forward_cancel, || Cmd::ForwardCancel);
    cmd!(on_select_begin, |id| Cmd::SelectBegin(id.into()));
    cmd!(on_toggle_select, |id| Cmd::ToggleSelect(id.into()));
    cmd!(on_select_cancel, || Cmd::SelectCancel);
    cmd!(on_at_bottom_changed, |b| Cmd::AtBottom(b));
    cmd!(on_retry_send, |id| Cmd::RetrySend(id.into()));
    cmd!(on_discard_send, |id| Cmd::DiscardSend(id.into()));
    cmd!(on_mark_chat_unread, |id| Cmd::MarkChatUnread(id.into()));
    cmd!(on_mark_all_read, || Cmd::MarkAllRead);
    ui::install_local(&window);
    cmd!(on_activity_refresh, || Cmd::Activity(ActivityCmd::Refresh));
    cmd!(on_activity_select, |id| Cmd::Activity(ActivityCmd::Select(
        id.into()
    )));
    cmd!(on_activity_set_filter, |f| Cmd::Activity(
        ActivityCmd::SetFilter(f.into())
    ));
    cmd!(on_activity_search, |q| Cmd::Activity(ActivityCmd::Search(
        q.into()
    )));
    cmd!(on_activity_mark, |id, read| Cmd::Activity(
        ActivityCmd::MarkRead(id.into(), read)
    ));
    cmd!(on_activity_mark_all, || Cmd::Activity(
        ActivityCmd::MarkAllRead
    ));
    cmd!(on_activity_go_to, || Cmd::Activity(ActivityCmd::GoTo));
    cmd!(on_activity_reply, |t| Cmd::Activity(ActivityCmd::Reply(
        t.into()
    )));
    cmd!(on_set_enter_sends, |on| Cmd::SetEnterSends(on));
    cmd!(on_draft_edited, |text| Cmd::DraftEdited(text.into()));
    cmd!(on_emoji_used, |e| Cmd::EmojiUsed(e.into()));
    cmd!(on_paste_image, || Cmd::PasteImage);
    cmd!(on_remove_attachment, |i| Cmd::RemoveAttachment(
        i.max(0) as usize
    ));
    cmd!(on_skin_tone_changed, |t| Cmd::SetSkinTone(
        t.clamp(0, 5) as u8
    ));
    {
        let tx = tx.clone();
        let weak = window.as_weak();
        window.on_send(move |text| {
            let subject = weak
                .upgrade()
                .map(|w| w.get_post_subject().to_string())
                .unwrap_or_default();
            if weak.upgrade().is_some_and(|w| w.get_important()) {
                let _ = tx.send(Cmd::SetImportant(true));
            }
            if weak.upgrade().is_some_and(|w| w.get_urgent()) {
                let _ = tx.send(Cmd::SetUrgent(true));
            }
            let _ = tx.send(Cmd::Send(text.into(), subject));
        });
    }
    {
        let tx = tx.clone();
        window.global::<SendStore>().on_command(move |name, arg| {
            if let Some(c) = backend::ScheduledCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Scheduled(c));
            }
        });
    }
    ui::install_calendar(&window, tx.clone());
    ui::install_apps(&window, tx.clone());
    ui::settings::install_system(&window, tx.clone());
    snippets::install_system(&window);
    ui::topbar::install(&window, session::Paths::new().ok().map(|p| p.config_dir));
    #[cfg(feature = "calls")]
    ui::call_direct::install(&window, tx.clone());
    #[cfg(feature = "calls")]
    ui::calls_page::install(&window, tx.clone());
    ui::install_composer_apps(&window, tx.clone());
    ui::install_voice(&window, tx.clone());
    ui::install_clip(&window, tx.clone());
    loop_compose::wire(&window, tx.clone());
    {
        let tx = tx.clone();
        window.global::<FindStore>().on_command(move |name, arg| {
            if let Some(c) = backend::FindCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Find(c));
            }
        });
    }
    {
        let tx = tx.clone();
        window.global::<PeopleStore>().on_command(move |name, arg| {
            if let Some(c) = backend::PeopleCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::People(c));
            }
        });
    }
    {
        let tx = tx.clone();
        window.global::<RemindStore>().on_command(move |name, arg| {
            if let Some(c) = backend::RemindCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Remind(c));
            }
        });
    }
    {
        let tx = tx.clone();
        window.global::<ListStore>().on_command(move |name, arg| {
            if let Some(c) = backend::ListsCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Lists(c));
            }
        });
    }
    {
        let tx = tx.clone();
        window
            .global::<ChannelStore>()
            .on_command(move |name, arg| {
                if let Some(c) = backend::ChannelCmd::parse(&name, &arg) {
                    let _ = tx.send(Cmd::Channels(c));
                }
            });
    }
    {
        let tx = tx.clone();
        window.global::<TeamsStore>().on_command(move |name, arg| {
            if let Some(c) = backend::TeamsCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Teams(c));
            }
        });
    }
    {
        let tx = tx.clone();
        window.global::<MediaStore>().on_command(move |name, arg| {
            if let Some(c) = backend::MediaCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Media(c));
            }
        });
    }
    {
        let tx = tx.clone();
        window.global::<GifStore>().on_command(move |name, arg| {
            if let Some(c) = backend::GifCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Gif(c));
            }
        });
    }
    {
        let tx = tx.clone();
        window.global::<EngageStore>().on_command(move |name, arg| {
            if let Some(c) = backend::EngageCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Engage(c));
            }
        });
    }
    ui::call::wire(&window);
    {
        let tx = tx.clone();
        window.global::<CallStore>().on_command(move |name, arg| {
            if let Some(c) = backend::call::CallCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Call(c));
            }
        });
    }
    {
        let tx = tx.clone();
        window.global::<DriveStore>().on_command(move |name, arg| {
            if let Some(c) = backend::DriveCmd::parse(&name, &arg) {
                let _ = tx.send(Cmd::Drive(c));
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_dismiss_banner(move || {
            if let Some(w) = weak.upgrade() {
                w.set_banner(slint::SharedString::new());
                w.set_banner_action(slint::SharedString::new());
                w.set_banner_info(false);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_copy_code(move || {
            if let Some(w) = weak.upgrade()
                && let Ok(mut cb) = arboard::Clipboard::new()
            {
                let _ = cb.set_text(w.get_user_code().to_string());
            }
        });
    }

    // Files dropped on the window become attachments. winit reports one event per file. Wayland has
    // no drop events in winit 0.30, so this only fires on X11, Windows and macOS. Right clicks are
    // counted for the message under the pointer (`right_click_released`); a press first closes
    // the menus it is outside of (`pointer_pressed`).
    {
        use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
        use slint::winit_030::{EventResult, WinitWindowAccessor};
        let tx = tx.clone();
        let weak = window.as_weak();
        let cursor = std::cell::Cell::new(slint::LogicalPosition::default());
        window.window().on_winit_window_event(move |win, event| {
            if let WindowEvent::CursorMoved { position, .. } = event {
                let p = position.to_logical::<f32>(f64::from(win.scale_factor()));
                cursor.set(slint::LogicalPosition::new(p.x, p.y));
            }
            if let WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } = event
                && let Some(w) = weak.upgrade()
            {
                pointer_pressed(&w, cursor.get());
            }
            if let WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Right,
                ..
            } = event
                && let Some(w) = weak.upgrade()
            {
                right_click_released(&w);
            }
            // The mouse's own Back and Forward buttons move through the history like the arrows.
            if let WindowEvent::MouseInput {
                state: ElementState::Released,
                button: button @ (MouseButton::Back | MouseButton::Forward),
                ..
            } = event
                && let Some(w) = weak.upgrade()
            {
                let name = if *button == MouseButton::Back {
                    "back"
                } else {
                    "forward"
                };
                w.global::<TopBarStore>()
                    .invoke_command(name.into(), "".into());
            }
            if let WindowEvent::DroppedFile(path) = event {
                // On the OneDrive view a drop uploads; elsewhere it attaches to the message.
                let on_drive = weak.upgrade().is_some_and(|w| w.get_view() == "onedrive");
                let _ = tx.send(if on_drive {
                    Cmd::Drive(backend::DriveCmd::Drop(path.clone()))
                } else {
                    Cmd::AddFile(path.clone())
                });
            }
            EventResult::Propagate
        });
    }

    // Track window focus for notifications and read markers.
    let focus_timer = slint::Timer::default();
    {
        use slint::winit_030::WinitWindowAccessor;
        let weak = window.as_weak();
        let focus = focus.clone();
        focus_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(400),
            move || {
                if let Some(w) = weak.upgrade() {
                    let f = w
                        .window()
                        .with_winit_window(|ww| ww.has_focus())
                        .unwrap_or(true);
                    focus.store(f, Ordering::Relaxed);
                }
            },
        );
    }

    // Saved size, position, last chat and collapsed teams; demo mode neither restores nor saves.
    let _window_state_timers = window_state::install(
        &window,
        compat::var_os("IRONTEAMS_DEMO").is_none(),
        tx.clone(),
    );
    let hidden = args.iter().any(|a| a == "--hidden");
    // A minute in the event loop: this start was clean (the crash-loop guard).
    let healthy_timer = slint::Timer::default();
    if let Some(dir) = data_dir {
        healthy_timer.start(
            slint::TimerMode::SingleShot,
            updater::guard::HEALTHY_AFTER,
            move || updater::guard::healthy_in(&dir),
        );
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.spawn(app_main(ui, rx, focus));
    // Tray, close-to-tray and the event loop (stays up with the window hidden).
    tray_wiring::run(&window, hidden)?;
    window_state::finish(&window);
    // A downloaded update is installed once this process has exited.
    ui::settings::apply_update_on_quit();
    // Give the engine a moment to write a draft typed just before closing and to take this
    // device out of presence.
    let _ = tx.send(Cmd::Flush);
    std::thread::sleep(std::time::Duration::from_millis(300));
    rt.shutdown_timeout(std::time::Duration::from_millis(500));
    Ok(())
}

#[cfg(test)]
mod app_id_tests {
    use super::APP_ID;

    #[test]
    fn the_app_id_names_the_flatpak_and_every_packaged_file() {
        let manifest =
            include_str!("../../../packaging/flatpak/io.github.mohamedeslam04.ironteams.yml");
        assert!(manifest.contains(&format!("app-id: {APP_ID}\n")));
        assert!(
            APP_ID.split('.').count() >= 2,
            "the portal refuses `ironteams`"
        );
        let desktop = include_str!("../../../packaging/io.github.mohamedeslam04.ironteams.desktop");
        assert!(desktop.contains(&format!("\nIcon={APP_ID}\n")));
        assert!(
            desktop.contains(&format!("\nStartupWMClass={APP_ID}\n")),
            "the X11 window class is the app id"
        );
        let metainfo =
            include_str!("../../../packaging/io.github.mohamedeslam04.ironteams.metainfo.xml");
        assert!(metainfo.contains(&format!("<id>{APP_ID}</id>")));
        assert!(metainfo.contains(&format!(
            "<launchable type=\"desktop-id\">{APP_ID}.desktop</launchable>"
        )));
    }
}
