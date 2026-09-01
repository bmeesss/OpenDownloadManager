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
///
/// # Content encoding
///
/// There is deliberately no knob for `Accept-Encoding` here. The workspace
/// builds `reqwest` without its `gzip`, `brotli` and `deflate` features, so
/// the client never advertises those encodings and never transparently
/// decodes a response body. Two consequences that a download manager cares
/// about:
///
/// * the bytes written to disk are exactly the bytes the server sent, and
/// * `Content-Length` always describes the file, never a compressed payload
///   of unknown expanded size.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// User-Agent header value.
    pub user_agent: String,
    /// Maximum time to wait for a TCP/TLS connect to complete.
    pub connect_timeout: Duration,
    /// Maximum time to wait for the whole request to complete.
    ///
    /// Defaults to zero, meaning *no* overall request timeout. Large
    /// downloads legitimately run for hours, so bounding the whole request
    /// is the wrong tool; a stalled-body watchdog belongs here instead.
    pub request_timeout: Duration,
    /// Redirect policy.
    pub redirect_policy: RedirectPolicy,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            user_agent: format!(
                "OpenDownloadManager/{} (+https://github.com/bmeesss/OpenDownloadManager)",
                env!("CARGO_PKG_VERSION")
            ),
            connect_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(0),
            redirect_policy: RedirectPolicy::default(),
        }
    }
}
