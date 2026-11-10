//! Tauri commands for the Sources table and for images dropped on a window.
//!
//! - `check_source { kind, location }` → [`CheckSourceData`]: starts the daemon when it doesn't
//!   answer, then sends IPC `check_source`.
//! - `download_now { id }` → [`DownloadNowData`]: IPC `download_now`.
//! - `delete_source_files { id }`: deletes `download_dir/<id>`, and nothing else.
//! - `open_source_folder { id }`: opens a source's folder in the file manager.
//! - `fetch_dropped_url { url }` → [`WallpaperInfo`]: IPC `fetch`.
//! - `import_dropped_file` (raw body, name in the `x-file-name` header) → [`WallpaperInfo`]:
//!   writes the bytes to a new file in `manual_dir`, then sends IPC `fetch` with its path.
//!
//! Errors reach the frontend as `{ kind, message }`, like [`CommandError`]. When the daemon
//! can't be reached (it isn't running and can't be started, or the connection fails or times
//! out) the kind is `daemon_unreachable` ([`SourceError::Unreachable`]).
//!
//! The file-system helpers are plain functions so tests run them against temp dirs.
//!
//! - `folders`: the folder a source owns, and deleting or opening it.
//! - `drop`: writing a dropped file into `manual_dir`.

use crate::commands::CommandError;
use crate::commands::{blocking, load_config};
use crate::daemon_link::{DaemonLink, LinkError};
use crate::error::{ErrorKind, Kinded, serialize_as_wire_error};
use crate::lifecycle::{self, CliRunner, LifecycleError, ProcessRunner};
use kakemono_config::ConfigError;
use kakemono_protocol::client::ClientError;
use kakemono_protocol::types::{CheckSourceData, DownloadNowData, Request, WallpaperInfo};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

mod drop;
mod folders;

pub use drop::*;
pub use folders::*;

/// The largest image `import_dropped_file` accepts, as the daemon's `fetch`.
pub const MAX_DROP_BYTES: usize = 64 * 1024 * 1024;

/// The header carrying the dropped file's name, percent-encoded (`encodeURIComponent`).
pub const FILE_NAME_HEADER: &str = "x-file-name";

/// A failed source command, serialised as `{ "kind": "...", "message": "..." }`.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The daemon isn't running and couldn't be started, or the connection failed or timed
    /// out. Kind `daemon_unreachable`; the UI offers "Add anyway" only for this one.
    #[error("cannot reach the kakemono daemon: {0}")]
    Unreachable(String),
    /// The daemon did not answer in time; it may still finish the request. Kind
    /// `daemon_timeout`.
    #[error("the kakemono daemon did not answer in time (it may still finish): {0}")]
    Timeout(String),
    #[error(transparent)]
    Command(#[from] CommandError),
}

impl Kinded for SourceError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Unreachable(_) => ErrorKind::DaemonUnreachable,
            Self::Timeout(_) => ErrorKind::DaemonTimeout,
            Self::Command(err) => err.kind(),
        }
    }
}

impl SourceError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Command(CommandError::Invalid(message.into()))
    }

    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Command(CommandError::Io {
            context: context.into(),
            source,
        })
    }
}

impl From<LinkError> for SourceError {
    fn from(err: LinkError) -> Self {
        match err {
            LinkError::Client(err @ ClientError::Timeout) => Self::Timeout(err.to_string()),
            LinkError::Client(err) => Self::Unreachable(err.to_string()),
            other => Self::Command(CommandError::Link(other)),
        }
    }
}

impl From<LifecycleError> for SourceError {
    fn from(err: LifecycleError) -> Self {
        Self::Unreachable(err.to_string())
    }
}

impl From<tauri::Error> for SourceError {
    fn from(err: tauri::Error) -> Self {
        Self::Command(CommandError::Tauri(err))
    }
}

impl From<ConfigError> for SourceError {
    fn from(err: ConfigError) -> Self {
        Self::Command(CommandError::Config(err))
    }
}

serialize_as_wire_error!(SourceError);

