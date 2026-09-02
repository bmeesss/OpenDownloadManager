//! Streaming partial-file storage with atomic finalization.

use std::path::{Path, PathBuf};

use odm_core::{Error, Result};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncWriteExt, BufWriter};
use tracing::debug;
use uuid::Uuid;

use crate::path::ensure_parent_dir;

/// A handle to an open partial file.
///
/// `BufWriter` is used to amortize small writes; `sync_all` is called
/// on `flush()` to ensure data reaches disk.
pub struct PartFileHandle {
    writer: BufWriter<File>,
    path: PathBuf,
    bytes_written: u64,
}

impl PartFileHandle {
    /// Returns the on-disk path of the partial file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the number of bytes written so far.
    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Writes a chunk to the partial file.
    ///
    /// Data is buffered in memory; it only reaches the operating system
    /// once the buffer fills or [`Self::flush`] is called.
    ///
    /// # Errors
    /// Returns [`Error::Filesystem`] on I/O failure.
    pub async fn write_chunk(&mut self, data: &[u8]) -> Result<()> {
        self.writer
            .write_all(data)
            .await
            .map_err(|e| Error::Filesystem(format!("write: {e}")))?;
        self.bytes_written = self.bytes_written.saturating_add(data.len() as u64);
        Ok(())
    }

    /// Flushes userspace buffers and calls `fsync` to durably persist data.
    ///
    /// # Errors
    /// Returns [`Error::Filesystem`] on I/O failure.
    pub async fn flush(&mut self) -> Result<()> {
        self.writer
            .flush()
            .await
            .map_err(|e| Error::Filesystem(format!("flush: {e}")))?;
        self.writer
            .get_ref()
            .sync_all()
            .await
            .map_err(|e| Error::Filesystem(format!("sync_all: {e}")))?;
        Ok(())
    }

    /// Closes the partial file without renaming it. Used on failure to
    /// retain the `.part` file for debugging.
    ///
    /// # Errors
    /// Returns [`Error::Filesystem`] on I/O failure.
    pub async fn close(self) -> Result<PathBuf> {
        self.writer
            .into_inner()
            .shutdown()
            .await
            .map_err(|e| Error::Filesystem(format!("close: {e}")))?;
        Ok(self.path)
    }
}

/// Filesystem abstraction used by the download engine.
#[derive(Debug, Clone)]
pub struct FileStorage;

impl FileStorage {
    /// Creates a new storage rooted at the current directory.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Creates a unique `.part` path for `final_path` and opens it for
    /// writing. The parent directory is created if missing.
    ///
    /// # Errors
    /// Returns [`Error::Filesystem`] on I/O failure.
    pub async fn create_part_file(&self, final_path: &Path) -> Result<PartFileHandle> {
        let final_path = final_path.to_path_buf();
        ensure_parent_dir(&final_path).await?;

        let id = Uuid::new_v4().simple().to_string();
        let part_name = match final_path.file_name().and_then(|n| n.to_str()) {
            Some(name) => format!(".{name}.{id}.part"),
            None => format!("download.{id}.part"),
        };
        let part_path = match final_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join(part_name),
            _ => PathBuf::from(part_name),
        };

        debug!(part = %part_path.display(), "creating partial file");
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&part_path)
            .await
            .map_err(|e| {
                Error::Filesystem(format!("create part file {}: {e}", part_path.display()))
            })?;
        let writer = BufWriter::with_capacity(64 * 1024, file);

        Ok(PartFileHandle {
            writer,
            path: part_path,
            bytes_written: 0,
        })
    }

    /// Atomically renames the partial file to its final path.
    ///
    /// When `overwrite` is `false` and something already exists at
    /// `final_path`, the rename is refused and the partial file is left in
    /// place for inspection. When it is `true` the existing file is
    /// replaced by the rename itself, which is atomic on both Unix and
    /// Windows.
    ///
    /// # Errors
    /// Returns [`Error::Filesystem`] or [`Error::AlreadyExists`].
    pub async fn finalize(
        &self,
        part: PartFileHandle,
        final_path: &Path,
        overwrite: bool,
    ) -> Result<()> {
        let part_path = part.close().await?;
        if !overwrite && tokio::fs::try_exists(final_path).await.unwrap_or(false) {
            return Err(Error::AlreadyExists(final_path.display().to_string()));
        }
        tokio::fs::rename(&part_path, final_path)
            .await
            .map_err(|e| {
                Error::Filesystem(format!(
                    "rename {} -> {}: {e}",
                    part_path.display(),
                    final_path.display()
                ))
            })?;
        debug!(final = %final_path.display(), "finalized");
        Ok(())
    }

    /// Removes a partial file. Used for cleanup.
    ///
    /// # Errors
    /// Returns [`Error::Filesystem`] if removal fails for a reason
    /// other than the file not existing.
    pub async fn remove_part(&self, part_path: &Path) -> Result<()> {
        match tokio::fs::remove_file(part_path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Filesystem(format!(
                "remove part file {}: {e}",
                part_path.display()
            ))),
        }
    }
}

impl Default for FileStorage {
    fn default() -> Self {
        Self::new()
    }
}
