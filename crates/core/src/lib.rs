//! Shared domain models and error types for OpenDownloadManager.
//!
//! This crate is the lowest layer of the dependency graph. It defines
//! the vocabulary used by every other crate: download resources,
//! metadata, progress snapshots, the canonical error type, and — for Phase 2 —
//! the protocol-neutral download lifecycle, the backend boundary and the
//! manager event types.
//!
//! Nothing in this crate is tied to a specific transport. HTTP-specific types
//! such as [`HttpMethod`], [`ResourceInfo`] and [`InspectInfo`] still live here
//! for historical reasons, but no new transport-specific concept is added; the
//! Phase 2 extensions ([`DownloadState`], [`Backend`], [`Event`], ...) are all
//! protocol-neutral.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod backend;
mod error;
mod events;
mod model;
mod state;

pub use backend::{
    Backend, BackendKind, BackendOutcome, BackendTask, DownloadId, RateLimiter,
};
pub use error::{Error, Result};
pub use events::Event;
pub use model::{
    DownloadProgress, DownloadRequest, DownloadSummary, HttpMethod, InspectInfo, ProgressSink,
    ResourceInfo,
};
pub use state::DownloadState;
