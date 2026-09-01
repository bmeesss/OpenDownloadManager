//! Download orchestration logic.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use futures_util::StreamExt;
use odm_core::{DownloadProgress, DownloadRequest, DownloadSummary, Error, ProgressSink, Result};
use odm_http_client::{HttpClient, HttpClientConfig, download_stream};
use odm_storage::{FileStorage, ensure_parent_dir, validate_output_path};
use tokio::sync::Notify;
use tracing::{info, warn};

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
    pub fn new(
        config: EngineConfig,
    ) -> std::result::Result<Self, odm_http_client::HttpClientError> {
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
            let chunk = match &cancel {
                Some(notify) => tokio::select! {
                    biased;
                    () = notify.notified() => return Err(Error::Cancelled),
                    item = stream.body.next() => item,
                },
                None => stream.body.next().await,
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
                        at: SystemTime::now(),
                    });
                    last_emit = now;
                }
            }
        }

        // Refuse to finalize a transfer that did not deliver everything the
        // server announced. The `.part` file is deliberately left on disk so
        // the failure can be inspected; it is not renamed into place.
        if let Err(err) = verify_length(total, downloaded) {
            warn!(
                expected = ?total,
                received = downloaded,
                part = %part.path().display(),
                "incomplete transfer; retaining partial file"
            );
            return Err(err);
        }

        part.flush().await?;
        self.storage
            .finalize(part, &request.output, opts.overwrite)
            .await?;

        let elapsed = started.elapsed();
        let bps = if elapsed.as_secs_f64() > 0.0 {
            downloaded as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        // The transfer is verified complete at this point, so if the server
        // never announced a size it is honest to close the bar out at the
        // byte count we actually received.
        if let Some(p) = progress {
            p.on_progress(DownloadProgress {
                downloaded_bytes: downloaded,
                total_bytes: total.or(Some(downloaded)),
                at: SystemTime::now(),
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
}

/// Confirms that the bytes received match the size the server announced.
///
/// A server that closes early, or that advertises a `Content-Length` it
/// does not honour, must not yield a file that looks complete. `None`
/// means the size was never announced, which is not an error.
fn verify_length(expected: Option<u64>, received: u64) -> Result<()> {
    match expected {
        None => Ok(()),
        Some(expected) if expected == received => Ok(()),
        Some(expected) => Err(Error::InvalidResponse(format!(
            "incomplete transfer: server announced {expected} bytes, received {received}"
        ))),
    }
}

fn validate_request(req: &DownloadRequest) -> Result<()> {
    match req.url.scheme() {
        "http" | "https" => {}
        other => return Err(Error::InvalidUrl(format!("unsupported scheme: {other}"))),
    }
    validate_output_path(Path::new(&req.output))?;
    if req.max_redirects == 0 {
        return Err(Error::InvalidUrl("max_redirects must be > 0".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_length_accepts_unknown_size() {
        assert!(verify_length(None, 0).is_ok());
        assert!(verify_length(None, 1_048_576).is_ok());
    }

    #[test]
    fn verify_length_accepts_exact_match() {
        assert!(verify_length(Some(0), 0).is_ok());
        assert!(verify_length(Some(1024), 1024).is_ok());
    }

    #[test]
    fn verify_length_rejects_short_transfer() {
        let err = verify_length(Some(100), 50).expect_err("must reject");
        assert!(matches!(err, Error::InvalidResponse(_)));
    }

    #[test]
    fn verify_length_rejects_overlong_transfer() {
        let err = verify_length(Some(100), 4096).expect_err("must reject");
        assert!(matches!(err, Error::InvalidResponse(_)));
    }

    #[test]
    fn verify_length_treats_zero_byte_response_as_complete() {
        assert!(verify_length(Some(0), 0).is_ok());
    }

    #[test]
    fn validate_request_rejects_non_http_scheme() {
        let req = DownloadRequest {
            url: url::Url::parse("ftp://example.com/f").unwrap(),
            output: "f.bin".into(),
            overwrite: false,
            max_redirects: 10,
        };
        assert!(matches!(
            validate_request(&req),
            Err(Error::InvalidUrl(_))
        ));
    }

    #[test]
    fn validate_request_rejects_zero_max_redirects() {
        let req = DownloadRequest {
            url: url::Url::parse("https://example.com/f").unwrap(),
            output: "f.bin".into(),
            overwrite: false,
            max_redirects: 0,
        };
        assert!(matches!(
            validate_request(&req),
            Err(Error::InvalidUrl(_))
        ));
    }
}
