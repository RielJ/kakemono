//! Client tests against an in-test interprocess listener that replays canned Go responses
//!.

use std::path::Path;

use futures::StreamExt;
use interprocess::local_socket::tokio::prelude::*;
use kakemono_protocol::client::{self, Client, ClientError};
use kakemono_protocol::endpoint::{Endpoint, test_socket_path};
use kakemono_protocol::types::{Command, Event, Request, Response, StatusData};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

fn wire(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wire")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

struct Server {
    _dir: tempfile::TempDir,
    endpoint: Endpoint,
}

fn server() -> (Server, LocalSocketListener) {
    let dir = tempfile::tempdir().expect("tempdir");
    let endpoint = Endpoint::new(test_socket_path(dir.path(), "k.sock")).expect("endpoint");
    let listener = endpoint.bind().expect("bind");
    (
        Server {
            _dir: dir,
            endpoint,
        },
        listener,
    )
}

/// Accepts one connection; for each canned reply, reads one request line (recorded with its
/// `\n`) and writes the reply verbatim. Then writes `trailer` and closes. Returns the bytes
/// received.
fn serve_one(
    listener: LocalSocketListener,
    replies: Vec<String>,
    trailer: String,
) -> JoinHandle<Vec<String>> {
    tokio::spawn(async move {
        let conn = listener.accept().await.expect("accept");
        let mut reader = BufReader::new(conn);
        let mut got = Vec::new();
        for reply in replies {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read");
            got.push(line);
            reader
                .get_mut()
                .write_all(reply.as_bytes())
                .await
                .expect("write");
        }
        let send = reader.get_mut();
        send.write_all(trailer.as_bytes())
            .await
            .expect("write trailer");
        send.flush().await.expect("flush");
        got
    })
}

#[tokio::test]
async fn multi_command_session_matches_go_bytes() {
    let requests = wire("22_multi_command_session.request.jsonl");
    let responses = wire("22_multi_command_session.response.jsonl");
    // Skip the blank and padded lines: the client never sends them.
    let pairs: Vec<(&str, &str)> = requests
        .lines()
        .zip(responses.lines())
        .filter(|(req, _)| req.starts_with('{'))
        .collect();
    assert_eq!(pairs.len(), 4, "pause, status, bogus, status");

    let (srv, listener) = server();
    let task = serve_one(
        listener,
        pairs.iter().map(|(_, resp)| format!("{resp}\n")).collect(),
        String::new(),
    );

    let mut client = Client::connect(&srv.endpoint).await.expect("connect");
    let mut got = Vec::new();
    for (req, _) in &pairs {
        let command: Command = serde_json::from_str(req).expect("fixture command");
        got.push(client.request(command).await.expect("request"));
    }

    let sent = task.await.expect("server");
    let want_sent: Vec<String> = pairs.iter().map(|(req, _)| format!("{req}\n")).collect();
    assert_eq!(sent, want_sent, "client bytes equal Go's request lines");

    for ((_, resp), response) in pairs.iter().zip(&got) {
        let want: Response = serde_json::from_str(resp).expect("fixture response");
        assert_eq!(response, &want);
    }
    assert_eq!(got[2].error, "unknown command: \"bogus\"");
    let status: StatusData = got[3].data_as().expect("decode").expect("data");
    assert!(status.current.is_some());
}

#[tokio::test]
async fn typed_set_request_matches_go_bytes() {
    let req = wire("06_set.request.jsonl");
    let resp = wire("06_set.response.jsonl");
    let path = serde_json::from_str::<serde_json::Value>(req.trim_end())
        .expect("json")
        .pointer("/args/path")
        .and_then(|p| p.as_str())
        .expect("path")
        .to_owned();

    let (srv, listener) = server();
    let task = serve_one(listener, vec![resp.clone()], String::new());
    let mut client = Client::connect(&srv.endpoint).await.expect("connect");
    let response = client
        .request(Request::Set { path })
        .await
        .expect("request");
    assert_eq!(task.await.expect("server"), vec![req]);
    assert_eq!(
        response,
        serde_json::from_str::<Response>(&resp).expect("resp")
    );
}

#[tokio::test]
async fn closed_before_response_is_an_error() {
    let (srv, listener) = server();
    let task = tokio::spawn(async move {
        let conn = listener.accept().await.expect("accept");
        let mut reader = BufReader::new(conn);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read");
    });
    let mut client = Client::connect(&srv.endpoint).await.expect("connect");
    let err = client.request(Request::Status).await.expect_err("closed");
    assert!(matches!(err, ClientError::Closed), "{err:?}");
    task.await.expect("server");
}

#[tokio::test]
async fn subscribe_streams_events_until_close() {
    let (srv, listener) = server();
    let trailer = concat!(
        r#"{"event":"changed","data":{"path":"/p.jpg","source":"bing","source_url":"","image_url":"","title":"t","author":"","width":1,"height":2,"set_at":""}}"#,
        "\n",
        r#"{"event":"paused","data":true}"#,
        "\n",
        r#"{"event":"theme_changed","data":"dark"}"#,
        "\n",
        r#"{"event":"from_the_future","data":{}}"#,
        "\n",
    );
    let task = serve_one(listener, vec!["{\"ok\":true}\n".into()], trailer.into());

    let client = Client::connect(&srv.endpoint).await.expect("connect");
    let events: Vec<Event> = client
        .subscribe()
        .await
        .expect("subscribe")
        .map(|e| e.expect("event"))
        .collect()
        .await;

    assert_eq!(
        task.await.expect("server"),
        vec!["{\"cmd\":\"subscribe\"}\n"]
    );
    assert_eq!(events.len(), 4);
    match &events[0] {
        Event::Changed(info) => {
            assert_eq!(
                (info.path.as_str(), info.width, info.height),
                ("/p.jpg", 1, 2)
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(events[1], Event::Paused(true));
    assert_eq!(events[2], Event::ThemeChanged("dark".into()));
    assert_eq!(events[3], Event::Unknown);
}

#[tokio::test]
async fn subscribe_rejected_by_old_daemon() {
    let (srv, listener) = server();
    let task = serve_one(
        listener,
        vec!["{\"ok\":false,\"error\":\"unknown command: \\\"subscribe\\\"\"}\n".into()],
        String::new(),
    );
    let client = Client::connect(&srv.endpoint).await.expect("connect");
    let err = client.subscribe().await.err().expect("rejected");
    task.await.expect("server");
    assert!(
        matches!(&err, ClientError::SubscribeRejected(m) if m == "unknown command: \"subscribe\""),
        "{err:?}"
    );
}

#[tokio::test]
async fn subscribe_stream_ends_after_bad_line() {
    let (srv, listener) = server();
    let task = serve_one(
        listener,
        vec!["{\"ok\":true}\n".into()],
        "not json\n{\"event\":\"paused\",\"data\":false}\n".into(),
    );
    let client = Client::connect(&srv.endpoint).await.expect("connect");
    let items: Vec<_> = client.subscribe().await.expect("subscribe").collect().await;
    task.await.expect("server");
    assert_eq!(items.len(), 1);
    assert!(
        matches!(items[0], Err(ClientError::Receive(_))),
        "{:?}",
        items[0]
    );
}

const HINT_PREFIX: &str =
    "kakemono daemon is not running.\nStart it with:  kakemono daemon\n(original error: ";

#[tokio::test]
async fn missing_socket_is_daemon_not_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = test_socket_path(dir.path(), "absent.sock");
    let endpoint = Endpoint::new(&path).expect("endpoint");
    let err = Client::connect(&endpoint).await.expect_err("no daemon");
    let msg = err.to_string();
    assert!(
        matches!(&err, ClientError::DaemonNotRunning { endpoint, .. } if endpoint == &path),
        "{err:?}"
    );
    assert!(msg.starts_with(HINT_PREFIX), "{msg}");
    // Quoted like Go's %q, so a Windows pipe path shows its `\` escaped.
    assert!(msg.contains(&format!("{:?}", path.display())), "{msg}");
    assert!(!client::is_running(&endpoint).await);
}

#[cfg(unix)]
#[tokio::test]
async fn stale_socket_file_is_daemon_not_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("stale.sock");
    drop(std::os::unix::net::UnixListener::bind(&path).expect("bind"));
    assert!(path.exists(), "socket file left behind");
    let endpoint = Endpoint::new(&path).expect("endpoint");
    let err = Client::connect(&endpoint).await.expect_err("refused");
    assert!(
        matches!(err, ClientError::DaemonNotRunning { .. }),
        "{err:?}"
    );
    assert!(err.to_string().starts_with(HINT_PREFIX));
}

#[cfg(unix)]
#[tokio::test]
async fn bind_creates_dir_0700_and_socket_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    // `run/` does not exist yet: `bind` creates it.
    let endpoint = Endpoint::new(dir.path().join("run/k.sock")).expect("endpoint");
    let _listener = endpoint.bind().expect("bind");
    let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode(endpoint.path()), 0o600);
    assert_eq!(mode(&dir.path().join("run")), 0o700);
    assert!(client::is_running(&endpoint).await);
}

