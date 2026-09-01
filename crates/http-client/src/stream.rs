//! Streaming GET downloads.

use bytes::Bytes;
use futures_util::Stream;
use odm_core::{Error, HttpMethod, Result, ResourceInfo};
use reqwest::Response;
use std::pin::Pin;
use tracing::debug;
use url::Url;

use crate::client::HttpClient;
use crate::inspect::{extract_resource_info, map_request_error};

/// A streaming HTTP response.
///
/// The body is exposed as a `Stream<Item = Result<Bytes>>` so that the
/// caller can write chunks to disk without buffering the entire file.
pub struct StreamingResponse {
    /// The final URL after redirects.
    pub final_url: Url,
    /// HTTP status code.
    pub status: u16,
    /// Resource metadata extracted from response headers.
    pub resource: ResourceInfo,
    /// Byte stream of the body.
    pub body: Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>,
}

/// Performs a streaming GET request and returns the response body as a stream.
///
/// # Errors
/// Returns an error if the request fails or the status is not successful.
pub async fn download_stream(client: &HttpClient, url: &Url) -> Result<StreamingResponse> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(Error::InvalidUrl(format!("unsupported scheme: {other}")));
        }
    }

    let resp = client
        .inner()
        .request(to_reqwest_method(HttpMethod::Get), url.as_str())
        .send()
        .await
        .map_err(map_request_error)?;

    build_streaming(resp).await
}

pub(crate) async fn build_streaming(resp: Response) -> Result<StreamingResponse> {
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Http {
            status: status.as_u16(),
            url: resp.url().to_string(),
        });
    }
    let final_url = resp.url().clone();
    let resource = extract_resource_info(&resp, &final_url);
    debug!(
        status = status.as_u16(),
        final_url = %final_url,
        size = ?resource.content_length,
        "streaming GET started"
    );

    let inner = resp.bytes_stream();
    let body = Box::pin(async_stream::try_stream! {
        futures_util::pin_mut!(inner);
        while let Some(chunk) = futures_util::StreamExt::next(&mut inner).await {
            let chunk = chunk.map_err(map_request_error)?;
            yield chunk;
        }
    });

    Ok(StreamingResponse {
        final_url,
        status: status.as_u16(),
        resource,
        body,
    })
}

fn to_reqwest_method(method: HttpMethod) -> reqwest::Method {
    match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Head => reqwest::Method::HEAD,
    }
}
