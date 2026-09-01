//! Shared domain models and error types for OpenDownloadManager.
//!
//! This crate is the lowest layer of the dependency graph. It defines
//! the vocabulary used by every other crate: download resources,
//! metadata, progress snapshots, and the canonical error type.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod model;

pub use error::{Error, Result};
pub use model::{
    DownloadProgress, DownloadRequest, DownloadSummary, HttpMethod, InspectInfo, ProgressSink,
    ResourceInfo,
};
