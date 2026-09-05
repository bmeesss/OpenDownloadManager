//! Torrent-specific path validation.
//!
//! Validates that every file path inside a torrent is safe to materialize
//! under a designated output root.

use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

/// Reasons a torrent path can be rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorrentPathError {
    /// Path contains `..` traversal.
    ParentTraversal,
    /// Path is absolute.
    Absolute,
    /// Path contains NUL bytes.
    NulByte,
    /// Path contains control characters.
    ControlCharacter,
    /// A component is a Windows reserved device name.
    ReservedName,
    /// A component contains a path separator embedded in a name.
    EmbeddedSeparator,
    /// Path escapes the output root after normalization.
    EscapesRoot,
    /// Duplicate or case-insensitive collision.
    DuplicateOrCollision,
}

impl std::fmt::Display for TorrentPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParentTraversal => write!(f, "parent traversal not allowed"),
            Self::Absolute => write!(f, "absolute path not allowed"),
            Self::NulByte => write!(f, "NUL byte in path"),
            Self::ControlCharacter => write!(f, "control character in path"),
            Self::ReservedName => write!(f, "reserved Windows name"),
            Self::EmbeddedSeparator => write!(f, "embedded path separator"),
            Self::EscapesRoot => write!(f, "path escapes output root"),
            Self::DuplicateOrCollision => write!(f, "duplicate or colliding path"),
        }
    }
}

impl std::error::Error for TorrentPathError {}

/// Validates a single file path from torrent metadata.
///
/// The path must be relative, must not contain `..` or absolute components,
/// must not contain NUL or control bytes, must not use Windows reserved
/// names, and must not contain embedded separators.
pub fn validate_torrent_path(path: &Path) -> std::result::Result<(), TorrentPathError> {
    if path.as_os_str().is_empty() {
        return Err(TorrentPathError::ParentTraversal);
    }
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(TorrentPathError::NulByte);
    }
    let raw = path.to_string_lossy();
    if raw.starts_with('\\')
        || raw.starts_with('/')
        || (raw.len() >= 2 && raw.as_bytes()[0].is_ascii_alphabetic() && raw.as_bytes()[1] == b':')
    {
        return Err(TorrentPathError::Absolute);
    }

    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(TorrentPathError::Absolute);
            }
            Component::ParentDir => {
                return Err(TorrentPathError::ParentTraversal);
            }
            Component::CurDir => {}
            Component::Normal(s) => {
                saw_normal = true;
                let name = s.to_str().ok_or(TorrentPathError::ControlCharacter)?;
                if name.contains('/') || name.contains('\\') {
                    return Err(TorrentPathError::EmbeddedSeparator);
                }
                odm_storage::validate_filename(name).map_err(|_| TorrentPathError::ReservedName)?;
            }
        }
    }
    if !saw_normal {
        return Err(TorrentPathError::ParentTraversal);
    }
    Ok(())
}

/// Checks whether a path escapes the given root after normalization.
pub fn escapes_root(root: &Path, path: &Path) -> bool {
    let resolved = root.join(path);
    !resolved.starts_with(root)
}

/// Rejects links in the root and in every already-existing path component.
/// `symlink_metadata` is deliberately used so validation never follows a link.
pub fn validate_existing_components(
    root: &Path,
    paths: &[PathBuf],
) -> std::result::Result<(), TorrentPathError> {
    fn validate_ancestors(path: &Path) -> std::result::Result<(), TorrentPathError> {
        let mut current = path.to_path_buf();
        loop {
            if std::fs::symlink_metadata(&current).is_ok() {
                reject_link(&current)?;
            }
            let Some(parent) = current.parent() else {
                break;
            };
            if parent == current {
                break;
            }
            current = parent.to_path_buf();
        }
        Ok(())
    }

    validate_ancestors(root)?;

    for relative in paths {
        let mut current = root.to_path_buf();
        for component in relative.components() {
            if let Component::Normal(name) = component {
                current.push(name);
                validate_ancestors(&current)?;
            }
        }
    }
    Ok(())
}

fn reject_link(path: &Path) -> std::result::Result<(), TorrentPathError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| TorrentPathError::EscapesRoot)?;
    #[cfg(unix)]
    if metadata.file_type().is_symlink() {
        return Err(TorrentPathError::EscapesRoot);
    }
    #[cfg(windows)]
    if metadata.file_attributes() & 0x400 != 0 {
        return Err(TorrentPathError::EscapesRoot);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_dotdot() {
        let p = Path::new("../outside.txt");
        assert!(validate_torrent_path(p).is_err());
    }

    #[test]
    fn rejects_absolute() {
        let p = Path::new("/etc/passwd");
        assert!(validate_torrent_path(p).is_err());
    }

    #[test]
    fn rejects_windows_reserved() {
        let p = Path::new("CON.txt");
        assert!(validate_torrent_path(p).is_err());
    }

    #[test]
    fn accepts_simple_relative() {
        let p = Path::new("dir/file.txt");
        assert!(validate_torrent_path(p).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_existing_symlink_component() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert!(
            validate_existing_components(root.path(), &[PathBuf::from("link/file.bin")]).is_err()
        );
    }
}
