//! Storage layer unit tests.

use std::path::PathBuf;

use odm_core::Error;
use odm_storage::{FileStorage, ensure_parent_dir, sanitize_filename, validate_filename, validate_path};
use tempfile::TempDir;

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
fn validate_filename_rejects_separators() {
    assert!(validate_filename("a/b").is_err());
    assert!(validate_filename("a\\b").is_err());
}

#[test]
fn validate_filename_rejects_empty() {
    assert!(validate_filename("").is_err());
}

#[test]
fn sanitize_replaces_unsafe() {
    let s = sanitize_filename("foo/bar:baz*?");
    assert!(validate_filename(&s).is_ok());
}

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
    storage.finalize(part, &final_path).await.expect("finalize");
    let content = std::fs::read(&final_path).unwrap();
    assert_eq!(content, b"hello world");
}

#[tokio::test]
async fn finalize_fails_if_target_exists() {
    let tmp = TempDir::new().unwrap();
    let final_path = tmp.path().join("output.bin");
    std::fs::write(&final_path, b"existing").unwrap();
    let storage = FileStorage::new();
    let part = storage.create_part_file(&final_path).await.expect("create");
    let err = storage.finalize(part, &final_path).await.expect_err("err");
    assert!(matches!(err, Error::AlreadyExists(_)));
}
