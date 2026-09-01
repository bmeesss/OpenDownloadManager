//! Download orchestration logic.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use odm_core::{DownloadProgress, DownloadRequest, DownloadSummary, Error, ProgressSink, Result};
use odm_http_client::{HttpClient, HttpClientConfig, download_stream};
use odm_storage::{FileStorage, ensure_parent_dir, validate_path, validate_filename};
use tokio::sync::Notify;
use tracing::{debug, info};
use url::Url;

/// Options that control a single download.
#[derive(Debug, Clone, Default)]
pub struct DownloadOptions {
    /// Whether to overwrite an existing file at the target path.
    pub overwrite: bool,
}

/// Top-level engine configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// HTTP client configuration.
    pub http: HttpClientConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            http: HttpClientConfig::default(),
        }
    }
}

/// The download engine.
pub struct DownloadEngine {
    client: HttpClient,
    storage: FileStorage,
}

impl DownloadEngine {
    /// Constructs a new engine with the given configuration.
    ///
    /// # Errors
    /// Returns an error if the underlying HTTP client cannot be built.
    pub fn new(config: EngineConfig) -> std::result::Result<Self, odm_http_client::HttpClientError> {
        let client = HttpClient::new(config.http)?;
        Ok(Self {
            client,
            storage: FileStorage::new(),
        })
    }

    /// Downloads the resource described by `request`, reporting progress
    /// to `progress` (if provided) and respecting cancellation through
    /// `cancel`.
    ///
    /// # Errors
    /// See [`odm_core::Error`] for the full enumeration.
    pub async fn download(
        &self,
        request: &DownloadRequest,
        opts: &DownloadOptions,
        progress: Option<Arc<dyn ProgressSink>>,
        cancel: Option<Arc<Notify>>,
    ) -> Result<DownloadSummary> {
        let started = Instant::now();
        validate_request(request)?;

        info!(url = %request.url, output = %request.output.display(), "download start");

        if request.output.exists() && !opts.overwrite {
            return Err(Error::AlreadyExists(request.output.display().to_string()));
        }

        ensure_parent_dir(&request.output).await?;

        let mut stream = download_stream(&self.client, &request.url).await?;
        let total = stream.resource.content_length;

        let mut part = self.storage.create_part_file(&request.output).await?;

        let mut last_emit = Instant::now();
        let mut downloaded: u64 = 0;

        loop {
            let next_chunk = async {
                stream.body.next().await
            };
            let chunk = if let Some(c) = &cancel {
                tokio::select! {
                    biased;
                    _ = c.notified() => {
                        return Err(Error::Cancelled);
                    }
                    chunk = next_chunk => chunk,
                }
            } else {
                next_chunk.await
            };

            let Some(chunk) = chunk else { break };
            let chunk = chunk?;
            if chunk.is_empty() {
                continue;
            }
            part.write_chunk(&chunk).await?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);

            if let Some(p) = &progress {
                let now = Instant::now();
                if now.duration_since(last_emit) >= Duration::from_millis(250) {
                    p.on_progress(DownloadProgress {
                        downloaded_bytes: downloaded,
                        total_bytes: total,
                        at: now,
                    });
                    last_emit = now;
                }
            }
        }

        part.flush().await?;
        self.storage.finalize(part, &request.output).await?;

        let elapsed = started.elapsed();
        let bps = if elapsed.as_secs_f64() > 0.0 {
            downloaded as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        if let Some(p) = progress {
            p.on_progress(DownloadProgress {
                downloaded_bytes: downloaded,
                total_bytes: Some(downloaded),
                at: Instant::now(),
            });
        }

        info!(
            bytes = downloaded,
            seconds = elapsed.as_secs_f64(),
            final_url = %stream.final_url,
            "download complete"
        );

        Ok(DownloadSummary {
            output: request.output.clone(),
            total_bytes: downloaded,
            duration: elapsed,
            average_bytes_per_sec: bps,
            final_url: stream.final_url,
        })
    }

    /// Returns a reference to the underlying HTTP client (used by tests).
    #[cfg(test)]
    pub fn client(&self) -> &HttpClient {
        &self.client
    }
}

fn validate_request(req: &DownloadRequest) -> Result<()> {
    match req.url.scheme() {
        "http" | "https" => {}
        other => return Err(Error::InvalidUrl(format!("unsupported scheme: {other}"))),
    }
    let path = Path::new(&req.output);
    validate_path(path)?;
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        validate_filename(name)?;
    } else {
        return Err(Error::InvalidFileName(format!(
            "could not extract file name from {}",
            path.display()
        )));
    }
    if req.max_redirects == 0 {
        return Err(Error::InvalidUrl("max_redirects must be > 0".to_string()));
    }
    Ok(())
}

/// Helper to derive a sensible default output path from a URL when the
/// user did not supply `--output`.
#[must_use]
pub fn default_output_for(url: &Url) -> PathBuf {
    let last = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("download.bin");
    let sanitized = odm_storage::sanitize_filename(last);
    PathBuf::from(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_output_for_strips_query() {
        let url = Url::parse("https://example.com/file.zip?token=abc").unwrap();
        let out = default_output_for(&url);
        assert_eq!(out, PathBuf::from("file.zip"));
    }

    #[test]
    fn default_output_for_empty_path() {
        let url = Url::parse("https://example.com/").unwrap();
        let out = default_output_for(&url);
        assert_eq!(out, PathBuf::from("download.bin"));
    }
}
