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
* `HttpMethod` (HTTP-specific, kept only for historical reasons — see below)
* `ProgressSink` trait
* **Phase 2 additions, all protocol-neutral:**
  * `DownloadState` — the download lifecycle state machine
    (`Queued`, `Starting`, `Downloading`, `Paused`, `Completed`,
    `Failed`, `Cancelled`) with explicit, validated transitions.
  * `DownloadId` — a stable, serialisable UUID used as the primary key.
  * `BackendKind` — the only protocol tag stored on a download
    (`Http` today; `Torrent` later).
  * `Backend`, `BackendTask`, `BackendOutcome`, `RateLimiter` — the
    protocol-neutral **backend boundary** the manager drives.
  * `Event` — the lifecycle event types emitted by the manager.

Has no internal dependencies.

> `ResourceInfo`, `InspectInfo` and `HttpMethod` are HTTP-specific types
> that still live here for historical reasons. They are *not* used to build
> any Phase 2 abstraction: the new `Backend`/`BackendTask` types are
> fully protocol-neutral, and an HTTP-specific request (with redirect cap
> and overwrite flag) is only constructed *inside* the HTTP backend.

### `odm-download-manager`

The protocol-neutral download manager (Phase 2). It owns everything that is
independent of *how* bytes travel:

* **Queue** (`queue`) — a FIFO of `Queued` download ids, in enqueue order.
* **Scheduler** (`scheduler`) — a pure function that, given the queue, the
  count of active downloads and the configured concurrency limit, returns
  which queued downloads may start now.
* **Lifecycle** (`manager`) — the state machine, the per-download runtime
  handles (cancel `Notify` + task `JoinHandle`), queue/scheduler driving and
  the `BackendTask` construction.
* **Persistence** (`persistence`) — SQLite (the only persistence layer), with
  explicit, replayable migrations.
* **Event bus** (`events`) — a `tokio::sync::broadcast` channel of
  `Event`s, with no Tauri/GUI dependency.
* **Bandwidth policy** (`bandwidth`) — a global cap shared across active
  downloads, expressed as a per-download `TokenBucket` rate limiter.
* **Backend ownership** — the manager holds a map from `BackendKind` to an
  `Arc<dyn Backend>` and routes each download to the matching backend.

The manager **never uses a transport directly**: it only ever talks to
`Arc<dyn Backend>`, so it has no `reqwest` dependency and a future
BitTorrent backend slots in as a sibling without touching the manager.

### `odm-download-engine`

The HTTP backend. It implements `Backend` for `DownloadEngine` by
translating a protocol-neutral `BackendTask` into the engine's own
`DownloadRequest` + `DownloadOptions`, runs the existing streaming
orchestration unchanged, and maps the result back into a `BackendOutcome`.
Also honours an optional `RateLimiter` (throttling the byte stream) and a
cancellation `Notify`.

End-to-end it still:

1. Validates the URL scheme and the destination path.
2. Refuses to overwrite an existing file unless asked.
3. Ensures the parent directory exists.
4. Opens a streaming `GET` and reads `Content-Length` from the response.
5. Streams bytes to a unique `.<filename>.<uuid>.part` file.
6. Verifies the byte count against `Content-Length` before finalising.
7. `fsync`s and atomically renames on success.

### `odm-http-client`

Wraps `reqwest` (rustls, bundled *and* native root certificates, HTTP/2).
Provides `HttpClient`, `inspect` (HEAD) and `download_stream` (streaming
GET), with the same transport guarantees as before (no transparent
decoding, uniform error mapping).

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
`O_EXCL`, and are atomically renamed on success.

The storage writer is **async and sequential**: one file, one append offset.
That is the right shape for HTTP and the wrong shape for piece-based,
multi-file, random-access writes. The two are not meant to share an API.

### `odm-cli`

The `download-manager` binary. Uses `clap` for parsing and `indicatif` for
progress bars. Maps `odm_core::Error` variants to documented exit codes
(`Exit::from_error`, exhaustively tested).

## Lifecycle state machine

Defined in `odm-core` and persisted as its `Display` string. Invalid
transitions are rejected by `DownloadState::transition`.

| From            | To                                                         |
|-----------------|------------------------------------------------------------|
| `Queued`        | `Starting`, `Failed`, `Cancelled`                          |
| `Starting`      | `Downloading`, `Completed`, `Paused`, `Failed`, `Cancelled` |
| `Downloading`   | `Paused`, `Completed`, `Failed`, `Cancelled`               |
| `Paused`        | `Queued`, `Cancelled`                                      |
| `Completed`     | — (terminal)                                               |
| `Failed`        | `Queued` (retry), `Cancelled`                              |
| `Cancelled`     | — (terminal)                                               |

`Paused` is a deliberate, resumable stop: pausing cancels the underlying
transfer (the engine returns `Cancelled`), and the manager maps that
cancellation to `Paused` instead of `Cancelled` because the pause was
requested. Resuming re-queues the download and lets the scheduler run it
again.

## Concurrency model

