//! HTTP transport for OpenDownloadManager.
//!
//! This crate wraps `reqwest` to provide a small, well-typed surface
//! for the download engine: HEAD/GET inspection, streaming downloads,
//! and a uniform error type that maps to [`odm_core::Error`].

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod client;
mod config;
mod error;
mod inspect;
mod stream;

pub use client::HttpClient;
pub use config::{HttpClientConfig, RedirectPolicy};
pub use error::HttpClientError;
pub use inspect::inspect;
pub use stream::{download_stream, StreamingResponse};
