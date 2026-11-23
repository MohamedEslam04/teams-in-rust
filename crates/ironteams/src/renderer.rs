//! Starting the window's renderer: the CPU one (Slint's software renderer, the default) or the
//! GPU one (femtovg), chosen before the Slint backend is selected.
//!
//! The choice is `IRONTEAMS_RENDERER` > the saved setting (`general.json`, read with a plain file
//! read, see `appearance::renderer`) > the CPU renderer. A renderer cannot be swapped while the app
//! runs, so Settings saves the choice and shows what is running now (see [`running`]).
//!
//! When the GPU renderer cannot start (no OpenGL, headless, a driver error) the app starts on the
//! CPU renderer instead and says so in Settings, so a bad GPU never keeps IronTeams from opening.

use appearance::Renderer;
use std::sync::OnceLock;

/// What was asked for at start and what is drawing now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Started {
    pub wanted: Renderer,
    pub running: Renderer,
}

impl Started {
    /// The GPU renderer was wanted but could not start, so the CPU one runs.
    pub fn fell_back(&self) -> bool {
        self.wanted != self.running
    }
}

impl Default for Started {
    fn default() -> Self {
        Started {
            wanted: Renderer::Cpu,
            running: Renderer::Cpu,
        }
    }
}

/// Starts `wanted` with `select` (which gets the Slint renderer name and selects it). A GPU
/// renderer that fails is logged and the CPU one is tried; the CPU one failing is an error.
pub fn start<E: std::fmt::Display>(
    wanted: Renderer,
    mut select: impl FnMut(&str) -> Result<(), E>,
) -> Result<Started, E> {
    match select(wanted.slint_name()) {
        Ok(()) => Ok(Started {
            wanted,
            running: wanted,
        }),
        Err(e) if wanted == Renderer::Gpu => {
            tracing::warn!(error = %e, "the GPU renderer could not start, using the CPU renderer");
            select(Renderer::Cpu.slint_name())?;
            Ok(Started {
                wanted,
                running: Renderer::Cpu,
            })
        }
        Err(e) => Err(e),
    }
}

/// The renderer to start with, from the process environment and the saved setting.
pub fn wanted(saved: Renderer) -> Renderer {
    appearance::renderer::resolve(env().as_deref(), saved)
}

/// `IRONTEAMS_RENDERER`, when set.
pub fn env() -> Option<String> {
    compat::var("IRONTEAMS_RENDERER").ok()
}

static STARTED: OnceLock<Started> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static FOR_TEST: std::cell::Cell<Option<Started>> = const { std::cell::Cell::new(None) };
}

/// Records what `start` returned, for Settings. Only the first call counts.
pub fn record(started: Started) {
    let _ = STARTED.set(started);
}

/// What is drawing now. The CPU renderer when nothing was recorded (tests, the other processes).
pub fn running() -> Started {
    #[cfg(test)]
    if let Some(s) = FOR_TEST.with(|c| c.get()) {
        return s;
    }
    STARTED.get().copied().unwrap_or_default()
}

/// Makes `running` answer `started` on this thread (the UI tests have no GPU to start).
#[cfg(test)]
pub fn set_running_for_test(started: Started) {
    FOR_TEST.with(|c| c.set(Some(started)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A selection that works for the renderers in `ok` and says why for the others.
    fn selecting<'a>(
        ok: &'a [&'a str],
        tried: &'a RefCell<Vec<String>>,
    ) -> impl FnMut(&str) -> Result<(), String> + 'a {
        move |name| {
            tried.borrow_mut().push(name.to_owned());
            if ok.contains(&name) {
                Ok(())
            } else {
                Err(format!("no {name} here"))
            }
        }
    }

    #[test]
    fn the_wanted_renderer_runs_when_it_starts() {
        let tried = RefCell::new(vec![]);
        let s = start(Renderer::Gpu, selecting(&["femtovg", "software"], &tried)).unwrap();
        assert_eq!(s.running, Renderer::Gpu);
        assert!(!s.fell_back());
        assert_eq!(*tried.borrow(), ["femtovg"], "one attempt, no fall-back");

        let tried = RefCell::new(vec![]);
        let s = start(Renderer::Cpu, selecting(&["software"], &tried)).unwrap();
        assert_eq!(s.running, Renderer::Cpu);
        assert!(!s.fell_back());
        assert_eq!(*tried.borrow(), ["software"]);
    }

    #[test]
    fn a_gpu_that_cannot_start_falls_back_to_the_cpu_renderer() {
        let tried = RefCell::new(vec![]);
        let s = start(Renderer::Gpu, selecting(&["software"], &tried)).unwrap();
        assert_eq!(s.wanted, Renderer::Gpu);
        assert_eq!(s.running, Renderer::Cpu);
        assert!(s.fell_back());
        assert_eq!(*tried.borrow(), ["femtovg", "software"]);
    }

    #[test]
    fn the_cpu_renderer_failing_is_an_error_and_is_tried_once() {
        let tried = RefCell::new(vec![]);
        let e = start(Renderer::Cpu, selecting(&[], &tried)).unwrap_err();
        assert_eq!(e, "no software here");
        assert_eq!(*tried.borrow(), ["software"]);

        // The GPU renderer failing and then the CPU one too: the CPU error is the one returned.
        let tried = RefCell::new(vec![]);
        let e = start(Renderer::Gpu, selecting(&[], &tried)).unwrap_err();
        assert_eq!(e, "no software here");
        assert_eq!(*tried.borrow(), ["femtovg", "software"]);
    }

    #[test]
    fn nothing_recorded_means_the_cpu_renderer() {
        assert_eq!(Started::default().running, Renderer::Cpu);
        assert!(!Started::default().fell_back());
    }
}
