//! Integration tests for OpenDownloadManager.
//!
//! This crate publishes no library surface: every module is gated behind
//! `cfg(test)` so that a non-test build compiles to nothing.

#![deny(unsafe_code)]

#[cfg(test)]
mod end_to_end;
#[cfg(test)]
mod http_client;
#[cfg(test)]
mod storage;
#[cfg(test)]
mod util;
