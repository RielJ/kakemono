//! CLI parity (plan Task 35): the real `kakemono` binary (assert_cmd) against the Task 23 IPC
//! server running in this test process on a temp socket, over the fake platform and a static
//! source. HOME, XDG dirs, `KAKEMONO_CONFIG` and the socket all point into a temp dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use assert_cmd::Command;
use chrono::{DateTime, FixedOffset};
use kakemono_cli::daemon::lock::{self, LockGuard};
use kakemono_cli::daemon::server::{self, Server};
use kakemono_config::Config;
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine};
use kakemono_platform::api::{Platform, Theme};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_protocol::endpoint::Endpoint;
use kakemono_sources::fake::StaticProvider;
use kakemono_sources::registry::Registry;
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-03-04T05:06:07.890+08:00";

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../protocol/tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

struct Daemon {
    _dir: TempDir,
    root: PathBuf,
    socket: PathBuf,
    setter: RecordingSetter,
    engine: Engine,
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
    _lock: LockGuard,
}

impl Daemon {
    /// The binary with every location inside the temp dir and `--socket` unset (tests pass it
    /// or `KAKEMONO_SOCKET`).
    fn bare_cmd(&self) -> Command {
        isolated_cmd(&self.root)
    }

    /// The binary talking to this daemon through `--socket`.
    fn cmd(&self) -> Command {
        let mut cmd = self.bare_cmd();
        cmd.arg("--socket").arg(&self.socket);
        cmd
    }

