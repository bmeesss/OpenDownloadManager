//! Adapts [`DownloadEngine`] to the protocol-neutral [`Backend`] boundary.
//!
//! The manager talks only to [`odm_core::Backend`]; this module is what lets
//! the HTTP engine sit behind that interface without the manager ever
//! depending on `reqwest` (or anything else transport-specific) directly.

use async_trait::async_trait;
use odm_core::{
    Backend, BackendKind, BackendOutcome, BackendTask, DownloadOptions, DownloadRequest, Result,
};

use crate::engine::DownloadEngine;

/// Default per-request redirect cap used when the manager hands us a task.
const DEFAULT_MAX_REDIRECTS: usize = 10;

#[async_trait]
impl Backend for DownloadEngine {
    fn kind(&self) -> BackendKind {
        BackendKind::Http
    }

    async fn run(&self, task: BackendTask) -> Result<BackendOutcome> {
        let request = DownloadRequest {
            url: task.url,
            output: task.destination,
            overwrite: task.overwrite,
            max_redirects: DEFAULT_MAX_REDIRECTS,
        };
        let options = DownloadOptions {
            overwrite: task.overwrite,
            rate_limiter: task.rate_limiter,
        };

        let summary = self
            .download(&request, &options, task.progress, task.cancel)
            .await?;

        Ok(BackendOutcome {
            downloaded_bytes: summary.total_bytes,
            total_bytes: Some(summary.total_bytes),
            backend_meta: serde_json::Value::Null,
        })
    }
}
