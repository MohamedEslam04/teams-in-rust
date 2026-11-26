//! Helpers the UI thread and the engine share without depending on each other: the banner's
//! error wording, the microphone meter's worker, the quiet rules (quiet hours, Focus mode,
//! keyword alerts), the freeze watchdog and the images placed between the composer's lines.

pub(crate) mod error_text;
pub(crate) mod inline_images;
pub(crate) mod meter;
pub(crate) mod quiet;
pub(crate) mod watchdog;
