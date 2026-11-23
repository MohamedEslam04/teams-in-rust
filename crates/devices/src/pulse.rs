//! The sound server (PulseAudio, or PipeWire through its PulseAudio service) on Linux: its sinks
//! and sources by name through `pactl`, and pointing the next stream at one of them.
//!
//! Opening the raw ALSA cards of a machine that runs a sound server fights the server for them
//! (`snd_pcm_avail_delay failed … I/O error`) and plays nowhere near the user's chosen
//! Bluetooth earbuds. Streams go through the server instead: cpal opens the `pulse` ALSA PCM (or
//! the system default one, which is the server's on a desktop), and libpulse reads the sink or
//! source to use from `PULSE_SINK` / `PULSE_SOURCE` when the stream connects.

use crate::AudioDevice;
use crate::names::{node_device, parse_nodes, pulse_node};
use std::process::Command;

/// `pactl` with a plain-C locale; its standard output, or `None` when it is missing, cannot reach
/// a server or fails.
fn pactl(args: &[&str]) -> Option<String> {
    let out = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The server's sinks (`input` false) or sources, `None` when no server answers (the ALSA list
/// is used then).
pub(crate) fn list(input: bool) -> Option<Vec<AudioDevice>> {
    let nodes = parse_nodes(&pactl(&[
        "-f",
        "json",
        "list",
        if input { "sources" } else { "sinks" },
    ])?)?;
    let default = pactl(&[if input {
        "get-default-source"
    } else {
        "get-default-sink"
    }])
    .map(|s| s.trim().to_owned());
    Some(
        nodes
            .iter()
            .map(|n| node_device(n, default.as_deref()))
            .collect(),
    )
}

/// Points the next stream of this direction at the node `id` names, or at the server's default
/// for `None`. Stays in force until the next call: the stream connects some moments after it is
/// opened.
pub(crate) fn route(id: Option<&str>, input: bool) {
    let var = if input { "PULSE_SOURCE" } else { "PULSE_SINK" };
    set_env(var, pulse_node(id));
}

/// Writes the process environment only when it would change: other threads (libpulse's, the
/// runtime's) read it, and rewriting it for nothing on every call would race them for no gain.
fn set_env(var: &str, value: Option<&str>) {
    let current = std::env::var(var).ok();
    match (value, current.as_deref()) {
        (Some(v), Some(c)) if v == c => {}
        (None, None) => {}
        // SAFETY: the only writers are these calls, one per stream opened; they are rare and
        // skipped when nothing changes, which keeps `setenv` clear of concurrent readers in
        // practice.
        (Some(v), _) => unsafe { std::env::set_var(var, v) },
        (None, Some(_)) => unsafe { std::env::remove_var(var) },
    }
}
