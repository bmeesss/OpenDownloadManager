//! Shared domain models and error types for OpenDownloadManager.

use std::time::SystemTime;

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
    /// Wall-clock time at which the snapshot was taken.
    ///
    /// This is a [`SystemTime`] rather than an [`std::time::Instant`]:
    /// an `Instant` is only meaningful inside the process that created
    /// it and cannot be serialised, whereas progress snapshots are
    /// expected to be forwarded to other layers (and, later, across
    /// process boundaries to a GUI).
    pub at: SystemTime,
}

impl DownloadProgress {
    /// Computes the completion percentage when the total size is known.
    ///
    /// The result is clamped to `0.0..=100.0`: a server may understate
    /// `Content-Length`, and reporting more than 100% is never useful.
    ///
    /// Returns `None` when the total is unknown *or* zero. A zero-byte
    /// expectation carries no information about how much has arrived, so
    /// reporting 100% for it would be a fabrication.
    #[must_use]
    pub fn percent(&self) -> Option<f64> {
        self.total_bytes
            .filter(|total| *total > 0)
            .map(|total| {
                let pct = (self.downloaded_bytes as f64 / total as f64) * 100.0;
                pct.clamp(0.0, 100.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(downloaded: u64, total: Option<u64>) -> DownloadProgress {
        DownloadProgress {
            downloaded_bytes: downloaded,
            total_bytes: total,
            at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn percent_is_none_when_total_is_unknown() {
        assert_eq!(progress(512, None).percent(), None);
    }

    #[test]
    fn percent_is_none_when_total_is_zero() {
        // A zero-byte expectation says nothing about progress; it must not
        // be reported as a fake 100%.
        assert_eq!(progress(0, Some(0)).percent(), None);
        assert_eq!(progress(1024, Some(0)).percent(), None);
    }

    #[test]
    fn percent_reports_fraction_complete() {
        let pct = progress(256, Some(1024)).percent().expect("some");
        assert!((pct - 25.0).abs() < f64::EPSILON, "got {pct}");
    }

    #[test]
    fn percent_reports_100_when_complete() {
        let pct = progress(1024, Some(1024)).percent().expect("some");
        assert!((pct - 100.0).abs() < f64::EPSILON, "got {pct}");
    }

    #[test]
    fn percent_is_clamped_when_server_understates_length() {
        // A lying server must not push the reported percentage past 100.
        let pct = progress(4096, Some(1024)).percent().expect("some");
        assert!((pct - 100.0).abs() < f64::EPSILON, "got {pct}");
    }
}
