//! HTTP client tests against a local mock server.

use odm_core::{Error, HttpMethod};
use odm_http_client::{HttpClient, HttpClientConfig, download_stream, inspect};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> HttpClient {
    HttpClient::new(HttpClientConfig::default()).expect("client")
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
                .insert_header(
                    "Content-Disposition",
                    "attachment; filename=\"hello.bin\"",
                ),
        )
        .mount(&server)
        .await;

    let c = client();
    let url = url::Url::parse(&format!("{}/file.bin", server.uri())).unwrap();
    let info = inspect(&c, &url).await.expect("inspect");
    assert_eq!(info.status, 200);
    assert_eq!(info.resource.content_length, Some(12345));
    assert_eq!(info.resource.etag.as_deref(), Some("\"abc\""));
    assert_eq!(info.resource.suggested_filename.as_deref(), Some("hello.bin"));
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
    let mut total = 0u64;
    let mut collected = Vec::new();
    use futures_util::StreamExt;
    let mut s = resp.body;
    while let Some(chunk) = s.next().await {
        let chunk = chunk.expect("chunk");
        total += chunk.len() as u64;
        collected.extend_from_slice(&chunk);
    }
    assert_eq!(total as usize, body.len());
    assert_eq!(collected, body);
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
    let err = download_stream(&c, &url).await.expect_err("err");
    assert!(matches!(err, Error::Http { status: 500, .. }));
}

#[tokio::test]
async fn rejects_non_http_scheme() {
    let c = client();
    let url = url::Url::parse("ftp://example.com/x").unwrap();
    let err = download_stream(&c, &url).await.expect_err("err");
    assert!(matches!(err, Error::InvalidUrl(_)));
    let _ = HttpMethod::Head;
}
