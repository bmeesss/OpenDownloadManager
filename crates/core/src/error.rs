//! Canonical error type for OpenDownloadManager.

use thiserror::Error;

/// Result alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Canonical error type used across the workspace.
///
/// Each variant carries enough context for callers to make decisions
/// and to be translated into CLI exit codes or GUI error dialogs.
#[derive(Debug, Error)]
pub enum Error {
    /// A URL could not be parsed or did not use an accepted scheme.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// A filesystem path was rejected by the path validator.
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// A file name was rejected (empty, control chars, reserved name, etc.).
    #[error("invalid file name: {0}")]
    InvalidFileName(String),

    /// A network-level failure (DNS, TCP, TLS handshake, timeout, ...).
    #[error("network error: {0}")]
    Network(String),

    /// An HTTP error response was received.
    #[error("HTTP error: status {status} for {url}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// URL the error originated from.
        url: String,
    },

    /// A redirect chain was too long or invalid.
    #[error("too many redirects (limit: {limit})")]
    TooManyRedirects {
        /// Configured redirect cap.
        limit: usize,
    },

    /// The server sent an unexpected or malformed response.
    #[error("invalid HTTP response: {0}")]
    InvalidResponse(String),

    /// The server reported an unsupported `Accept-Ranges` value when
    /// one was required (reserved for future Phase 2 logic, but the
    /// variant exists for type stability).
    #[error("range requests not supported by server")]
    RangeRequestsUnsupported,

    /// A filesystem error occurred.
    #[error("filesystem error: {0}")]
    Filesystem(String),

    /// The target file already exists and overwriting was not allowed.
    #[error("file already exists at {0}")]
    AlreadyExists(String),

    /// There was not enough free disk space for the download.
    #[error("insufficient disk space: need {needed} bytes, have {available}")]
    InsufficientDiskSpace {
        /// Required bytes.
        needed: u64,
        /// Available bytes.
        available: u64,
    },

    /// The download was cancelled by the user or the orchestrator.
    #[error("download cancelled")]
    Cancelled,

    /// The download was interrupted unexpectedly and could not be
    /// resumed within the same run.
    #[error("download interrupted")]
    Interrupted,

    /// An internal invariant was violated. This is a bug.
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Returns `true` if the error is fatal and should not be retried.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::InvalidUrl(_)
                | Self::InvalidPath(_)
                | Self::InvalidFileName(_)
                | Self::Http { .. }
                | Self::TooManyRedirects { .. }
                | Self::InvalidResponse(_)
                | Self::AlreadyExists(_)
                | Self::InsufficientDiskSpace { .. }
                | Self::Cancelled
                | Self::Internal(_)
        )
    }
}
