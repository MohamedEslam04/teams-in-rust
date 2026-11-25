//! Helpers the UI thread and the engine share without depending on each other: the banner's
//! error wording, the quiet rules (quiet hours, Focus mode, keyword alerts) and the freeze
//! watchdog.

pub(crate) mod error_text;
pub(crate) mod quiet;
pub(crate) mod watchdog;
