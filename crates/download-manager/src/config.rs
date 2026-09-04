//! Manager configuration.

use crate::bandwidth::BandwidthPolicy;

/// How the manager treats downloads that were in flight when the process
/// exited without finishing them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPolicy {
    /// Interrupted downloads become [`DownloadState::Failed`] with a clear
    /// error, and must be retried explicitly.
    ///
    /// [`DownloadState::Failed`]: odm_core::DownloadState::Failed
    Failed,
    /// Interrupted downloads become [`DownloadState::Queued`] again and can be
    /// restarted by the scheduler.
    ///
    /// [`DownloadState::Queued`]: odm_core::DownloadState::Queued
    Queued,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self::Failed
    }
}

/// Configuration for [`DownloadManager`](crate::DownloadManager).
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// Maximum number of downloads allowed to transfer at the same time.
    pub max_concurrent_downloads: usize,
    /// Bandwidth policy applied across active downloads.
    pub bandwidth: BandwidthPolicy,
    /// How to recover in-flight downloads discovered at startup.
    pub recover_interrupted: RecoveryPolicy,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 3,
            bandwidth: BandwidthPolicy::unlimited(),
            recover_interrupted: RecoveryPolicy::Failed,
        }
    }
}
