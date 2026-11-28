//! Telemetry-free diagnostics: "Report a problem" writes a local `.tar.gz` that the user can
//! read and attach to a bug report themselves. Nothing is ever uploaded.
//!
//! Every text that goes into the bundle passes through [`redact`] first.

mod bundle;
mod redact;

pub use bundle::{
    Error, default_output_dir, report_from_args, report_from_args_with, write_bundle,
    write_bundle_with,
};
pub use redact::redact;
