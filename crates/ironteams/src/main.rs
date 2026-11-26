//! IronTeams: an unofficial native Microsoft Teams client written in Rust.
// A release build on Windows is a GUI program: no console window next to it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(test)]
mod architecture_tests;
mod backend;
mod catch_up;
mod composer;
mod composer_format;
mod composer_lines;
mod data;
mod demo;
mod desktop_cards;
#[cfg(target_os = "linux")]
mod desktop_entry;
mod external;
mod file_hover;
mod find_data;
mod instance;
mod key_layout;
mod language;
mod lists_data;
mod lists_sort;
mod logging;
mod loop_compose;
mod malloc_arenas;
mod markdown;
mod media_data;
mod mention;
#[cfg(not(target_os = "linux"))]
mod native_tray;
mod offline_start;
mod reactions;
mod relaunch;
mod renderer;
mod send_options;
mod shortcut_keys;
mod shortcuts;
mod sign_in_failure;
mod snippets;
mod spell;
mod start_timing;
mod suggest;
mod support;
mod topbar;
mod tray_wiring;
mod ui;
mod ui_api;
#[cfg(test)]
mod ui_tests;
mod update_check;
mod updater;
mod window_state;
mod window_watch;

slint::include_modules!();

use backend::{ActivityCmd, Cmd, Engine, QUIT_WITHIN, QuitDone};
use slint::ComponentHandle;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use teams_proto::Client;
use ui_api::UiHandle;

