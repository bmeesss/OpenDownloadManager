//! The BitTorrent backend implementing the `odm_core::Backend` boundary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use librqbit::{AddTorrent, AddTorrentOptions, ManagedTorrent, TorrentStats, TorrentStatsState};

use odm_core::{
    Backend, BackendKind, BackendOutcome, BackendTask, DownloadProgress, Error, Result,
};

use crate::error::{Error as TorrentError, Result as TorrentResult};
use crate::input::{inspect_magnet, inspect_torrent_bytes, ParsedInput};
use crate::metadata::{BackendMeta, TorrentSource};
use crate::path::{escapes_root, validate_existing_components, validate_torrent_path};
use crate::session::TorrentSession;

/// The BitTorrent backend.
pub struct TorrentBackend {
    session: Arc<TorrentSession>,
    initial_peers: Vec<std::net::SocketAddr>,
}

impl TorrentBackend {
    /// Creates a new backend with its own session.
    pub async fn new(output_root: PathBuf) -> TorrentResult<Self> {
        let session = Arc::new(TorrentSession::new(output_root.clone()).await?);
        Ok(Self {
            session,
            initial_peers: Vec::new(),
        })
    }

    /// Creates a backend with explicit peers for deterministic local transfers.
    ///
    /// This is also useful to callers that already discovered peers through a
    /// tracker or another trusted discovery mechanism.
    pub async fn new_with_initial_peers(
        output_root: PathBuf,
        initial_peers: Vec<std::net::SocketAddr>,
    ) -> TorrentResult<Self> {
        let mut backend = Self::new(output_root).await?;
        backend.initial_peers = initial_peers;
        Ok(backend)
    }

    /// Returns whether a torrent remains registered in the live session.
    #[must_use]
    pub fn has_active_torrent(&self, info_hash: &str) -> bool {
        let Ok(id) = librqbit::api::TorrentIdOrHash::parse(info_hash) else {
            return false;
        };
        self.session.session().get(id).is_some()
    }

    /// Returns the current librqbit state for a managed torrent.
    #[must_use]
    pub fn torrent_state(&self, info_hash: &str) -> Option<TorrentStatsState> {
        let id = librqbit::api::TorrentIdOrHash::parse(info_hash).ok()?;
        self.session
            .session()
            .get(id)
            .map(|handle| handle.stats().state)
    }

    /// Validates every file path in the torrent metadata.
    fn validate_torrent_paths(
        info: &librqbit::ValidatedTorrentMetaV1Info<impl std::convert::AsRef<[u8]>>,
    ) -> TorrentResult<()> {
        for fd in info.iter_file_details() {
            let path = fd.filename.to_pathbuf();
            validate_torrent_path(&path)
                .map_err(|e| TorrentError::InvalidPath(format!("{}: {e}", path.display())))?;
        }
        Ok(())
    }

    /// Validates no duplicate or case-insensitive collisions.
    fn validate_no_duplicates(
        info: &librqbit::ValidatedTorrentMetaV1Info<impl std::convert::AsRef<[u8]>>,
    ) -> TorrentResult<()> {
        let mut seen = std::collections::HashSet::new();
        let mut files = std::collections::HashSet::new();
        for fd in info.iter_file_details() {
            let path = fd.filename.to_pathbuf();
            if path
                .components()
                .any(|component| component.as_os_str() == ".odm-owned")
            {
                return Err(TorrentError::DuplicatePath(
                    "reserved ODM ownership marker".into(),
                ));
            }
            let lower = path.to_string_lossy().to_lowercase();
            let mut prefix = PathBuf::new();
            let mut collides_with_file = false;
            for part in path.components() {
                prefix.push(part.as_os_str());
                if prefix != path && files.contains(&prefix.to_string_lossy().to_lowercase()) {
                    collides_with_file = true;
                }
            }
            if !seen.insert(lower.clone()) || collides_with_file {
                return Err(TorrentError::DuplicatePath(format!("{}", path.display())));
            }
            files.insert(lower);
        }
        Ok(())
    }

