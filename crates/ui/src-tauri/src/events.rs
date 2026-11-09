//! Live updates: one background task holds an IPC `subscribe` stream and turns daemon events
//! into Tauri events, the tray label and the tooltip.
//!
//! The subscription has its own [`Client`]. It never borrows the shared
//! [`DaemonLink`](crate::daemon_link::DaemonLink): a subscribed connection only streams events,
//! so a request sent on it would never get an answer.
//!
//! When the daemon is not running or goes away, the task retries with [`Backoff`] (0.5 s
//! doubling to 10 s, reset after each successful subscribe). It logs a state change once,
//! not every retry, and emits [`CONNECTION`] only when the state changes, except that every
//! successful (re)subscribe emits `connected` with a fresh `status`, because events sent
//! while disconnected are lost.
//!
//! Frontend events (payloads are JSON):
//!
//! | name | payload |
//! |---|---|
//! | [`CHANGED`] `wallpaper://changed` | `WallpaperInfo` |
//! | [`PAUSED`] `wallpaper://paused` | `boolean` |
//! | [`CONNECTION`] `daemon://connection` | [`Connection`] |
//!
//! A window that opens later reads the last [`Connection`] with the `daemon_connection` command.

use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt;
use kakemono_protocol::client::{Client, ClientError};
use kakemono_protocol::endpoint::Endpoint;
use kakemono_protocol::types::{Event, Request, StatusData};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::tray;

pub const CHANGED: &str = "wallpaper://changed";
pub const PAUSED: &str = "wallpaper://paused";
pub const CONNECTION: &str = "daemon://connection";

/// The subscription's view of the daemon, as sent on [`CONNECTION`].
///
/// JSON: `{"state":"connecting"}`, `{"state":"connected","status":StatusData|null}`,
/// `{"state":"not_running"}`, `{"state":"error","message":"..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Connection {
    /// Before the first attempt finishes.
    Connecting,
    /// Subscribed. `status` was read right after subscribing (`None` if that failed).
    Connected { status: Option<Box<StatusData>> },
    /// Nothing listens on the endpoint.
    NotRunning,
    /// Anything else: permission denied, a timeout, a daemon that rejects `subscribe`.
    Error { message: String },
}

impl Connection {
    fn from_client_error(err: &ClientError) -> Self {
        match err {
            ClientError::DaemonNotRunning { .. } => Self::NotRunning,
            other => Self::Error {
                message: other.to_string(),
            },
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected { .. } => "connected",
            Self::NotRunning => "not_running",
            Self::Error { .. } => "error",
        }
    }
}

/// Reconnect delays: `min`, doubling, capped at `max`.
#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub const MIN: Duration = Duration::from_millis(500);
    pub const MAX: Duration = Duration::from_secs(10);

    pub fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            next: min,
        }
    }

    /// The delay before the next attempt; each call doubles the following one.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (delay * 2).min(self.max);
        delay
    }

    pub fn reset(&mut self) {
        self.next = self.min;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Self::MIN, Self::MAX)
    }
}

/// Where the loop delivers what it sees. The Tauri implementation is [`TauriSink`]; tests
/// record.
pub trait Sink: Send + Sync + 'static {
    fn connection(&self, state: &Connection);
    fn event(&self, event: &Event);
}

/// Subscribes forever. Never returns; abort the task to stop it.
pub async fn run<S: Sink>(endpoint: Endpoint, sink: S, mut backoff: Backoff) {
    let mut last: Option<Connection> = None;
    loop {
        match subscribe(&endpoint).await {
            Ok(mut stream) => {
                backoff.reset();
                let status = fetch_status(&endpoint).await;
                publish(&sink, &mut last, Connection::Connected { status });
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(event) => sink.event(&event),
                        Err(err) => {
                            tracing::debug!(error = %err, "daemon event stream failed");
                            break;
                        }
                    }
                }
                tracing::info!("daemon event stream ended; reconnecting");
            }
            Err(err) => publish(&sink, &mut last, Connection::from_client_error(&err)),
        }
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

async fn subscribe(
    endpoint: &Endpoint,
) -> Result<impl futures::Stream<Item = Result<Event, ClientError>> + Unpin, ClientError> {
    Client::connect(endpoint).await?.subscribe().await
}

/// One `status` on a short-lived connection, after subscribing, so no change between the
/// two is missed.
async fn fetch_status(endpoint: &Endpoint) -> Option<Box<StatusData>> {
    let result = async {
        let mut client = Client::connect(endpoint).await.ok()?;
        let response = client.request(Request::Status).await.ok()?;
        if !response.ok {
            return None;
        }
        response
            .data_as::<StatusData>()
            .ok()
            .flatten()
            .map(Box::new)
    }
    .await;
    if result.is_none() {
        tracing::debug!("status after subscribe failed; the frontend will ask again");
    }
    result
}