/// The engine runs and answers commands (so quitting has something to wait for).
static ENGINE_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Signs in (stored session or device code), then runs the engine until the app quits.
async fn app_main(
    ui: UiHandle,
    mut cmds: tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    mut window: tokio::sync::watch::Receiver<window_watch::WindowStatus>,
) {
    if compat::var_os("IRONTEAMS_DEMO").is_some() {
        let engine = demo_engine(&ui).expect("demo setup");
        engine.start_offline();
        ENGINE_UP.store(true, Ordering::SeqCst);
        if let Ok(id) = compat::var("IRONTEAMS_DEMO_OPEN") {
            tokio::spawn(engine.clone().handle(Cmd::SelectChat(id)));
        }
        while let Some(cmd) = cmds.recv().await {
            engine.dispatch(cmd);
        }
        return;
    }
    let (engine, cached) = loop {
        match sign_in(&ui).await {
            Ok(SignedIn::Online(e)) => break (e, false),
            Ok(SignedIn::Cached(e)) => break (e, true),
            Err(err) => {
                tracing::error!(%err, "sign-in failed");
                show_sign_in_failure(&ui, &err);
                if !wait_for_retry(&mut cmds).await {
                    return;
                }
                ui.login_checking();
            }
        }
    };
    ENGINE_UP.store(true, Ordering::SeqCst);
    // The window's focus and visibility, as they change (`window_watch`).
    let e = engine.clone();
    tokio::spawn(async move {
        loop {
            let status = *window.borrow_and_update();
            e.focused.store(status.focused, Ordering::Relaxed);
            e.set_visible(status.visible);
            e.set_display_scale(status.scale_pct);
            e.window_changed();
            if window.changed().await.is_err() {
                break;
            }
        }
    });
    if cached {
        // Cache-first: the saved chats paint now; the session refreshes in the background and
        // the engine goes online on success (`reconnect`).
        engine.start_cached("");
        let ui_ref = &ui;
        let refreshed = reconnect(
            ui_ref,
            &engine,
            &mut cmds,
            true,
            || engine.refresh_session(),
            |cfg| async move { device_code(ui_ref, &cfg).await },
        )
        .await;
        if !refreshed {
            return;
        }
        start_timing::mark(start_timing::Stage::SignedIn);
        engine.go_online().await;
    } else {
        start_timing::mark(start_timing::Stage::SignedIn);
        engine.start();
        start_timing::mark(start_timing::Stage::Online);
    }
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

/// How [`sign_in`] ended: with fresh tokens, or with the saved chats to show first (the session
/// is refreshed afterwards, in the background: `reconnect`).
enum SignedIn {
    Online(Arc<Engine>),
    Cached(Arc<Engine>),
}

/// Waits for the sign-in screen's Retry, dropping other commands. False when the app quits.
async fn wait_for_retry(cmds: &mut tokio::sync::mpsc::UnboundedReceiver<Cmd>) -> bool {
    while let Some(c) = cmds.recv().await {
        if matches!(c, Cmd::RetryLogin) {
            return true;
        }
    }
    false
}

/// Cache-first start: runs the commands of the saved-chats view while the session is refreshed
/// (at once when `cache_first`, the first start of the process; after that, and for a start
/// that already failed once, 5 s, 10, 20, 40, then every minute) until it works. No answer from
/// Microsoft shows the offline notice once and keeps trying. A session Microsoft rejects goes to
/// the sign-in screen, and the new sign-in takes over the engine's client. False when the app
/// quits.
///
/// `refresh_session` refreshes the stored session; `sign_in_again` signs in from scratch with the
/// config (the device-code flow). They are parameters so a test can answer for the network.
async fn reconnect<R, RF, S, SF>(
    ui: &UiHandle,
    engine: &Arc<Engine>,
    cmds: &mut tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    cache_first: bool,
    refresh_session: R,
    sign_in_again: S,
) -> bool
where
    R: Fn() -> RF,
    RF: std::future::Future<Output = teams_proto::Result<()>>,
    S: Fn(session::Config) -> SF,
    SF: std::future::Future<Output = anyhow::Result<Client>>,
{
    let mut attempt = 0;
    let mut failed_shown = false;
    let mut offline_shown = false;
    loop {
        let delay = offline_start::refresh_delay(attempt, cache_first);
        let pending = async {
            tokio::time::sleep(delay).await;
            refresh_session().await
        };
        tokio::pin!(pending);
        // The saved chats stay usable while a refresh waits for its answer.
        let refreshed = loop {
            tokio::select! {
                r = &mut pending => break r,
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => engine.dispatch(cmd),
                    None => return false,
                },
            }
        };
        attempt = attempt.saturating_add(1);
        let err = match refreshed {
            Ok(()) => {
                tracing::info!(attempt, "back online");
                return true;
            }
            Err(err) => err,
        };
        match offline_start::on_refresh_error(&err, true) {
            offline_start::OnRefreshError::Offline => {
                tracing::info!(%err, attempt, "still offline");
                if !offline_shown {
                    offline_shown = true;
                    ui.info(offline_start::OFFLINE_NOTICE.into());
                }
                continue;
            }
            // Not a rejected session and not a missing network (a 403 from Teams, say): the
            // saved chats stay, the error shows once and the refresh keeps trying.
            offline_start::OnRefreshError::Fail => {
                tracing::warn!(%err, attempt, "refreshing the session failed");
                if !failed_shown {
                    failed_shown = true;
                    ui.banner(format!("Sign-in failed: {err}"));
                }
                continue;
            }
            offline_start::OnRefreshError::SignIn => {}
        }
        tracing::warn!(%err, "stored session rejected, signing in again");
        // The saved chats give way to the sign-in screen at once, not when the code arrives.
        ui.login_checking();
        loop {
            let cfg = engine.config();
            match sign_in_again(cfg).await {
                Ok(client) => {
                    engine.adopt_sign_in(client).await;
                    ui.ready(data::initials(engine.my_name()));
                    return true;
                }
                Err(err) => {
                    tracing::error!(%err, "sign-in failed");
                    show_sign_in_failure(ui, &err);
                    if !wait_for_retry(cmds).await {
                        return false;
                    }
                    ui.login_checking();
                }
            }
        }
    }
}

