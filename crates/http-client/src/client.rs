//! The HTTP client wrapper.

use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, ClientBuilder};
use tracing::debug;

use crate::config::{HttpClientConfig, RedirectPolicy};

/// HTTP client used by the download engine.
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: Client,
    config: HttpClientConfig,
}

impl HttpClient {
    /// Constructs a new client from a configuration.
    ///
    /// # Errors
    /// Returns an error if the underlying `reqwest` builder fails
    /// (for example because the user-agent is invalid).
    pub fn new(config: HttpClientConfig) -> Result<Self, reqwest::Error> {
        let mut builder = ClientBuilder::new()
            .user_agent(&config.user_agent)
            .connect_timeout(config.connect_timeout)
            // Load *both* root certificate sources: the bundled (webpki)
            // roots, enabled by the `rustls-tls` feature, and the operating
            // system's roots, enabled by `rustls-tls-native-roots`.
            // Certificate validation is always on; nothing in this crate
            // ever calls `danger_accept_invalid_certs`.
            .tls_built_in_root_certs(true)
            // Multiplex concurrent requests over one HTTP/2 connection where
            // the server negotiates it via ALPN.
            .http2_adaptive_window(true);

        if config.request_timeout > Duration::ZERO {
            builder = builder.timeout(config.request_timeout);
        }

        let policy = match config.redirect_policy {
            RedirectPolicy::Limited { max } => Policy::limited(max),
            RedirectPolicy::None => Policy::none(),
        };

        let inner = builder.redirect(policy).build()?;

        debug!(user_agent = %config.user_agent, "HTTP client initialized");
        Ok(Self { inner, config })
    }

    /// Returns a reference to the inner `reqwest::Client`.
    #[must_use]
    pub fn inner(&self) -> &Client {
        &self.inner
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &HttpClientConfig {
        &self.config
    }
}
