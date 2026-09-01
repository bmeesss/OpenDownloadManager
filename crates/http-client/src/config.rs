//! Configuration for the HTTP client.

use std::time::Duration;

/// Redirect policy for the HTTP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectPolicy {
    /// Follow up to N redirects, erroring past that.
    Limited {
        /// Maximum number of redirect hops.
        max: usize,
    },
    /// Do not follow redirects at all.
    None,
}

impl Default for RedirectPolicy {
    fn default() -> Self {
        Self::Limited { max: 10 }
    }
}

/// Configuration for [`crate::HttpClient`].
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// User-Agent header value.
    pub user_agent: String,
    /// Maximum time to wait for a TCP/TLS connect to complete.
    pub connect_timeout: Duration,
    /// Maximum time to wait for the whole request to complete.
    pub request_timeout: Duration,
    /// Redirect policy.
    pub redirect_policy: RedirectPolicy,
    /// Whether to accept gzip/brotli/deflate compressed responses.
    pub accept_encoding: bool,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            user_agent: format!(
                "OpenDownloadManager/{} (+https://github.com/opendownloadmanager)",
                env!("CARGO_PKG_VERSION")
            ),
            connect_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(0),
            redirect_policy: RedirectPolicy::default(),
            accept_encoding: true,
        }
    }
}
