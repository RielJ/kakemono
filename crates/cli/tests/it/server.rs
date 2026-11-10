//! IPC server + instance lock: a real server on a tempdir socket over a fake
//! platform and a static source, driven through `protocol::Client` and raw socket lines.
//! Never touches the real socket or config.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use futures::StreamExt;
use interprocess::local_socket::tokio::Stream as LocalSocketStream;
use interprocess::local_socket::traits::tokio::Stream as _;
use kakemono_cli::daemon::lock::{self, LockError, LockGuard};
use kakemono_cli::daemon::server::{
    self, ApplyOutcome, DoctorData, NOTHING_TO_APPLY, Server, VersionData,
};
use kakemono_config::Config;
use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine};
use kakemono_platform::api::{Fit, Platform, Theme};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_platform::linux::detect::Env;
use kakemono_protocol::client::{self, Client};
use kakemono_protocol::endpoint::Endpoint;
use kakemono_protocol::types::{Command, Event, Request, Response, StatusData, WallpaperInfo};
use kakemono_sources::SourceSet;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-03-04T05:06:07.890+08:00";
const STEP: Duration = Duration::from_secs(5);

struct Daemon {
    _dir: TempDir,
    root: PathBuf,
    endpoint: Endpoint,
    lock: Option<LockGuard>,
    setter: RecordingSetter,
    engine: Engine,
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl Daemon {
    fn runtime_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Replaces the root (as it appears inside JSON strings) with `[ROOT]`, and this build's
    /// version with `[VERSION]`.
    fn redact(&self, text: &str) -> String {
        let root = serde_json::to_string(&self.root).unwrap();
        let root = root.trim_matches('"');
        text.replace(root, "[ROOT]")
            .replace(env!("CARGO_PKG_VERSION"), "[VERSION]")
            .replace(
                &format!("\"pid\":{},", std::process::id()),
                "\"pid\":\"[PID]\",",
            )
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(STEP, task)
                .await
                .expect("server stops")
                .expect("server task");
        }
        self.lock.take();
    }
}

fn config(root: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.daemon.download_dir = root.join("downloads");
    cfg.daemon.trash_dir = root.join("trash");
    cfg.favorites.dir = root.join("favorites");
    cfg.daemon.interval = "45m".to_owned();
    cfg.daemon.socket_path = Some(root.join("run/k.sock"));
    cfg.desktop.fit = Fit::Fill;
    cfg.daemon.internet = false;
    cfg.sources = vec![pics_row(root)];
    cfg
}

/// The folder source `<tmp>/pics`, the only pool `next` picks from.
fn pics_row(root: &Path) -> SourceEntry {
    SourceEntry {
        id: "pics".to_owned(),
        enabled: true,
        kind: SourceType::Folder,
        location: root.join("pics").to_string_lossy().into_owned(),
    }
}

/// Lock, bind and serve on `<tmp>/run/k.sock`. The folder source `<tmp>/pics` holds two
/// images (`one.jpg`, `two.jpg`); `next` picks one of them at random.
async fn start() -> Daemon {
    start_with(|srv| srv).await
}

