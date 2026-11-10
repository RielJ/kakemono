//! `browse` and `browse_use` through the daemon:
//! the dispatch arms, the wire texts the picker matches on, and session resume.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::tokio::Stream as LocalSocketStream;
use interprocess::local_socket::traits::tokio::Stream as _;
use kakemono_cli::daemon::lock::{self, LockGuard};
use kakemono_cli::daemon::server::{self, Server};
use kakemono_config::Config;
use kakemono_engine::browse::{BrowseDeps, BrowseLimits};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine};
use kakemono_platform::api::{Platform, Theme};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_protocol::codec::MAX_LINE_BYTES;
use kakemono_protocol::endpoint::Endpoint;
use kakemono_protocol::types::{BrowseAction, BrowseData, Request, Response};
use kakemono_sources::fake::StaticProvider;
use kakemono_sources::{GuardConfig, Provider, SourceSet};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const STEP: Duration = Duration::from_secs(5);

/// Keys that must never reach the wire.
const SECRETS: [&str; 3] = ["SECRET-WALLHAVEN", "SECRET-UNSPLASH", "SECRET-APOD"];

/// How the test daemon is set up.
struct Setup {
    internet: bool,
    sources: SourceSet,
    limits: BrowseLimits,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            internet: true,
            sources: SourceSet::default(),
            limits: BrowseLimits::default(),
        }
    }
}

struct Daemon {
    _dir: TempDir,
    root: PathBuf,
    endpoint: Endpoint,
    server: Server,
    _lock: LockGuard,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl Daemon {
    async fn start(setup: Setup) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let cfg = config(&root, setup.internet);
        let platform = Platform {
            setter: Arc::new(RecordingSetter::new()),
            signals: Arc::new(ManualSignals::new(Theme::Light)),
        };
        let browse = BrowseDeps::new(
            kakemono_sources::http::client().unwrap(),
            GuardConfig::permissive_loopback(),
            root.join("cache/previews"),
        )
        .with_limits(setup.limits);
        let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
            .with_sources(setup.sources)
            .with_browse(browse);
        let engine = Engine::new(cfg.clone(), State::new(), root.join("state.json"), deps).unwrap();

        let lock = lock::acquire(&root.join("run")).unwrap();
        let endpoint = Endpoint::new(root.join("run/k.sock")).unwrap();
        let listener = server::bind(&endpoint, &lock).await.unwrap();
        let server = Server::new(engine, move || Ok(cfg.clone()));
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(server.clone().serve(listener, shutdown.clone()));
        Self {
            _dir: dir,
            root,
            endpoint,
            server,
            _lock: lock,
            shutdown,
            task,
        }
    }

    /// Sends each raw line on one connection and returns each raw response line.
    async fn exchange(&self, lines: &[&str]) -> Vec<String> {
        let stream = LocalSocketStream::connect(self.endpoint.name().unwrap())
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

    /// `browse` through [`Server::dispatch`], which must succeed.
    async fn browse(&self, session: Option<&str>, page: u32) -> BrowseData {
        let response = tokio::time::timeout(
            STEP,
            self.server.dispatch(Request::Browse {
                session: session.map(str::to_owned),
                page,
                source: None,
            }),
        )
        .await
        .expect("browse answers in time");
        ok_data(&response)
    }

    /// Replaces the root (as it appears inside JSON strings) with `[ROOT]`.
    fn redact(&self, text: &str) -> String {
        let root = serde_json::to_string(&self.root).unwrap();
        text.replace(root.trim_matches('"'), "[ROOT]")
    }

    async fn stop(self) {
        self.shutdown.cancel();
        tokio::time::timeout(STEP, self.task)
            .await
            .expect("server stops")
            .expect("server task");
    }
}

fn config(root: &Path, internet: bool) -> Config {
    let mut cfg = Config::default();
    cfg.daemon.download_dir = root.join("downloads");
    cfg.daemon.trash_dir = root.join("trash");
    cfg.favorites.dir = root.join("favorites");
    cfg.daemon.internet = internet;
    cfg.sources = Vec::new();
    cfg.wallhaven.api_key = SECRETS[0].to_owned();
    cfg.unsplash.access_key = SECRETS[1].to_owned();
    cfg.apod.api_key = SECRETS[2].to_owned();
    cfg
}

fn ok_data<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    assert!(response.ok, "{response:?}");
    response.data_as::<T>().unwrap().expect("data")
}

/// Online sources that answer without any network: these providers can't browse.
fn sources_that_cannot_browse() -> SourceSet {
    let provider = |name| Arc::new(StaticProvider::new(name, Vec::new())) as Arc<dyn Provider>;
    SourceSet::fake(vec![
        ("wh-1", provider("wallhaven"), "forest"),
        ("us-1", provider("unsplash"), "lake"),
    ])
}

