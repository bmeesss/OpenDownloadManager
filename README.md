# OpenDownloadManager

A free and open-source desktop download manager written in Rust.

> **Status:** Phase 1 — CLI + core download engine. No GUI, no browser
> extensions, no SQLite, no archive extraction. These are scheduled for
> later phases.

## Workspace layout

```
apps/                         - composition roots (Tauri, future)
binaries/cli/                 - the `download-manager` binary
crates/core/                  - domain models and error types
crates/http-client/           - reqwest wrapper (HEAD + streaming GET)
crates/storage/               - filesystem abstraction + path validation
crates/download-engine/       - orchestration
tests/                        - integration tests (shared library)
docs/                         - design and architecture documentation
```

## Dependency direction

```
binaries/cli  ->  download-engine  ->  http-client  ->  (reqwest)
                          |                |
                          v                v
                       storage  ->------- core
```

The `core` crate has no internal dependencies. There are no cycles.

## Build

```sh
cargo build --release
```

The release binary lives at `target/release/download-manager`.

## Run

```sh
# Print version
download-manager version

# Download a file
download-manager download https://example.com/file.zip \
  --output ./file.zip --verbose
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
| 7    | Internal error                       |

## Test

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

## Security notes

* TLS certificate validation is **enabled** (rustls native roots + built-in roots).
* Cookies, Authorization headers, and credentials are **never logged**.
* Path validation rejects `..`, absolute paths, NUL bytes, control characters,
  and Windows-reserved device names.
* Downloads are streamed to a unique `.part` file and atomically renamed on
  success. If finalization fails, the `.part` file is retained for inspection.