/// [`start`], with `customize` applied to the [`Server`] before it serves.
async fn start_with(customize: impl FnOnce(Server) -> Server) -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("pics")).unwrap();
    for name in ["one.jpg", "two.jpg"] {
        std::fs::write(root.join("pics").join(name), b"fake image data").unwrap();
    }

    let cfg = config(&root);
    // `reload` re-reads this file.
    std::fs::write(root.join("config.toml"), "[daemon]\ninterval = \"10m\"\n").unwrap();

    let setter = RecordingSetter::new();
    let platform = Platform {
        setter: Arc::new(setter.clone()),
        signals: Arc::new(ManualSignals::new(Theme::Light)),
    };
    let now: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(NOW).unwrap();
    let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
        .with_sources(SourceSet::default())
        .with_clock(move || now);
    let engine = Engine::new(cfg.clone(), State::new(), root.join("state.json"), deps).unwrap();

    let lock = lock::acquire(&root.join("run")).unwrap();
    let endpoint = Endpoint::new(root.join("run/k.sock")).unwrap();
    let listener = server::bind(&endpoint, &lock).await.unwrap();

    let config_path = root.join("config.toml");
    let home = root.clone();
    let download_dir = root.join("downloads");
    let srv = Server::new(engine.clone(), move || {
        let mut cfg = kakemono_config::load_with_home(&config_path, &home)?;
        // Keep the test's dirs; only `interval` comes from the file.
        cfg.daemon.download_dir = download_dir.clone();
        cfg.daemon.trash_dir = download_dir.with_file_name("trash");
        cfg.favorites.dir = download_dir.with_file_name("favorites");
        cfg.daemon.internet = false;
        cfg.sources = vec![pics_row(&home)];
        Ok(cfg)
    });
    let srv = customize(srv);
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(srv.serve(listener, shutdown.clone()));
    Daemon {
        _dir: dir,
        root,
        endpoint,
        lock: Some(lock),
        setter,
        engine,
        shutdown,
        task: Some(task),
    }
}

/// Sends each raw line on one connection and returns each raw response line (without `\n`).
async fn raw_exchange(endpoint: &Endpoint, lines: &[&str]) -> Vec<String> {
    let stream = LocalSocketStream::connect(endpoint.name().unwrap())
        .await
        .unwrap();
    let mut conn = BufReader::new(stream);
    let mut out = Vec::new();
    for line in lines {
        conn.get_mut()
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        let mut reply = String::new();
        tokio::time::timeout(STEP, conn.read_line(&mut reply))
            .await
            .expect("reply in time")
            .unwrap();
        assert!(reply.ends_with('\n'), "unterminated reply {reply:?}");
        reply.pop();
        out.push(reply);
    }
    out
}

fn ok_data<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    assert!(response.ok, "{response:?}");
    response.data_as::<T>().unwrap().expect("data")
}

#[tokio::test]
async fn every_command_through_client() {
    let d = start().await;
    assert!(client::is_running(&d.endpoint).await);
    let mut c = Client::connect(&d.endpoint).await.unwrap();

    let status: StatusData = ok_data(&c.request(Request::Status).await.unwrap());
    assert!(status.current.is_none());
    assert_eq!(status.history_index, -1);
    assert_eq!(status.interval, "45m");
    assert_eq!(status.theme, "light");

    let first: WallpaperInfo = ok_data(&c.request(Request::Next).await.unwrap());
    assert_eq!(first.source, "folder");
    assert_eq!(d.setter.last().unwrap().0, PathBuf::from(&first.path));

    let image = d.root.join("three.jpg");
    std::fs::write(&image, b"fake image data").unwrap();
    let set: WallpaperInfo = ok_data(
        &c.request(Request::Set {
            path: image.to_str().unwrap().to_owned(),
        })
        .await
        .unwrap(),
    );
    assert_eq!(set.path, image.to_str().unwrap());

    let prev: WallpaperInfo = ok_data(&c.request(Request::Prev).await.unwrap());
    assert_eq!(prev.path, first.path);

    // favorite/pause/resume answer bare `{"ok":true}`.
    for request in [Request::Favorite, Request::Pause] {
        let r = c.request(request).await.unwrap();
        assert!(r.ok && r.data.is_none(), "{r:?}");
    }
    assert!(d.engine.is_paused());
    let status: StatusData = ok_data(&c.request(Request::Status).await.unwrap());
    assert!(status.paused);
    let r = c.request(Request::Resume).await.unwrap();
    assert!(r.ok && r.data.is_none(), "{r:?}");
    assert!(!d.engine.is_paused());

    let version: VersionData = ok_data(&c.request(Request::Version).await.unwrap());
    assert_eq!(version, VersionData::current());

    let doctor: DoctorData = ok_data(&c.request(Request::Doctor { apply: false }).await.unwrap());
    assert_eq!(doctor.backend.name, "fake");
    assert_eq!(doctor.theme, "light");
    assert_eq!(doctor.pid, std::process::id());
    assert_eq!(doctor.env, None, "no env probe installed");
    assert_eq!(doctor.apply, None, "plain doctor sets nothing");

    // reload re-reads config.toml (interval 10m).
    let r = c.request(Request::Reload).await.unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(d.engine.config().daemon.interval, "10m");
    let status: StatusData = ok_data(&c.request(Request::Status).await.unwrap());
    assert_eq!(status.interval, "10m");

    // A broken config file: reload fails and the old config stays.
    std::fs::write(d.config_path(), "[daemon\n").unwrap();
    let r = c.request(Request::Reload).await.unwrap();
    assert!(!r.ok);
    assert!(r.error.starts_with("reload: "), "{}", r.error);
    assert_eq!(d.engine.config().daemon.interval, "10m");

    // trash with a stale `expected` is refused and moves nothing (Task 45b).
    let status: StatusData = ok_data(&c.request(Request::Status).await.unwrap());
    let current = status.current.unwrap().path;
    let r = c
        .request(Request::Trash {
            expected: Some("/not/the/current.jpg".into()),
        })
        .await
        .unwrap();
    assert!(!r.ok);
    assert_eq!(
        r.error,
        format!(
            "manager: current wallpaper changed: expected /not/the/current.jpg, current {current}"
        )
    );
    assert!(Path::new(&current).exists());

    // trash moves the current image (`first`, after `prev`) away and advances forward in
    // history to the `set` image (source "local").
    let trashed: WallpaperInfo =
        ok_data(&c.request(Request::Trash { expected: None }).await.unwrap());
    assert_eq!(trashed.source, "local");
    assert_eq!(trashed.path, set.path);
    assert!(!Path::new(&first.path).exists());

    let r = c.request(Command::new("bogus")).await.unwrap();
    assert!(!r.ok);
    assert_eq!(
        r.error,
        kakemono_protocol::types::unknown_command_error("bogus")
    );

    drop(c);
    d.stop().await;
}