pub(crate) type SrcResult<T> = Result<T, SourceError>;

// ── Daemon requests ─────────────────────────────────────────────────────────

/// Sends `request` on a connection of its own ([`DaemonLink::call_alone`]) and decodes the
/// answer's `data` (missing data is a decode error).
async fn call_for<T: DeserializeOwned>(
    link: &DaemonLink,
    request: Request,
    what: &'static str,
) -> SrcResult<T> {
    let response = link.call_alone(request).await?;
    let data = response
        .data_as::<T>()
        .map_err(|source| LinkError::Decode { what, source })?;
    data.ok_or_else(|| {
        SourceError::Command(CommandError::Link(LinkError::Decode {
            what,
            source: serde::de::Error::custom("the daemon sent no data"),
        }))
    })
}

/// [`check_source`] with the CLI runner and path passed in, for tests.
pub async fn check_source_with(
    link: &DaemonLink,
    runner: &impl CliRunner,
    cli: Option<&Path>,
    kind: String,
    location: String,
) -> SrcResult<CheckSourceData> {
    let socket = link.endpoint().path().to_owned();
    lifecycle::ensure_running(link, runner, cli, &socket).await?;
    let request = Request::CheckSource { kind, location };
    call_for(link, request, "check_source").await
}

/// `check_source { kind, location }` → `{ count }`. Starts the daemon first when it doesn't
/// answer. A source that gives no images fails with kind `daemon` and the daemon's text.
#[tauri::command]
pub async fn check_source(
    link: State<'_, Arc<DaemonLink>>,
    kind: String,
    location: String,
) -> SrcResult<CheckSourceData> {
    let cli = lifecycle::find_cli();
    check_source_with(&link, &ProcessRunner, cli.as_deref(), kind, location).await
}

/// `download_now { id }` → `{ path }`: one image now for source `id`, or with `id` `null` for
/// every enabled online source that has none yet.
#[tauri::command]
pub async fn download_now(
    link: State<'_, Arc<DaemonLink>>,
    id: Option<String>,
) -> SrcResult<DownloadNowData> {
    download_now_on(&link, id).await
}

pub async fn download_now_on(link: &DaemonLink, id: Option<String>) -> SrcResult<DownloadNowData> {
    call_for(link, Request::DownloadNow { id }, "download_now").await
}

/// `fetch_dropped_url { url }` → the new wallpaper. The daemon accepts http(s) URLs, `file://`
/// URLs and absolute paths, saves the image into `manual_dir` and sets it.
#[tauri::command]
pub async fn fetch_dropped_url(
    link: State<'_, Arc<DaemonLink>>,
    url: String,
) -> SrcResult<WallpaperInfo> {
    fetch_on(&link, url).await
}

pub async fn fetch_on(link: &DaemonLink, url: String) -> SrcResult<WallpaperInfo> {
    let url = url.trim();
    if url.is_empty() {
        return Err(SourceError::invalid("fetch_dropped_url: the url is empty"));
    }
    let request = Request::Fetch {
        url: url.to_owned(),
    };
    call_for(link, request, "fetch").await
}

