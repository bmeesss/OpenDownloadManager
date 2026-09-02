//! Storage layer unit tests.

use std::path::PathBuf;

use odm_core::Error;
use odm_storage::{
    ensure_parent_dir, sanitize_filename, validate_filename, validate_output_path, validate_path,
    FileStorage,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// validate_path: untrusted *relative* paths. Absolute must stay rejected.
// ---------------------------------------------------------------------------

#[test]
fn validate_path_rejects_parent_traversal() {
    let p = PathBuf::from("a").join("..").join("b");
    let err = validate_path(&p).expect_err("err");
    assert!(matches!(err, Error::InvalidPath(_)));
}

#[test]
fn validate_path_rejects_absolute() {
    let p = if cfg!(windows) {
        PathBuf::from("C:\\foo\\bar")
    } else {
        PathBuf::from("/etc/passwd")
    };
    let err = validate_path(&p).expect_err("err");
    assert!(matches!(err, Error::InvalidPath(_)));
}

#[test]
fn validate_path_rejects_empty() {
    let p = PathBuf::new();
    let err = validate_path(&p).expect_err("err");
    assert!(matches!(err, Error::InvalidPath(_)));
}

#[test]
fn validate_path_rejects_control_chars() {
    let mut p = PathBuf::from("foo");
    p.push("ba\rr");
    let err = validate_path(&p).expect_err("err");
    assert!(matches!(err, Error::InvalidPath(_)));
}

#[test]
fn validate_path_accepts_a_plain_relative_path() {
    assert!(validate_path(&PathBuf::from("downloads/file.zip")).is_ok());
    assert!(validate_path(&PathBuf::from("./file.zip")).is_ok());
}

// ---------------------------------------------------------------------------
// validate_output_path: user-chosen destinations. Absolute must be allowed.
// ---------------------------------------------------------------------------

#[test]
fn validate_output_path_accepts_absolute() {
    let p = if cfg!(windows) {
        PathBuf::from("C:\\Users\\me\\Downloads\\file.zip")
    } else {
        PathBuf::from("/home/me/Downloads/file.zip")
    };
    validate_output_path(&p).expect("an absolute destination is valid");
}

#[test]
fn validate_output_path_accepts_relative() {
    validate_output_path(&PathBuf::from("file.zip")).expect("relative is valid");
    validate_output_path(&PathBuf::from("./downloads/file.zip")).expect("relative is valid");
}

#[test]
fn validate_output_path_rejects_parent_traversal() {
    // Even with absolute paths allowed, escaping the destination is not.
    let p = PathBuf::from("/home/me")
        .join("..")
        .join("etc")
        .join("passwd");
    let err = validate_output_path(&p).expect_err("err");
    assert!(matches!(err, Error::InvalidPath(_)), "unexpected: {err}");
}

#[test]
fn validate_output_path_rejects_empty() {
    let err = validate_output_path(&PathBuf::new()).expect_err("err");
    assert!(matches!(err, Error::InvalidPath(_)), "unexpected: {err}");
}

#[test]
fn validate_output_path_rejects_a_bare_root() {
    let p = if cfg!(windows) {
        PathBuf::from("C:\\")
    } else {
        PathBuf::from("/")
    };
    let err = validate_output_path(&p).expect_err("err");
    assert!(
        matches!(err, Error::InvalidPath(_) | Error::InvalidFileName(_)),
        "unexpected: {err}"
    );
}

#[test]
fn validate_output_path_rejects_control_characters() {
    let p = PathBuf::from("/tmp/ba\nd.bin");
    let err = validate_output_path(&p).expect_err("err");
    assert!(matches!(err, Error::InvalidPath(_)), "unexpected: {err}");
}

#[test]
fn validate_output_path_rejects_reserved_names_on_every_platform() {
    for name in ["CON", "con", "NUL", "LPT1", "COM9", "aux.txt"] {
        let p = PathBuf::from("/tmp").join(name);
        let err = validate_output_path(&p).unwrap_err();
        assert!(
            matches!(err, Error::InvalidFileName(_)),
            "{name} should be rejected, got {err}"
        );
    }
}

#[test]
fn validate_output_path_rejects_a_name_with_a_separator() {
    // A final component can never contain a separator, but guard the
    // invariant anyway.
    assert!(validate_filename("a/b").is_err());
    assert!(validate_filename("a\\b").is_err());
}

// ---------------------------------------------------------------------------
// validate_filename / sanitize_filename
// ---------------------------------------------------------------------------

#[test]
fn validate_filename_rejects_separators() {
    assert!(validate_filename("a/b").is_err());
    assert!(validate_filename("a\\b").is_err());
}

#[test]
fn validate_filename_rejects_empty() {
    assert!(validate_filename("").is_err());
}

#[test]
fn sanitize_replaces_unsafe_characters() {
    assert_eq!(sanitize_filename("foo/bar:baz*?"), "foo_bar_baz__");
    assert_eq!(sanitize_filename("../../etc/passwd"), ".._.._etc_passwd");
    assert_eq!(sanitize_filename("a\nb"), "a_b");
}

#[test]
fn sanitize_never_produces_an_empty_or_invalid_name() {
    for input in ["", ".", "..", "...", "   ", "/", "\\", ":", "*"] {
        let s = sanitize_filename(input);
        assert!(!s.is_empty(), "empty result for input {input:?}");
        assert!(
            validate_filename(&s).is_ok(),
            "{input:?} sanitized to {s:?}, which does not validate"
        );
    }
}

#[test]
fn sanitize_result_contains_no_path_separator() {
    for input in ["a/b", "..\\..\\x", "/etc/passwd", "a\u{0}b"] {
        let s = sanitize_filename(input);
        assert!(!s.contains('/'), "{input:?} -> {s:?}");
        assert!(!s.contains('\\'), "{input:?} -> {s:?}");
    }
}

// ---------------------------------------------------------------------------
// Part files and finalization
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ensure_parent_creates_missing_dir() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("a").join("b").join("c.bin");
    ensure_parent_dir(&nested).await.expect("ensure");
    assert!(nested.parent().unwrap().exists());
}