#[tokio::test]
async fn raw_response_lines_match_go_serialisation() {
    let d = start().await;
    let image = d.root.join("a<b>&c.jpg");
    std::fs::write(&image, b"fake image data").unwrap();
    let set_line = serde_json::json!({"cmd":"set","args":{"path":image}}).to_string();

    let lines = raw_exchange(
        &d.endpoint,
        &[
            r#"{"cmd":"status"}"#,
            r#"{"cmd":"prev"}"#,
            r#"{"cmd":"favorite"}"#,
            &set_line,
            r#"{"cmd":"status"}"#,
            r#"{"cmd":"favorite"}"#,
            r#"{"cmd":"pause"}"#,
            r#"{"cmd":"pause"}"#,
            r#"{"cmd":"resume"}"#,
            r#"{"cmd":"set"}"#,
            r#"{"cmd":"set","args":{"path":""}}"#,
            r#"{"cmd":"set","args":{"path":"/definitely/not/here.jpg"}}"#,
            r#"{"cmd":"version"}"#,
            r#"{"cmd":"doctor"}"#,
            r#"{"cmd":"bogus"}"#,
            r#"{"args":{}}"#,
            r#"{"cmd":"status","extra":1}"#,
        ],
    )
    .await;
    let transcript = d.redact(&lines.join("\n"));
    insta::assert_snapshot!("raw_lines", transcript);

    // Invalid JSON gets an error line and the connection stays open.
    let lines = raw_exchange(&d.endpoint, &["not json", r#"{"cmd":"version"}"#]).await;
    assert!(
        lines[0].starts_with(r#"{"ok":false,"error":"invalid JSON: "#),
        "{}",
        lines[0]
    );
    assert!(lines[1].starts_with(r#"{"ok":true,"data":{"version":""#));

    d.stop().await;
}

#[tokio::test]
async fn subscribe_streams_events_until_disconnect() {
    let d = start().await;
    let mut events = Client::connect(&d.endpoint)
        .await
        .unwrap()
        .subscribe()
        .await
        .unwrap();

    let mut c = Client::connect(&d.endpoint).await.unwrap();
    let info: WallpaperInfo = ok_data(&c.request(Request::Next).await.unwrap());
    assert!(c.request(Request::Pause).await.unwrap().ok);
    assert!(c.request(Request::Resume).await.unwrap().ok);

    let mut got = Vec::new();
    for _ in 0..3 {
        let event = tokio::time::timeout(STEP, events.next())
            .await
            .expect("event in time")
            .expect("stream open")
            .unwrap();
        got.push(event);
    }
    assert_eq!(
        got,
        vec![
            Event::Changed(info),
            Event::Paused(true),
            Event::Paused(false)
        ]
    );

    // The raw handshake is exactly one `{"ok":true}` line.
    let stream = LocalSocketStream::connect(d.endpoint.name().unwrap())
        .await
        .unwrap();
    let mut raw = BufReader::new(stream);
    raw.get_mut()
        .write_all(b"{\"cmd\":\"subscribe\"}\n")
        .await
        .unwrap();
    let mut line = String::new();
    raw.read_line(&mut line).await.unwrap();
    assert_eq!(line, "{\"ok\":true}\n");
    assert!(c.request(Request::Pause).await.unwrap().ok);
    line.clear();
    tokio::time::timeout(STEP, raw.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(line, "{\"event\":\"paused\",\"data\":true}\n");

    // The first subscriber saw that pause too.
    let event = tokio::time::timeout(STEP, events.next()).await.unwrap();
    assert!(matches!(event, Some(Ok(Event::Paused(true)))), "{event:?}");

    // Shutdown ends subscriber streams.
    drop(c);
    d.stop().await;
    let end = tokio::time::timeout(STEP, events.next()).await.unwrap();
    assert!(end.is_none(), "{end:?}");
}

#[tokio::test]
async fn second_lock_attempt_fails_while_daemon_runs() {
    let d = start().await;
    let err = lock::acquire(&d.runtime_dir()).unwrap_err();
    assert!(matches!(err, LockError::AlreadyRunning { .. }), "{err:?}");
    assert!(err.to_string().starts_with("daemon is already running"));
    let rt = d.runtime_dir();
    d.stop().await;
    // Lock released on stop.
    drop(lock::acquire(&rt).unwrap());
}

#[tokio::test]
async fn shutdown_removes_the_socket() {
    let d = start().await;
    let path = d.endpoint.path().to_owned();
    assert!(path.exists());
    let endpoint = d.endpoint.clone();
    d.stop().await;
    if cfg!(unix) {
        assert!(!path.exists());
    }
    assert!(!client::is_running(&endpoint).await);
}

#[cfg(unix)]
#[tokio::test]
async fn bind_removes_only_a_stale_socket_under_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let guard = lock::acquire(&root.join("run")).unwrap();
    let endpoint = Endpoint::new(root.join("run/k.sock")).unwrap();

    // A stale socket file: bound, then dropped without unlinking (std doesn't unlink).
    drop(std::os::unix::net::UnixListener::bind(endpoint.path()).unwrap());
    assert!(endpoint.path().exists());
    assert!(!client::is_running(&endpoint).await);
    let listener = server::bind(&endpoint, &guard).await.unwrap();

    // A live socket is never removed.
    let err = server::bind(&endpoint, &guard).await.unwrap_err();
    assert!(matches!(err, server::BindError::InUse(_)), "{err:?}");
    assert!(err.to_string().contains("already listening"), "{err}");
    drop(listener);

    // A regular file at the socket path is never removed.
    std::fs::write(endpoint.path(), b"not a socket").unwrap();
    let err = server::bind(&endpoint, &guard).await.unwrap_err();
    assert!(matches!(err, server::BindError::NotASocket(_)), "{err:?}");
    assert_eq!(std::fs::read(endpoint.path()).unwrap(), b"not a socket");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::remove_file(endpoint.path()).unwrap();
        let _listener = server::bind(&endpoint, &guard).await.unwrap();
        let mode = std::fs::metadata(endpoint.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[tokio::test]
async fn dispatch_subscribe_without_a_connection_is_an_error() {
    let d = start().await;
    let srv = Server::new(d.engine.clone(), || Ok(Config::default()));
    let r = srv.dispatch(Request::Subscribe).await;
    assert!(!r.ok);
    d.stop().await;
}

/// `reload` reads the config off the async runtime: a slow disk does not hold up the other
/// connections' requests.
#[tokio::test]
async fn a_slow_config_read_does_not_block_other_requests() {
    let d = start().await;
    let cfg = d.engine.config();
    let srv = Server::new(d.engine.clone(), move || {
        std::thread::sleep(Duration::from_millis(500));
        Ok((*cfg).clone())
    });
    let started = std::time::Instant::now();
    let status = async {
        let r = srv.dispatch(Request::Status).await;
        (r, started.elapsed())
    };
    let (reload, (status, waited)) = tokio::join!(srv.dispatch(Request::Reload), status);
    assert!(reload.ok, "{reload:?}");
    assert!(status.ok, "{status:?}");
    assert!(
        waited < Duration::from_millis(250),
        "status waited {waited:?} for the config read"
    );
    d.stop().await;
}

/// `doctor` with `apply` re-sets the current wallpaper through the daemon's own setter, and a
/// failed set is reported inside an `ok:true` report.
#[tokio::test]
async fn doctor_apply_resets_the_current_wallpaper_through_the_daemon() {
    let d = start_with(|srv| {
        srv.with_env_probe(|| {
            Box::pin(async { Env::default().with_var("XDG_CURRENT_DESKTOP", "Hyprland") })
        })
    })
    .await;
    let mut c = Client::connect(&d.endpoint).await.unwrap();

    // Nothing to re-set yet: no setter call.
    let doctor: DoctorData = ok_data(&c.request(Request::Doctor { apply: true }).await.unwrap());
    assert_eq!(
        doctor.apply,
        Some(ApplyOutcome {
            ok: false,
            path: None,
            error: Some(NOTHING_TO_APPLY.to_owned()),
        })
    );
    assert!(d.setter.calls().is_empty());
    assert_eq!(
        doctor.env.unwrap().xdg_current_desktop.as_deref(),
        Some("Hyprland")
    );

    let current: WallpaperInfo = ok_data(&c.request(Request::Next).await.unwrap());
    let history = d.engine.state().history_count();
    let doctor: DoctorData = ok_data(&c.request(Request::Doctor { apply: true }).await.unwrap());
    assert_eq!(
        doctor.apply,
        Some(ApplyOutcome {
            ok: true,
            path: Some(current.path.clone()),
            error: None,
        })
    );
    let calls = d.setter.calls();
    assert_eq!(calls.len(), 2, "next + apply");
    assert_eq!(calls[1], (PathBuf::from(&current.path), Fit::Fill));
    assert_eq!(
        d.engine.state().history_count(),
        history,
        "history untouched"
    );

    // A failing setter: still an ok:true report, with the error inside.
    d.setter.fail_next(1);
    let r = c.request(Request::Doctor { apply: true }).await.unwrap();
    let doctor: DoctorData = ok_data(&r);
    let apply = doctor.apply.unwrap();
    assert!(!apply.ok && apply.path.is_none());
    assert!(apply.error.is_some_and(|e| !e.is_empty()));

    // On the wire, `apply` is a JSON object and plain doctor has none.
    let lines = raw_exchange(
        &d.endpoint,
        &[
            r#"{"cmd":"doctor"}"#,
            r#"{"cmd":"doctor","args":{"apply":"yes"}}"#,
        ],
    )
    .await;
    assert!(!lines[0].contains("\"apply\""), "{}", lines[0]);
    assert_eq!(
        lines[1],
        r#"{"ok":false,"error":"doctor: argument 'apply' must be a boolean"}"#
    );
    d.stop().await;
}

/// Task 16: `check_source`, `fetch` and `download_now` on the wire (`[daemon] internet` is off
/// in this harness).
#[tokio::test]
async fn source_requests_on_the_wire() {
    let d = start().await;
    let pics = d.root.join("pics");
    let dropped = d.root.join("drop/My Pic.png");
    std::fs::create_dir_all(dropped.parent().unwrap()).unwrap();
    std::fs::write(&dropped, b"\x89PNG\r\n\x1a\nnot decoded").unwrap();
    let check_pics =
        serde_json::json!({"cmd":"check_source","args":{"type":"folder","location":pics}});
    let fetch_file = serde_json::json!({"cmd":"fetch","args":{"url":dropped}});

    let lines = raw_exchange(
        &d.endpoint,
        &[
            &check_pics.to_string(),
            r#"{"cmd":"check_source","args":{"type":"wallhaven","location":"forest"}}"#,
            r#"{"cmd":"check_source","args":{"type":"folder","location":"/definitely/not/here"}}"#,
            r#"{"cmd":"check_source","args":{"type":"nope"}}"#,
            r#"{"cmd":"check_source","args":{"location":"x"}}"#,
            &fetch_file.to_string(),
            r#"{"cmd":"fetch","args":{"url":"https://example.com/a.jpg"}}"#,
            r#"{"cmd":"fetch","args":{"url":"a.jpg"}}"#,
            r#"{"cmd":"fetch"}"#,
            r#"{"cmd":"download_now"}"#,
            r#"{"cmd":"download_now","args":{"id":"pics"}}"#,
            r#"{"cmd":"download_now","args":{"id":7}}"#,
        ],
    )
    .await;
    let lines: Vec<String> = lines.iter().map(|l| d.redact(l)).collect();
    assert_eq!(
        lines,
        [
            r#"{"ok":true,"data":{"count":2}}"#,
            r#"{"ok":false,"error":"Internet access is off"}"#,
            r#"{"ok":false,"error":"no images found"}"#,
            r#"{"ok":false,"error":"check_source: unknown source type \"nope\""}"#,
            r#"{"ok":false,"error":"check_source: missing required argument 'type'"}"#,
            concat!(
                r#"{"ok":true,"data":{"path":"[ROOT]/downloads/manual/My Pic.png","#,
                r#""source":"manual","source_id":"manual","source_url":"","image_url":"","#,
                r#""title":"My Pic.png","author":"","width":0,"height":0,"#,
                r#""set_at":"2026-03-04T05:06:07+08:00"}}"#
            ),
            r#"{"ok":false,"error":"Internet access is off"}"#,
            r#"{"ok":false,"error":"not an http(s) URL or absolute file path: \"a.jpg\""}"#,
            r#"{"ok":false,"error":"fetch: missing required argument 'url'"}"#,
            r#"{"ok":false,"error":"Internet access is off"}"#,
            r#"{"ok":false,"error":"no enabled online source \"pics\""}"#,
            r#"{"ok":false,"error":"download_now: argument 'id' must be a string"}"#,
        ]
    );
    let manual = d.root.join("downloads/manual/My Pic.png");
    assert_eq!(d.setter.calls(), vec![(manual.clone(), Fit::Fill)]);
    let status = d.engine.status();
    assert_eq!(
        status.current.map(|c| (c.path, c.source_id)),
        Some((manual.to_string_lossy().into_owned(), "manual".to_owned()))
    );
    d.stop().await;
}

/// The server serves at most `max_connections` connections at once (a same-user client that
/// opens connections in a loop can't make it buffer without bound). The next one waits until a
/// slot frees up.
#[tokio::test]
async fn connections_over_the_cap_wait_for_a_free_slot() {
    let d = start_with(|srv| srv.with_max_connections(1)).await;
    let idle = LocalSocketStream::connect(d.endpoint.name().unwrap())
        .await
        .unwrap();
    // Let the server accept the idle connection first.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let endpoint = d.endpoint.clone();
    let waiting = tokio::spawn(async move {
        let mut client = Client::connect(&endpoint).await.unwrap();
        client.request(Request::Status).await.unwrap()
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!waiting.is_finished(), "served over the cap");

    drop(idle);
    let response = tokio::time::timeout(STEP, waiting)
        .await
        .expect("served once a slot is free")
        .unwrap();
    assert!(response.ok, "{response:?}");
    d.stop().await;
}
