//! Terminal progress reporting.

use std::io::IsTerminal;
use std::sync::Mutex;
use std::time::Instant;

use indicatif::{HumanBytes, HumanDuration, ProgressBar, ProgressStyle};
use odm_core::{DownloadProgress, DownloadSummary, ProgressSink};

/// Adapter that renders progress to the terminal (or suppresses it on
/// non-TTY / when `--quiet` is set).
pub struct ProgressReporter {
    name: String,
    inner: Mutex<Option<ProgressBar>>,
    started: Instant,
}

impl ProgressReporter {
    /// Creates a new reporter for the given output filename.
    #[must_use]
    pub fn new(name: String) -> Self {
        let started = Instant::now();
        let bar = if std::io::stderr().is_terminal() {
            let pb = ProgressBar::new(0);
            pb.set_style(
                ProgressStyle::with_template(
                    "{msg:>20.bold}  [{bar:30.cyan/blue}] {bytes:>10}/{total:>10}  {eta:>4}  {bytes_per_sec:>10}",
                )
                .expect("template")
                .progress_chars("##-"),
            );
            pb.set_message(name.clone());
            Some(pb)
        } else {
            None
        };
        Self {
            name,
            inner: Mutex::new(bar),
            started,
        }
    }

    /// Marks the download as finished and renders a final summary line.
    pub fn finish(&self, summary: &DownloadSummary) {
        if let Some(bar) = self.inner.lock().unwrap().take() {
            bar.finish_and_clear();
        }
        eprintln!(
            "finished: {}  {} in {} ({}/s)",
            self.name,
            HumanBytes(summary.total_bytes),
            HumanDuration(summary.duration),
            HumanBytes(summary.average_bytes_per_sec as u64)
        );
    }
}

impl ProgressSink for ProgressReporter {
    fn on_progress(&self, p: DownloadProgress) {
        if let Some(bar) = self.inner.lock().unwrap().as_ref() {
            if let Some(total) = p.total_bytes {
                bar.set_length(total);
            }
            bar.set_position(p.downloaded_bytes);
            let elapsed = p.at.duration_since(self.started).as_secs_f64();
            let bps = if elapsed > 0.0 {
                (p.downloaded_bytes as f64 / elapsed) as u64
            } else {
                0
            };
            bar.set_message(format!(
                "{}  {}/s",
                self.name,
                HumanBytes(bps)
            ));
        }
    }
}