/// Emits `next` unless it equals the last emitted disconnected state, and logs only when the
/// kind of state changes. `Connected` always emits: it carries a fresh status.
fn publish<S: Sink>(sink: &S, last: &mut Option<Connection>, next: Connection) {
    let connected = matches!(next, Connection::Connected { .. });
    if !connected && last.as_ref() == Some(&next) {
        return;
    }
    if last.as_ref().map(Connection::kind) != Some(next.kind()) {
        match &next {
            Connection::Connected { .. } => tracing::info!("subscribed to daemon events"),
            Connection::NotRunning => {
                tracing::info!("daemon not running; will reconnect when it starts")
            }
            Connection::Error { message } => {
                tracing::warn!(error = %message, "cannot subscribe to daemon events; retrying")
            }
            Connection::Connecting => {}
        }
    }
    sink.connection(&next);
    *last = Some(next);
}

/// The last [`Connection`], for windows that open after it was emitted.
pub struct LiveState(Mutex<Connection>);

impl Default for LiveState {
    fn default() -> Self {
        Self(Mutex::new(Connection::Connecting))
    }
}

impl LiveState {
    pub fn get(&self) -> Connection {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, state: Connection) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = state;
    }
}

/// The last connection state; `connecting` until the first attempt finishes.
#[tauri::command]
pub fn daemon_connection(live: tauri::State<'_, LiveState>) -> Connection {
    live.get()
}

/// Forwards to the webviews, the tray and [`LiveState`].
pub struct TauriSink<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> Sink for TauriSink<R> {
    fn connection(&self, state: &Connection) {
        if let Some(live) = self.0.try_state::<LiveState>() {
            live.set(state.clone());
        }
        match state {
            Connection::Connected { status } => {
                if let Some(status) = status {
                    tray::set_pause_label(&self.0, status.paused);
                }
                tray::set_problem(&self.0, None);
            }
            Connection::NotRunning => tray::set_problem(&self.0, Some("daemon not running")),
            Connection::Error { message } => tray::set_problem(&self.0, Some(message)),
            Connection::Connecting => {}
        }
        emit(&self.0, CONNECTION, state);
    }

    fn event(&self, event: &Event) {
        match event {
            Event::Changed(info) => emit(&self.0, CHANGED, info),
            Event::Paused(paused) => {
                tray::set_pause_label(&self.0, *paused);
                emit(&self.0, PAUSED, paused);
            }
            // Windows follow the system theme themselves; nothing shows the daemon's.
            Event::ThemeChanged(theme) => tracing::debug!(%theme, "daemon theme changed"),
            Event::Unknown => tracing::debug!("ignoring an unknown daemon event"),
        }
    }
}

fn emit<R: Runtime, T: Serialize + Clone>(app: &AppHandle<R>, name: &str, payload: T) {
    if let Err(err) = app.emit(name, payload) {
        tracing::debug!(event = name, error = %err, "cannot emit event");
    }
}

