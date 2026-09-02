//! Command line interface for OpenDownloadManager.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod cli;
mod exit;
mod progress;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let args = Cli::parse();
    let is_download = matches!(args.command, Command::Download(_));
    init_tracing(args.verbose, args.quiet, is_download);

    let code = match run(args).await {
        Ok(()) => exit::Exit::Success,
        Err(err) => {
            tracing::error!(error = %err, "command failed");
            exit::Exit::from_error(&root_cause(&err))
        }
    };
    code.into()
}

fn root_cause(err: &anyhow::Error) -> odm_core::Error {
    for cause in err.chain() {
        if let Some(e) = cause.downcast_ref::<odm_core::Error>() {
            return e.clone();
        }
    }
    odm_core::Error::Internal(err.to_string())
}

fn init_tracing(verbose: u8, quiet: bool, has_progress: bool) {
    let default_level = if quiet {
        "warn"
    } else {
        match verbose {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let filter = if has_progress && verbose == 0 && !quiet {
        EnvFilter::new("warn,odm_download_engine=info,odm_http_client=warn,odm_storage=warn")
    } else {
        filter
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .init();
}

async fn run(args: Cli) -> anyhow::Result<()> {
    match args.command {
        Command::Download(cmd) => crate::cli::download::run(cmd).await,
        Command::Version => {
            println!(
                "OpenDownloadManager {} (Phase 1 CLI)",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}
