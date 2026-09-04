//! The lifecycle state machine for a managed download.
//!
//! States are protocol-neutral and persisted as their [`std::fmt::Display`]
//! string; they round-trip through [`std::str::FromStr`] so a state stored in
//! SQLite survives process boundaries and later crate versions.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;

/// The lifecycle state of a managed download.
///
/// These states are the only ones the manager reasons about; protocol-specific
/// detail (HTTP headers, torrent pieces, ...) never leaks into this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    /// Accepted by the manager but not yet handed to a backend.
    Queued,
    /// A backend has been asked to begin; no bytes have arrived yet.
    Starting,
    /// Bytes are actively being transferred.
    Downloading,
    /// The running transfer was stopped by the user and can be resumed.
    Paused,
    /// The transfer finished and the file is on disk.
    Completed,
    /// The transfer ended in an error.
    Failed,
    /// The user cancelled the transfer; it will not be retried.
    Cancelled,
}

impl DownloadState {
    /// Returns the states a download in `self` may legally move to.
    #[must_use]
    pub fn next_states(self) -> &'static [DownloadState] {
        match self {
            DownloadState::Queued => &[
                DownloadState::Starting,
                DownloadState::Failed,
                DownloadState::Cancelled,
            ],
            DownloadState::Starting => &[
                DownloadState::Downloading,
                DownloadState::Completed,
                DownloadState::Paused,
                DownloadState::Failed,
                DownloadState::Cancelled,
            ],
            DownloadState::Downloading => &[
                DownloadState::Paused,
                DownloadState::Completed,
                DownloadState::Failed,
                DownloadState::Cancelled,
            ],
            DownloadState::Paused => &[DownloadState::Queued, DownloadState::Cancelled],
            DownloadState::Completed => &[],
            DownloadState::Failed => &[DownloadState::Queued, DownloadState::Cancelled],
            DownloadState::Cancelled => &[],
        }
    }

    /// Returns `true` if a transition from `self` to `next` is legal.
    #[must_use]
    pub fn is_valid_transition(self, next: DownloadState) -> bool {
        self.next_states().contains(&next)
    }

    /// Validates and applies a transition, returning the new state.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the transition is not legal.
    pub fn transition(self, next: DownloadState) -> crate::Result<DownloadState> {
        if self.is_valid_transition(next) {
            Ok(next)
        } else {
            Err(Error::Internal(format!(
                "invalid download state transition: {self} -> {next}"
            )))
        }
    }
}

impl fmt::Display for DownloadState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            DownloadState::Queued => "queued",
            DownloadState::Starting => "starting",
            DownloadState::Downloading => "downloading",
            DownloadState::Paused => "paused",
            DownloadState::Completed => "completed",
            DownloadState::Failed => "failed",
            DownloadState::Cancelled => "cancelled",
        };
        f.write_str(s)
    }
}

impl FromStr for DownloadState {
    type Err = Error;

    /// Parses a state from its stored string form.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] for an unknown state string.
    fn from_str(s: &str) -> crate::Result<Self> {
        match s {
            "queued" => Ok(DownloadState::Queued),
            "starting" => Ok(DownloadState::Starting),
            "downloading" => Ok(DownloadState::Downloading),
            "paused" => Ok(DownloadState::Paused),
            "completed" => Ok(DownloadState::Completed),
            "failed" => Ok(DownloadState::Failed),
            "cancelled" => Ok(DownloadState::Cancelled),
            other => Err(Error::Internal(format!("unknown download state: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_transitions_are_allowed() {
        assert!(DownloadState::Queued.transition(DownloadState::Starting).is_ok());
        assert!(DownloadState::Starting.transition(DownloadState::Downloading).is_ok());
        assert!(DownloadState::Downloading.transition(DownloadState::Paused).is_ok());
        assert!(DownloadState::Paused.transition(DownloadState::Queued).is_ok());
        assert!(DownloadState::Downloading.transition(DownloadState::Completed).is_ok());
        assert!(DownloadState::Starting.transition(DownloadState::Completed).is_ok());
        assert!(DownloadState::Starting.transition(DownloadState::Failed).is_ok());
        assert!(DownloadState::Failed.transition(DownloadState::Queued).is_ok());
        assert!(DownloadState::Queued.transition(DownloadState::Cancelled).is_ok());
        assert!(DownloadState::Paused.transition(DownloadState::Cancelled).is_ok());
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        assert!(DownloadState::Completed.transition(DownloadState::Downloading).is_err());
        assert!(DownloadState::Cancelled.transition(DownloadState::Queued).is_err());
        assert!(DownloadState::Paused.transition(DownloadState::Downloading).is_err());
        assert!(DownloadState::Queued.transition(DownloadState::Completed).is_err());
        assert!(DownloadState::Downloading.transition(DownloadState::Queued).is_err());
        assert!(DownloadState::Completed.transition(DownloadState::Failed).is_err());
    }

    #[test]
    fn terminal_states_have_no_successors() {
        assert!(DownloadState::Completed.next_states().is_empty());
        assert!(DownloadState::Cancelled.next_states().is_empty());
    }

    #[test]
    fn display_and_parse_round_trip() {
        for state in [
            DownloadState::Queued,
            DownloadState::Starting,
            DownloadState::Downloading,
            DownloadState::Paused,
            DownloadState::Completed,
            DownloadState::Failed,
            DownloadState::Cancelled,
        ] {
            let text = state.to_string();
            assert_eq!(text.parse::<DownloadState>().unwrap(), state);
        }
    }
}