    fn redact(&self, text: &str) -> String {
        text.replace(self.root.to_str().unwrap(), "[ROOT]")
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

/// `kakemono` with HOME, XDG dirs and `KAKEMONO_CONFIG` inside `root`, and no inherited
/// `KAKEMONO_SOCKET`. Never reaches the real config or socket.
fn isolated_cmd(root: &Path) -> Command {
    let mut cmd = Command::cargo_bin("kakemono").unwrap();
    cmd.current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_RUNTIME_DIR", root.join("xdg-run"))
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .env("XDG_CACHE_HOME", root.join("xdg-cache"))
        .env("KAKEMONO_CONFIG", root.join("config.toml"))
        .env_remove("KAKEMONO_SOCKET")
        .env_remove("VISUAL")
        .env_remove("EDITOR");
    cmd
}

fn config(root: &Path, interval: &str) -> Config {
    let mut cfg = Config::default();
    cfg.daemon.download_dir = root.join("downloads");
    cfg.daemon.trash_dir = root.join("trash");
    cfg.favorites.dir = root.join("favorites");
    cfg.daemon.interval = interval.to_owned();
    cfg.light.sources = vec!["local".to_owned()];
    cfg.dark.sources = vec!["local".to_owned()];
    cfg
}

/// A daemon on `<tmp>/run/k.sock` whose `next` always returns `<tmp>/one.jpg`.
async fn start_with(state: State, theme: Theme, interval: &str) -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let image = root.join("one.jpg");
    std::fs::write(&image, b"fake image data").unwrap();
    let registry = Registry::new().with(Arc::new(StaticProvider::new(
        "local",
        vec![StaticProvider::result("local", image.to_str().unwrap())],
    )));
    let setter = RecordingSetter::new();
    let platform = Platform {
        setter: Arc::new(setter.clone()),
        signals: Arc::new(ManualSignals::new(theme)),
    };
    let now: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(NOW).unwrap();
    let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
        .with_registry(registry)
        .with_clock(move || now);
    let cfg = config(&root, interval);
    let engine = Engine::new(cfg.clone(), state, root.join("state.json"), deps).unwrap();

    let lock = lock::acquire(&root.join("run")).unwrap();
    let socket = root.join("run/k.sock");
    let listener = server::bind(&Endpoint::new(&socket).unwrap(), &lock)
        .await
        .unwrap();
    let srv = Server::new(engine.clone(), move || Ok(cfg.clone()));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(srv.serve(listener, shutdown.clone()));
    Daemon {
        _dir: dir,
        root,
        socket,
        setter,
        engine,
        shutdown,
        task: Some(task),
        _lock: lock,
    }
}

async fn start() -> Daemon {
    start_with(State::new(), Theme::Light, "30m").await
}

/// The Go golden state, with `history_index` 2, so `status` equals `status_json.txt`.
fn golden_state() -> State {
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("state_populated.json")).unwrap();
    value["history_index"] = 2.into();
    State::from_json_slice(value.to_string().as_bytes()).unwrap()
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs the (blocking) binary off the async workers so the in-process server keeps serving.
async fn run(mut cmd: Command) -> Output {
    let out = tokio::task::spawn_blocking(move || cmd.timeout(Duration::from_secs(30)).output())
        .await
        .unwrap()
        .unwrap();
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

async fn run_args(d: &Daemon, args: &[&str]) -> Output {
    let mut cmd = d.cmd();
    cmd.args(args);
    run(cmd).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_json_equals_go_fixture() {
    let d = start_with(golden_state(), Theme::Dark, "1h30m").await;
    let out = run_args(&d, &["status", "--json"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let want = fixture("status_json.txt");
    let got: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    let want_value: serde_json::Value = serde_json::from_str(&want).unwrap();
    assert_eq!(got, want_value);
    // Byte-for-byte too: sorted keys, two-space indent, Go escaping, trailing newline.
    assert_eq!(out.stdout.trim_end(), want.trim_end());
    assert!(out.stdout.ends_with("}\n"));
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_text_matches_go_layout() {
    let d = start_with(golden_state(), Theme::Dark, "1h30m").await;
    let out = run_args(&d, &["status"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    insta::assert_snapshot!("status_text_golden", out.stdout);

    let empty = start().await;
    let out = run_args(&empty, &["status"]).await;
    assert_eq!(
        out.stdout,
        "Daemon:   running\nTheme:    light\nInterval: 30m\n\
         History:  0 entries (index -1)\nCurrent:  (none)\n"
    );
    empty.stop().await;
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_client_command_prints_go_text() {
    let d = start().await;
    let mut transcript = String::new();
    let image = d.root.join("two.jpg");
    std::fs::write(&image, b"fake image data").unwrap();
    for args in [
        vec!["next"],
        vec!["set", "two.jpg"],
        vec!["prev"],
        vec!["favorite"],
        vec!["pause"],
        vec!["resume"],
        vec!["trash"],
    ] {
        let out = run_args(&d, &args).await;
        transcript.push_str(&format!(
            "$ kakemono {}\n[exit {}]\n{}",
            args.join(" "),
            out.code,
            out.stdout
        ));
        if !out.stderr.is_empty() {
            transcript.push_str(&format!("[stderr]\n{}", out.stderr));
        }
    }
    insta::assert_snapshot!("client_commands", d.redact(&transcript));
    // `set two.jpg` from the temp dir sent an absolute path.
    assert!(
        d.setter.calls().iter().any(|(p, _)| p == &image),
        "{:?}",
        d.setter.calls()
    );
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_error_exits_1_with_message() {
    let d = start().await;
    let out = run_args(&d, &["prev"]).await;
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    assert_eq!(
        out.stderr,
        "daemon error: manager: already at the beginning of history\n"
    );

    let out = run_args(&d, &["set", "missing.jpg"]).await;
    assert_eq!(out.code, 1);
    let want = format!(
        "daemon error: manager: file not found: stat {}: no such file or directory\n",
        d.root.join("missing.jpg").display()
    );
    assert_eq!(out.stderr, want);
    d.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socket_from_env_and_config() {
    let d = start().await;
    // $KAKEMONO_SOCKET
    let mut cmd = d.bare_cmd();
    cmd.env("KAKEMONO_SOCKET", &d.socket).arg("pause");
    let out = run(cmd).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(d.engine.is_paused());

    // config `socket_path` beats $KAKEMONO_SOCKET.
    std::fs::write(
        d.root.join("config.toml"),
        format!("[daemon]\nsocket_path = {:?}\n", d.socket.to_str().unwrap()),
    )
    .unwrap();
    let mut cmd = d.bare_cmd();
    cmd.env("KAKEMONO_SOCKET", d.root.join("nope.sock"))
        .arg("resume");
    let out = run(cmd).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "▶  Auto-rotation resumed.\n");
    assert!(!d.engine.is_paused());
    d.stop().await;
}

#[tokio::test]
async fn daemon_not_running_message() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let socket = root.join("nobody.sock");
    let mut cmd = isolated_cmd(&root);
    cmd.arg("--socket").arg(&socket).arg("status");
    let out = run(cmd).await;
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    let want = format!(
        "kakemono daemon is not running.\nStart it with:  kakemono daemon\n\
         (original error: cannot connect to daemon socket {:?}: ",
        socket.display()
    );
    assert!(out.stderr.starts_with(&want), "{}", out.stderr);
    assert!(out.stderr.ends_with(")\n"), "{}", out.stderr);
}

#[tokio::test]
async fn config_path_and_init() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut cmd = isolated_cmd(&root);
    cmd.args(["config", "path"]);
    let out = run(cmd).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        format!("{}\n", root.join("config.toml").display())
    );

    let mut cmd = isolated_cmd(&root);
    cmd.args(["config", "init"]);
    let out = run(cmd).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(root.join("config.toml").exists());

    let mut cmd = isolated_cmd(&root);
    cmd.args(["config", "init"]);
    let out = run(cmd).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("already exists"), "{}", out.stderr);
}

#[tokio::test]
async fn completions_and_unavailable_commands() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut cmd = isolated_cmd(&root);
    cmd.args(["completions", "zsh"]);
    let out = run(cmd).await;
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("#compdef kakemono"));

    let mut cmd = isolated_cmd(&root);
    cmd.args(["pick"]);
    let out = run(cmd).await;
    assert_eq!(out.code, 1);
    assert_eq!(
        out.stderr,
        "`kakemono pick` is not available in this build yet\n"
    );

    // No subcommand: help on stderr, usage exit code.
    let out = run(isolated_cmd(&root)).await;
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("Usage: kakemono"), "{}", out.stderr);
}
