//! End-to-end download engine tests against a local mock server.

use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use odm_core::{DownloadRequest, Error, ProgressSink};
use odm_download_engine::{DownloadEngine, DownloadOptions, EngineConfig};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn temp_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("odm-e2e-")
        .tempdir()
        .expect("tempdir")
}

fn make_engine() -> DownloadEngine {
    DownloadEngine::new(EngineConfig::default()).expect("engine")
}

#[tokio::test]
async fn downloads_a_file() {
    let server = MockServer::start().await;
    let body = b"hello world".to_vec();
    Mock::given(method("GET"))
        .and(path("/file.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/plain")
                .insert_header("Content-Length", body.len().to_string())
                .set_body_bytes(body.clone()),
        )
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("file.txt");
    let url = format!("{}/file.txt", server.uri());

    let engine = make_engine();
    let req = DownloadRequest {
        url: url::Url::parse(&url).unwrap(),
        output: out.clone(),
        overwrite: false,
        max_redirects: 10,
    };
    let summary = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect("download");
    assert_eq!(summary.total_bytes, body.len() as u64);
    assert_eq!(std::fs::read(&out).unwrap(), body);
}

#[tokio::test]
async fn follows_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "/final"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", "2")
                .set_body_string("ok"),
        )
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("final.txt");
    let engine = make_engine();
    let req = DownloadRequest {
        url: url::Url::parse(&format!("{}/redirect", server.uri())).unwrap(),
        output: out.clone(),
        overwrite: false,
        max_redirects: 10,
    };
    let summary = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect("download");
    assert_eq!(summary.total_bytes, 2);
    assert_eq!(summary.final_url.path(), "/final");
}

#[tokio::test]
async fn handles_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("missing.bin");
    let engine = make_engine();
    let req = DownloadRequest {
        url: url::Url::parse(&format!("{}/x", server.uri())).unwrap(),
        output: out,
        overwrite: false,
        max_redirects: 10,
    };
    let err = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect_err("should fail");
    assert!(matches!(err, Error::Http { status: 404, .. }));
}

#[tokio::test]
async fn handles_403() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let tmp = temp_dir();
    let out = tmp.path().join("forbidden.bin");
    let engine = make_engine();
    let req = DownloadRequest {
        url: url::Url::parse(&format!("{}/x", server.uri())).unwrap(),
        output: out,
        overwrite: false,
        max_redirects: 10,
    };
    let err = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect_err("should fail");
    assert!(matches!(err, Error::Http { status: 403, .. }));
}

#[tokio::test]
async fn handles_500() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let tmp = temp_dir();
    let out = tmp.path().join("oops.bin");
    let engine = make_engine();
    let req = DownloadRequest {
        url: url::Url::parse(&format!("{}/x", server.uri())).unwrap(),
        output: out,
        overwrite: false,
        max_redirects: 10,
    };
    let err = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect_err("should fail");
    assert!(matches!(err, Error::Http { status: 500, .. }));
}

#[tokio::test]
async fn connection_failure_to_invalid_host() {
    let tmp = temp_dir();
    let out: PathBuf = tmp.path().join("x.bin");
    let engine = make_engine();
    // RFC 5737 TEST-NET-1 + invalid port => connect error.
    let req = DownloadRequest {
        url: url::Url::parse("http://192.0.2.1:1/x").unwrap(),
        output: out,
        overwrite: false,
        max_redirects: 10,
    };
    let err = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect_err("should fail");
    assert!(matches!(err, Error::Network(_)));
}

#[tokio::test]
async fn invalid_url_is_rejected() {
    let tmp = temp_dir();
    let out = tmp.path().join("x.bin");
    let engine = make_engine();
    let req = DownloadRequest {
        url: url::Url::parse("ftp://example.com/file").unwrap(),
        output: out,
        overwrite: false,
        max_redirects: 10,
    };
    let err = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect_err("should fail");
    assert!(matches!(err, Error::InvalidUrl(_)));
}

#[tokio::test]
async fn output_parent_dir_is_created() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", "1")
                .set_body_string("a"),
        )
        .mount(&server)
        .await;
    let tmp = temp_dir();
    let out = tmp.path().join("a").join("b").join("c.bin");
    let engine = make_engine();
    let req = DownloadRequest {
        url: url::Url::parse(&format!("{}/x", server.uri())).unwrap(),
        output: out.clone(),
        overwrite: false,
        max_redirects: 10,
    };
    engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect("download");
    assert!(out.exists());
}

#[tokio::test]
async fn invalid_filename_is_rejected() {
    let engine = make_engine();
    let tmp = temp_dir();
    // Path containing '..' is rejected by validate_path.
    let req = DownloadRequest {
        url: url::Url::parse("https://example.com/").unwrap(),
        output: tmp.path().join("..").join("evil.bin"),
        overwrite: false,
        max_redirects: 10,
    };
    let err = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect_err("should fail");
    assert!(matches!(err, Error::InvalidPath(_)));
}

#[tokio::test]
async fn progress_sink_is_invoked() {
    use std::sync::Mutex;

    struct Count {
        n: Mutex<u32>,
    }
    impl ProgressSink for Count {
        fn on_progress(&self, _: odm_core::DownloadProgress) {
            *self.n.lock().unwrap() += 1;
        }
    }

    let server = MockServer::start().await;
    let body: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", body.len().to_string())
                .set_body_bytes(Bytes::from(body.clone())),
        )
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("data.bin");
    let engine = make_engine();
    let count = Arc::new(Count { n: Mutex::new(0) });
    let req = DownloadRequest {
        url: url::Url::parse(&format!("{}/x", server.uri())).unwrap(),
        output: out,
        overwrite: false,
        max_redirects: 10,
    };
    engine
        .download(
            &req,
            &DownloadOptions::default(),
            Some(count.clone() as Arc<dyn ProgressSink>),
            None,
        )
        .await
        .expect("download");
    // Even with throttling, we expect at least one update for >0 bytes.
    assert!(*count.n.lock().unwrap() >= 1);
}

#[tokio::test]
async fn streaming_never_loads_full_file_into_memory() {
    // We can't directly measure memory, but we can verify that a large
    // response with small chunks completes successfully.
    let server = MockServer::start().await;
    let size: usize = 256 * 1024; // 256 KiB
    let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", body.len().to_string())
                .set_body_bytes(Bytes::from(body.clone())),
        )
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("big.bin");
    let engine = make_engine();
    let req = DownloadRequest {
        url: url::Url::parse(&format!("{}/x", server.uri())).unwrap(),
        output: out.clone(),
        overwrite: false,
        max_redirects: 10,
    };
    let summary = engine
        .download(&req, &DownloadOptions::default(), None, None)
        .await
        .expect("download");
    assert_eq!(summary.total_bytes as usize, size);
    let on_disk = std::fs::read(&out).unwrap();
    assert_eq!(on_disk.len(), size);
    assert_eq!(on_disk, body);
}