/// `import_dropped_file`: the request body is the file's bytes (raw, not JSON) and the
/// `x-file-name` header its percent-encoded name. Writes a new file into `manual_dir`, then
/// sets it through IPC `fetch` → the new wallpaper.
#[tauri::command]
pub async fn import_dropped_file(
    link: State<'_, Arc<DaemonLink>>,
    request: tauri::ipc::Request<'_>,
) -> SrcResult<WallpaperInfo> {
    let name = request
        .headers()
        .get(FILE_NAME_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(percent_decode)
        .unwrap_or_default();
    let bytes = body_bytes(request.body())?;
    let manual_dir =
        blocking(|| SrcResult::Ok(load_config()?.daemon.manual_dir_resolved())).await?;
    import_on(&link, manual_dir, name, bytes).await
}

/// [`import_dropped_file`] after the request is unpacked.
pub async fn import_on(
    link: &DaemonLink,
    manual_dir: PathBuf,
    name: String,
    bytes: Vec<u8>,
) -> SrcResult<WallpaperInfo> {
    let path = blocking(move || write_dropped_file(&manual_dir, &name, &bytes)).await?;
    fetch_on(link, path.to_string_lossy().into_owned()).await
}

/// Fixtures the source and listing tests share.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::Path;

    use kakemono_config::Config;
    use kakemono_config::schema::{SourceEntry, SourceType};

    pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n rest";
    pub const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
    pub const WEBP: &[u8] = b"RIFF\0\0\0\0WEBPVP8 ";

    pub fn cfg_in(root: &Path) -> Config {
        kakemono_config::defaults_for_home(root)
    }

    pub fn row(id: &str, enabled: bool, kind: SourceType, location: &str) -> SourceEntry {
        SourceEntry {
            id: id.to_owned(),
            enabled,
            kind,
            location: location.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use interprocess::local_socket::tokio::prelude::*;
    use kakemono_protocol::endpoint::Endpoint;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // ── Errors ──

    #[test]
    fn transport_failures_are_daemon_unreachable() {
        let not_running = LinkError::Client(kakemono_protocol::client::ClientError::Closed);
        let err = SourceError::from(not_running);
        assert_eq!(err.kind(), ErrorKind::DaemonUnreachable);
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "daemon_unreachable");
        assert!(
            json["message"]
                .as_str()
                .unwrap()
                .starts_with("cannot reach")
        );

        let refused = SourceError::from(LinkError::Daemon("no images found".into()));
        assert_eq!(refused.kind(), ErrorKind::Daemon);
        assert_eq!(refused.to_string(), "no images found");

        let no_cli = SourceError::from(LifecycleError::CliNotFound);
        assert_eq!(no_cli.kind(), ErrorKind::DaemonUnreachable);
    }

    // ── Daemon requests against a fake daemon ──

    fn temp_endpoint() -> (tempfile::TempDir, Endpoint) {
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoint = Endpoint::new(dir.path().join("run/k.sock")).expect("endpoint");
        (dir, endpoint)
    }

    /// Answers each request line with the next reply, on any connection (long requests dial
    /// their own). Returns the requests it read, parsed as JSON, once every reply is sent.
    fn serve(
        endpoint: &Endpoint,
        replies: Vec<String>,
    ) -> tokio::task::JoinHandle<Vec<serde_json::Value>> {
        use std::sync::Mutex;
        let listener = endpoint.bind().expect("bind");
        let total = replies.len();
        let replies = Arc::new(Mutex::new(std::collections::VecDeque::from(replies)));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(total.max(1));
        tokio::spawn(async move {
            let acceptor = tokio::spawn({
                let seen = Arc::clone(&seen);
                async move {
                    loop {
                        let conn = listener.accept().await.expect("accept");
                        let (replies, seen, done) =
                            (Arc::clone(&replies), Arc::clone(&seen), done_tx.clone());
                        tokio::spawn(async move {
                            let mut reader = BufReader::new(conn);
                            let mut line = String::new();
                            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                                let request = serde_json::from_str(&line).expect("request json");
                                seen.lock().unwrap().push(request);
                                line.clear();
                                let Some(reply) = replies.lock().unwrap().pop_front() else {
                                    break;
                                };
                                let reply = format!("{reply}\n");
                                reader
                                    .get_mut()
                                    .write_all(reply.as_bytes())
                                    .await
                                    .expect("write");
                                let _ = done.send(()).await;
                            }
                        });
                    }
                }
            });
            for _ in 0..total {
                done_rx.recv().await;
            }
            acceptor.abort();
            std::mem::take(&mut *seen.lock().unwrap())
        })
    }

    const STATUS: &str = concat!(
        "{\"ok\":true,\"data\":{\"current\":null,\"paused\":false,\"interval\":\"30m\",",
        "\"theme\":\"dark\",\"history_count\":0,\"history_index\":-1}}"
    );

    struct NoCli;
    impl CliRunner for NoCli {
        async fn run(&self, _call: lifecycle::CliCall) -> std::io::Result<lifecycle::CliOutput> {
            panic!("the daemon is running; no CLI call expected")
        }
    }

    #[tokio::test]
    async fn check_source_asks_the_running_daemon() {
        let (_dir, endpoint) = temp_endpoint();
        let server = serve(
            &endpoint,
            vec![
                STATUS.into(),
                "{\"ok\":true,\"data\":{\"count\":24}}".into(),
            ],
        );
        let link = DaemonLink::new(endpoint);
        let got = check_source_with(&link, &NoCli, None, "wallhaven".into(), "forest".into())
            .await
            .expect("count");
        assert_eq!(got, CheckSourceData { count: 24 });
        let seen = server.await.unwrap();
        assert_eq!(
            seen[1],
            serde_json::json!({"cmd":"check_source","args":{"type":"wallhaven","location":"forest"}})
        );
    }

    #[tokio::test]
    async fn check_source_reports_the_daemons_refusal() {
        let (_dir, endpoint) = temp_endpoint();
        let _server = serve(
            &endpoint,
            vec![
                STATUS.into(),
                "{\"ok\":false,\"error\":\"no images found\"}".into(),
            ],
        );
        let link = DaemonLink::new(endpoint);
        let err = check_source_with(&link, &NoCli, None, "bing".into(), String::new())
            .await
            .expect_err("refused");
        assert_eq!(err.kind(), ErrorKind::Daemon);
        assert_eq!(err.to_string(), "no images found");
    }

    #[tokio::test]
    async fn check_source_without_a_daemon_or_cli_is_unreachable() {
        let (_dir, endpoint) = temp_endpoint();
        let link = DaemonLink::new(endpoint);
        let err = check_source_with(&link, &NoCli, None, "bing".into(), String::new())
            .await
            .expect_err("unreachable");
        assert_eq!(err.kind(), ErrorKind::DaemonUnreachable);
    }

    #[tokio::test]
    async fn requests_without_a_daemon_are_unreachable() {
        let (_dir, endpoint) = temp_endpoint();
        let link = DaemonLink::new(endpoint);
        let err = download_now_on(&link, None).await.expect_err("no daemon");
        assert_eq!(err.kind(), ErrorKind::DaemonUnreachable);
        let err = fetch_on(&link, "https://x/y.jpg".into())
            .await
            .expect_err("no daemon");
        assert_eq!(err.kind(), ErrorKind::DaemonUnreachable);
    }

    #[tokio::test]
    async fn download_now_sends_the_id_and_decodes_the_path() {
        let (_dir, endpoint) = temp_endpoint();
        let server = serve(
            &endpoint,
            vec![
                "{\"ok\":true,\"data\":{\"path\":\"/d/wh-1/a.jpg\"}}".into(),
                "{\"ok\":true,\"data\":{\"path\":null}}".into(),
            ],
        );
        let link = DaemonLink::new(endpoint);
        let one = download_now_on(&link, Some("wh-1".into())).await.unwrap();
        assert_eq!(one.path.as_deref(), Some("/d/wh-1/a.jpg"));
        let none = download_now_on(&link, None).await.unwrap();
        assert_eq!(none.path, None);
        let seen = server.await.unwrap();
        assert_eq!(
            seen,
            vec![
                serde_json::json!({"cmd":"download_now","args":{"id":"wh-1"}}),
                serde_json::json!({"cmd":"download_now"}),
            ]
        );
    }

    fn info_reply(path: &str) -> String {
        let info = WallpaperInfo {
            path: path.to_owned(),
            source: "manual".into(),
            ..Default::default()
        };
        format!(
            "{{\"ok\":true,\"data\":{}}}",
            serde_json::to_string(&info).unwrap()
        )
    }

    #[tokio::test]
    async fn fetch_sends_the_url_and_refuses_an_empty_one() {
        let (_dir, endpoint) = temp_endpoint();
        let server = serve(&endpoint, vec![info_reply("/m/y.jpg")]);
        let link = DaemonLink::new(endpoint);
        let err = fetch_on(&link, "  ".into()).await.expect_err("empty");
        assert_eq!(err.kind(), ErrorKind::Invalid);
        let info = fetch_on(&link, " https://x/y.jpg ".into()).await.unwrap();
        assert_eq!(info.path, "/m/y.jpg");
        let seen = server.await.unwrap();
        assert_eq!(
            seen,
            vec![serde_json::json!({"cmd":"fetch","args":{"url":"https://x/y.jpg"}})]
        );
    }

    #[tokio::test]
    async fn import_writes_into_manual_dir_then_fetches_that_path() {
        let (dir, endpoint) = temp_endpoint();
        let manual = dir.path().join("manual");
        let expected = manual.join("drop.png");
        let server = serve(&endpoint, vec![info_reply(&expected.to_string_lossy())]);
        let link = DaemonLink::new(endpoint);
        let info = import_on(&link, manual.clone(), "drop.png".into(), PNG.to_vec())
            .await
            .unwrap();
        assert_eq!(info.path, expected.to_string_lossy());
        assert_eq!(std::fs::read(&expected).unwrap(), PNG);
        let seen = server.await.unwrap();
        assert_eq!(
            seen,
            vec![serde_json::json!({"cmd":"fetch","args":{"url": expected.to_string_lossy()}})]
        );
    }

    /// A daemon that serves every connection at once: `status` is answered right away, and
    /// any other command is read and never answered. `seen_slow` fires when that happens.
    fn serve_with_a_hanging_fetch(
        endpoint: &Endpoint,
    ) -> (tokio::task::JoinHandle<()>, tokio::sync::mpsc::Receiver<()>) {
        let listener = endpoint.bind().expect("bind");
        let (seen_slow, rx) = tokio::sync::mpsc::channel(1);
        let server = tokio::spawn(async move {
            loop {
                let conn = listener.accept().await.expect("accept");
                let seen_slow = seen_slow.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(conn);
                    let mut line = String::new();
                    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                        if line.contains("\"status\"") {
                            let reply = format!("{STATUS}\n");
                            let _ = reader.get_mut().write_all(reply.as_bytes()).await;
                        } else {
                            let _ = seen_slow.send(()).await;
                            std::future::pending::<()>().await;
                        }
                        line.clear();
                    }
                });
            }
        });
        (server, rx)
    }

    #[tokio::test]
    async fn a_slow_fetch_does_not_block_status() {
        let (_dir, endpoint) = temp_endpoint();
        let (server, mut seen_slow) = serve_with_a_hanging_fetch(&endpoint);
        let link = Arc::new(DaemonLink::new(endpoint));
        link.status().await.expect("the shared connection is open");

        let fetch = tokio::spawn({
            let link = Arc::clone(&link);
            async move { fetch_on(&link, "https://x/y.jpg".into()).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), seen_slow.recv())
            .await
            .expect("the daemon got the fetch");

        let status = tokio::time::timeout(std::time::Duration::from_secs(2), link.status()).await;
        assert!(
            matches!(status, Ok(Ok(_))),
            "status waited behind the fetch: {status:?}"
        );
        fetch.abort();
        server.abort();
    }

    #[test]
    fn a_timeout_has_its_own_kind() {
        let timeout = LinkError::Client(kakemono_protocol::client::ClientError::Timeout);
        let err = SourceError::from(timeout);
        assert_eq!(err.kind(), ErrorKind::DaemonTimeout);
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "daemon_timeout");
    }

    #[tokio::test]
    async fn import_refuses_a_non_image_without_calling_the_daemon() {
        let (dir, endpoint) = temp_endpoint();
        let link = DaemonLink::new(endpoint);
        let err = import_on(
            &link,
            dir.path().join("m"),
            "a.png".into(),
            b"nope".to_vec(),
        )
        .await
        .expect_err("not an image");
        assert_eq!(err.kind(), ErrorKind::Invalid);
    }
}
