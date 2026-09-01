//! Application-level composition root for the OpenDownloadManager.
//!
//! This crate exists to give the workspace a non-binary shared
//! library surface for future Phase 2 applications (e.g. the Tauri
//! GUI). In Phase 1 it is intentionally minimal.

#![deny(unsafe_code)]
#![warn(missing_docs)]

/// Returns the human-readable application name.
#[must_use]
pub fn app_name() -> &'static str {
    "OpenDownloadManager"
}

/// Returns the current application version.
#[must_use]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
