//! The boundary between the protocol-neutral manager and protocol-specific
//! backends.
//!
//! The manager drives downloads exclusively through [`Backend`]. A backend
//! receives a [`BackendTask`] (protocol-neutral) and reports progress and a
//! final [`BackendOutcome`]. This keeps `odm-core` free of any transport
//! (HTTP, BitTorrent, ...) and lets new protocols be added as sibling
//! backends without the manager knowing about them.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use core::fmt;
use core::str::FromStr;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use uuid::Uuid;
use url::Url;

use crate::{Error, ProgressSink, Result};

/// Identifies which protocol a download uses.
///
/// This is the only protocol tag stored on a download. Protocol-specific
/// details (HTTP resume info, a torrent's info hash, ...) live opaquely in the
/// download's `backend_meta` blob, never as generic columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// HTTP/HTTPS transfers executed by `odm-download-engine`.
    Http,
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendKind::Http => f.write_str("http"),
        }
    }
}

impl FromStr for BackendKind {
    type Err = Error;

    /// Parses a backend kind from its stored string form.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] for an unknown kind.
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "http" => Ok(BackendKind::Http),
            other => Err(Error::Internal(format!("unknown backend kind: {other}"))),
        }
    }
}

/// A stable, process-independent identifier for a download.
///
/// Persisted as text and safe to use as a database primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DownloadId(pub Uuid);

impl DownloadId {
    /// Generates a new random identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DownloadId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for DownloadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for DownloadId {
    type Err = Error;

    /// Parses a download id from its textual form.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if `s` is not a valid UUID.
    fn from_str(s: &str) -> Result<Self> {
        Uuid::parse_str(s)
            .map(DownloadId)
            .map_err(|e| Error::Internal(format!("invalid download id: {e}")))
    }
}

/// A byte-rate limiter shared between the manager's bandwidth policy and a
/// backend's transfer loop.
///
/// Implementations must sleep (not busy-wait) when capacity is exhausted, so
/// they can be awaited from an async transfer loop without pinning a runtime
/// thread.
#[async_trait]
pub trait RateLimiter: Send + Sync + 'static {
    /// Waits until `bytes` units of capacity are available and then consumes
    /// them.
    async fn acquire(&self, bytes: u64);
}

/// Everything a backend needs to execute one download, handed to it by the
/// manager. Protocol-neutral: the backend translates it into its own request
/// type (e.g. the HTTP backend builds an [`crate::DownloadRequest`]).
pub struct BackendTask {
    /// Stable id of the download.
    pub id: DownloadId,
    /// Source URL.
    pub url: Url,
    /// Final on-disk destination.
    pub destination: PathBuf,
    /// Whether an existing file at `destination` may be replaced.
    pub overwrite: bool,
    /// Protocol-specific metadata (HTTP resume info, torrent info hash, ...).
    /// Owned by the backend; the manager stores it opaquely.
    pub backend_meta: serde_json::Value,
    /// Progress reporter. The backend should call it as bytes arrive.
    pub progress: Option<Arc<dyn ProgressSink>>,
    /// Cancellation signal. Once notified the backend must stop and return
    /// [`Error::Cancelled`] promptly.
    pub cancel: Option<Arc<Notify>>,
    /// Optional byte-rate limiter applied to the transfer.
    pub rate_limiter: Option<Arc<dyn RateLimiter>>,
}

/// The result a backend reports when a transfer ends.
pub struct BackendOutcome {
    /// Bytes transferred (authoritative once the transfer succeeds).
    pub downloaded_bytes: u64,
    /// Total expected bytes, if known.
    pub total_bytes: Option<u64>,
    /// Updated protocol-specific metadata to persist (e.g. an observed ETag).
    pub backend_meta: serde_json::Value,
}

/// A protocol-specific download backend owned by the manager.
///
/// The manager never uses a transport directly; it routes a download to the
/// backend whose [`Backend::kind`] matches the download's [`BackendKind`] and
/// asks that backend to [`Backend::run`] the download.
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    /// Which protocol this backend serves.
    fn kind(&self) -> BackendKind;

    /// Executes `task` to completion.
    ///
    /// # Errors
    /// Returns the terminal [`Error`] for the transfer: [`Error::Cancelled`]
    /// when `task.cancel` was signalled, or any other error that ended the
    /// transfer.
    async fn run(&self, task: BackendTask) -> Result<BackendOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_round_trips() {
        assert_eq!("http".parse::<BackendKind>().unwrap(), BackendKind::Http);
        assert_eq!(BackendKind::Http.to_string(), "http");
        assert!("ftp".parse::<BackendKind>().is_err());
    }

    #[test]
    fn download_id_round_trips() {
        let id = DownloadId::new();
        let text = id.to_string();
        assert_eq!(text.parse::<DownloadId>().unwrap(), id);
        assert!("not-a-uuid".parse::<DownloadId>().is_err());
    }
}