async fn sign_in(ui: &UiHandle) -> anyhow::Result<SignedIn> {
    let paths = session::Paths::new()?;
    let mut cfg = paths.load_config()?;
    let client = match session::stored_session(&cfg)? {
        Some(sess) => {
            let client = Client::restore(sess)?;
            // Cache-first: an account that ran here before shows its saved chats at once; the
            // session is refreshed in the background (`reconnect`). Nothing below waits for
            // the network on this path.
            if offline_start::cache_first(
                cfg.self_mri.as_deref(),
                paths.db_path_for(&cfg).is_file(),
            ) {
                match saved_engine(ui, &paths, cfg.clone(), client.clone()) {
                    Ok(engine) => {
                        tracing::info!("starting from the saved chats");
                        return Ok(SignedIn::Cached(engine));
                    }
                    Err(err) => {
                        tracing::warn!(%err, "the saved chats do not open, signing in first");
                    }
                }
            }
            match client.sign_in_stored().await {
                Ok(()) => client,
                Err(e) => {
                    // Nothing saved to show: a refresh that cannot reach Teams is a failure.
                    match offline_start::on_refresh_error(&e, false) {
                        offline_start::OnRefreshError::Offline
                        | offline_start::OnRefreshError::Fail => return Err(e.into()),
                        offline_start::OnRefreshError::SignIn => {
                            tracing::warn!(err = %e, "stored session rejected, signing in again");
                            device_code(ui, &cfg).await?
                        }
                    }
                }
            }
        }
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
    Ok(SignedIn::Online(Engine::new(
        client,
        store,
        cfg,
        ui.clone(),
        me,
        my_name,
    )))
}

/// The engine of a cache-first start: who the user is comes from the config, the chats from the
/// account's database. `client` has the stored session, not refreshed yet.
fn saved_engine(
    ui: &UiHandle,
    paths: &session::Paths,
    cfg: session::Config,
    client: Client,
) -> anyhow::Result<Arc<Engine>> {
    let me = cfg
        .self_mri
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no saved account"))?;
    let my_name = cfg.display_name.clone().unwrap_or_else(|| "Me".into());
    let store = store::Store::open(&paths.db_path_for(&cfg))?;
    ui.ready(data::initials(&my_name));
    Ok(Engine::new(client, store, cfg, ui.clone(), me, my_name))
}

/// Puts a failed sign-in on the error screen: a policy block with its copyable report, anything
/// else as the generic "Sign-in failed".
fn show_sign_in_failure(ui: &UiHandle, err: &anyhow::Error) {
    let shown = sign_in_failure::describe(err);
    ui.login_error(shown.message, shown.report);
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

/// IronTeams's desktop-file id (the Flatpak's app id), which is also its Wayland app id and X11 window
/// class, so the compositor and the task manager match the window to the installed desktop file.
/// The desktop portal gets it too: the ScreenCast and RemoteDesktop dialogs then show IronTeams's
/// name instead of the terminal's. The portal takes reverse-DNS ids only.
const APP_ID: &str = tray::APP_ID;

/// The bundled color emoji fonts, in the order they are tried. Slint's renderer draws COLRv0 and
/// CBDT color glyphs but not COLRv1, which is what most distributions ship as their emoji font.
/// Fluent Emoji (3D, as in Teams) is a CBDT bitmap font; it has no country flags and none of the
/// newest emoji, so Twemoji (COLRv0) stays behind it for whatever it lacks. Licenses: MIT and
/// CC-BY 4.0, see ui/fonts/README.md.
const EMOJI_FONTS: [(&str, &[u8]); 2] = [
    (
        "FluentEmoji3D.ttf",
        include_bytes!("../ui/fonts/FluentEmoji3D.ttf"),
    ),
    (
        "TwemojiMozilla.ttf",
        include_bytes!("../ui/fonts/TwemojiMozilla.ttf"),
    ),
];

/// The window saw a right button go up: the message under the pointer opens its "…" menu
/// (`MessagePointer` in `message_row.slint`; its text takes the press itself).
fn right_click_released(w: &AppWindow) {
    let p = w.global::<MessagePointer>();
    p.set_right_clicks(p.get_right_clicks().wrapping_add(1));
}

/// The window saw a left button go up: the message under the pointer hands the keyboard to the
/// window (`MessagePointer` in `message_row.slint`), so E over it edits it even when the
/// composer had the focus.
fn left_click_released(w: &AppWindow) {
    let p = w.global::<MessagePointer>();
    p.set_left_clicks(p.get_left_clicks().wrapping_add(1));
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

/// Whether `u` is a web address the sign-in page may open itself.
fn is_web_url(u: &str) -> bool {
    u.starts_with("https://") || u.starts_with("http://")
}

/// Builds the font chain: the UI font in each weight, then the bundled color emoji fonts. Linux
/// only (fontconfig): Windows and macOS keep the system UI font and its color emoji fallback.
fn configure_fonts() {
    if std::env::var_os("SLINT_FONT_PATH").is_some() || !cfg!(target_os = "linux") {
        return;
    }
    let Ok(paths) = session::Paths::new() else {
        return;
    };
    let dir = paths.cache_dir.join("fonts");
    let mut emoji = Vec::new();
    for (name, bytes) in EMOJI_FONTS {
        let path = dir.join(name);
        let fresh = std::fs::metadata(&path)
            .map(|m| m.len() == bytes.len() as u64)
            .unwrap_or(false);
        if !fresh
            && (std::fs::create_dir_all(&dir).is_err() || std::fs::write(&path, bytes).is_err())
        {
            return;
        }
        emoji.push(path);
    }
    // Static files per weight: weights of a variable font loaded this way are not applied.
    // Arabic and other scripts keep using the system fallback.
    let mut chain: Vec<std::path::PathBuf> =
        ["sans-serif", "sans-serif:semibold", "sans-serif:bold"]
            .into_iter()
            .filter_map(fc_match)
            .collect();
    chain.dedup();
    chain.extend(emoji);
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
    start_timing::init();
    // Before any thread exists: two malloc arenas instead of one per thread (`malloc_arenas`).
    malloc_arenas::limit();
    // Then Velopack's install, update and uninstall hooks run (and exit) here. A
    // downloaded update is never applied at the start; only on "Restart to update" or on quit.
    velopack::VelopackApp::build()
        .set_auto_apply_on_startup(false)
        .run();
    // reqwest is built without a TLS provider of its own (one provider, ring, for the whole
    // app), so it is installed before anything can build an HTTP client.
    teams_proto::client::install_crypto_provider();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--report-problem") {
        return report_problem(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("--self-check") {
        return updater::self_check::run(args.iter().any(|a| a == "--packaged"));
    }
    if args.first().map(String::as_str) == Some(desktop_cards::HOST_ARG) {
        // The notification cards on the desktop (`desktop_cards`): the app reads stdout, so the
        // log goes to stderr.
        configure_fonts();
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(
                compat::var("IRONTEAMS_LOG")
                    .ok()
                    .and_then(|v| tracing_subscriber::EnvFilter::try_new(v).ok())
                    .unwrap_or_else(|| "warn".into()),
            )
            .init();
        return desktop_cards::host::run();
    }
    if args.iter().any(|a| a == "--call-demo") {
        // SAFETY: called at the start of main, before any other thread exists.
        unsafe { std::env::set_var("IRONTEAMS_DEMO", "1") };
        unsafe { std::env::set_var("IRONTEAMS_CALL_DEMO", "1") };
    }
    // One IronTeams per data folder: a second launch asks the running one to raise its window
    // and exits, or waits while the old process of an account switch lets go. Before anything
    // reads the config, the store or the keyring. The log is not up yet; `lock_error` is
    // reported once it is.
    let (mut single, lock_error) = match single_instance() {
        Ok(Some(instance::Acquired::Activated)) => return Ok(()),
        Ok(Some(instance::Acquired::Primary(held))) => (Some(held), None),
        Ok(None) => (None, None),
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            return Err(anyhow::anyhow!("IronTeams is already running: {e}"));
        }
        Err(e) => (None, Some(e)),
    };
    configure_fonts();
    // Demo mode keeps its hands off the real data folder, so it logs to stdout as before.
    let logs_dir = session::Paths::new()
        .ok()
        .filter(|_| compat::var_os("IRONTEAMS_DEMO").is_none())
        .map(|paths| paths.data_dir.join("logs"));
    logging::init(logs_dir.as_deref());
    match (&single, lock_error) {
        (Some(_), _) => tracing::info!("instance: this is the only IronTeams on this data folder"),
        // The data folder cannot be locked (a read-only folder, a file system without locks):
        // start anyway, as before the lock existed.
        (None, Some(e)) => tracing::warn!(error = %e, "instance: no lock, starting without it"),
        (None, None) => {}
    }
    // Before anything opens the store, the session or the preferences: the first run after the
    // rename from kelma moves the old directories.
    compat::migrate_legacy_dirs();
    // After a relaunch the old process named the account to open: apply it to the config before
    // anything reads the config to sign in. Demo mode never touches the real data.
    if compat::var_os("IRONTEAMS_DEMO").is_none()
        && let Ok(paths) = session::Paths::new()
    {
        relaunch::apply_args(&paths, &args);
    }
    // Before anything talks to the desktop portal: it registers the app id only for an installed
    // desktop file, and nothing installs one for a cargo run or the AppImage.
    #[cfg(target_os = "linux")]
    desktop_entry::ensure(APP_ID);
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
            // Before the engine: the cache older versions shared between accounts moves into
            // the active account's folder (a rename; nothing is deleted if it fails). After the
            // instance lock, so two processes never move it at once, and after `apply_args`, so
            // the account a relaunch named is the one it moves into.
            if let Ok(cfg) = paths.load_config() {
                backend::migrate_shared_cache(&paths.cache_dir, &cfg.active_token_key());
            }
            paths.data_dir
        });

    // Software rendering keeps memory low (docs/adr/0001-ui-toolkit.md). The GPU renderer
    // (femtovg) is a Settings choice or IRONTEAMS_RENDERER=femtovg; when it cannot start, the
    // software one runs (renderer.rs).
    let wanted = renderer::wanted(support::quiet::load_prefs().renderer);
    renderer::record(renderer::start(wanted, |name| {
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name(name.into())
            .select()
    })?);
    // The desktop file is named after the app id (inside Flatpak too), so the window uses it.
    let _ = slint::set_xdg_app_id(std::env::var("FLATPAK_ID").unwrap_or_else(|_| APP_ID.into()));
    // The ScreenCast, RemoteDesktop and Camera dialogs name IronTeams, not the terminal it ran from.
    #[cfg(feature = "calls")]
    calling::video::portal::set_host_app_id(APP_ID);

    let window = AppWindow::new()?;
    // A second launch raises this window. The listener runs on its own thread, so the request
    // is handed to the UI thread.
    if let Some(held) = single.as_mut() {
        let weak = window.as_weak();
        if let Err(e) = held.listen(move || {
            let _ = weak.upgrade_in_event_loop(|w| tray_wiring::show_and_raise(&w));
        }) {
            tracing::warn!(error = %e, "instance: not listening, a second launch cannot raise this window");
        }
    }
    composer::wire(&window);
    if let Ok(view) = compat::var("IRONTEAMS_VIEW") {
        window.set_view(view.into());
    }
    let ui = UiHandle::new(window.as_weak());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    let window_status = window_watch::install();

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
    cmd!(on_rows_shown, |top, bottom| Cmd::RowsShown(
        top.into(),
        bottom.into()
    ));
    cmd!(on_react, |id, key| Cmd::React(id.into(), key.into()));
    cmd!(on_reply, |id| Cmd::Reply(id.into()));
    cmd!(on_cancel_reply, || Cmd::CancelReply);
    // The editor of a sent message, in the message's row (`EditStore`), sends what the composer's
    // own callbacks send.
    {
        let edit = window.global::<EditStore>();
        let t = tx.clone();
        edit.on_save(move |text| {
            let _ = t.send(Cmd::SaveEdit(text.into()));
        });
        let t = tx.clone();
        edit.on_cancel(move || {
            let _ = t.send(Cmd::CancelReply);
        });
        let t = tx.clone();
        edit.on_draft_edited(move |text| {
            let _ = t.send(Cmd::DraftEdited(text.into()));
        });
        let t = tx.clone();
        edit.on_emoji_used(move |e| {
            let _ = t.send(Cmd::EmojiUsed(e.into()));
        });
        let t = tx.clone();
        edit.on_skin_tone_changed(move |tone| {
            let _ = t.send(Cmd::SetSkinTone(tone.clamp(0, 5) as u8));
        });
    }
    cmd!(on_edit, |id| Cmd::Edit(id.into()));
    cmd!(on_delete, |id| Cmd::Delete(id.into()));
    // Until sign-in is done the backend loop is busy signing in and reads no commands, so a link
    // on the sign-in page (the device-code address) opens here instead of through it.
    {
        let tx = tx.clone();
        let weak = window.as_weak();
        window.on_open_url(move |u| {
            let signing_in = weak
                .upgrade()
                .is_some_and(|w| w.get_login_state() != "ready");
            if signing_in && is_web_url(&u) {
                let _ = external::open(u.as_str());
                return;
            }
            let _ = tx.send(Cmd::OpenUrl(u.into()));
        });
    }
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
    ui::message_toast::install(&window, tx.clone());
    ui::settings::install_system(&window, tx.clone());
    snippets::install_system(&window);
    shortcut_keys::install_system(&window);
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
        window.global::<ChatNoteStore>().on_edited(move |text| {
            let _ = tx.send(Cmd::NoteEdited(text.into()));
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
    {
        let weak = window.as_weak();
        window.on_copy_login_report(move || {
            if let Some(w) = weak.upgrade()
                && let Ok(mut cb) = arboard::Clipboard::new()
            {
                let _ = cb.set_text(w.get_login_report().to_string());
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
        let shortcuts = key_layout::Shortcuts::default();
        window.window().on_winit_window_event(move |win, event| {
            // Ctrl+V and every other shortcut by the key's place on a non-Latin layout.
            if shortcuts.event(win, event) == EventResult::PreventDefault {
                return EventResult::PreventDefault;
            }
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
            if let WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } = event
                && let Some(w) = weak.upgrade()
            {
                left_click_released(&w);
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
            if matches!(event, WindowEvent::Focused(_)) {
                window_watch::focus_changed(weak.clone());
            }
            // A file dragged over the window shows the chat's drop zone until it leaves or drops.
            if let Some(over) = file_hover::of(event)
                && let Some(w) = weak.upgrade()
            {
                w.set_file_hover(over);
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

    // Saved size and position, and the active account's last chat and collapsed teams; demo mode
    // neither restores nor saves. The account is the one the config names now, after any
    // account chosen on the command line was applied to it.
    let _window_state_timers = window_state::install(
        &window,
        compat::var_os("IRONTEAMS_DEMO").is_none(),
        &window_state::active_account_key(),
        tx.clone(),
    );
    let hidden = args.iter().any(|a| a == "--hidden");
    // A minute in the event loop: this start was clean (the crash-loop guard).
    let healthy_timer = slint::Timer::default();
    let relaunch_dir = data_dir.clone();
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
    rt.spawn(app_main(ui, rx, window_status));
    // The first turn of the event loop has the window up.
    slint::Timer::single_shot(std::time::Duration::ZERO, || {
        start_timing::mark(start_timing::Stage::WindowShown);
    });
    // Tray, close-to-tray and the event loop (stays up with the window hidden).
    tray_wiring::run(&window, hidden)?;
    window_state::finish(&window);
    // A downloaded update is installed once this process has exited.
    ui::settings::apply_update_on_quit();
    // The engine ends a live call (the service keeps a call that was only dropped for its keepAlive,
    // 45 minutes), writes a draft typed just before closing and takes this device out of
    // presence. Waiting is bounded: a request that hangs does not hold the exit.
    if ENGINE_UP.load(Ordering::SeqCst) {
        let (done, finished) = QuitDone::new();
        if tx.send(Cmd::Quit(done)).is_ok() {
            // The timer is made inside the runtime.
            let waited = rt.block_on(async { tokio::time::timeout(QUIT_WITHIN, finished).await });
            if waited.is_err() {
                tracing::warn!("quitting: the engine did not finish in time");
            }
        }
    } else {
        // Still signing in: there is no call.
        tracing::info!(call_ended = false, "quitting");
    }
    rt.shutdown_timeout(std::time::Duration::from_millis(500));
    // Last: the next process may take the data now. When Settings asked for a relaunch (an
    // account switch, a restart) this also starts it, with the guard marked healthy first.
    relaunch::run_if_requested(relaunch_dir.as_deref(), single);
    Ok(())
}

/// Takes the lock of the data folder (see `instance`). `None` in demo mode, which runs next to
/// the real app and never touches its data.
fn single_instance() -> std::io::Result<Option<instance::Acquired>> {
    if compat::var_os("IRONTEAMS_DEMO").is_some() {
        return Ok(None);
    }
    let paths = session::Paths::new().map_err(|e| std::io::Error::other(e.to_string()))?;
    instance::acquire(&paths.data_dir, instance::WAIT).map(Some)
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

#[cfg(test)]
mod sign_in_link_tests {
    use super::is_web_url;

    #[test]
    fn only_web_addresses_open_from_the_sign_in_page() {
        assert!(is_web_url("https://login.microsoft.com/device"));
        assert!(is_web_url("http://example.com"));
        assert!(!is_web_url("kelma-file:docx:https://x"));
        assert!(!is_web_url("tel:+20100"));
        assert!(!is_web_url(""));
    }
}
