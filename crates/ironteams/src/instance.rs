//! One IronTeams per data folder.
//!
//! A lock file (`<data_dir>/instance.lock`, an exclusive advisory lock held for the life of the
//! process) says which process owns the data; a crash frees it by itself. A local socket lets a
//! second launch hand over one request, `activate`, to the owner, which raises its window while
//! the second launch exits (docs/designs/account-switching-relaunch.md, section 1).
//!
//! - Linux and macOS: a Unix socket in `$XDG_RUNTIME_DIR` (a folder only the user can enter),
//!   else in the data folder, with mode 0600.
//! - Windows: a named pipe whose name holds a hash of the data folder, so two `IRONTEAMS_HOME`s
//!   do not meet. Remote clients are refused.
//!
//! The socket takes one line and acts on `activate` only. Nothing crosses it that reads or
//! changes data, so a local process gains no power from it.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a launch waits for the running instance, or for the old process of an account switch
/// to let go of the lock, before it gives up.
pub const WAIT: Duration = Duration::from_secs(15);
/// How often a waiting launch tries the lock and the socket again.
const RETRY_EVERY: Duration = Duration::from_millis(100);
/// One connection gets this long to send its line.
const READ_TIMEOUT: Duration = Duration::from_secs(1);
/// A line is at most this many bytes, the newline included; a longer one is dropped.
const MAX_LINE: usize = 64;
const LOCK_FILE: &str = "instance.lock";
const ACTIVATE: &[u8] = b"activate";

/// What [`acquire`] found.
#[derive(Debug)]
pub enum Acquired {
    /// No other process owns the data: this one does, until the `Instance` is dropped.
    Primary(Instance),
    /// Another instance owns it and was asked to raise its window: this process should exit.
    Activated,
}

/// The ownership of the data folder: the lock and, once [`Instance::listen`] ran, the socket.
/// Dropping it removes the socket and frees the lock.
#[derive(Debug)]
pub struct Instance {
    /// `None` once released.
    lock: Option<File>,
    endpoint: sys::Endpoint,
    listener: Option<Listener>,
}

#[derive(Debug)]
struct Listener {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// Takes the lock of `data_dir`, or asks its owner to raise the window.
///
/// While another process holds the lock and its socket does not answer (it may be starting, or
/// shutting down for an account switch), this tries again for up to `wait`. The lock coming free
/// makes this process the primary one. Running out of time is a `TimedOut` error: starting a
/// second engine on the same data is worse than not starting.
pub fn acquire(data_dir: &Path, wait: Duration) -> io::Result<Acquired> {
    acquire_with(data_dir, runtime_dir().as_deref(), wait)
}

/// [`acquire`] with the socket folder given, so tests do not read the environment.
fn acquire_with(
    data_dir: &Path,
    runtime_dir: Option<&Path>,
    wait: Duration,
) -> io::Result<Acquired> {
    fs::create_dir_all(data_dir)?;
    let lock = open_lock(&data_dir.join(LOCK_FILE))?;
    let endpoint = sys::Endpoint::new(data_dir, runtime_dir);
    let give_up = Instant::now() + wait;
    loop {
        match lock.try_lock() {
            Ok(()) => {
                // Holding the lock proves no other process owns the socket: what is left of
                // one that crashed goes.
                endpoint.remove();
                return Ok(Acquired::Primary(Instance {
                    lock: Some(lock),
                    endpoint,
                    listener: None,
                }));
            }
            Err(TryLockError::WouldBlock) => {
                if endpoint.connect(b"activate\n").is_ok() {
                    return Ok(Acquired::Activated);
                }
            }
            Err(TryLockError::Error(e)) => return Err(e),
        }
        if Instant::now() >= give_up {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "another IronTeams holds the data folder and does not answer",
            ));
        }
        std::thread::sleep(RETRY_EVERY);
    }
}

