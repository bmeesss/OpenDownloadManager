//! CLI argument parsing.

pub mod download;

pub use download::DownloadCmd;

use clap::{Parser, Subcommand};

/// Top-level CLI structure.
#[derive(Debug, Parser)]
#[command(
    name = "download-manager",
    bin_name = "download-manager",
    about = "OpenDownloadManager command line interface",
    long_about = "A free and open-source desktop download manager. This binary is the Phase 1 CLI.",
    version
)]
pub struct Cli {
    /// Increase verbosity. May be repeated (-v, -vv).
    #[arg(long, short = 'v', action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress all non-error output.
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Download a single URL.
    Download(DownloadCmd),
    /// Print version information and exit.
    Version,
}
