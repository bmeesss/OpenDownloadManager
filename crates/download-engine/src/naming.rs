//! Naming policy for downloads whose destination is derived from the URL.
//!
//! This lives apart from the orchestration in [`crate::engine`] so the
//! policy can grow — for example, preferring a sanitized
//! `Content-Disposition` filename over the URL's last path segment —
//! without touching the download loop.

use std::path::PathBuf;

use url::Url;

/// Derives a default output path from a URL, for when the caller did not
/// supply an explicit destination.
///
/// The last path segment is used, passed through
/// [`odm_storage::sanitize_filename`]. Query strings and fragments never
/// reach the result because they are not part of [`Url::path_segments`].
///
/// The result is always a single, relative file name.
#[must_use]
pub fn default_output_for(url: &Url) -> PathBuf {
    let last = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("download.bin");
    let sanitized = odm_storage::sanitize_filename(last);
    PathBuf::from(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_output_for_strips_query() {
        let url = Url::parse("https://example.com/file.zip?token=abc").unwrap();
        let out = default_output_for(&url);
        assert_eq!(out, PathBuf::from("file.zip"));
    }

    #[test]
    fn default_output_for_empty_path() {
        let url = Url::parse("https://example.com/").unwrap();
        let out = default_output_for(&url);
        assert_eq!(out, PathBuf::from("download.bin"));
    }

    #[test]
    fn default_output_for_is_always_relative() {
        // The derived name must never be able to escape the working
        // directory, however hostile the URL path is.
        let url = Url::parse("https://example.com/a/../../etc/passwd").unwrap();
        let out = default_output_for(&url);
        assert_eq!(out.components().count(), 1, "got {out:?}");
        assert!(out.is_relative(), "got {out:?}");
    }
}
