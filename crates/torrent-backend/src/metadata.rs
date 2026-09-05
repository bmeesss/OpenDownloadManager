//! Minimal durable metadata for a torrent download.
//!
//! Stored opaquely in `backend_meta`. No piece bitfields, no progress,
//! no file lists, no `TorrentId`, no `preferred_id`.

use serde::{Deserialize, Serialize};
use url::Url;

/// Persistent metadata for a single torrent download.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendMeta {
    /// Schema version.
    pub v: u32,
    /// 40-hex info hash (lowercase).
    pub info_hash: String,
    /// Where the torrent bytes came from.
    pub source: TorrentSource,
    /// ODM-owned path to a stored copy of the torrent file, or `None`.
    pub torrent_file: Option<String>,
}

/// Where the torrent bytes originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TorrentSource {
    /// A magnet URI.
    Magnet,
    /// An HTTP/HTTPS/FTP torrent URL.
    TorrentUrl,
    /// A local `.torrent` file.
    TorrentFile,
}

impl BackendMeta {
    /// Creates new metadata for a magnet URI.
    #[must_use]
    pub fn for_magnet(info_hash: impl Into<String>) -> Self {
        Self {
            v: 1,
            info_hash: info_hash.into(),
            source: TorrentSource::Magnet,
            torrent_file: None,
        }
    }

    /// Creates new metadata for a `.torrent` URL or local file.
    #[must_use]
    pub fn for_torrent(info_hash: impl Into<String>, stored_path: Option<String>) -> Self {
        Self {
            v: 1,
            info_hash: info_hash.into(),
            source: TorrentSource::TorrentUrl,
            torrent_file: stored_path,
        }
    }

    /// Creates new metadata for a local `.torrent` file.
    #[must_use]
    pub fn for_torrent_file(info_hash: impl Into<String>, stored_path: String) -> Self {
        Self {
            v: 1,
            info_hash: info_hash.into(),
            source: TorrentSource::TorrentFile,
            torrent_file: Some(stored_path),
        }
    }
}

/// Determines the source type from a URL or file path.
#[allow(dead_code)]
pub fn classify_input(url: &Url) -> TorrentSource {
    if url.scheme() == "magnet" {
        TorrentSource::Magnet
    } else {
        TorrentSource::TorrentUrl
    }
}
