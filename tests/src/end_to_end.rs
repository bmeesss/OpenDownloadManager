//! End-to-end download engine tests against local servers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use odm_core::{DownloadProgress, DownloadRequest, Error, ProgressSink};
use odm_download_engine::{default_output_for, DownloadEngine, DownloadOptions, EngineConfig};
use odm_storage::validate_output_path;
use tempfile::TempDir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::util::{refused_port, serve_once};

fn temp_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("odm-e2e-")
        .tempdir()
        .expect("tempdir")
}

fn make_engine() -> DownloadEngine {
    DownloadEngine::new(EngineConfig::default()).expect("engine")
}

fn request(url: Url, output: PathBuf) -> DownloadRequest {
    DownloadRequest {
        url,
        output,
        overwrite: false,
        max_redirects: 10,
    }
}

/// The `.<name>.<uuid>.part` files currently sitting in `dir`.
fn part_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("part"))
        .collect()
}

// ---------------------------------------------------------------------------
// Completeness: a transfer that did not deliver what the server announced
// must never be finalized.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn short_body_is_not_finalized() {
    // Content-Length promises 100 bytes; only 50 arrive before the server
    // hangs up. Neither the transport nor the engine may treat this as a
    // successful download.
    let mut response = b"HTTP/1.1 200 OK\r\n".to_vec();
    response.extend_from_slice(b"Content-Length: 100\r\n");
    response.extend_from_slice(b"Connection: close\r\n\r\n");
    response.extend_from_slice(&[b'a'; 50]);

    let addr = serve_once(response).await;
    let tmp = temp_dir();
    let out = tmp.path().join("truncated.bin");
    let url = Url::parse(&format!("http://{addr}/file.bin")).unwrap();

    let err = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect_err("a short body must not be accepted");

    // Depending on where the truncation is noticed this surfaces either as
    // a transport error or as the engine's own length check. What must not
    // happen is a finalized file.
    assert!(
        matches!(err, Error::Network(_) | Error::InvalidResponse(_)),
        "unexpected error: {err}"
    );
    assert!(!out.exists(), "the final file must not exist");
    assert_eq!(
        part_files(tmp.path()).len(),
        1,
        "the .part must be retained"
    );
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

    let summary = make_engine()
        .download(
            &request(Url::parse(&url).unwrap(), out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("download");

    assert_eq!(summary.total_bytes, body.len() as u64);
    assert_eq!(std::fs::read(&out).unwrap(), body);
}

// ---------------------------------------------------------------------------
// Size unknown up front
// ---------------------------------------------------------------------------

#[tokio::test]
async fn zero_byte_download_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", "0")
                .set_body_bytes(Vec::<u8>::new()),
        )
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("empty.bin");
    let url = Url::parse(&format!("{}/empty", server.uri())).unwrap();

    let summary = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("a zero-byte body is a valid download");

    assert_eq!(summary.total_bytes, 0);
    assert!(out.exists(), "the empty file must still be created");
    assert_eq!(std::fs::metadata(&out).unwrap().len(), 0);
    assert!(part_files(tmp.path()).is_empty());
}

#[tokio::test]
async fn unknown_content_length_succeeds() {
    // No Content-Length at all: the body runs until the connection closes.
    let response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nhello-unknown-length".to_vec();
    let addr = serve_once(response).await;

    let tmp = temp_dir();
    let out = tmp.path().join("unknown.bin");
    let url = Url::parse(&format!("http://{addr}/x")).unwrap();

    let summary = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("download");

    assert_eq!(summary.total_bytes, "hello-unknown-length".len() as u64);
    assert_eq!(std::fs::read(&out).unwrap(), b"hello-unknown-length");
}

#[tokio::test]
async fn chunked_response_succeeds() {
    let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    let chunks = "5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    let addr = serve_once(format!("{head}{chunks}").into_bytes()).await;

    let tmp = temp_dir();
    let out = tmp.path().join("chunked.bin");
    let url = Url::parse(&format!("http://{addr}/x")).unwrap();

    let summary = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("download");

    assert_eq!(summary.total_bytes, 11);
    assert_eq!(std::fs::read(&out).unwrap(), b"hello world");
}

// ---------------------------------------------------------------------------
// Overwrite handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overwrite_false_refuses_existing_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("new"))
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("existing.bin");
    std::fs::write(&out, b"old").unwrap();
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    let err = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect_err("must refuse to clobber");

    assert!(matches!(err, Error::AlreadyExists(_)), "unexpected: {err}");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"old",
        "original must survive"
    );
}

