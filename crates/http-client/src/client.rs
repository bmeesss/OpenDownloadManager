//! The HTTP client wrapper.

use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, ClientBuilder};
use tracing::debug;

use crate::config::HttpClientConfig;

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
            .cookie_store(false)
            .http2_adaptive_window(true)
            .tls_built_in_root_certs(true)
            .tls_native_roots(true);

        if config.request_timeout > Duration::ZERO {
            builder = builder.timeout(config.request_timeout);
        }

        if !config.accept_encoding {
            builder = builder.no_gzip().no_brotli().no_deflate();
        }

        let inner = match config.redirect_policy {
            crate::config::RedirectPolicy::Limited { max } => {
                builder = builder.redirect(Policy::limited(max));
                builder.build()?
            }
            crate::config::RedirectPolicy::None => {
                builder = builder.redirect(Policy::none());
                builder.build()?
            }
        };

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