/// Starts the subscription task for the app's lifetime.
pub fn spawn<R: Runtime>(app: &AppHandle<R>, endpoint: Endpoint) {
    let sink = TauriSink(app.clone());
    tauri::async_runtime::spawn(run(endpoint, sink, Backoff::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_the_cap_and_resets() {
        let mut b = Backoff::default();
        let secs: Vec<f64> = (0..7).map(|_| b.next_delay().as_secs_f64()).collect();
        assert_eq!(secs, [0.5, 1.0, 2.0, 4.0, 8.0, 10.0, 10.0]);
        b.reset();
        assert_eq!(b.next_delay(), Backoff::MIN);
    }

    #[test]
    fn connection_json_matches_the_frontend_type() {
        let json = |c: &Connection| serde_json::to_string(c).expect("json");
        assert_eq!(json(&Connection::Connecting), r#"{"state":"connecting"}"#);
        assert_eq!(json(&Connection::NotRunning), r#"{"state":"not_running"}"#);
        assert_eq!(
            json(&Connection::Connected { status: None }),
            r#"{"state":"connected","status":null}"#
        );
        assert_eq!(
            json(&Connection::Error {
                message: "boom".into()
            }),
            r#"{"state":"error","message":"boom"}"#
        );
    }

    #[derive(Default)]
    struct Count(Mutex<Vec<Connection>>);
    impl Sink for Count {
        fn connection(&self, state: &Connection) {
            self.0.lock().unwrap().push(state.clone());
        }
        fn event(&self, _: &Event) {}
    }

    #[test]
    fn repeated_disconnected_states_emit_once_but_connected_always_emits() {
        let sink = Count::default();
        let mut last = None;
        publish(&sink, &mut last, Connection::NotRunning);
        publish(&sink, &mut last, Connection::NotRunning);
        publish(&sink, &mut last, Connection::NotRunning);
        publish(&sink, &mut last, Connection::Connected { status: None });
        publish(&sink, &mut last, Connection::Connected { status: None });
        let err = || Connection::Error {
            message: "denied".into(),
        };
        publish(&sink, &mut last, err());
        publish(&sink, &mut last, err());
        let seen: Vec<_> = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .map(Connection::kind)
            .collect();
        assert_eq!(
            seen,
            ["not_running", "connected", "connected", "error"],
            "one entry per change; every (re)subscribe"
        );
    }
}

#[cfg(all(test, unix))]
mod socket_tests {
    use std::sync::Arc;

    use interprocess::local_socket::tokio::prelude::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::mpsc;

    use super::*;

    #[derive(Debug, PartialEq)]
    enum Seen {
        Conn(Connection),
        Ev(Event),
    }

    struct Recorder(mpsc::UnboundedSender<Seen>);
    impl Sink for Recorder {
        fn connection(&self, state: &Connection) {
            let _ = self.0.send(Seen::Conn(state.clone()));
        }
        fn event(&self, event: &Event) {
            let _ = self.0.send(Seen::Ev(event.clone()));
        }
    }

    const STATUS: &str = concat!(
        "{\"ok\":true,\"data\":{\"current\":null,\"paused\":false,\"interval\":\"30m\",",
        "\"theme\":\"dark\",\"history_count\":0,\"history_index\":-1}}\n"
    );

    fn status() -> StatusData {
        serde_json::from_str::<serde_json::Value>(STATUS)
            .map(|v| serde_json::from_value(v["data"].clone()).expect("status"))
            .expect("json")
    }

    /// A fake daemon that serves `subscribe` connections (ack, then `events`, then hang up)
    /// and `status` ones, until `connections` have been handled. Then it drops the listener
    /// and removes the socket file, as a stopping daemon does.
    fn daemon(
        endpoint: &Endpoint,
        subscribe_reply: &'static str,
        events: Vec<&'static str>,
        connections: usize,
    ) -> tokio::task::JoinHandle<()> {
        let listener = endpoint.bind().expect("bind");
        let path = endpoint.path().to_owned();
        let events = Arc::new(events);
        tokio::spawn(async move {
            for _ in 0..connections {
                let conn = listener.accept().await.expect("accept");
                let mut reader = BufReader::new(conn);
                let mut line = String::new();
                reader.read_line(&mut line).await.expect("read");
                let out = reader.get_mut();
                if line.contains("\"subscribe\"") {
                    out.write_all(subscribe_reply.as_bytes())
                        .await
                        .expect("ack");
                    for event in events.iter() {
                        out.write_all(event.as_bytes()).await.expect("event");
                    }
                } else {
                    out.write_all(STATUS.as_bytes()).await.expect("status");
                }
            }
            drop(listener);
            let _ = std::fs::remove_file(path);
        })
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<Seen>) -> Seen {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for the sink")
            .expect("sink closed")
    }

    fn fast() -> Backoff {
        Backoff::new(Duration::from_millis(50), Duration::from_millis(100))
    }

    #[tokio::test]
    async fn forwards_events_reports_not_running_once_and_recovers_after_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoint = Endpoint::new(dir.path().join("run/k.sock")).expect("endpoint");
        let first = daemon(
            &endpoint,
            "{\"ok\":true}\n",
            vec![
                "{\"event\":\"changed\",\"data\":{\"path\":\"/w/a.jpg\",\"source\":\"local\"}}\n",
                "{\"event\":\"paused\",\"data\":true}\n",
                "{\"event\":\"from_the_future\",\"data\":1}\n",
            ],
            2,
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(endpoint.clone(), Recorder(tx), fast()));

        assert_eq!(
            next(&mut rx).await,
            Seen::Conn(Connection::Connected {
                status: Some(Box::new(status()))
            })
        );
        match next(&mut rx).await {
            Seen::Ev(Event::Changed(info)) => assert_eq!(info.path, "/w/a.jpg"),
            other => panic!("expected changed, got {other:?}"),
        }
        assert_eq!(next(&mut rx).await, Seen::Ev(Event::Paused(true)));
        assert_eq!(next(&mut rx).await, Seen::Ev(Event::Unknown));
        first.await.expect("first daemon");

        // The daemon is gone: one not_running, however many retries happen.
        assert_eq!(next(&mut rx).await, Seen::Conn(Connection::NotRunning));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            rx.try_recv().is_err(),
            "retries must not re-emit not_running"
        );

        // A new daemon on the same socket: the loop subscribes again on its own.
        let second = daemon(
            &endpoint,
            "{\"ok\":true}\n",
            vec!["{\"event\":\"paused\",\"data\":false}\n"],
            2,
        );
        assert!(matches!(
            next(&mut rx).await,
            Seen::Conn(Connection::Connected { status: Some(_) })
        ));
        assert_eq!(next(&mut rx).await, Seen::Ev(Event::Paused(false)));
        second.await.expect("second daemon");
        task.abort();
    }

    #[tokio::test]
    async fn a_rejected_subscribe_is_an_error_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoint = Endpoint::new(dir.path().join("run/k.sock")).expect("endpoint");
        let server = daemon(
            &endpoint,
            "{\"ok\":false,\"error\":\"unknown command: \\\"subscribe\\\"\"}\n",
            vec![],
            1,
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(endpoint, Recorder(tx), fast()));

        match next(&mut rx).await {
            Seen::Conn(Connection::Error { message }) => {
                assert!(message.contains("subscribe rejected"), "{message}")
            }
            other => panic!("expected an error state, got {other:?}"),
        }
        server.await.expect("server");
        task.abort();
    }
}