The manager is eager rather than driven by a background loop: every
operation that can free or consume capacity (`enqueue`, `start`, `resume`,
`retry`, `remove`, and the completion of a running transfer) calls
`schedule()`, which starts queued downloads up to
`max_concurrent_downloads`. Because `begin_download` marks a download
`Starting` synchronously before spawning its task, the active count is
always exact and the limit is respected even before any bytes move.

## Persistence (SQLite)

SQLite is the only persistence layer. The schema is versioned through
explicit migrations recorded in a `schema_migrations` table, so upgrades are
reproducible and idempotent. The generic `downloads` table stores only
protocol-neutral columns:

* `id` (TEXT primary key, the stable `DownloadId`)
* `url`
* `destination`
* `backend` (`BackendKind` tag)
* `state` (`DownloadState` string)
* `overwrite`
* `total_bytes` (nullable)
* `downloaded_bytes`
* `error`
* `created_at`, `updated_at`, `started_at`, `completed_at` (unix millis)
* `backend_meta` (TEXT, an opaque JSON blob for protocol-specific data)

Protocol-specific detail (HTTP resume info, a torrent info hash, …) lives
**only** in `backend_meta`; no generic column is ever made HTTP- or
BitTorrent-specific. Adding a new protocol therefore needs no schema change.

### Crash / restart recovery

On `open`, the manager loads every persisted download and recovers
in-flight ones: anything found in `Starting` or `Downloading` (a transfer
that was interrupted when the process exited) is moved to `Failed` with a
clear error, or — if configured with `RecoveryPolicy::Queued` — back to
`Queued` so the scheduler can restart it. `Paused` and `Queued` downloads
survive untouched. Recovery deliberately does **not** fabricate an HTTP
`Range` resume: a crash is surfaced as a clear terminal/queued state, not a
fake continued transfer.

## Backend boundary

The manager owns lifecycle, queue, scheduler, persistence and events. A
backend (`odm-download-engine` for HTTP) owns protocol-specific execution.
The manager drives a backend exclusively through `Backend::run`, handing it
a `BackendTask` (url, destination, overwrite, opaque `backend_meta`,
progress sink, cancel `Notify`, optional `RateLimiter`) and receiving a
`BackendOutcome`. New protocols (BitTorrent via `librqbit`) attach as
sibling `Backend` implementations keyed by `BackendKind`.

## Events

The manager publishes `Event`s (queued, started, progress, paused, resumed,
completed, failed, cancelled, state-changed) on a broadcast channel. A
future GUI subscribes through `DownloadManager::subscribe()`; nothing in the
event path depends on Tauri.

## What is *not* implemented

Recorded here so that this document never describes aspirational behaviour as
shipped:

* **BitTorrent / `librqbit`** — no sibling backend exists yet.
* **HTTP `Range` resume** — pausing stops the transfer; resuming re-runs it
  from scratch. `backend_meta` is reserved for future resume metadata.
* **Mirrors**, **peer-to-peer**, **browser extension**, **GUI/Tauri** — none.
* `DownloadRequest::max_redirects` is still validated but not applied (the
  redirect cap is a client-wide setting).
* `Content-Disposition` filenames are still parsed but unused.
* `inspect()` is still not called by the HTTP download path.
* A stalled server can still hang a download (no idle/read watchdog yet).
* Orphaned `.part` files are still not cleaned up automatically.

## Future direction (locked decisions)

```text
Phase 2 (DONE): protocol-neutral download manager
   queue, scheduler, lifecycle, persistence, event bus, bandwidth policy

Phase 3: BitTorrent backend using librqbit

HTTP remains HTTP-specific.
BitTorrent remains BitTorrent-specific.
Shared concerns live above the protocol-specific layers.
```

Concretely, the target shape is:

```text
odm-core                    protocol-neutral vocabulary
   ^   models / events / state / backend boundary
odm-download-manager  (P2)  queue, scheduler, lifecycle, persistence,
   |                        event bus, bandwidth policy, backend ownership
   ├── odm-download-engine  HTTP backend  ->  odm-http-client
   ├── odm-torrent-backend  BitTorrent backend (P3)  ->  librqbit
   └── odm-storage          shared path validation, sanitization, download root
```

Decisions that are already locked and must not be re-litigated:

1. `odm-core` stays protocol-neutral. No new HTTP-specific types there.
2. BitTorrent is a **sibling backend**, never part of `odm-http-client`.
3. The HTTP sequential writer and the BitTorrent positioned multi-file
   writer are **not** unified. They share the validation and download-root
   layer only.
4. `DownloadProgress::at` is a `SystemTime`, not an `Instant`, so progress
   snapshots survive serialization and process boundaries.
5. OpenDownloadManager owns persistence (SQLite) and drives `librqbit` with
   its own persistence disabled, so there is exactly one source of truth for
   download state.

No premature abstraction: the `Backend` trait is the single boundary
actually needed to manage the existing HTTP engine, and no generic
trait was invented for a backend that does not yet exist.
