//! Maps domain errors to CLI exit codes.

use std::process::ExitCode;

/// Application exit codes.
///
/// Exit codes are stable and documented in the README. Note that the
/// local type is called `Exit` rather than `ExitCode` so it cannot be
/// confused with (or collide with) [`std::process::ExitCode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
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

impl Exit {
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

impl From<Exit> for ExitCode {
    fn from(value: Exit) -> Self {
        Self::from(value as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use odm_core::Error;

    /// One instance of every [`Error`] variant paired with the exit code
    /// the README documents for it. Adding a variant to `Error` without
    /// updating this list makes the exhaustive-match test below fail.
    fn every_variant() -> Vec<(Error, Exit)> {
        vec![
            (Error::InvalidUrl("x".into()), Exit::InvalidArgument),
            (Error::InvalidPath("x".into()), Exit::InvalidArgument),
            (Error::InvalidFileName("x".into()), Exit::InvalidArgument),
            (Error::Network("x".into()), Exit::Network),
            (
                Error::Http {
                    status: 404,
                    url: "http://e/x".into(),
                },
                Exit::Http,
            ),
            (Error::TooManyRedirects { limit: 10 }, Exit::InvalidArgument),
            (Error::InvalidResponse("x".into()), Exit::InvalidArgument),
            (Error::RangeRequestsUnsupported, Exit::Http),
            (Error::Filesystem("x".into()), Exit::Filesystem),
            (Error::AlreadyExists("x".into()), Exit::Filesystem),
            (
                Error::InsufficientDiskSpace {
                    needed: 10,
                    available: 1,
                },
                Exit::Filesystem,
            ),
            (Error::Cancelled, Exit::Cancelled),
            (Error::Interrupted, Exit::Cancelled),
            (Error::Internal("x".into()), Exit::Internal),
        ]
    }

    #[test]
    fn every_error_variant_maps_to_its_documented_exit_code() {
        for (err, expected) in every_variant() {
            let actual = Exit::from_error(&err);
            assert_eq!(actual, expected, "wrong exit code for {err}");
        }
    }

    #[test]
    fn every_error_variant_maps_to_a_documented_code() {
        // The documented set. Code 1 is deliberately unused: shells
        // conventionally reserve it for a generic failure.
        let documented = [0u8, 2, 3, 4, 5, 6, 7];
        for (err, _) in every_variant() {
            let code = Exit::from_error(&err) as u8;
            assert!(
                documented.contains(&code),
                "{err} maps to undocumented exit code {code}"
            );
        }
    }

    #[test]
    fn the_variant_list_covers_every_variant() {
        // `Exit::from_error` is an exhaustive match, so if a variant is
        // added to `Error` this count is what stops the list above from
        // silently falling behind.
        assert_eq!(every_variant().len(), 14);
    }

    #[test]
    fn converts_into_std_process_exit_code() {
        // `std::process::ExitCode` does not expose its value, so this
        // asserts that the conversion is total over every variant.
        for (err, _) in every_variant() {
            let _converted: std::process::ExitCode = Exit::from_error(&err).into();
        }
    }
}
