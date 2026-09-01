//! Path validation and sanitization.
//!
//! Every user-provided or remote-provided path or filename goes
//! through these functions. They are deliberately strict: any
//! ambiguity is treated as an error rather than silently coerced.

use std::path::{Component, Path, PathBuf};

use odm_core::{Error, Result};

/// Reasons a path can be rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidPathReason {
    /// The path was empty.
    Empty,
    /// A component was an absolute path (e.g. `/etc/passwd` on Unix).
    Absolute,
    /// A component was a parent traversal (`..`).
    ParentTraversal,
    /// A component contained a NUL byte.
    NulByte,
    /// A component contained a control character.
    ControlCharacter,
}

/// Reasons a filename can be rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidFileNameReason {
    /// The filename was empty.
    Empty,
    /// The filename was a Windows reserved device name.
    ReservedName,
    /// The filename contained a control character.
    ControlCharacter,
    /// The filename contained a NUL byte.
    NulByte,
    /// The filename contained a path separator.
    PathSeparator,
    /// The filename was too long for the host filesystem.
    TooLong,
}

/// Validates a full output path. The path must be relative, free of
/// `..` components, and contain no control characters or NUL bytes.
///
/// # Errors
/// Returns [`Error::InvalidPath`] describing the first violation.
pub fn validate_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(Error::InvalidPath(reason_str_path(InvalidPathReason::Empty)));
    }
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(Error::InvalidPath(reason_str_path(
            InvalidPathReason::NulByte,
        )));
    }

    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(Error::InvalidPath(reason_str_path(
                    InvalidPathReason::Absolute,
                )));
            }
            Component::ParentDir => {
                return Err(Error::InvalidPath(reason_str_path(
                    InvalidPathReason::ParentTraversal,
                )));
            }
            Component::CurDir => {}
            Component::Normal(s) => {
                saw_normal = true;
                for b in s.as_encoded_bytes() {
                    if *b < 0x20 || *b == 0x7F {
                        return Err(Error::InvalidPath(reason_str_path(
                            InvalidPathReason::ControlCharacter,
                        )));
                    }
                }
            }
        }
    }
    if !saw_normal {
        return Err(Error::InvalidPath(reason_str_path(
            InvalidPathReason::Empty,
        )));
    }
    Ok(())
}

/// Validates a file name (not a path). The name must be non-empty,
/// contain no separators, no control characters, and must not be a
/// Windows reserved name.
///
/// # Errors
/// Returns [`Error::InvalidFileName`] describing the first violation.
pub fn validate_filename(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidFileName(format!(
            "{}: name is empty",
            reason_str_file(InvalidFileNameReason::Empty)
        )));
    }
    if name.len() > 255 {
        return Err(Error::InvalidFileName(format!(
            "{}: name is too long",
            reason_str_file(InvalidFileNameReason::TooLong)
        )));
    }
    if name.contains('\0') {
        return Err(Error::InvalidFileName(format!(
            "{}: NUL byte in name",
            reason_str_file(InvalidFileNameReason::NulByte)
        )));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(Error::InvalidFileName(format!(
            "{}: path separator in name",
            reason_str_file(InvalidFileNameReason::PathSeparator)
        )));
    }
    for c in name.chars() {
        if c.is_control() {
            return Err(Error::InvalidFileName(format!(
                "{}: control character in name",
                reason_str_file(InvalidFileNameReason::ControlCharacter)
            )));
        }
    }
    #[cfg(windows)]
    {
        let upper = name.to_ascii_uppercase();
        let stem = upper.split('.').next().unwrap_or("");
        if matches!(
            stem,
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return Err(Error::InvalidFileName(format!(
                "{}: reserved Windows name",
                reason_str_file(InvalidFileNameReason::ReservedName)
            )));
        }
    }
    Ok(())
}

/// Replaces unsafe characters in `name` with `_`, then validates the
/// result. The result is always safe to use on the host OS.
#[must_use]
pub fn sanitize_filename(name: &str) -> String {
    let mut buf = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_control() || c == '/' || c == '\\' || c == '\0' || c == ':' || c == '*'
            || c == '?' || c == '"' || c == '<' || c == '>' || c == '|'
        {
            buf.push('_');
        } else {
            buf.push(c);
        }
    }
    let trimmed = buf.trim_matches([' ', '.']).to_string();
    if trimmed.is_empty() {
        "download.bin".to_string()
    } else if validate_filename(&trimmed).is_ok() {
        trimmed
    } else {
        format!("{trimmed}.bin")
    }
}

/// Creates the parent directory of `path` if it does not exist.
///
/// # Errors
/// Returns [`Error::Filesystem`] on I/O failure.
pub async fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !tokio::fs::try_exists(parent).await.unwrap_or(false)
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Filesystem(format!("create_dir_all({}): {e}", parent.display())))?;
        }
    }
    Ok(())
}

/// Joins a base directory with a relative path, validating the result.
pub(crate) fn join_validated(base: &Path, rel: &Path) -> Result<PathBuf> {
    validate_path(rel)?;
    Ok(base.join(rel))
}

fn reason_str_path(r: InvalidPathReason) -> String {
    match r {
        InvalidPathReason::Empty => "empty path".to_string(),
        InvalidPathReason::Absolute => "absolute path not allowed".to_string(),
        InvalidPathReason::ParentTraversal => "parent traversal not allowed".to_string(),
        InvalidPathReason::NulByte => "NUL byte in path".to_string(),
        InvalidPathReason::ControlCharacter => "control character in path".to_string(),
    }
}

fn reason_str_file(r: InvalidFileNameReason) -> &'static str {
    match r {
        InvalidFileNameReason::Empty => "empty",
        InvalidFileNameReason::ReservedName => "reserved",
        InvalidFileNameReason::ControlCharacter => "control",
        InvalidFileNameReason::NulByte => "nul",
        InvalidFileNameReason::PathSeparator => "separator",
        InvalidFileNameReason::TooLong => "toolong",
    }
}
