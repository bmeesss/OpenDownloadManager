//! Test helpers.
//!
//! `wiremock` produces well-formed responses, which is exactly why some of
//! the cases that matter most for a download manager cannot be tested with
//! it: a body shorter than the `Content-Length` that announced it, or a
//! response carrying no length information at all. Those tests drive a raw
//! TCP listener instead, so the bytes on the wire are specified exactly.

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves `response` verbatim to the next client that connects, then
/// closes the connection.
///
/// Returns the address the server is listening on. The request head is read
/// and discarded first so the client has finished sending before the
/// response is written.
pub async fn serve_once(response: Vec<u8>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0_u8; 2048];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(&response).await;
            let _ = socket.flush().await;
            let _ = socket.shutdown().await;
        }
    });
    addr
}

/// Returns a port on `127.0.0.1` that nothing is listening on.
///
/// Connecting to it produces an immediate, guaranteed connection refusal
/// without depending on an externally unreachable address, which is both
/// slower and dependent on the network the test happens to run on.
pub async fn refused_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}
