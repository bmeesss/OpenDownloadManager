//! BitTorrent backend for OpenDownloadManager.
//!
//! Implements the [`odm_core::Backend`] trait using `librqbit`.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod backend;
mod error;
mod input;
mod metadata;
mod path;
mod session;

pub use backend::TorrentBackend;
pub use error::{Error, Result};
pub use metadata::{BackendMeta, TorrentSource};
pub use path::TorrentPathError;