#[tokio::test]
async fn create_and_finalize_part_file() {
    let tmp = TempDir::new().unwrap();
    let final_path = tmp.path().join("output.bin");
    let storage = FileStorage::new();
    let mut part = storage.create_part_file(&final_path).await.expect("create");
    part.write_chunk(b"hello ").await.unwrap();
    part.write_chunk(b"world").await.unwrap();
    part.flush().await.unwrap();
    storage
        .finalize(part, &final_path, false)
        .await
        .expect("finalize");
    let content = std::fs::read(&final_path).unwrap();
    assert_eq!(content, b"hello world");
}

#[tokio::test]
async fn part_file_is_hidden_and_unique() {
    let tmp = TempDir::new().unwrap();
    let final_path = tmp.path().join("output.bin");
    let storage = FileStorage::new();

    let a = storage.create_part_file(&final_path).await.expect("create");
    let b = storage.create_part_file(&final_path).await.expect("create");

    assert_ne!(a.path(), b.path(), "each attempt needs its own part file");
    assert!(a.path().exists());
    assert!(
        a.path()
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.') && n.ends_with(".part")),
        "unexpected part file name: {:?}",
        a.path()
    );
}

#[tokio::test]
async fn finalize_fails_if_target_exists() {
    let tmp = TempDir::new().unwrap();
    let final_path = tmp.path().join("output.bin");
    std::fs::write(&final_path, b"existing").unwrap();
    let storage = FileStorage::new();
    let part = storage.create_part_file(&final_path).await.expect("create");

    let err = storage
        .finalize(part, &final_path, false)
        .await
        .expect_err("err");

    assert!(matches!(err, Error::AlreadyExists(_)));
    assert_eq!(
        std::fs::read(&final_path).unwrap(),
        b"existing",
        "the pre-existing file must be untouched"
    );
}

#[tokio::test]
async fn finalize_overwrite_replaces_target() {
    let tmp = TempDir::new().unwrap();
    let final_path = tmp.path().join("output.bin");
    std::fs::write(&final_path, b"old-content").unwrap();

    let storage = FileStorage::new();
    let mut part = storage.create_part_file(&final_path).await.expect("create");
    part.write_chunk(b"new").await.unwrap();
    part.flush().await.unwrap();

    storage
        .finalize(part, &final_path, true)
        .await
        .expect("overwrite was requested");

    assert_eq!(std::fs::read(&final_path).unwrap(), b"new");
}

#[tokio::test]
async fn remove_part_tolerates_a_missing_file() {
    let tmp = TempDir::new().unwrap();
    let storage = FileStorage::new();
    let missing = tmp.path().join("never-created.part");
    storage
        .remove_part(&missing)
        .await
        .expect("removing a missing part file is not an error");
}
