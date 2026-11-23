//! The app's side of the card host: starts it on the first card (`ironteams --toast-host`, the
//! same executable), keeps it running, writes cards to its stdin and hands what the user did to
//! each card's handler. One host per app process.
//!
//! [`show`] waits (at most [`START_TIMEOUT`], once) for the host to say whether it can draw. When
//! it cannot, or dies, or never answers, `show` returns false and the caller shows the system
//! notification instead. A host that crashed is started again on the next card, at most
//! [`MAX_STARTS`] times per run; after that the app stays on system notifications. When the app
//! exits, the host reads the end of its stdin and quits too.

use super::protocol::{self, Action, Card, FromHost, ToHost};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// What to do with the user's actions on one card. Called on the host's reader thread: hand
/// slow work to the runtime.
pub type Handler = Box<dyn Fn(Action) + Send + Sync>;

/// How long the first card waits for the host to start.
pub const START_TIMEOUT: Duration = Duration::from_secs(5);
/// Host starts per run of the app.
pub const MAX_STARTS: u32 = 3;
/// How many cards taken away before they were shown are remembered.
const WITHDRAWN_KEPT: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Not running (yet, or again after a crash).
    Stopped,
    Starting,
    Ready,
    /// Cannot draw here, or crashed too often: system notifications for the rest of the run.
    Failed,
}

struct Inner {
    phase: Phase,
    starts: u32,
    stdin: Option<ChildStdin>,
    child: Option<Child>,
    handlers: HashMap<u32, Arc<Handler>>,
    /// Cards taken away before the host had them (a call stopped ringing while the host was
    /// still starting): `show` skips them. The oldest go first.
    withdrawn: Vec<u32>,
}

struct Client {
    inner: Mutex<Inner>,
    changed: Condvar,
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| Client {
        inner: Mutex::new(Inner {
            phase: Phase::Stopped,
            starts: 0,
            stdin: None,
            child: None,
            handlers: HashMap::new(),
            withdrawn: Vec::new(),
        }),
        changed: Condvar::new(),
    })
}

fn lock(c: &Client) -> MutexGuard<'_, Inner> {
    c.inner.lock().unwrap_or_else(|e| e.into_inner())
}

/// A new card number.
pub fn next_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn write(g: &mut Inner, msg: &ToHost) -> std::io::Result<()> {
    let stdin = g
        .stdin
        .as_mut()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
    writeln!(stdin, "{}", protocol::line(msg))?;
    stdin.flush()
}

fn start(g: &mut Inner) {
    g.starts += 1;
    if g.starts > MAX_STARTS {
        tracing::warn!("desktop cards: the card host keeps failing, system notifications from now");
        g.phase = Phase::Failed;
        return;
    }
    let spawned = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .arg(super::HOST_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The host's log lines go into the app's own log (nobody reads this process's stderr).
            .stderr(Stdio::piped())
            .spawn()
    });
    match spawned {
        Ok(mut child) => {
            g.stdin = child.stdin.take();
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            g.child = Some(child);
            g.phase = Phase::Starting;
            if let Some(stdout) = stdout {
                std::thread::spawn(move || read(stdout));
            }
            if let Some(stderr) = stderr {
                std::thread::spawn(move || forward_log(stderr));
            }
        }
        Err(err) => {
            tracing::warn!(%err, "desktop cards: could not start the card host");
            g.phase = Phase::Failed;
        }
    }
}

/// The host's log lines (it logs to stderr, its stdout is the protocol) into this process's log,
/// until it exits. The host applied its own filter, so every line is kept.
fn forward_log(stderr: ChildStderr) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        tracing::warn!("card host: {line}");
    }
}

/// The host's answers, until it exits.
fn read(stdout: ChildStdout) {
    let c = client();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Some(msg) = protocol::parse::<FromHost>(&line) else {
            continue;
        };
        match msg {
            FromHost::Ready { backend } => {
                tracing::info!(?backend, "desktop cards: the card host is ready");
                lock(c).phase = Phase::Ready;
                c.changed.notify_all();
            }
            FromHost::Unavailable { reason } => {
                tracing::info!(%reason, "desktop cards: unavailable, system notifications");
                lock(c).phase = Phase::Failed;
                c.changed.notify_all();
            }
            FromHost::Action { id, action } => {
                let handler = {
                    let mut g = lock(c);
                    if action == Action::Closed {
                        g.handlers.remove(&id)
                    } else {
                        g.handlers.get(&id).cloned()
                    }
                };
                if let Some(h) = handler {
                    h(action);
                }
            }
        }
    }
    // The host is gone; its cards with it.
    let child = {
        let mut g = lock(c);
        if g.phase != Phase::Failed {
            g.phase = Phase::Stopped;
        }
        g.stdin = None;
        g.handlers.clear();
        g.child.take()
    };
    c.changed.notify_all();
    if let Some(mut child) = child {
        let _ = child.wait();
    }
}

/// Shows (or replaces, for a known id) a card on the desktop; `on_action` gets what the user
/// does with it. Starts the host on first use and may block until it answers, so call it off the
/// UI thread and off async tasks (`spawn_blocking`). False when the cards cannot be shown: show
/// the system notification instead.
pub fn show(card: Card, on_action: Handler) -> bool {
    // Tests never start a host: it would be the test binary.
    if cfg!(test) {
        return false;
    }
    let c = client();
    let mut g = lock(c);
    if g.phase == Phase::Stopped {
        start(&mut g);
    }
    let deadline = Instant::now() + START_TIMEOUT;
    while g.phase == Phase::Starting {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            tracing::warn!("desktop cards: the card host did not answer in time");
            if let Some(child) = g.child.as_mut() {
                let _ = child.kill();
            }
            g.phase = Phase::Failed;
            break;
        }
        g = c
            .changed
            .wait_timeout(g, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    let id = card.id;
    if let Some(at) = g.withdrawn.iter().position(|w| *w == id) {
        g.withdrawn.remove(at);
        return true;
    }
    if g.phase != Phase::Ready {
        return false;
    }
    g.handlers.insert(id, Arc::new(on_action));
    if let Err(err) = write(&mut g, &ToHost::Show(card)) {
        tracing::warn!(%err, "desktop cards: the card host went away");
        g.handlers.remove(&id);
        g.stdin = None;
        return false;
    }
    true
}

/// Takes a card away (a call stopped ringing). Nothing happens for a card the host does not have.
pub fn dismiss(id: u32) {
    let c = client();
    let mut g = lock(c);
    if g.handlers.remove(&id).is_some() {
        if g.phase == Phase::Ready {
            let _ = write(&mut g, &ToHost::Dismiss { id });
        }
    } else {
        // Not shown (yet): `show` must not put it up later.
        if g.withdrawn.len() >= WITHDRAWN_KEPT {
            g.withdrawn.remove(0);
        }
        g.withdrawn.push(id);
    }
}