#[tokio::test]
async fn browse_with_no_online_source_is_an_empty_done_page() {
    let d = Daemon::start(Setup::default()).await;
    let lines = d.exchange(&[r#"{"cmd":"browse","args":{"page":0}}"#]).await;
    let response: Response = serde_json::from_str(&lines[0]).unwrap();
    let data: BrowseData = ok_data(&response);
    assert_eq!(
        (
            data.page,
            data.restarted,
            data.done,
            data.items.len(),
            data.sources.len(),
            d.redact(&data.preview_dir),
        ),
        (0, false, true, 0, 0, "[ROOT]/cache/previews".to_owned())
    );
    d.stop().await;
}

#[tokio::test]
async fn browse_use_of_an_unknown_item_is_an_error() {
    let d = Daemon::start(Setup::default()).await;
    let lines = d
        .exchange(&[
            r#"{"cmd":"browse_use","args":{"source_id":"wh-1","remote_id":"nope","action":"set"}}"#,
        ])
        .await;
    assert_eq!(
        lines,
        [r#"{"ok":false,"error":"That preview expired — scroll or reopen to refresh."}"#]
    );
    d.stop().await;
}

/// The picker matches `/internet access is off/i`: both commands answer with the text every
/// other online command uses.
#[tokio::test]
async fn internet_off_refuses_both_with_the_shared_text() {
    let d = Daemon::start(Setup {
        internet: false,
        ..Setup::default()
    })
    .await;
    let lines = d
        .exchange(&[
            r#"{"cmd":"browse","args":{"page":0}}"#,
            r#"{"cmd":"browse","args":{"page":0,"session":"old"}}"#,
            r#"{"cmd":"browse_use","args":{"source_id":"wh-1","remote_id":"x","action":"favorite"}}"#,
            r#"{"cmd":"check_source","args":{"type":"bing","location":""}}"#,
        ])
        .await;
    let off = r#"{"ok":false,"error":"Internet access is off"}"#;
    assert_eq!(lines, [off, off, off, off]);
    d.stop().await;
}

#[tokio::test]
async fn responses_carry_no_keys_and_fit_in_a_line() {
    let d = Daemon::start(Setup {
        sources: sources_that_cannot_browse(),
        ..Setup::default()
    })
    .await;
    let lines = d
        .exchange(&[
            r#"{"cmd":"browse","args":{"page":0}}"#,
            r#"{"cmd":"browse","args":{"page":0,"source":"wh-1"}}"#,
            r#"{"cmd":"browse_use","args":{"source_id":"wh-1","remote_id":"x","action":"download"}}"#,
        ])
        .await;
    let first: Response = serde_json::from_str(&lines[0]).unwrap();
    let first: BrowseData = ok_data(&first);
    let states: Vec<(&str, String)> = first
        .sources
        .iter()
        .map(|s| {
            (
                s.source_id.as_str(),
                serde_json::to_string(&s.state).unwrap(),
            )
        })
        .collect();
    assert_eq!(
        states,
        [
            ("wh-1", r#"{"state":"unsupported"}"#.to_owned()),
            ("us-1", r#"{"state":"unsupported"}"#.to_owned())
        ]
    );
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        for needle in ["apikey", "api_key", "client_id", "access_key"]
            .into_iter()
            .chain(
                SECRETS
                    .map(|s| s.to_ascii_lowercase())
                    .iter()
                    .map(String::as_str),
            )
        {
            assert!(!lower.contains(needle), "{needle:?} in {line}");
        }
        assert!(line.len() < MAX_LINE_BYTES, "{} bytes", line.len());
    }
    d.stop().await;
}

/// The picker reopens within 10 minutes by asking page 0 of its old session.
#[tokio::test]
async fn page_zero_of_a_live_session_is_its_first_page_again() {
    let d = Daemon::start(Setup::default()).await;
    let first = d.browse(None, 0).await;
    let resumed = d.browse(Some(&first.session), 0).await;
    assert_eq!(
        (resumed.session.as_str(), resumed.restarted, resumed.page),
        (first.session.as_str(), false, 0)
    );
    d.stop().await;
}

#[tokio::test]
async fn page_zero_of_an_unknown_session_restarts() {
    let d = Daemon::start(Setup::default()).await;
    let resumed = d.browse(Some("not-a-session"), 0).await;
    assert_eq!(
        (
            resumed.restarted,
            resumed.page,
            resumed.session != "not-a-session"
        ),
        (true, 0, true)
    );
    d.stop().await;
}

#[tokio::test]
async fn page_zero_of_an_expired_session_restarts() {
    let ttl = Duration::from_millis(100);
    let d = Daemon::start(Setup {
        limits: BrowseLimits {
            idle_ttl: ttl,
            ..BrowseLimits::default()
        },
        ..Setup::default()
    })
    .await;
    let first = d.browse(None, 0).await;
    tokio::time::sleep(ttl * 3).await;
    let resumed = d.browse(Some(&first.session), 0).await;
    assert_eq!(
        (
            resumed.restarted,
            resumed.page,
            resumed.session != first.session
        ),
        (true, 0, true)
    );
    d.stop().await;
}

/// The dispatch arm passes the action through: `favorite` of an unknown item fails the same
/// way as `set`, with no download attempted.
#[tokio::test]
async fn browse_use_dispatches_every_action() {
    let d = Daemon::start(Setup::default()).await;
    for action in [
        BrowseAction::Set,
        BrowseAction::Favorite,
        BrowseAction::Download,
    ] {
        let response = d
            .server
            .dispatch(Request::BrowseUse {
                source_id: "wh-1".to_owned(),
                remote_id: "gone".to_owned(),
                action,
            })
            .await;
        assert_eq!(
            (response.ok, response.error.as_str()),
            (false, "That preview expired — scroll or reopen to refresh."),
            "{action:?}"
        );
    }
    d.stop().await;
}
