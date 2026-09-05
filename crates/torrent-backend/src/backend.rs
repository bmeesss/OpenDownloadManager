//! The BitTorrent backend implementing the `odm_core::Backend` boundary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use librqbit::{AddTorrent, AddTorrentOptions, ManagedTorrent, TorrentStats, TorrentStatsState};

use odm_core::{
    Backend, BackendKind, BackendOutcome, BackendTask, DownloadProgress, Error, Result,
};

use crate::error::{Error as TorrentError, Result as TorrentResult};
use crate::input::{inspect_magnet, inspect_torrent_bytes, inspect_torrent_url, ParsedInput};
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
        let paths: Vec<PathBuf> = info
            .iter_file_details()
            .map(|fd| fd.filename.to_pathbuf())
            .collect();
        for path in &paths {
            if path
                .components()
                .any(|component| component.as_os_str() == ".odm-owned")
            {
                return Err(TorrentError::DuplicatePath(
                    "reserved ODM ownership marker".into(),
                ));
            }
            let lower = path.to_string_lossy().to_lowercase();
            if !seen.insert(lower.clone()) {
                return Err(TorrentError::DuplicatePath(format!("{}", path.display())));
            }
            files.insert(lower);
        }
        for path in paths {
            let mut prefix = PathBuf::new();
            let collides_with_file = path.components().any(|part| {
                prefix.push(part.as_os_str());
                prefix != path && files.contains(&prefix.to_string_lossy().to_lowercase())
            });
            if collides_with_file {
                return Err(TorrentError::DuplicatePath(format!("{}", path.display())));
            }
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
        let stored_meta = serde_json::from_value::<BackendMeta>(task.backend_meta.clone()).ok();
        let input = if let Some(path) = stored_meta.as_ref().and_then(|m| m.torrent_file.as_deref())
        {
            if Path::new(path).exists() {
                Ok(ParsedInput::TorrentBytes {
                    bytes: std::fs::read(path)
                        .map_err(|e| map_error(TorrentError::Filesystem(e.to_string())))?,
                })
            } else {
                ParsedInput::parse(&task.url).map_err(map_error)
            }
        } else {
            ParsedInput::parse(&task.url).map_err(map_error)
        };
        let input = match input {
            Ok(i) => i,
            Err(e) => return Err(e),
        };

        let (info_hash, source, list_resp, initial_peers) = match &input {
            ParsedInput::Magnet { uri, .. } => {
                let resp = inspect_magnet(self.session.session(), uri)
                    .await
                    .map_err(map_error)?;
                let info_hash_owned = resp.info_hash.as_string();
                let peers = resp.seen_peers.clone();
                (info_hash_owned, TorrentSource::Magnet, Some(resp), peers)
            }
            ParsedInput::TorrentBytes { bytes } => {
                let resp = inspect_torrent_bytes(self.session.session(), bytes.clone())
                    .await
                    .map_err(map_error)?;
                let info_hash_owned = resp.info_hash.as_string();
                let peers = resp.seen_peers.clone();
                let source = if task.url.scheme() == "file" {
                    TorrentSource::TorrentFile
                } else {
                    TorrentSource::TorrentUrl
                };
                (info_hash_owned, source, Some(resp), peers)
            }
            ParsedInput::TorrentUrl { url } => {
                let resp = inspect_torrent_url(self.session.session(), url)
                    .await
                    .map_err(map_error)?;
                let info_hash_owned = resp.info_hash.as_string();
                let peers = resp.seen_peers.clone();
                (
                    info_hash_owned,
                    TorrentSource::TorrentUrl,
                    Some(resp),
                    peers,
                )
            }
        };

        let output_folder = self.output_folder_for(&info_hash);
        let meta = match source {
            TorrentSource::Magnet => BackendMeta::for_magnet(&info_hash),
            TorrentSource::TorrentFile => BackendMeta::for_torrent_file(
                &info_hash,
                self.store_torrent_copy(&info_hash, &list_resp.as_ref().unwrap().torrent_bytes)
                    .map_err(map_error)?,
            ),
            TorrentSource::TorrentUrl => BackendMeta::for_torrent(&info_hash, None),
        };

        let info = list_resp
            .as_ref()
            .map(|r| &r.info)
            .ok_or_else(|| Error::Internal("missing info after inspection".into()))?;

        Self::validate_torrent_paths(info).map_err(map_error)?;
        Self::validate_no_duplicates(info).map_err(map_error)?;
        let paths: Vec<PathBuf> = info
            .iter_file_details()
            .map(|fd| fd.filename.to_pathbuf())
            .collect();
        Self::validate_root_confinement(&output_folder, &paths).map_err(map_error)?;

        validate_existing_components(&output_folder, &paths).map_err(map_error)?;
        validate_existing_components(&output_folder, &[PathBuf::from(".odm-owned")])
            .map_err(map_error)?;

        prepare_output_folder(&output_folder, &info_hash).map_err(map_error)?;

        if self.has_active_torrent(&info_hash) {
            return Err(Error::AlreadyExists(format!(
                "torrent {info_hash} is already active"
            )));
        }

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
        } else {
            AddTorrent::from_bytes(
                list_resp
                    .as_ref()
                    .expect("list response exists for torrent input")
                    .torrent_bytes
                    .clone(),
            )
        };

        let response = self
            .session
            .session()
            .add_torrent(add, Some(opts))
            .await
            .map_err(|e| Error::Network(e.to_string()))?;

        let handle: Arc<ManagedTorrent> = match response {
            librqbit::AddTorrentResponse::AlreadyManaged(_, _) => {
                return Err(Error::AlreadyExists(format!(
                    "torrent {info_hash} is already active"
                )));
            }
            librqbit::AddTorrentResponse::Added(_, h) => h,
            librqbit::AddTorrentResponse::ListOnly(_) => {
                return Err(Error::Internal(
                    "list-only response unexpectedly returned".into(),
                ))
            }
        };

        let poll_interval = std::time::Duration::from_millis(250);
        let initial_progress = handle.stats().progress_bytes;
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
                    let _ = self.session.session().delete(
                        librqbit::api::TorrentIdOrHash::Hash(handle.info_hash()),
                        false,
                    ).await;
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
                            downloaded_bytes: total.saturating_sub(initial_progress),
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
                                downloaded_bytes: stats.progress_bytes.saturating_sub(initial_progress),
                                total_bytes: Some(stats.total_bytes),
                                at: std::time::SystemTime::now(),
                            });
                        }
                    }
                }
            }
        }
    }

    async fn dispose(&self, task: odm_core::BackendTask) -> Result<()> {
        let meta = serde_json::from_value::<BackendMeta>(task.backend_meta).ok();
        let input = if let Some(path) = meta.as_ref().and_then(|m| m.torrent_file.as_deref()) {
            if Path::new(path).exists() {
                ParsedInput::TorrentBytes {
                    bytes: std::fs::read(path).map_err(|e| Error::Filesystem(e.to_string()))?,
                }
            } else {
                ParsedInput::parse(&task.url).map_err(map_error)?
            }
        } else {
            ParsedInput::parse(&task.url).map_err(map_error)?
        };
        let response = match input {
            ParsedInput::Magnet { uri } => inspect_magnet(self.session.session(), &uri)
                .await
                .map_err(map_error)?,
            ParsedInput::TorrentBytes { bytes } => {
                inspect_torrent_bytes(self.session.session(), bytes)
                    .await
                    .map_err(map_error)?
            }
            ParsedInput::TorrentUrl { url } => inspect_torrent_url(self.session.session(), &url)
                .await
                .map_err(map_error)?,
        };
        let info_hash = response.info_hash.as_string();
        let id = librqbit::api::TorrentIdOrHash::parse(&info_hash)
            .map_err(|e| Error::InvalidUrl(e.to_string()))?;
        let _ = self.session.session().delete(id, true).await;
        remove_owned_files(&self.output_folder_for(&info_hash), &response.info);
        remove_owned_directory(&self.output_folder_for(&info_hash), &info_hash);
        let _ = std::fs::remove_file(
            self.session
                .output_root()
                .join(".odm-torrents")
                .join(format!("{info_hash}.torrent")),
        );
        Ok(())
    }
}

