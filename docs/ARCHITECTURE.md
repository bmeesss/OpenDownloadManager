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

Protocol-neutral domain vocabulary shared by every other crate:

* `Error`, `Result`
* `DownloadRequest`, `DownloadSummary`, `DownloadProgress`
* `ResourceInfo`, `InspectInfo`
* `HttpMethod`
* `ProgressSink` trait

Has no internal dependencies.

> `ResourceInfo`, `InspectInfo` and `HttpMethod` are HTTP-specific types
> that currently live here for historical reasons. They are slated to move
> into `odm-http-client` in Phase 2, before a second protocol exists to
> make the boundary real. **Do not build new shared abstractions on top of
> them.**

### `odm-http-client`

Wraps `reqwest` (rustls, bundled *and* native root certificates, HTTP/2).
Provides:

* `HttpClient::new(HttpClientConfig)`
* `inspect(&client, &url) -> InspectInfo` (HEAD)
* `download_stream(&client, &url) -> StreamingResponse` (streaming GET)
* `HttpClientConfig` controls User-Agent, connect timeout, an optional
  whole-request timeout, and the redirect policy (limited to 10 hops by
  default).

The body is exposed as a `Stream<Item = Result<Bytes>>` so that callers
never need to buffer the full file.

`reqwest` is built **without** its `gzip`/`brotli`/`deflate` features, so
responses are never transparently decoded. The bytes handed to the caller
are the bytes on the wire.

### `odm-storage`

Filesystem helpers with strict path validation:

* `validate_path` — for **untrusted relative** paths (names derived from a
  URL). Rejects absolute paths.
* `validate_output_path` — for the **destination the user chose**. Allows
  absolute paths; still rejects `..`, NUL bytes, control characters,
  unusable final components, and Windows reserved device names on every
  platform.
* `validate_filename`, `sanitize_filename`
* `ensure_parent_dir`
* `FileStorage::create_part_file`, `finalize`, `remove_part`

Partial files use a unique `.<filename>.<uuid>.part` suffix created with
`O_EXCL`, and are atomically renamed on success. Finalization failures
intentionally leave the `.part` file behind for debugging.

The storage writer is **async and sequential**: one file, one append
offset. That is the right shape for HTTP and the wrong shape for
piece-based, multi-file, random-access writes. The two are not meant to
share an API.

### `odm-download-engine`

Orchestrates a single HTTP download end-to-end:

1. Validate the URL scheme and the destination path.
2. Refuse to overwrite an existing file unless asked.
3. Ensure the parent directory exists.
4. Open a streaming `GET` and read `Content-Length` from the response.
5. Open the `.part` file.
6. Stream bytes, writing each chunk to disk, with throttled progress
   reporting (250 ms) and optional cancellation.
7. **Verify the byte count against `Content-Length`.** A transfer that did
   not deliver what the server announced is an error; nothing is renamed
   and the `.part` file is kept.
8. `fsync`, then atomic rename.
9. Emit the final summary.

The engine never holds the file in memory and accepts a
`tokio::sync::Notify` for cancellation.

`default_output_for(url)` lives in its own `naming` module so the naming
policy can grow — for example, preferring a sanitized
`Content-Disposition` filename — without touching the download loop.

### `odm-cli`

The `download-manager` binary. Uses `clap` for parsing and `indicatif`
for progress bars. Maps `odm_core::Error` variants to documented exit
codes (`Exit::from_error`, exhaustively tested).

## What is *not* implemented

Recorded here so that this document never describes aspirational
behaviour as shipped:

* **Cancellation is not reachable from the CLI.** The engine accepts a
  cancellation handle; `download-manager` passes none, so Ctrl+C kills the
  process and orphans the `.part` file.
* **`inspect()` is not called by the engine.** HEAD inspection exists and
  is tested, but the download path takes its size from the `GET` response.
* **`DownloadRequest::max_redirects` is validated but not applied.** The
  redirect cap is a client-wide setting.
* **`Content-Disposition` filenames are parsed but unused.** Output naming
  comes from the URL only.
* No resume, Range requests, retries, mirrors, checksums, speed limits,
  queue, persistence or crash recovery.

## Phase 2 hooks (already present, still unused)

* `ResourceInfo::accepts_ranges` is surfaced.
* `Error::RangeRequestsUnsupported` is reserved.
* `Notify`-based cancellation is supported by the engine.

---

## Future direction

Locked in now so that later phases do not require rewriting the current
crate boundaries.

```
Phase 2:
protocol-neutral download manager

Phase 3:
BitTorrent backend using librqbit

HTTP remains HTTP-specific.
BitTorrent remains BitTorrent-specific.
Shared concerns live above the protocol-specific layers.
```

Concretely, the target shape is:

```
odm-core                    protocol-neutral vocabulary
   ^
odm-download-manager  (P2)  queue, scheduler, persistence, event bus
   |                        owns a set of download backends
   ├── odm-download-engine  HTTP backend
   │     └── odm-http-client
   ├── odm-torrent-backend  BitTorrent backend (P3, thin adapter over
   │     └── librqbit       librqbit's `Api`/`Session`)
   └── odm-storage          shared by both: path validation, sanitization,
                            download root, disk space. Exposes an async
                            sequential surface for HTTP *and* an
                            implementation of librqbit's `TorrentStorage`
                            for pieces — two surfaces, one validation layer.
```

The shared surface is deliberately small — lifecycle, progress, state,
destination, pause/resume/cancel, events, persistence, bandwidth limits.
Everything protocol-specific (peers, pieces, trackers, DHT, availability,
ratio and seeding policy, swarm state) stays below that line and reaches
the UI through an opaque, serializable per-protocol payload.

Decisions that are already locked and must not be re-litigated:

1. `odm-core` stays protocol-neutral. No new HTTP-specific types there.
2. BitTorrent is a **sibling backend**, never part of `odm-http-client`.
3. The HTTP sequential writer and the BitTorrent positioned multi-file
   writer are **not** unified. They share the validation and
   download-root layer only.
4. `DownloadProgress::at` is a `SystemTime`, not an `Instant`, so progress
   snapshots survive serialization and process boundaries.
5. OpenDownloadManager owns persistence (SQLite, Phase 2) and drives
   `librqbit` with its own persistence disabled, so there is exactly one
   source of truth for download state.

**None of the Phase 2/3 layers exist yet, and none should be introduced
speculatively.** A trait with a single implementation is a guess, not an
abstraction.