/// A request abandoned mid-round-trip (timed out or cancelled) leaves its reply in flight. The
/// client must not hand that late reply to the next request: it is poisoned and must reconnect.
#[tokio::test]
async fn abandoned_request_poisons_the_client() {
    let (srv, listener) = server();
    let (late_tx, late_rx) = tokio::sync::oneshot::channel::<()>();
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let conn = listener.accept().await.expect("accept");
        let mut reader = BufReader::new(conn);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read");
        // Answer only after the client gave up on this request.
        late_rx.await.expect("signal");
        reader
            .get_mut()
            .write_all(b"{\"ok\":true,\"data\":{\"late\":true}}\n")
            .await
            .expect("write");
        sent_tx.send(()).expect("test waiting");
        // Keep the connection open for a second request.
        let mut next = String::new();
        let _ = reader.read_line(&mut next).await;
    });
    let mut client = Client::connect(&srv.endpoint).await.expect("connect");
    let abandoned = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        client.request(Request::Status),
    )
    .await;
    assert!(abandoned.is_err(), "the server never answered in time");
    late_tx.send(()).expect("server waiting");
    sent_rx.await.expect("late reply written");

    let err = client
        .request(Request::Next)
        .await
        .expect_err("a poisoned client must not read the late reply");
    assert!(matches!(err, ClientError::Poisoned), "{err:?}");
    drop(client);
    task.await.expect("server");
}
