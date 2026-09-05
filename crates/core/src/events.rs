//! Phase 2 event types emitted by the manager.
//!
//! Events are protocol-neutral and carry no Tauri/GUI dependency, so a future
//! GUI can subscribe to the manager's event bus and render them directly.

use serde::{Deserialize, Serialize};

use crate::{DownloadId, DownloadState};

/// A lifecycle event for a single download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// A download was accepted into the queue.
    Queued(DownloadId),
    /// The manager asked a backend to begin the transfer.
    Started(DownloadId),
    /// Bytes were transferred.
    Progress {
        /// The download the progress belongs to.
        id: DownloadId,
        /// Bytes transferred so far.
        downloaded_bytes: u64,
        /// Total expected bytes, if known.
        total_bytes: Option<u64>,
    },
    /// The running transfer was paused by the user.
    Paused(DownloadId),
    /// A paused download was resumed (re-queued).
    Resumed(DownloadId),
    /// The transfer finished and the file is on disk.
    Completed(DownloadId),
    /// The transfer ended in an error.
    Failed {
        /// The download that failed.
        id: DownloadId,
        /// Human-readable error description.
        error: String,
    },
    /// The user cancelled the transfer.
    Cancelled(DownloadId),
    /// The download moved from one lifecycle state to another.
    StateChanged {
        /// The download whose state changed.
        id: DownloadId,
        /// The previous state.
        from: DownloadState,
        /// The new state.
        to: DownloadState,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DownloadState;

    #[test]
    fn events_are_cloneable_and_comparable() {
        let a = Event::Started(DownloadId::new());
        let b = a.clone();
        assert_eq!(a, b);

        let c = Event::StateChanged {
            id: DownloadId::new(),
            from: DownloadState::Queued,
            to: DownloadState::Starting,
        };
        let d = c.clone();
        assert_eq!(c, d);
    }
}
