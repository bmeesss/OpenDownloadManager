//! Session management for the BitTorrent backend.
//!
//! Owns one long-lived `Arc<librqbit::Session>` for the lifetime of the backend.

use std::path::PathBuf;
use std::sync::Arc;

use librqbit::{Session, SessionOptions};
use tracing::info;

use crate::error::{Error, Result};

/// Owns the librqbit session for the lifetime of the backend.
pub struct TorrentSession {
    session: Arc<Session>,
    output_root: PathBuf,
}

impl TorrentSession {
    /// Creates a new session with the given output root.
    ///
    /// Configures:
    /// * `persistence = None`
    /// * `fastresume = false`
    /// * DHT persistence disabled (`dht.persistence = None`)
    pub async fn new(output_root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&output_root)
            .map_err(|e| Error::Filesystem(format!("create torrent output root: {e}")))?;
        let opts = SessionOptions {
            persistence: None,
            fastresume: false,
            dht: Some(librqbit::DhtSessionConfig {
                persistence: None,
                ..Default::default()
            }),
            listen: Some(librqbit::ListenerOptions {
                mode: librqbit::ListenerMode::TcpOnly,
                listen_addr: ([0, 0, 0, 0], 0).into(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let session = Session::new_with_opts(output_root.clone(), opts)
            .await
            .map_err(|e| Error::Internal(format!("session init: {e}")))?;

        if let Some(addr) = session.listen_addr() {
            info!(%addr, "torrent session ready");
        } else {
            info!("torrent session ready without incoming listener");
        }
        Ok(Self {
            session,
            output_root,
        })
    }

    /// Returns the configured output root.
    #[must_use]
    pub fn output_root(&self) -> &PathBuf {
        &self.output_root
    }

    /// Returns a reference to the underlying session.
    #[must_use]
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Updates the session-level download rate limit.
    pub fn set_download_rate_limit(&self, bps: Option<u32>) {
        use librqbit::limits::LimitsConfig;
        let config = LimitsConfig {
            download_bps: bps.and_then(|b| b.try_into().ok()),
            upload_bps: None,
        };
        self.session
            .ratelimits
            .set_download_bps(config.download_bps);
    }

    /// Updates the session-level upload rate limit.
    #[allow(dead_code)]
    pub fn set_upload_rate_limit(&self, bps: Option<u32>) {
        use librqbit::limits::LimitsConfig;
        let config = LimitsConfig {
            download_bps: None,
            upload_bps: bps.and_then(|b| b.try_into().ok()),
        };
        self.session.ratelimits.set_upload_bps(config.upload_bps);
    }

    /// Stops all managed torrents and shuts down the session.
    #[allow(dead_code)]
    pub async fn stop(self) {
        let _ = self.session.stop().await;
    }
}
