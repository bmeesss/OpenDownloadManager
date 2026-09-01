//! Filesystem abstraction and path validation for OpenDownloadManager.
//!
//! This crate is responsible for two things:
//!
//! 1. **Path safety.** Validating that user-supplied or remote-supplied
//!    file names are safe to use on the host OS.
//! 2. **Atomic writes.** Streaming bytes to a unique `.part` file and
//!    atomically renaming it to its final name on success.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod path;
mod storage;

pub use path::{
    InvalidFileNameReason, InvalidPathReason, ensure_parent_dir, sanitize_filename,
    validate_filename, validate_path,
};
pub use storage::{FileStorage, PartFileHandle, Storage};