impl TorrentBackend {
    fn store_torrent_copy(&self, info_hash: &str, bytes: &[u8]) -> TorrentResult<String> {
        let dir = self.session.output_root().join(".odm-torrents");
        std::fs::create_dir_all(&dir).map_err(|e| {
            TorrentError::Filesystem(format!("create torrent metadata directory: {e}"))
        })?;
        let path = dir.join(format!("{info_hash}.torrent"));
        if !path.exists() {
            std::fs::write(&path, bytes)
                .map_err(|e| TorrentError::Filesystem(format!("store torrent file: {e}")))?;
        }
        Ok(path.to_string_lossy().into_owned())
    }
}

fn map_error(error: impl std::fmt::Display) -> Error {
    let text = error.to_string();
    if text.contains("invalid torrent path")
        || text.contains("path escapes")
        || text.contains("duplicate torrent path")
    {
        Error::InvalidPath(text)
    } else if text.contains("already exists") || text.contains("already active") {
        Error::AlreadyExists(text)
    } else if text.contains("filesystem") || text.contains("output root") {
        Error::Filesystem(text)
    } else if text.contains("cancelled") {
        Error::Cancelled
    } else {
        Error::InvalidUrl(text)
    }
}

fn prepare_output_folder(folder: &Path, info_hash: &str) -> TorrentResult<()> {
    if !folder.exists() {
        std::fs::create_dir_all(folder)
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
        let has_unrelated = std::fs::read_dir(folder)
            .map(|entries| entries.flatten().any(|entry| entry.path() != marker))
            .unwrap_or(true);
        if !has_unrelated && std::fs::remove_file(&marker).is_ok() {
            let _ = std::fs::remove_dir(folder);
        }
    }
}

fn remove_owned_files(
    folder: &Path,
    info: &librqbit::ValidatedTorrentMetaV1Info<impl std::convert::AsRef<[u8]>>,
) {
    let mut files: Vec<PathBuf> = info
        .iter_file_details()
        .map(|fd| folder.join(fd.filename.to_pathbuf()))
        .collect();
    files.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in files {
        if validate_existing_components(
            folder,
            &[path.strip_prefix(folder).unwrap_or(&path).to_path_buf()],
        )
        .is_ok()
        {
            let _ = std::fs::remove_file(&path);
        }
    }
    let mut dirs = Vec::new();
    for path in info
        .iter_file_details()
        .map(|fd| folder.join(fd.filename.to_pathbuf()))
    {
        let mut current = path.parent();
        while let Some(dir) = current {
            if dir != folder {
                dirs.push(dir.to_path_buf());
            }
            current = dir.parent();
        }
    }
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    dirs.dedup();
    for dir in dirs {
        let _ = std::fs::remove_dir(dir);
    }
}