#[tokio::test]
async fn overwrite_true_replaces_existing_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("new"))
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("existing.bin");
    std::fs::write(&out, b"much-longer-old-content").unwrap();
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    let opts = DownloadOptions { overwrite: true };
    let summary = make_engine()
        .download(&request(url, out.clone()), &opts, None, None)
        .await
        .expect("overwrite was requested");

    assert_eq!(summary.total_bytes, 3);
    assert_eq!(std::fs::read(&out).unwrap(), b"new");
    assert!(part_files(tmp.path()).is_empty());
}

// ---------------------------------------------------------------------------
// Partial file hygiene
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_part_file_remains_after_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("payload"))
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("clean.bin");
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("download");

    assert!(out.exists());
    let leftovers = part_files(tmp.path());
    assert!(
        leftovers.is_empty(),
        "leftover partial files: {leftovers:?}"
    );

    let entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 1, "the directory must hold only the output");
}

// ---------------------------------------------------------------------------
// Destination validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn absolute_output_path_is_accepted() {
    // Regression test. The engine used to run the untrusted-relative-path
    // validator over the destination, which rejected every absolute path
    // and made `--output /tmp/x` impossible.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("absolute"))
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("absolute.bin");
    assert!(out.is_absolute(), "precondition: the test path is absolute");
    validate_output_path(&out).expect("an absolute destination must validate");

    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();
    make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("download");

    assert_eq!(std::fs::read(&out).unwrap(), b"absolute");
}

#[tokio::test]
async fn default_output_path_is_derived_and_used() {
    // The CLI lets the engine name the file after the URL when `--output`
    // is omitted. Check the derived name, and that it works as a real
    // destination.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_string("pdf-bytes"))
        .mount(&server)
        .await;

    let url = Url::parse(&format!("{}/report.pdf?token=secret", server.uri())).unwrap();
    let derived = default_output_for(&url);
    assert_eq!(
        derived,
        PathBuf::from("report.pdf"),
        "query must be dropped"
    );

    let tmp = temp_dir();
    let out = tmp.path().join(&derived);
    validate_output_path(&out).expect("derived name must validate");

    let summary = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("download");

    assert_eq!(summary.output.file_name().unwrap(), "report.pdf");
    assert_eq!(std::fs::read(&out).unwrap(), b"pdf-bytes");
}

#[tokio::test]
async fn output_path_with_parent_traversal_is_rejected() {
    let tmp = temp_dir();
    let out = tmp.path().join("..").join("evil.bin");
    let url = Url::parse("https://example.com/x").unwrap();

    let err = make_engine()
        .download(&request(url, out), &DownloadOptions::default(), None, None)
        .await
        .expect_err("traversal must be rejected");

    assert!(matches!(err, Error::InvalidPath(_)), "unexpected: {err}");
}

#[tokio::test]
async fn output_path_with_reserved_name_is_rejected() {
    // Rejected on every platform, not just Windows: a download set should
    // stay portable between operating systems.
    let tmp = temp_dir();
    let out = tmp.path().join("CON");
    let url = Url::parse("https://example.com/x").unwrap();

    let err = make_engine()
        .download(&request(url, out), &DownloadOptions::default(), None, None)
        .await
        .expect_err("reserved device names must be rejected");

    assert!(
        matches!(err, Error::InvalidFileName(_)),
        "unexpected: {err}"
    );
}

#[tokio::test]
async fn output_path_with_control_character_is_rejected() {
    let tmp = temp_dir();
    let out = tmp.path().join("bad\u{1}name.bin");
    let url = Url::parse("https://example.com/x").unwrap();

    let err = make_engine()
        .download(&request(url, out), &DownloadOptions::default(), None, None)
        .await
        .expect_err("control characters must be rejected");

    assert!(matches!(err, Error::InvalidPath(_)), "unexpected: {err}");
}

// ---------------------------------------------------------------------------
// Transport failures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connection_refused_is_a_network_error() {
    // A port that was just released, so the refusal is immediate and does
    // not depend on an externally unreachable address.
    let port = refused_port().await;
    let tmp = temp_dir();
    let out = tmp.path().join("x.bin");
    let url = Url::parse(&format!("http://127.0.0.1:{port}/x")).unwrap();

    let err = make_engine()
        .download(&request(url, out), &DownloadOptions::default(), None, None)
        .await
        .expect_err("must fail");

    assert!(matches!(err, Error::Network(_)), "unexpected: {err}");
}

