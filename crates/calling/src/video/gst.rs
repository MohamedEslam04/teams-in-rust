//! A `gst-launch-1.0` child writing raw I420 frames of one fixed size on its stdout, as the
//! screen and the camera capture both run it (no GStreamer or PipeWire development files at build
//! time).

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::I420Frame;
use crate::{Error, Result};

/// Frames buffered between the capture and its consumer; older ones are dropped, not queued.
const FRAME_QUEUE: usize = 2;

/// Overrides where `gst-launch-1.0` is found.
pub const GST_LAUNCH_ENV: &str = "IRONTEAMS_GST_LAUNCH";

/// The `gst-launch-1.0` to run: `$IRONTEAMS_GST_LAUNCH`, else where the official GStreamer installers
/// put it when that is not on `PATH` (an app started from Finder or the Start menu does not see a
/// shell's `PATH`), else the bare name, found on `PATH`.
pub(crate) fn gst_launch() -> std::path::PathBuf {
    if let Some(p) = compat::var_os(GST_LAUNCH_ENV).filter(|p| !p.is_empty()) {
        return p.into();
    }
    installed_gst_launch()
        .filter(|p| p.is_file())
        .unwrap_or_else(|| "gst-launch-1.0".into())
}

#[cfg(windows)]
fn installed_gst_launch() -> Option<std::path::PathBuf> {
    // The MSVC installer sets this to its root (for example `C:\gstreamer\1.0\msvc_x86_64\`).
    [
        "GSTREAMER_1_0_ROOT_MSVC_X86_64",
        "GSTREAMER_1_0_ROOT_X86_64",
    ]
    .into_iter()
    .find_map(std::env::var_os)
    .map(|root| {
        std::path::Path::new(&root)
            .join("bin")
            .join("gst-launch-1.0.exe")
    })
}

#[cfg(target_os = "macos")]
fn installed_gst_launch() -> Option<std::path::PathBuf> {
    [
        "/Library/Frameworks/GStreamer.framework/Versions/1.0/bin/gst-launch-1.0",
        "/opt/homebrew/bin/gst-launch-1.0",
        "/usr/local/bin/gst-launch-1.0",
    ]
    .into_iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.is_file())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn installed_gst_launch() -> Option<std::path::PathBuf> {
    None
}

/// Whether a line `gst-launch-1.0` wrote to stderr reports a failed pipeline
/// (`ERROR: from element ...`, `ERROR: pipeline doesn't want to preroll.`).
pub(crate) fn is_error_line(line: &str) -> bool {
    line.contains("ERROR:")
}

/// A running `gst-launch-1.0` and the frames it writes.
pub(crate) struct GstFrames {
    pub frames: mpsc::Receiver<I420Frame>,
    /// Its stderr lines that report an error ([`is_error_line`]); `gst-launch-1.0` can print one
    /// and then keep its pipes open for a long time, with no frame and no end of file.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub errors: mpsc::Receiver<String>,
    pub child: Child,
    pub reader: JoinHandle<()>,
}

/// Starts `gst-launch-1.0 <args>` with `stdin` as its fd 0 and reads `size` I420 frames from its
/// stdout. Its stderr is logged at info, error lines also go to [`GstFrames::errors`]. `what`
/// names the capture in logs.
pub(crate) fn spawn(
    args: Vec<String>,
    stdin: Stdio,
    size: (u32, u32),
    what: &'static str,
) -> Result<GstFrames> {
    let mut command = Command::new(gst_launch());
    // No console window flashing up next to the app on Windows.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = command
        .args(args)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            Error::Media(format!(
                "gst-launch-1.0 (GStreamer) not started: {e}. {}",
                if cfg!(target_os = "linux") {
                    "Install GStreamer and its PipeWire plugin."
                } else {
                    "Install the GStreamer runtime from gstreamer.freedesktop.org."
                }
            ))
        })?;
    let (err_tx, errors) = mpsc::channel(1);
    let mut out = child
        .stdout
        .take()
        .ok_or_else(|| Error::Media("gst-launch-1.0 has no stdout".into()))?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut stderr = BufReader::new(stderr);
            let mut raw = Vec::new();
            loop {
                raw.clear();
                match stderr.read_until(b'\n', &mut raw).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&raw);
                let line = line.trim_end();
                tracing::debug!("{what} (gst-launch-1.0): {line}");
                if is_error_line(line) {
                    // Only the first one matters, and nobody may be listening.
                    let _ = err_tx.try_send(line.to_owned());
                }
            }
        });
    }
    let (tx, frames) = mpsc::channel(FRAME_QUEUE);
    let (w, h) = (size.0 as usize, size.1 as usize);
    let reader = tokio::spawn(async move {
        let len = I420Frame::len_for(w, h);
        loop {
            let mut data = vec![0; len];
            if let Err(e) = out.read_exact(&mut data).await {
                tracing::info!(%e, "{what} ended");
                return;
            }
            let Ok(frame) = I420Frame::new(w, h, data) else {
                return;
            };
            match tx.try_send(frame) {
                // The consumer is behind: this frame is dropped, a newer one follows.
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
    });
    Ok(GstFrames {
        frames,
        errors,
        child,
        reader,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_lines_of_gst_launch_are_told_from_the_rest() {
        assert!(is_error_line(
            "ERROR: from element /GstPipeline:pipeline0/GstPipeWireSrc:pipewiresrc0: stream error: target not found"
        ));
        assert!(is_error_line("ERROR: pipeline doesn't want to preroll."));
        assert!(!is_error_line("Setting pipeline to PAUSED ..."));
        assert!(!is_error_line(
            "WARNING: from element /GstPipeline:pipeline0: Could not read"
        ));
        assert!(!is_error_line(""));
    }
}
