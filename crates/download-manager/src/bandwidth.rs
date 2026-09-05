//! Bandwidth policy and a token-bucket rate limiter.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use odm_core::RateLimiter;
use tokio::sync::Mutex;
use tokio::time::Duration;

/// The manager's bandwidth policy: a single global cap shared across the
/// active downloads.
#[derive(Debug, Clone)]
pub struct BandwidthPolicy {
    /// Global cap in bytes per second, or `None` for unlimited.
    pub max_bytes_per_sec: Option<u64>,
}

impl BandwidthPolicy {
    /// A policy with no bandwidth limit.
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            max_bytes_per_sec: None,
        }
    }

    /// A policy capped at `max_bytes_per_sec` (a cap of `0` means unlimited).
    #[must_use]
    pub fn new(max_bytes_per_sec: Option<u64>) -> Self {
        Self { max_bytes_per_sec }
    }

    /// The per-download share of the global cap given `active` transfers.
    #[must_use]
    pub fn per_download_budget(&self, active: usize) -> Option<u64> {
        self.max_bytes_per_sec
            .map(|total| total / active.max(1) as u64)
    }

    /// Builds a rate limiter for one download, or `None` when unlimited.
    #[must_use]
    pub fn limiter_for(&self, active: usize) -> Option<Arc<dyn RateLimiter>> {
        self.per_download_budget(active)
            .filter(|&b| b > 0)
            .map(|b| Arc::new(TokenBucket::new(b)) as Arc<dyn RateLimiter>)
    }
}

/// A token-bucket byte-rate limiter.
///
/// `acquire` refills tokens proportional to elapsed time and sleeps until
/// enough are available, so it throttles without busy-waiting.
pub struct TokenBucket {
    rate: u64,
    tokens: Mutex<f64>,
    last: Mutex<Instant>,
}

impl TokenBucket {
    /// Creates a limiter that allows `bytes_per_sec` bytes per second.
    #[must_use]
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            rate: bytes_per_sec,
            tokens: Mutex::new(bytes_per_sec as f64),
            last: Mutex::new(Instant::now()),
        }
    }
}

#[async_trait]
impl RateLimiter for TokenBucket {
    async fn acquire(&self, bytes: u64) {
        if self.rate == 0 {
            return;
        }
        let need = bytes as f64;
        loop {
            let now = Instant::now();
            let mut last = self.last.lock().await;
            let elapsed = now.duration_since(*last).as_secs_f64();
            *last = now;
            let mut tokens = self.tokens.lock().await;
            *tokens += elapsed * self.rate as f64;
            if *tokens > self.rate as f64 {
                *tokens = self.rate as f64;
            }
            if *tokens >= need {
                *tokens -= need;
                break;
            }
            let deficit = need - *tokens;
            let wait = Duration::from_secs_f64(deficit / self.rate as f64);
            drop(tokens);
            drop(last);
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_policy_has_no_budget() {
        assert_eq!(BandwidthPolicy::unlimited().per_download_budget(5), None);
        assert!(BandwidthPolicy::unlimited().limiter_for(5).is_none());
    }

    #[test]
    fn budget_is_shared_across_active_downloads() {
        let p = BandwidthPolicy::new(Some(1000));
        assert_eq!(p.per_download_budget(1), Some(1000));
        assert_eq!(p.per_download_budget(4), Some(250));
        assert_eq!(p.per_download_budget(0), Some(1000));
    }

    #[tokio::test]
    async fn token_bucket_allows_without_sleeping_when_full() {
        let bucket = TokenBucket::new(1_000_000);
        // Well under the initial token balance, so this returns immediately.
        bucket.acquire(10).await;
    }
}
