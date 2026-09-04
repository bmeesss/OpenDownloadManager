//! End-to-end test: the manager driving the real HTTP backend.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use odm_core::{Backend, DownloadState, Event};
use odm_download_engine::{DownloadEngine, EngineConfig};
use odm_download_manager::{DownloadManager, DownloadSpec, ManagerConfig};
use tempfile::TempDir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn manager_downloads_via_http_backend() {
    let server = MockServer::start().await;
    let body = b"hello world".to_vec();
    Mock::given(method("GET"))
        .and(path("/file.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", body.len().to_string())
                .set_body_bytes(body.clone()),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let db = dir.path().join("mgr.sqlite");
    let engine = Arc::new(DownloadEngine::new(EngineConfig::default()).unwrap());
    let backend: Arc<dyn Backend> = engine;

    let mgr = DownloadManager::open(ManagerConfig::default(), &db, vec![backend]).unwrap();

    let out = dir.path().join("file.txt");
    let url = Url::parse(&format!("{}/file.txt", server.uri())).unwrap();
    let id = mgr.enqueue(DownloadSpec::new(url, out.clone())).unwrap();

    let mut rx = mgr.subscribe();
    let mut done = false;
    let timeout = Duration::from_secs(10);
    while let Ok(Ok(ev)) = tokio::time::timeout(timeout, rx.recv()).await {
        if let Event::Completed(cid) = ev {
            if cid == id {
                done = true;
                break;
            }
        }
    }
    assert!(done, "download should complete");

    assert_eq!(mgr.get(id).unwrap().state, DownloadState::Completed);
    assert_eq!(std::fs::read(&out).unwrap(), body);
}

#[tokio::test]
async fn manager_reports_failed_http_download() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let db = dir.path().join("mgr.sqlite");
    let engine = Arc::new(DownloadEngine::new(EngineConfig::default()).unwrap());
    let backend: Arc<dyn Backend> = engine;
    let mgr = DownloadManager::open(ManagerConfig::default(), &db, vec![backend]).unwrap();

    let out = dir.path().join("missing.bin");
    let url = Url::parse(&format!("{}/missing", server.uri())).unwrap();
    let id = mgr.enqueue(DownloadSpec::new(url, out)).unwrap();

    let mut rx = mgr.subscribe();
    let mut failed = false;
    let timeout = Duration::from_secs(10);
    while let Ok(Ok(ev)) = tokio::time::timeout(timeout, rx.recv()).await {
        match ev {
            Event::Failed { id: cid, .. } if cid == id => {
                failed = true;
                break;
            }
            Event::Completed(cid) if cid == id => break,
            _ => {}
        }
    }
    assert!(failed, "a 404 must end in failure");
    assert_eq!(mgr.get(id).unwrap().state, DownloadState::Failed);
}
