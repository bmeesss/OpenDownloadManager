//! Protocol-neutral download manager for OpenDownloadManager.
//!
//! This crate owns everything that is independent of *how* bytes travel:
//! the queue, the scheduler, the download lifecycle state machine, SQLite
//! persistence, the event bus and the bandwidth policy. It drives
//! protocol-specific backends (such as the HTTP backend in
//! `odm-download-engine`) exclusively through the [`odm_core::Backend`]
//! boundary, so it never depends on a transport such as `reqwest` directly.
//!
//! Design rules honoured here:
//! * `odm-core` stays protocol-neutral; no HTTP-specific concept lives in it.
//! * The manager never touches a transport; it only owns backends.
//! * The generic download record stores only protocol-neutral columns plus an
//!   opaque `backend_meta` blob, so a future BitTorrent backend fits without
//!   new generic columns.
//! * No premature abstraction: the backend interface is the single boundary
//!   actually needed to manage the existing HTTP engine today.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod bandwidth;
mod config;
mod events;
mod manager;
mod persistence;
mod queue;
mod scheduler;

pub use bandwidth::{BandwidthPolicy, TokenBucket};
pub use config::{ManagerConfig, RecoveryPolicy};
pub use events::EventBus;
pub use manager::{DownloadManager, DownloadSpec};
pub use odm_core::{Backend, BackendKind, DownloadId, DownloadState, Event};
pub use persistence::Download;
pub use queue::Queue;
pub use scheduler::select_for_start;
