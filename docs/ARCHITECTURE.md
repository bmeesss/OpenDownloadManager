# Architecture

## Goals and priorities

1. Correctness
2. Reliability
3. Crash recovery
4. Security
5. Testability
6. Performance
7. Usability

## Crates

### `odm-core`

Domain vocabulary shared by every other crate:

* `Error`, `Result`
* `DownloadRequest`, `DownloadSummary`, `DownloadProgress`
* `ResourceInfo`, `InspectInfo`
* `HttpMethod`
* `ProgressSink` trait

Has no internal dependencies.

### `odm-http-client`

Wraps `reqwest` (with `rustls` and built-in root CAs). Provides:

* `HttpClient::new(HttpClientConfig)`
* `inspect(&client, &url) -> InspectInfo` (HEAD)
* `download_stream(&client, &url) -> StreamingResponse` (streaming GET)
* `HttpClientConfig` controls User-Agent, connect timeout, request timeout,
  redirect policy (limited by default, 10 hops), and `Accept-Encoding`.

The body is exposed as a `Stream<Item = Result<Bytes>>` so that callers
never need to buffer the full file.

### `odm-storage`

Filesystem abstraction with strict path validation:

* `validate_path`, `validate_filename`, `sanitize_filename`
* `ensure_parent_dir`
* `FileStorage::create_part_file`, `finalize`, `remove_part`

Partial files use a unique `.<filename>.<uuid>.part` suffix and are
atomically renamed on success. Finalization failures intentionally leave
the `.part` file behind for debugging.

### `odm-download-engine`

Orchestrates a single download end-to-end:

1. Validate URL scheme and path.
2. (Optionally) HEAD-inspect to discover size / suggested filename.
3. Ensure parent directory exists; refuse to overwrite unless asked.
4. Open `.part` file.
5. Stream bytes, writing each chunk to disk, throttled progress reporting.
6. `fsync`, then atomic rename.
7. Emit final summary.

The engine never holds the file in memory and exposes a
`tokio::sync::Notify` for cancellation.

### `odm-cli`

The `download-manager` binary. Uses `clap` for parsing and `indicatif`
for progress bars. Maps `odm_core::Error` variants to documented exit
codes.

## Phase 2 hooks (out of scope for Phase 1)

* `ResourceInfo::accepts_ranges` is already surfaced.
* `DownloadRequest::max_redirects` is already plumbed.
* `Notify`-based cancellation is already supported by the engine.
* The error enum reserves `RangeRequestsUnsupported` for future use.

This allows adding range requests, multi-connection, and pause/resume
without breaking the public API.
