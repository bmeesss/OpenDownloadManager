# OpenDownloadManager

A free and open-source desktop download manager written in Rust.

> **Status:** Phase 2 — protocol-neutral download manager.
> A queue, scheduler, lifecycle state machine, SQLite persistence, event bus
> and bandwidth policy manage one or more downloads across protocol-neutral
> backends. The HTTP backend (`odm-download-engine`) is wired in. No GUI, no
> browser extension, no BitTorrent, and no HTTP `Range` resume yet; see
> [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Workspace layout

```
apps/                         - composition roots (Tauri, future)
binaries/cli/                 - the `download-manager` binary
crates/core/                  - domain models and error types
crates/http-client/           - reqwest wrapper (HEAD + streaming GET)
crates/storage/               - filesystem abstraction + path validation
crates/download-engine/       - HTTP backend (implements the `Backend` trait)
crates/download-manager/      - Phase 2: queue, scheduler, lifecycle,
                               persistence, event bus, bandwidth policy
tests/                        - integration tests (shared library)
docs/                         - design and architecture documentation
```

## Dependency direction

```
binaries/cli  ->  download-manager  ->  download-engine  ->  http-client  ->  (reqwest)
                          |                   |                    |
                          v                   v                    v
                       storage  ->---------> core <------------- storage
```

The `core` crate has no internal dependencies. There are no cycles. The
`download-manager` owns the queue, scheduler, lifecycle, persistence and event
bus and drives `download-engine` (and, later, a BitTorrent backend) only
through the protocol-neutral `Backend` trait — it never depends on `reqwest`
directly.

## Build

```sh
cargo build --release
```

The release binary lives at `target/release/download-manager`.

The toolchain is pinned by `rust-toolchain.toml` (1.88.0), which matches
the declared MSRV. 1.88 is the floor imposed by the resolved dependency
graph — the `icu_*` crates that `url` → `idna` pull in require it — not
by this project's own code.

## Run

```sh
# Print version
download-manager version

# Download to the current directory, naming the file after the URL
download-manager download https://example.com/file.zip

# Download to an explicit destination (relative or absolute)
download-manager download https://example.com/file.zip \
  --output ~/Downloads/file.zip --verbose

# Replace an existing file
download-manager download https://example.com/file.zip \
  --output ./file.zip --overwrite
```

### Exit codes

| Code | Meaning                              |
|------|--------------------------------------|
| 0    | Success                              |
| 2    | Invalid argument (bad URL/path)      |
| 3    | Network error                        |
| 4    | HTTP error (4xx/5xx)                 |
| 5    | Filesystem error                     |
| 6    | Cancelled                            |
| 7    | Internal error                      |

## Test

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

CI runs these on Ubuntu, macOS and Windows — see
[.github/workflows/ci.yml](.github/workflows/ci.yml).

## Security notes

What is actually true of the code as it stands:

* **TLS.** Certificate validation is always on. Root certificates come
  from two sources: the bundled webpki set (`rustls-tls`) and the
  operating system's trust store (`rustls-tls-native-roots`). Nothing in
  this repository ever calls `danger_accept_invalid_certs`.
* **Content encoding.** `reqwest` is built without its `gzip`, `brotli`
  and `deflate` features, so the client never advertises those encodings
  and never transparently decodes a body. The bytes written to disk are
  exactly the bytes the server sent, and `Content-Length` always
  describes the file rather than a compressed payload of unknown expanded
  size.
* **Completeness.** If the server announced a `Content-Length`, the
  transfer is checked against it before anything is finalized. A short or
  overlong transfer is an error; the `.part` file is retained and the
  final file is never created.
* **Path validation.** Two validators, deliberately distinct:
  * untrusted relative paths (names derived from a URL) must not contain
    `..`, absolute components, NUL bytes or control characters;
  * the destination the *user* chose may be absolute, but `..`, NUL
    bytes, control characters, invalid final components and Windows
    reserved device names are still rejected — on every platform, so a
    download set stays portable.
* **Temporary files.** Downloads stream to a unique `.<name>.<uuid>.part`
  file created with `O_EXCL`, and are atomically renamed on success. If
  finalization fails, the `.part` file is retained for inspection.
* **Logging.** Cookies, `Authorization` headers and credentials are never
  logged. Note that full URLs — including any query string and userinfo —
  currently *are* logged; redacting those is scheduled for Phase 1.7.

## Known limitations

Stated plainly so nothing here is mistaken for a finished feature:

* **The CLI does not yet use the manager.** The `download-manager` binary still
  drives the engine directly for a single download; wiring it to the
  `odm-download-manager` queue/persistence is a later step. The manager is
  fully exercised by its own tests and integration tests.
* **No real resume.** Pausing stops the underlying transfer and resuming
  re-runs it from scratch; there is no HTTP `Range` support yet.
  `backend_meta` is reserved for future resume metadata.
* **No redirect-limit plumbing per request.** `DownloadRequest::max_redirects`
  is validated but not applied; the redirect cap is a client-wide setting
  (10 hops by default).
* **No HEAD inspection in the download path.** `inspect()` exists and is
  tested, but the engine does not call it. Size comes from the `GET`
  response headers, and `Content-Disposition` filenames are parsed but
  not used to name the output.
* **No BitTorrent, mirrors, checksums, proxy support, or browser extension.**
  `reqwest` is built without its `system-proxy` feature, so
  `HTTP_PROXY`/`HTTPS_PROXY` are ignored.
* **Orphaned `.part` files are never cleaned up.**
* **A stalled server can hang a download indefinitely.** There is a 15 s
  connect timeout but no idle/read watchdog yet.
