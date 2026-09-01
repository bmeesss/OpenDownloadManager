//! HTTP client error type.

use thiserror::Error;

/// Errors that can occur while constructing or using the HTTP client.
#[derive(Debug, Error)]
pub enum HttpClientError {
    /// The underlying `reqwest` client builder failed.
    #[error("failed to build HTTP client: {0}")]
    Build(String),
}

impl From<reqwest::Error> for HttpClientError {
    fn from(e: reqwest::Error) -> Self {
        Self::Build(e.to_string())
    }
}
