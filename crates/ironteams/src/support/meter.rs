//! The microphone level meter, off the UI thread. Opening a microphone asks the sound server for
//! its devices (`pactl`, a subprocess) and then the device itself; either can take seconds when
//! PulseAudio or PipeWire is slow, and the Slint timers that run a meter must not wait for it.
//!
//! A [`Probe`] runs one worker thread per meter: the worker opens the microphone (the stream stays
//! on that thread, `cpal` streams are not `Send` everywhere), then reads it every
//! [`READ_EVERY`] into a slot holding only the newest reading. The UI timer calls [`Probe::poll`],
//! which never blocks: it takes what the slot holds. Choosing another microphone or calling
//! [`Probe::stop`] abandons the worker with its slot, so a reading of the old microphone is never
//! shown for the new one. At most one worker is alive per probe: while a worker is still opening
//! (or winding down) a new one is not started, the next poll tries again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// Reads what a microphone hears, 0 to 1 (peak), or `None` until it has a reading.
pub(crate) type Reader = Box<dyn FnMut() -> Option<f32>>;

/// How often the worker looks at the open microphone (it reports about every 50 ms).
const READ_EVERY: Duration = Duration::from_millis(25);

/// What the worker left for the UI since the last poll.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Reading {
    /// The newest level, when there is one since the last poll.
    pub level: Option<f32>,
    /// Why the microphone could not be opened (reported once; the probe is idle again).
    pub failed: Option<String>,
}

#[derive(Default)]
struct Slot {
    level: Option<f32>,
    failed: Option<String>,
}

/// The worker of one microphone.
struct Run {
    mic: Option<String>,
    slot: Arc<Mutex<Slot>>,
    stop: Arc<AtomicBool>,
}

/// Cleared when the worker's thread ends, wherever it ends.
struct Alive(Arc<AtomicBool>);

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// One level meter's worker, started and read from the UI thread.
#[derive(Default)]
pub(crate) struct Probe {
    /// A worker thread exists (opening, reading or winding down).
    alive: Arc<AtomicBool>,
    run: Option<Run>,
}

impl Probe {
    /// The newest reading of `mic` (`None`: the system default), starting a worker that runs
    /// `open` (blocking, on its own thread) when there is none for it. Returns at once.
    pub(crate) fn poll<F>(&mut self, mic: Option<&str>, open: F) -> Reading
    where
        F: FnOnce(Option<&str>) -> Result<Reader, String> + Send + 'static,
    {
        if self.run.as_ref().is_some_and(|r| r.mic.as_deref() != mic) {
            self.stop();
        }
        if self.run.is_none() {
            if self.alive.swap(true, Ordering::AcqRel) {
                return Reading::default();
            }
            match self.spawn(mic, open) {
                Ok(run) => self.run = Some(run),
                Err(e) => {
                    return Reading {
                        level: None,
                        failed: Some(e.to_string()),
                    };
                }
            }
        }
        let Some(run) = &self.run else {
            return Reading::default();
        };
        let mut slot = run.slot.lock().unwrap_or_else(PoisonError::into_inner);
        let reading = Reading {
            level: slot.level.take(),
            failed: slot.failed.take(),
        };
        drop(slot);
        if reading.failed.is_some() {
            self.run = None;
        }
        reading
    }

    /// Lets the microphone go: the worker closes it and ends, and what it read is dropped.
    pub(crate) fn stop(&mut self) {
        if let Some(run) = self.run.take() {
            run.stop.store(true, Ordering::Release);
        }
    }

    fn spawn<F>(&self, mic: Option<&str>, open: F) -> std::io::Result<Run>
    where
        F: FnOnce(Option<&str>) -> Result<Reader, String> + Send + 'static,
    {
        let run = Run {
            mic: mic.map(str::to_owned),
            slot: Arc::default(),
            stop: Arc::default(),
        };
        let (id, slot, stop) = (run.mic.clone(), run.slot.clone(), run.stop.clone());
        // Moved into the closure: if the thread cannot start it is dropped, and clears the flag.
        let alive = Alive(self.alive.clone());
        std::thread::Builder::new()
            .name("mic-meter".into())
            .spawn(move || work(id.as_deref(), open, &slot, &stop, alive))?;
        Ok(run)
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop();
    }
}