impl Instance {
    /// Starts answering `activate` on a std thread (the tokio runtime need not be up yet).
    /// `on_activate` runs on that thread, so it must hand work to the UI thread itself.
    pub fn listen(&mut self, on_activate: impl Fn() + Send + 'static) -> io::Result<()> {
        if self.listener.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "already listening",
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread = self.endpoint.spawn(stop.clone(), move |line: &[u8]| {
            if is_activate(line) {
                on_activate();
            }
        })?;
        self.listener = Some(Listener { stop, thread });
        Ok(())
    }

    /// Stops the listener, removes the socket and frees the lock. Dropping does the same.
    pub fn release(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        // Nothing to do once released: a later `drop` must not remove the socket of the next
        // process that took the lock.
        let Some(lock) = self.lock.take() else {
            return;
        };
        if let Some(Listener { stop, thread }) = self.listener.take() {
            stop.store(true, Ordering::SeqCst);
            // The thread waits in `accept`: a connection of our own wakes it. A client that
            // keeps it busy (at most `READ_TIMEOUT`) delays this, so give up waiting and let the
            // thread end on its own rather than hold the exit.
            for _ in 0..300 {
                let _ = self.endpoint.connect(b"");
                if thread.is_finished() {
                    let _ = thread.join();
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.endpoint.remove();
        let _ = lock.unlock();
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn open_lock(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// A folder only the user can enter, for the socket.
#[cfg(unix)]
fn runtime_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
}

#[cfg(not(unix))]
fn runtime_dir() -> Option<std::path::PathBuf> {
    None
}

/// Whether `raw`, what one connection sent, is the `activate` line (newline optional at the end
/// of the data). A line that did not fit in [`MAX_LINE`] is not.
fn is_activate(raw: &[u8]) -> bool {
    let line = match raw.iter().position(|&b| b == b'\n') {
        Some(end) => &raw[..end],
        None if raw.len() >= MAX_LINE => return false,
        None => raw,
    };
    line.trim_ascii() == ACTIVATE
}

/// 16 hex digits naming `data_dir` in a socket or pipe name. FNV-1a, so it is the same in every
/// build (the std hasher is not promised to be).
fn name_hash(data_dir: &Path) -> String {
    let dir = fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in dir.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(unix)]
mod sys {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    /// The path of the Unix socket.
    #[derive(Clone, Debug)]
    pub(super) struct Endpoint(pub(super) PathBuf);

    impl Endpoint {
        pub(super) fn new(data_dir: &Path, runtime_dir: Option<&Path>) -> Endpoint {
            Endpoint(match runtime_dir {
                Some(dir) => dir.join(format!("ironteams-{}.sock", name_hash(data_dir))),
                None => data_dir.join("instance.sock"),
            })
        }

        /// Connects and sends `payload` (nothing for an empty one).
        pub(super) fn connect(&self, payload: &[u8]) -> io::Result<()> {
            let mut stream = UnixStream::connect(&self.0)?;
            let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
            if payload.is_empty() {
                return Ok(());
            }
            stream.write_all(payload)
        }

        /// Removes the socket file; only the lock holder may call it.
        pub(super) fn remove(&self) {
            let _ = fs::remove_file(&self.0);
        }

        /// Binds the socket (mode 0600) and serves it on a thread named `instance-socket`.
        pub(super) fn spawn(
            &self,
            stop: Arc<AtomicBool>,
            handle: impl Fn(&[u8]) + Send + 'static,
        ) -> io::Result<JoinHandle<()>> {
            let listener = UnixListener::bind(&self.0)?;
            // A connection needs write access to the socket file, so 0600 keeps other users out
            // (the bind itself leaves the umask's mode for an instant).
            if let Err(e) = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o600)) {
                self.remove();
                return Err(e);
            }
            std::thread::Builder::new()
                .name("instance-socket".into())
                .spawn(move || serve(&listener, &stop, handle))
        }
    }

    fn serve(listener: &UnixListener, stop: &AtomicBool, handle: impl Fn(&[u8])) {
        loop {
            let conn = listener.accept();
            if stop.load(Ordering::SeqCst) {
                return;
            }
            match conn {
                Ok((conn, _)) => handle(read_line(&conn).as_slice()),
                // Out of descriptors, say: do not spin on it.
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    /// What the client sends: up to [`MAX_LINE`] bytes or the first newline, within
    /// [`READ_TIMEOUT`] in all.
    fn read_line(mut conn: &UnixStream) -> Vec<u8> {
        let deadline = Instant::now() + READ_TIMEOUT;
        let mut line = Vec::with_capacity(MAX_LINE);
        let mut chunk = [0u8; MAX_LINE];
        while line.len() < MAX_LINE && !line.contains(&b'\n') {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            // macOS refuses the option (EINVAL) once the client has closed. Its data is already
            // here and the read ends at once, so the refusal is not an error.
            let _ = conn.set_read_timeout(Some(left));
            match conn.read(&mut chunk[..MAX_LINE - line.len()]) {
                Ok(0) | Err(_) => break,
                Ok(n) => line.extend_from_slice(&chunk[..n]),
            }
        }
        line
    }
}

#[cfg(windows)]
mod sys {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;
    use tokio::io::AsyncReadExt;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    /// The name of the named pipe.
    #[derive(Clone, Debug)]
    pub(super) struct Endpoint(pub(super) String);

    impl Endpoint {
        pub(super) fn new(data_dir: &Path, _runtime_dir: Option<&Path>) -> Endpoint {
            Endpoint(format!(r"\\.\pipe\ironteams-{}", name_hash(data_dir)))
        }

        /// Opens the pipe and sends `payload` (nothing for an empty one). Plain file calls: no
        /// tokio runtime is needed to be a client.
        pub(super) fn connect(&self, payload: &[u8]) -> io::Result<()> {
            let mut pipe = OpenOptions::new().write(true).open(&self.0)?;
            if payload.is_empty() {
                return Ok(());
            }
            pipe.write_all(payload)
        }

        /// A pipe needs no clean-up: it ends with the last handle of the server.
        pub(super) fn remove(&self) {}

        /// Creates the pipe and serves it on a thread named `instance-pipe` with a small
        /// current-thread runtime. Returns once the first pipe instance exists, or why not.
        pub(super) fn spawn(
            &self,
            stop: Arc<AtomicBool>,
            handle: impl Fn(&[u8]) + Send + 'static,
        ) -> io::Result<JoinHandle<()>> {
            let name = self.0.clone();
            let (ready_tx, ready_rx) = mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("instance-pipe".into())
                .spawn(move || serve(&name, &stop, handle, &ready_tx))?;
            match ready_rx.recv() {
                Ok(Ok(())) => Ok(thread),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(io::Error::other("the pipe thread ended at its start")),
            }
        }
    }

    fn serve(
        name: &str,
        stop: &AtomicBool,
        handle: impl Fn(&[u8]),
        ready: &mpsc::Sender<io::Result<()>>,
    ) {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        rt.block_on(async {
            let first = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(name);
            let mut server = match first {
                Ok(server) => server,
                Err(e) => {
                    let _ = ready.send(Err(e));
                    return;
                }
            };
            let _ = ready.send(Ok(()));
            loop {
                let connected = server.connect().await;
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                // The next client finds a pipe instance at once.
                let Ok(next) = ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(name)
                else {
                    return;
                };
                let conn = std::mem::replace(&mut server, next);
                if connected.is_ok() {
                    handle(read_line(conn).await.as_slice());
                }
            }
        });
    }

    /// What the client sends: up to [`MAX_LINE`] bytes or the first newline, within
    /// [`READ_TIMEOUT`] in all.
    async fn read_line(mut conn: NamedPipeServer) -> Vec<u8> {
        let mut line = Vec::with_capacity(MAX_LINE);
        let _ = tokio::time::timeout(READ_TIMEOUT, async {
            let mut chunk = [0u8; MAX_LINE];
            while line.len() < MAX_LINE && !line.contains(&b'\n') {
                match conn.read(&mut chunk[..MAX_LINE - line.len()]).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => line.extend_from_slice(&chunk[..n]),
                }
            }
        })
        .await;
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn primary(dir: &Path) -> Instance {
        match acquire_with(dir, None, Duration::ZERO).unwrap() {
            Acquired::Primary(instance) => instance,
            Acquired::Activated => panic!("expected to be the primary instance"),
        }
    }

    /// A primary that sends `()` on the channel for every activation.
    fn listening(dir: &Path) -> (Instance, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let mut instance = primary(dir);
        instance
            .listen(move || {
                let _ = tx.send(());
            })
            .unwrap();
        (instance, rx)
    }

    /// Sends one connection's data. A Windows pipe has one free instance, so a client that comes
    /// while the server serves another gets "all pipe instances are busy": it tries again, as
    /// `acquire` does.
    fn send(instance: &Instance, payload: &[u8]) {
        let give_up = Instant::now() + Duration::from_secs(5);
        loop {
            match instance.endpoint.connect(payload) {
                Ok(()) => return,
                Err(e) if Instant::now() >= give_up => panic!("could not send: {e}"),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[test]
    fn the_second_launch_activates_the_first_and_does_not_become_primary() {
        let dir = tempfile::tempdir().unwrap();
        let (first, activations) = listening(dir.path());
        let second = acquire_with(dir.path(), None, Duration::from_secs(5)).unwrap();
        assert!(matches!(second, Acquired::Activated), "got {second:?}");
        activations
            .recv_timeout(Duration::from_secs(5))
            .expect("the first instance was told to raise its window");
        first.release();
    }

    #[test]
    fn a_new_acquire_is_primary_after_release_and_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = primary(dir.path());
        first.release();
        let second = primary(dir.path());
        drop(second);
        let _third = primary(dir.path());
    }

    #[test]
    fn an_instance_that_listens_is_released_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let (instance, _rx) = listening(dir.path());
        let started = Instant::now();
        instance.release();
        assert!(started.elapsed() < Duration::from_secs(2));
        // The data is free again, and nobody answers on the old socket.
        let (again, _rx) = listening(dir.path());
        again.release();
    }

    #[test]
    fn listening_twice_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (mut instance, _rx) = listening(dir.path());
        let err = instance.listen(|| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn a_line_other_than_activate_does_not_call_back() {
        let dir = tempfile::tempdir().unwrap();
        let (instance, activations) = listening(dir.path());
        send(&instance, b"hello\n");
        send(&instance, b"activate now\n");
        send(&instance, b"ACTIVATE\n");
        // Longer than a line may be, with `activate` first: dropped.
        let mut long = b"activate".to_vec();
        long.extend_from_slice(&[b' '; 200]);
        send(&instance, &long);
        send(&instance, b"");
        // One connection is served at a time, in order: when this one is seen, the others were
        // read before it.
        send(&instance, b"activate\n");
        activations.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(activations.try_recv().is_err(), "a second activation");
    }

    #[test]
    fn a_line_without_a_newline_that_ends_the_connection_still_counts() {
        let dir = tempfile::tempdir().unwrap();
        let (instance, activations) = listening(dir.path());
        send(&instance, b"activate");
        activations.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn what_counts_as_the_activate_line() {
        assert!(is_activate(b"activate\n"));
        assert!(is_activate(b"activate"));
        assert!(is_activate(b"  activate \r\n"));
        assert!(is_activate(b"activate\nmore"));
        assert!(!is_activate(b""));
        assert!(!is_activate(b"\n"));
        assert!(!is_activate(b"activat\n"));
        assert!(!is_activate(b"Activate\n"));
        assert!(!is_activate(b"hello\nactivate\n"));
        let mut long = b"activate".to_vec();
        long.resize(MAX_LINE, b' ');
        assert!(!is_activate(&long));
    }

    #[test]
    fn a_held_lock_that_never_answers_ends_in_a_timeout() {
        let dir = tempfile::tempdir().unwrap();
        // The lock, but no listener: the window of a process that is still starting.
        let _first = primary(dir.path());
        let started = Instant::now();
        let err = acquire_with(dir.path(), None, Duration::from_millis(250)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(250));
    }

    #[test]
    fn a_launch_during_a_switch_waits_for_the_old_process_and_takes_over() {
        let dir = tempfile::tempdir().unwrap();
        let old = primary(dir.path());
        // The old process is shutting down (no socket answers) and lets go a moment later.
        let leaving = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            old.release();
        });
        let got = acquire_with(dir.path(), None, Duration::from_secs(10)).unwrap();
        assert!(matches!(got, Acquired::Primary(_)), "got {got:?}");
        leaving.join().unwrap();
    }

    #[test]
    fn a_launch_that_starts_before_the_listener_activates_it_once_it_is_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = primary(dir.path());
        let (tx, rx) = mpsc::channel();
        let starting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            first
                .listen(move || {
                    let _ = tx.send(());
                })
                .unwrap();
            first
        });
        let second = acquire_with(dir.path(), None, Duration::from_secs(10)).unwrap();
        assert!(matches!(second, Acquired::Activated), "got {second:?}");
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        starting.join().unwrap().release();
    }

    #[test]
    fn two_data_folders_do_not_meet() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (first, activations) = listening(a.path());
        let second = primary(b.path());
        assert!(activations.try_recv().is_err());
        assert_ne!(name_hash(a.path()), name_hash(b.path()));
        first.release();
        second.release();
    }

    #[test]
    fn the_name_hash_is_16_hex_digits_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let hash = name_hash(dir.path());
        assert_eq!(hash.len(), 16);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(hash, name_hash(dir.path()));
        // FNV-1a of the empty input is its offset basis: the function is the same everywhere.
        assert_eq!(name_hash(Path::new("")), "cbf29ce484222325");
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        #[test]
        fn a_stale_socket_file_without_a_lock_is_replaced() {
            let dir = tempfile::tempdir().unwrap();
            let stale = dir.path().join("instance.sock");
            // A crashed process leaves its socket file behind.
            drop(UnixListener::bind(&stale).unwrap());
            assert!(stale.exists());
            let (first, activations) = listening(dir.path());
            // Binding over the stale file would have failed with `AddrInUse`.
            let second = acquire_with(dir.path(), None, Duration::from_secs(5)).unwrap();
            assert!(matches!(second, Acquired::Activated), "got {second:?}");
            activations.recv_timeout(Duration::from_secs(5)).unwrap();
            first.release();
        }

        #[test]
        fn the_socket_is_private_to_the_user_and_goes_with_the_instance() {
            let dir = tempfile::tempdir().unwrap();
            let (instance, _rx) = listening(dir.path());
            let socket = instance.endpoint.0.clone();
            assert_eq!(socket, dir.path().join("instance.sock"));
            let mode = fs::metadata(&socket).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "mode {mode:o}");
            let lock = fs::metadata(dir.path().join(LOCK_FILE)).unwrap();
            assert_eq!(lock.permissions().mode() & 0o777, 0o600);
            instance.release();
            assert!(!socket.exists());
        }

        #[test]
        fn the_socket_lives_in_the_runtime_folder_when_there_is_one() {
            let (data, runtime) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let Acquired::Primary(mut instance) =
                acquire_with(data.path(), Some(runtime.path()), Duration::ZERO).unwrap()
            else {
                panic!("expected the primary instance");
            };
            instance.listen(|| {}).unwrap();
            let socket = instance.endpoint.0.clone();
            assert_eq!(socket.parent(), Some(runtime.path()));
            let name = socket.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                name.starts_with("ironteams-") && name.ends_with(".sock"),
                "{name}"
            );
            assert!(!data.path().join("instance.sock").exists());
            // A launch that finds the same folders reaches it.
            let second =
                acquire_with(data.path(), Some(runtime.path()), Duration::from_secs(5)).unwrap();
            assert!(matches!(second, Acquired::Activated), "got {second:?}");
            instance.release();
            assert!(!socket.exists());
        }
    }
}
