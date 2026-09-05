//! Input parsing and inspection for torrent sources.
//!
//! Supports:
//! * magnet URIs
//! * HTTP/HTTPS `.torrent` URLs
//! * local `.torrent` files

use std::sync::Arc;

use librqbit::{AddTorrent, AddTorrentOptions, ListOnlyResponse, Session};
use url::Url;

use crate::error::{Error, Result};

/// Parsed input ready for inspection or adding to the session.
pub enum ParsedInput {
    Magnet { uri: String },
    TorrentBytes { bytes: Vec<u8> },
    TorrentUrl { url: String },
}

impl ParsedInput {
    /// Recognizes a URL or path as magnet, torrent URL, or local file.
    pub fn parse(url: &Url) -> Result<Self> {
        if url.scheme() == "magnet" {
            let _ = extract_info_hash_from_magnet(url.as_str())
                .ok_or_else(|| Error::Internal("magnet missing info_hash".into()))?;
            Ok(Self::Magnet {
                uri: url.to_string(),
            })
        } else if url.scheme() == "http" || url.scheme() == "https" {
            Ok(Self::TorrentUrl {
                url: url.to_string(),
            })
        } else if url.scheme() == "file" {
            let path = url
                .to_file_path()
                .map_err(|_| Error::Internal("cannot convert file URL to path".into()))?;
            let bytes = std::fs::read(&path)
                .map_err(|e| Error::Internal(format!("read torrent file: {e}")))?;
            Ok(Self::TorrentBytes { bytes })
        } else {
            Err(Error::Internal(format!(
                "unsupported URL scheme: {}",
                url.scheme()
            )))
        }
    }
}

/// Inspects a magnet link using list-only mode.
pub async fn inspect_magnet(session: &Arc<Session>, uri: &str) -> Result<ListOnlyResponse> {
    let add = AddTorrent::from_url(uri);
    let opts = AddTorrentOptions {
        list_only: true,
        overwrite: true,
        ..Default::default()
    };
    let response = session
        .add_torrent(add, Some(opts))
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;
    match response {
        librqbit::AddTorrentResponse::ListOnly(resp) => Ok(resp),
        _ => Err(Error::Internal(
            "unexpected response for list-only magnet".into(),
        )),
    }
}

/// Inspects raw torrent bytes using list-only mode.
pub async fn inspect_torrent_bytes(
    session: &Arc<Session>,
    bytes: Vec<u8>,
) -> Result<ListOnlyResponse> {
    let add = AddTorrent::from_bytes(bytes);
    let opts = AddTorrentOptions {
        list_only: true,
        overwrite: true,
        ..Default::default()
    };
    let response = session
        .add_torrent(add, Some(opts))
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;
    match response {
        librqbit::AddTorrentResponse::ListOnly(resp) => Ok(resp),
        _ => Err(Error::Internal(
            "unexpected response for list-only torrent".into(),
        )),
    }
}

/// Inspects a remote `.torrent` URL through librqbit's list-only path.
pub async fn inspect_torrent_url(session: &Arc<Session>, url: &str) -> Result<ListOnlyResponse> {
    let response = session
        .add_torrent(
            AddTorrent::from_url(url),
            Some(AddTorrentOptions {
                list_only: true,
                overwrite: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    match response {
        librqbit::AddTorrentResponse::ListOnly(resp) => Ok(resp),
        _ => Err(Error::InvalidInput(
            "unexpected response for torrent URL".into(),
        )),
    }
}

/// Extracts the info hash from a magnet URI.
fn extract_info_hash_from_magnet(uri: &str) -> Option<String> {
    url::Url::parse(uri)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == "xt")?
        .1
        .strip_prefix("urn:btih:")
        .map(str::to_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnet_info_hash_extraction() {
        let uri = "magnet:?xt=urn:btih:CAB507494D02EBB1178B38F2E9D7BE299C86B862&dn=test";
        assert_eq!(
            extract_info_hash_from_magnet(uri),
            Some("cab507494d02ebb1178b38f2e9d7be299c86b862".into())
        );
    }
}