#[tokio::test]
async fn invalid_url_scheme_is_rejected() {
    let tmp = temp_dir();
    let out = tmp.path().join("x.bin");
    let url = Url::parse("ftp://example.com/file").unwrap();

    let err = make_engine()
        .download(&request(url, out), &DownloadOptions::default(), None, None)
        .await
        .expect_err("must fail");

    assert!(matches!(err, Error::InvalidUrl(_)), "unexpected: {err}");
}

#[tokio::test]
async fn follows_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/final"))
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
    let url = Url::parse(&format!("{}/redirect", server.uri())).unwrap();

    let summary = make_engine()
        .download(&request(url, out), &DownloadOptions::default(), None, None)
        .await
        .expect("download");

    assert_eq!(summary.total_bytes, 2);
    assert_eq!(summary.final_url.path(), "/final");
}

#[tokio::test]
async fn http_404_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("missing.bin");
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    let err = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect_err("should fail");

    assert!(matches!(err, Error::Http { status: 404, .. }));
    assert!(!out.exists());
    assert!(
        part_files(tmp.path()).is_empty(),
        "no partial file for a 404"
    );
}

#[tokio::test]
async fn http_403_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("forbidden.bin");
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    let err = make_engine()
        .download(&request(url, out), &DownloadOptions::default(), None, None)
        .await
        .expect_err("should fail");

    assert!(matches!(err, Error::Http { status: 403, .. }));
}

#[tokio::test]
async fn http_500_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("oops.bin");
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    let err = make_engine()
        .download(&request(url, out), &DownloadOptions::default(), None, None)
        .await
        .expect_err("should fail");

    assert!(matches!(err, Error::Http { status: 500, .. }));
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
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("download");

    assert!(out.exists());
}

// ---------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn progress_is_monotonic_and_ends_at_the_total() {
    struct Recorder {
        snapshots: std::sync::Mutex<Vec<DownloadProgress>>,
    }
    impl ProgressSink for Recorder {
        fn on_progress(&self, p: DownloadProgress) {
            self.snapshots.lock().unwrap().push(p);
        }
    }

    let server = MockServer::start().await;
    let body: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", body.len().to_string())
                .set_body_bytes(body.clone()),
        )
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("data.bin");
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();
    let recorder = Arc::new(Recorder {
        snapshots: std::sync::Mutex::new(Vec::new()),
    });

    let summary = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            Some(recorder.clone() as Arc<dyn ProgressSink>),
            None,
        )
        .await
        .expect("download");

    let snapshots = recorder.snapshots.lock().unwrap();
    assert!(!snapshots.is_empty(), "the sink was never called");

    for pair in snapshots.windows(2) {
        assert!(
            pair[1].downloaded_bytes >= pair[0].downloaded_bytes,
            "progress went backwards: {} -> {}",
            pair[0].downloaded_bytes,
            pair[1].downloaded_bytes
        );
    }

    let last = snapshots.last().expect("at least one snapshot");
    assert_eq!(last.downloaded_bytes, body.len() as u64);
    assert_eq!(last.total_bytes, Some(body.len() as u64));
    assert_eq!(
        last.percent(),
        Some(100.0),
        "the final snapshot must read 100%"
    );

    // Everything agrees: the summary, the last snapshot and the bytes on
    // disk all describe the same size.
    assert_eq!(summary.total_bytes, body.len() as u64);
    assert_eq!(std::fs::metadata(&out).unwrap().len(), body.len() as u64);
}

#[tokio::test]
async fn large_body_is_written_intact() {
    // Formerly named `streaming_never_loads_full_file_into_memory`, which
    // promised something the test could not actually measure. What it does
    // prove is that a body many times larger than the write buffer arrives
    // byte-for-byte intact.
    let server = MockServer::start().await;
    let size: usize = 256 * 1024;
    let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", body.len().to_string())
                .set_body_bytes(body.clone()),
        )
        .mount(&server)
        .await;

    let tmp = temp_dir();
    let out = tmp.path().join("big.bin");
    let url = Url::parse(&format!("{}/x", server.uri())).unwrap();

    let summary = make_engine()
        .download(
            &request(url, out.clone()),
            &DownloadOptions::default(),
            None,
            None,
        )
        .await
        .expect("download");

    assert_eq!(summary.total_bytes as usize, size);
    assert_eq!(std::fs::read(&out).unwrap(), body);
}
