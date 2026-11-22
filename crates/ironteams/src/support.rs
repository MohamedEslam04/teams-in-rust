//! Helpers the UI thread and the engine share without depending on each other: the banner's
//! error wording and the quiet rules (quiet hours, Focus mode, keyword alerts).

pub(crate) mod error_text;
pub(crate) mod quiet;
