//! BitTorrent backend error type.

use thiserror::Error;

/// Result alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Canonical error type for the torrent backend.
#[derive(Debug, Clone, Error)]
pub enum Error {
    /// The input could not be parsed as a magnet URI or torrent file.
    #[error("invalid torrent input: {0}")]
    InvalidInput(String),

    /// A file path inside the torrent was rejected by the path validator.
    #[error("invalid torrent path: {0}")]
    InvalidPath(String),

    /// The torrent metadata contains duplicate or colliding paths.
    #[error("duplicate torrent path: {0}")]
    DuplicatePath(String),

    /// The torrent contains a path that would escape the output root.
    #[error("path escapes output root: {0}")]
    PathEscapesRoot(String),

    /// The destination already exists and overwrite was not allowed.
    #[error("destination already exists: {0}")]
    AlreadyExists(String),

    /// The download was cancelled by the user or the orchestrator.
    #[error("torrent cancelled")]
    Cancelled,

    /// A network-level failure.
    #[error("torrent network error: {0}")]
    Network(String),

    /// A filesystem error occurred.
    #[error("torrent filesystem error: {0}")]
    Filesystem(String),

    /// The backend received an unexpected internal state.
    #[error("torrent internal error: {0}")]
    Internal(String),
}