    /// Validates that all paths stay within the output root.
    fn validate_root_confinement(root: &Path, paths: &[PathBuf]) -> TorrentResult<()> {
        for p in paths {
            if escapes_root(root, p) {
                return Err(TorrentError::PathEscapesRoot(format!("{}", p.display())));
            }
        }
        Ok(())
    }

    /// Builds the per-torrent output folder under the session root.
    fn output_folder_for(&self, info_hash: &str) -> PathBuf {
        self.session.output_root().join(format!("odm-{info_hash}"))
    }

    /// Checks if the stats indicate actual live downloading (not initialization).
    fn is_live_downloading(stats: &TorrentStats) -> bool {
        matches!(stats.state, TorrentStatsState::Live)
    }
}

#[async_trait]
impl Backend for TorrentBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Torrent
    }

    async fn run(&self, task: BackendTask) -> Result<BackendOutcome> {
        let input = match ParsedInput::parse(&task.url) {
            Ok(i) => i,
            Err(e) => return Err(Error::Internal(e.to_string())),
        };

        let (info_hash, source, list_resp, initial_peers) = match &input {
            ParsedInput::Magnet { uri, .. } => {
                let resp = inspect_magnet(self.session.session(), uri)
                    .await
                    .map_err(|e| Error::Internal(e.to_string()))?;
                let info_hash_owned = resp.info_hash.as_string();
                let peers = resp.seen_peers.clone();
                (info_hash_owned, TorrentSource::Magnet, Some(resp), peers)
            }
            ParsedInput::TorrentBytes { bytes } => {
                let resp = inspect_torrent_bytes(self.session.session(), bytes.clone())
                    .await
                    .map_err(|e| Error::Internal(e.to_string()))?;
                let info_hash_owned = resp.info_hash.as_string();
                let peers = resp.seen_peers.clone();
                let source = if task.url.scheme() == "file" {
                    TorrentSource::TorrentFile
                } else {
                    TorrentSource::TorrentUrl
                };
                (info_hash_owned, source, Some(resp), peers)
            }
        };

        let output_folder = self.output_folder_for(&info_hash);
        let meta = match source {
            TorrentSource::Magnet => BackendMeta::for_magnet(&info_hash),
            TorrentSource::TorrentFile => {
                BackendMeta::for_torrent_file(&info_hash, task.url.to_string())
            }
            TorrentSource::TorrentUrl => BackendMeta::for_torrent(&info_hash, None),
        };

        let info = list_resp
            .as_ref()
            .map(|r| &r.info)
            .ok_or_else(|| Error::Internal("missing info after inspection".into()))?;

        Self::validate_torrent_paths(info).map_err(|e| Error::Internal(e.to_string()))?;
        Self::validate_no_duplicates(info).map_err(|e| Error::Internal(e.to_string()))?;
        let paths: Vec<PathBuf> = info
            .iter_file_details()
            .map(|fd| fd.filename.to_pathbuf())
            .collect();
        Self::validate_root_confinement(&output_folder, &paths)
            .map_err(|e| Error::Internal(e.to_string()))?;

        validate_existing_components(&output_folder, &paths)
            .map_err(|e| Error::Internal(e.to_string()))?;
        validate_existing_components(&output_folder, &[PathBuf::from(".odm-owned")])
            .map_err(|e| Error::Internal(e.to_string()))?;

        if output_folder.exists() && !task.overwrite {
            return Err(Error::AlreadyExists(format!("{}", output_folder.display())));
        }

        prepare_output_folder(&output_folder, &info_hash)
            .map_err(|e| Error::Internal(e.to_string()))?;

        if let Some(global_bps) = task.global_max_bytes_per_sec {
            self.session
                .set_download_rate_limit(Some(global_bps as u32));
        }

        let opts = AddTorrentOptions {
            overwrite: true,
            output_folder: Some(output_folder.to_string_lossy().to_string()),
            initial_peers: Some(
                self.initial_peers
                    .iter()
                    .copied()
                    .chain(initial_peers)
                    .collect(),
            ),
            ..Default::default()
        };

        let add = if matches!(&input, ParsedInput::Magnet { .. }) {
            // The list-only response already contains resolved metainfo. Add
            // those bytes directly so a magnet is not resolved twice.
            AddTorrent::from_bytes(
                list_resp
                    .as_ref()
                    .expect("list response exists for magnet")
                    .torrent_bytes
                    .clone(),
            )
        } else if let ParsedInput::TorrentBytes { bytes } = &input {
            AddTorrent::from_bytes(bytes.clone())
        } else {
            return Err(Error::Internal("unsupported input type".into()));
        };

        let response = self
            .session
            .session()
            .add_torrent(add, Some(opts))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        let handle: Arc<ManagedTorrent> = match response {
            librqbit::AddTorrentResponse::AlreadyManaged(_, h) => {
                if matches!(
                    h.stats().state,
                    TorrentStatsState::Paused | TorrentStatsState::Initializing { paused: true }
                ) {
                    self.session
                        .session()
                        .unpause(&h)
                        .await
                        .map_err(|e| Error::Internal(e.to_string()))?;
                }
                h
            }
            librqbit::AddTorrentResponse::Added(_, h) => h,
            librqbit::AddTorrentResponse::ListOnly(_) => {
                return Err(Error::Internal(
                    "list-only response unexpectedly returned".into(),
                ))
            }
        };

        let poll_interval = std::time::Duration::from_millis(250);
        let mut reported_live = false;

        loop {
            tokio::select! {
                biased;
                _ = async {
                    if let Some(d) = &task.dispose { d.notified().await; } else { std::future::pending().await }
                } => {
                    let _ = self.session.session().pause(&handle).await;
                    let _ = self.session.session().delete(
                        librqbit::api::TorrentIdOrHash::Hash(handle.info_hash()),
                        true,
                    ).await;
                    remove_owned_directory(&output_folder, &info_hash);
                    return Err(Error::Cancelled);
                }
                _ = async {
                    if let Some(c) = &task.cancel { c.notified().await; }
                } => {
                    let _ = self.session.session().pause(&handle).await;
                    return Err(Error::Cancelled);
                }
                _ = tokio::time::sleep(poll_interval) => {
                    let stats = handle.stats();
                    if stats.finished {
                        let _ = self.session.session().pause(&handle).await;
                        let _ = self.session.session().delete(
                            librqbit::api::TorrentIdOrHash::Hash(handle.info_hash()),
                            false,
                        ).await;
                        let total = stats.total_bytes;
                        return Ok(BackendOutcome {
                            downloaded_bytes: total,
                            total_bytes: Some(total),
                            backend_meta: serde_json::to_value(meta).unwrap_or_default(),
                        });
                    }

                    if Self::is_live_downloading(&stats) {
                        reported_live = true;
                    }

                    if reported_live {
                        if let Some(progress) = &task.progress {
                            progress.on_progress(DownloadProgress {
                                downloaded_bytes: stats.progress_bytes,
                                total_bytes: Some(stats.total_bytes),
                                at: std::time::SystemTime::now(),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn prepare_output_folder(folder: &Path, info_hash: &str) -> TorrentResult<()> {
    if !folder.exists() {
        std::fs::create_dir(folder)
            .map_err(|e| TorrentError::Filesystem(format!("create torrent directory: {e}")))?;
        std::fs::write(
            folder.join(".odm-owned"),
            format!("odm-torrent-v1:{info_hash}\n"),
        )
        .map_err(|e| TorrentError::Filesystem(format!("create ownership marker: {e}")))?;
    } else {
        let marker = folder.join(".odm-owned");
        let contents = std::fs::read_to_string(&marker).map_err(|_| {
            TorrentError::Filesystem("existing torrent directory is not ODM-owned".into())
        })?;
        if contents != format!("odm-torrent-v1:{info_hash}\n") {
            return Err(TorrentError::Filesystem(
                "torrent directory ownership marker mismatch".into(),
            ));
        }
    }
    Ok(())
}

fn remove_owned_directory(folder: &Path, info_hash: &str) {
    let marker = folder.join(".odm-owned");
    let owned = std::fs::read_to_string(&marker)
        .map(|s| s == format!("odm-torrent-v1:{info_hash}\n"))
        .unwrap_or(false);
    if owned {
        let _ = std::fs::remove_file(marker);
        let _ = std::fs::remove_dir(folder);
    }
}
