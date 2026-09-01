//! Inspection of remote resources via HEAD requests.

use odm_core::{Error, HttpMethod, InspectInfo, ResourceInfo, Result};
use reqwest::Response;
use tracing::debug;
use url::Url;

use crate::client::HttpClient;

/// Performs a `HEAD` request against the given URL and extracts metadata.
///
/// # Errors
/// Returns [`Error::InvalidUrl`], [`Error::Network`], [`Error::Http`],
/// or [`Error::InvalidResponse`] as appropriate.
pub async fn inspect(client: &HttpClient, url: &Url) -> Result<InspectInfo> {
    validate_scheme(url)?;

    let resp = client
        .inner()
        .request(to_reqwest_method(HttpMethod::Head), url.as_str())
        .send()
        .await
        .map_err(map_request_error)?;

    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Http {
            status: status.as_u16(),
            url: url.as_str().to_string(),
        });
    }

    let final_url = resp.url().clone();
    let resource = extract_resource_info(&resp, &final_url);
    debug!(
        status = status.as_u16(),
        final_url = %final_url,
        size = ?resource.content_length,
        "inspection complete"
    );
    Ok(InspectInfo {
        status: status.as_u16(),
        resource,
    })
}

fn validate_scheme(url: &Url) -> Result<()> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(Error::InvalidUrl(format!("unsupported scheme: {other}"))),
    }
}

fn to_reqwest_method(method: HttpMethod) -> reqwest::Method {
    match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Head => reqwest::Method::HEAD,
    }
}

/// Extracts the [`ResourceInfo`] from a `reqwest::Response`.
///
/// Visible to other modules in the crate so that the streaming path can
/// also surface metadata.
pub(crate) fn extract_resource_info(resp: &Response, final_url: &Url) -> ResourceInfo {
    let headers = resp.headers();
    let content_length = headers
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());

    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let etag = headers
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let last_modified = headers
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let accepts_ranges = headers
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("bytes"))
        .unwrap_or(false);

    let suggested_filename = headers
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_disposition_filename);

    ResourceInfo {
        final_url: final_url.clone(),
        content_length,
        content_type,
        etag,
        last_modified,
        accepts_ranges,
        suggested_filename,
    }
}

/// Parses `Content-Disposition` to extract a filename, supporting both
/// `filename="..."` and `filename*=UTF-8''...` (RFC 5987) forms.
fn parse_content_disposition_filename(value: &str) -> Option<String> {
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename*=") {
            // RFC 5987: charset'lang'percent-encoded
            if let Some((_charset, encoded)) = rest.split_once("''") {
                if let Ok(decoded) = percent_decode(encoded) {
                    return Some(decoded);
                }
            }
        } else if let Some(rest) = part.strip_prefix("filename=") {
            let trimmed = rest.trim_matches('"');
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn percent_decode(input: &str) -> std::result::Result<String, std::convert::Infallible> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn map_request_error(err: reqwest::Error) -> Error {
    if err.is_timeout() || err.is_connect() {
        Error::Network(err.to_string())
    } else if let Some(status) = err.status() {
        Error::Http {
            status: status.as_u16(),
            url: err
                .url()
                .map(|u| u.to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
        }
    } else {
        Error::Network(err.to_string())
    }
}
