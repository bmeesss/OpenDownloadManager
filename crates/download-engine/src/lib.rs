//! Download orchestration.
//!
//! Glues together the HTTP client and the storage layer, with
//! end-to-end streaming, atomic finalization, and progress reporting.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod backend;
mod engine;
mod naming;

pub use engine::{DownloadEngine, DownloadOptions, EngineConfig};
pub use naming::default_output_for;
