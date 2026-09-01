//! The `download` subcommand.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;
use odm_core::{DownloadRequest, Error, ProgressSink};
use odm_download_engine::{DownloadEngine, DownloadOptions, EngineConfig, default_output_for};
use odm_storage::validate_output_path;
use url::Url;

use crate::progress::ProgressReporter;

/// Download a single resource to disk.
#[derive(Debug, Args)]
pub struct DownloadCmd {
    /// The URL to download. Must be `http` or `https`.
    #[arg(value_name = "URL")]
    pub url: String,

    /// Local file path to write the download to. May be relative or
    /// absolute. If omitted, the filename is derived from the URL and the
    /// file is written to the current directory.
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Overwrite the output file if it already exists.
    #[arg(long)]
    pub overwrite: bool,
}

/// Executes the `download` subcommand.
pub async fn run(cmd: DownloadCmd) -> anyhow::Result<()> {
    let url = Url::parse(&cmd.url).map_err(|e| {
        anyhow::anyhow!(Error::InvalidUrl(format!("could not parse URL: {e}")))
    })?;

    let output = match cmd.output {
        Some(p) => {
            validate_output_path(&p).map_err(anyhow::Error::from)?;
            p
        }
        None => default_output_for(&url),
    };

    let request = DownloadRequest {
        url,
        output,
        overwrite: cmd.overwrite,
        max_redirects: 10,
    };

    let engine = DownloadEngine::new(EngineConfig::default())
        .map_err(|e| anyhow::anyhow!(Error::Internal(format!("engine init: {e}"))))?;

    let opts = DownloadOptions {
        overwrite: cmd.overwrite,
    };

    let reporter = Arc::new(ProgressReporter::new(
        request.output.display().to_string(),
    ));

    let summary = engine
        .download(
            &request,
            &opts,
            Some(reporter.clone() as Arc<dyn ProgressSink>),
            None,
        )
        .await
        .map_err(anyhow::Error::from)?;

    reporter.finish(&summary);

    Ok(())
}