fn work<F>(mic: Option<&str>, open: F, slot: &Mutex<Slot>, stop: &AtomicBool, _alive: Alive)
where
    F: FnOnce(Option<&str>) -> Result<Reader, String>,
{
    let mut read = match open(mic) {
        Ok(read) => read,
        Err(e) => {
            slot.lock().unwrap_or_else(PoisonError::into_inner).failed = Some(e);
            return;
        }
    };
    // `stop` may have been set while opening: the microphone is let go straight away.
    while !stop.load(Ordering::Acquire) {
        if let Some(level) = read() {
            slot.lock().unwrap_or_else(PoisonError::into_inner).level = Some(level);
        }
        std::thread::sleep(READ_EVERY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::Instant;

    /// Calls `f` until it returns `Some`, for at most 5 s.
    fn eventually<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < until, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn steady(level: f32) -> Result<Reader, String> {
        Ok(Box::new(move || Some(level)))
    }

    /// An `open` that counts itself, then waits for the gate, like a sound server that is slow to
    /// answer.
    fn slow(
        gate: mpsc::Receiver<()>,
        opened: Arc<AtomicUsize>,
        level: f32,
    ) -> impl FnOnce(Option<&str>) -> Result<Reader, String> + Send + 'static {
        move |_| {
            opened.fetch_add(1, Ordering::SeqCst);
            let _ = gate.recv();
            steady(level)
        }
    }

    /// An `open` that counts itself and answers at once.
    fn quick(
        opened: Arc<AtomicUsize>,
        level: f32,
    ) -> impl FnOnce(Option<&str>) -> Result<Reader, String> + Send + 'static {
        move |_| {
            opened.fetch_add(1, Ordering::SeqCst);
            steady(level)
        }
    }

    #[test]
    fn a_slow_open_does_not_hold_the_caller() {
        let (release, gate) = mpsc::channel();
        let opened = Arc::new(AtomicUsize::new(0));
        let mut probe = Probe::default();
        // `poll` returns although `open` is still waiting for the gate.
        let first = probe.poll(None, slow(gate, opened.clone(), 0.5));
        assert_eq!(first, Reading::default());
        eventually(|| (opened.load(Ordering::SeqCst) == 1).then_some(()));
        assert_eq!(probe.poll(None, |_| steady(0.1)), Reading::default());

        release.send(()).unwrap();
        let level = eventually(|| probe.poll(None, |_| steady(0.1)).level);
        assert_eq!(level, 0.5);
    }

    #[test]
    fn at_most_one_open_is_in_flight_per_probe() {
        let (release, gate) = mpsc::channel();
        let opened = Arc::new(AtomicUsize::new(0));
        let mut probe = Probe::default();
        probe.poll(Some("a"), slow(gate, opened.clone(), 0.5));
        eventually(|| (opened.load(Ordering::SeqCst) == 1).then_some(()));

        // Polling again, and even choosing another microphone, starts nothing while it waits.
        for mic in ["a", "b", "a", "b"] {
            let r = probe.poll(Some(mic), quick(opened.clone(), 0.9));
            assert_eq!(r, Reading::default());
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(opened.load(Ordering::SeqCst), 1, "no second open started");

        // Once the first has ended, the microphone chosen last is opened.
        release.send(()).unwrap();
        let level = eventually(|| probe.poll(Some("b"), quick(opened.clone(), 0.2)).level);
        assert_eq!(level, 0.2);
        assert_eq!(opened.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_reading_of_the_old_microphone_is_never_shown_for_the_new_one() {
        let mut probe = Probe::default();
        let level = eventually(|| probe.poll(Some("old"), |_| steady(0.9)).level);
        assert_eq!(level, 0.9);

        let mut seen = vec![];
        eventually(|| {
            let r = probe.poll(Some("new"), |_| steady(0.1));
            seen.extend(r.level);
            r.level
        });
        assert_eq!(seen, [0.1], "only the new microphone's readings");
    }

    #[test]
    fn stopping_lets_the_microphone_go() {
        struct Mic(Arc<AtomicBool>);
        impl Drop for Mic {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let closed = Arc::new(AtomicBool::new(false));
        let held = closed.clone();
        let mut probe = Probe::default();
        probe.poll(None, move |_| {
            let mic = Mic(held);
            Ok(Box::new(move || {
                let _ = &mic;
                Some(0.3)
            }) as Reader)
        });
        probe.stop();
        eventually(|| closed.load(Ordering::SeqCst).then_some(()));
    }

    #[test]
    fn a_microphone_that_cannot_be_opened_is_reported_once() {
        let mut probe = Probe::default();
        let failed = eventually(|| probe.poll(None, |_| Err("no server".into())).failed);
        assert_eq!(failed, "no server");
        // The probe is idle again: it can be started once the worker has ended.
        let level = eventually(|| probe.poll(None, |_| steady(0.4)).level);
        assert_eq!(level, 0.4);
    }
}
