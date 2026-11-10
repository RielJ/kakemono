//! `Client::request_with_timeout` (`docs/plans/picker-browse-impl.md` Task 2): a caller-chosen
//! round-trip deadline, used by the UI for the slow `browse_use` download.

use std::time::Duration;

use kakemono_protocol::client::{Client, ClientError};
use kakemono_protocol::endpoint::Endpoint;
use kakemono_protocol::types::Request;
use tokio::io::{AsyncBufReadExt, BufReader};

/// The server reads the request and never answers. The client gives up after its own short
/// timeout, well before `REQUEST_TIMEOUT` (30 s); the outer 5 s bound keeps a broken
/// implementation from hanging the suite.
#[tokio::test]
async fn request_with_timeout_gives_timeout_when_the_server_is_silent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let endpoint = Endpoint::new(dir.path().join("run/k.sock")).expect("endpoint");
    let listener = endpoint.bind().expect("bind");
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        use interprocess::local_socket::tokio::prelude::*;
        let conn = listener.accept().await.expect("accept");
        let mut reader = BufReader::new(conn);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read");
        // Hold the connection open, silent, until the client has given up.
        let _ = done_rx.await;
        line
    });

    let mut client = Client::connect(&endpoint).await.expect("connect");
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        client.request_with_timeout(Request::Status, Duration::from_millis(50)),
    )
    .await
    .expect("request_with_timeout must return on its own deadline");
    let elapsed = started.elapsed();

    assert!(
        matches!(result, Err(ClientError::Timeout)),
        "expected Timeout, got {result:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(50) && elapsed < Duration::from_secs(2),
        "timed out after {elapsed:?}"
    );
    done_tx.send(()).expect("server waiting");
    assert_eq!(server.await.expect("server"), "{\"cmd\":\"status\"}\n");
}
