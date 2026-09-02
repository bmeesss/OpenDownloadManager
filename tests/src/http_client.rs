//! HTTP client tests against a local mock server.

use odm_core::Error;
use odm_http_client::{download_stream, inspect, HttpClient, HttpClientConfig};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> HttpClient {
    HttpClient::new(HttpClientConfig::default()).expect("client")
}

#[tokio::test]
async fn default_client_builds() {
    // Exercises the whole `reqwest` builder configuration: rustls with both
    // bundled and native root certificates, HTTP/2, the redirect policy and
    // the connect timeout. A feature mismatch in the manifest shows up here
    // as a compile error rather than a surprise at runtime.
    let c = client();
    assert!(c.config().request_timeout.is_zero(), "no whole-request cap");
    assert!(
        !c.config().connect_timeout.is_zero(),
        "connect must be capped"
    );
}

#[tokio::test]
async fn inspect_returns_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/file.bin"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/octet-stream")
                .insert_header("Content-Length", "12345")
                .insert_header("ETag", "\"abc\"")
                .insert_header("Last-Modified", "Wed, 21 Oct 2026 07:28:00 GMT")
                .insert_header("Accept-Ranges", "bytes")
                .insert_header("Content-Disposition", "attachment; filename=\"hello.bin\""),
        )
        .mount(&server)
        .await;

    let c = client();
    let url = url::Url::parse(&format!("{}/file.bin", server.uri())).unwrap();
    let info = inspect(&c, &url).await.expect("inspect");

    assert_eq!(info.status, 200);
    assert_eq!(info.resource.content_length, Some(12345));
    assert_eq!(info.resource.etag.as_deref(), Some("\"abc\""));
    assert_eq!(
        info.resource.suggested_filename.as_deref(),
        Some("hello.bin")
    );
    assert!(info.resource.accepts_ranges);
}

#[tokio::test]
async fn download_stream_yields_bytes() {
    let server = MockServer::start().await;
    let body: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", body.len().to_string())
                .set_body_bytes(body.clone()),
        )
        .mount(&server)
        .await;

    let c = client();
    let url = url::Url::parse(&format!("{}/x", server.uri())).unwrap();
    let resp = download_stream(&c, &url).await.expect("stream");

    use futures_util::StreamExt;
    let mut s = resp.body;
    let mut collected = Vec::new();
    while let Some(chunk) = s.next().await {
        collected.extend_from_slice(&chunk.expect("chunk"));
    }

    assert_eq!(collected, body);
    assert_eq!(resp.resource.content_length, Some(body.len() as u64));
}

#[tokio::test]
async fn http_error_status_propagates() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let c = client();
    let url = url::Url::parse(&format!("{}/x", server.uri())).unwrap();
    let err = download_stream(&c, &url).await.err().expect("err");

    assert!(matches!(err, Error::Http { status: 500, .. }));
}

#[tokio::test]
async fn rejects_non_http_scheme() {
    let c = client();
    let url = url::Url::parse("ftp://example.com/x").unwrap();
    let err = download_stream(&c, &url).await.err().expect("err");

    assert!(matches!(err, Error::InvalidUrl(_)));
}

#[tokio::test]
async fn rejects_non_http_scheme_on_inspect() {
    let c = client();
    let url = url::Url::parse("file:///etc/passwd").unwrap();
    let err = inspect(&c, &url).await.expect_err("err");

    assert!(matches!(err, Error::InvalidUrl(_)));
}
