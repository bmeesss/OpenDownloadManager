//! Maps domain errors to CLI exit codes.

use std::process::ExitCode;

/// Application exit codes.
///
/// Exit codes are stable and documented in the user-facing docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Successful execution.
    Success = 0,
    /// Invalid argument (bad URL, bad path, bad flag).
    InvalidArgument = 2,
    /// Network error (DNS, connect, TLS, timeout).
    Network = 3,
    /// HTTP error (4xx, 5xx).
    Http = 4,
    /// Filesystem error (permissions, disk full, ...).
    Filesystem = 5,
    /// User-initiated cancellation.
    Cancelled = 6,
    /// Internal/unexpected error.
    Internal = 7,
}

impl ExitCode {
    /// Maps a domain error into the appropriate exit code.
    #[must_use]
    pub fn from_error(err: &odm_core::Error) -> Self {
        use odm_core::Error;
        match err {
            Error::InvalidUrl(_)
            | Error::InvalidPath(_)
            | Error::InvalidFileName(_)
            | Error::InvalidResponse(_)
            | Error::TooManyRedirects { .. } => Self::InvalidArgument,
            Error::Network(_) => Self::Network,
            Error::Http { .. } | Error::RangeRequestsUnsupported => Self::Http,
            Error::Filesystem(_)
            | Error::InsufficientDiskSpace { .. }
            | Error::AlreadyExists(_) => Self::Filesystem,
            Error::Cancelled | Error::Interrupted => Self::Cancelled,
            Error::Internal(_) => Self::Internal,
        }
    }
}

impl From<ExitCode> for ExitCode /* std::process::ExitCode */ {
    fn from(value: ExitCode) -> Self {
        value
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(value: ExitCode) -> Self {
        std::process::ExitCode::from(value as u8)
    }
}
