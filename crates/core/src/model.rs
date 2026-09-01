//! Domain models used across the workspace.

use std::time::Instant;

use url::Url;

/// HTTP methods supported by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// HTTP GET.
    Get,
    /// HTTP HEAD.
    Head,
}

impl HttpMethod {
    /// Returns the canonical method name (uppercase).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
        }
    }
}

/// A user request to download a single resource.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    /// The URL to download from. Must be `http` or `https`.
    pub url: Url,
    /// The final output path on disk.
    pub output: std::path::PathBuf,
    /// Whether to overwrite an existing file at `output`.
    pub overwrite: bool,
    /// Optional cap on the number of redirect hops.
    pub max_redirects: usize,
}

/// Information extracted from an `InspectInfo` HEAD/GET request.
#[derive(Debug, Clone, Default)]
pub struct ResourceInfo {
    /// Final URL after redirects.
    pub final_url: Url,
    /// `Content-Length` if announced by the server.
    pub content_length: Option<u64>,
    /// `Content-Type` if announced by the server.
    pub content_type: Option<String>,
    /// `ETag` if announced by the server.
    pub etag: Option<String>,
    /// `Last-Modified` if announced by the server.
    pub last_modified: Option<String>,
    /// `Accept-Ranges: bytes` capability flag.
    pub accepts_ranges: bool,
    /// A filename suggested by the server (e.g. via `Content-Disposition`).
    pub suggested_filename: Option<String>,
}

/// Result of a `HEAD` or small initial `GET` used to inspect a resource.
#[derive(Debug, Clone)]
pub struct InspectInfo {
    /// HTTP status code.
    pub status: u16,
    /// Resource metadata.
    pub resource: ResourceInfo,
}

/// A progress update emitted during a download.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Number of bytes transferred so far.
    pub downloaded_bytes: u64,
    /// Total bytes expected, if known.
    pub total_bytes: Option<u64>,
    /// Wall-clock instant the snapshot was taken.
    pub at: Instant,
}

impl DownloadProgress {
    /// Computes the percentage (0..=100) when total is known.
    #[must_use]
    pub fn percent(&self) -> Option<f64> {
        self.total_bytes.and_then(|t| {
            if t == 0 {
                Some(100.0)
            } else {
                Some((self.downloaded_bytes as f64 / t as f64) * 100.0)
            }
        })
    }
}

/// Final summary of a completed download.
#[derive(Debug, Clone)]
pub struct DownloadSummary {
    /// Final output path.
    pub output: std::path::PathBuf,
    /// Total bytes transferred.
    pub total_bytes: u64,
    /// Wall-clock duration of the transfer.
    pub duration: std::time::Duration,
    /// Average throughput, in bytes per second.
    pub average_bytes_per_sec: f64,
    /// Final URL after redirects.
    pub final_url: Url,
}

/// Callback for streaming progress updates.
pub trait ProgressSink: Send + Sync {
    /// Called with each progress snapshot. Implementations should be cheap.
    fn on_progress(&self, progress: DownloadProgress);
}
