//! UI ↔ daemon lifecycle (plan §C, Task 46).
//!
//! - At startup the UI starts the daemon when nothing answers on its socket
//!   ([`ensure_running`]). It never hosts the engine and never starts a second daemon: it runs
//!   `kakemono daemon` (no `-f`), which refuses while the daemon lock is held or the socket
//!   answers, otherwise detaches a daemon and waits up to 10 s for its socket. The `ensure_daemon`
//!   command (the "Start" button) is the same call.
//! - "Start at login" is two switches kept in step: the daemon's login service
//!   (`kakemono service install|uninstall`, read with `service status --json`) and the UI's own
//!   autostart entry (`tauri-plugin-autostart`). It reads as on only when both are on. A change
//!   that fails halfway undoes the half that succeeded.
//!
//! The `kakemono` CLI is found beside this executable (the bundle's sidecar), then on `PATH`.
//! It runs directly, never through a shell. Everything that touches a process, the socket or the
//! autostart store sits behind a trait ([`CliRunner`], [`DaemonProbe`], [`Autostart`]) so tests
//! use fakes.

use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_autostart::{AutoLaunchManager, ManagerExt as _};

use crate::daemon_link::DaemonLink;
use crate::error::{ErrorKind, Kinded, serialize_as_wire_error};

/// How long one CLI call may take. `kakemono daemon` waits up to 10 s for the new daemon's
/// socket; `service install` may wait on the service manager.
pub const CLI_TIMEOUT: Duration = Duration::from_secs(20);

// ── Errors ──────────────────────────────────────────────────────────────────

/// A failed lifecycle command, serialised for the frontend as `{ "kind", "message" }` like
/// `commands::CommandError`.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error(
        "cannot find the kakemono command-line tool next to kakemono-ui or on PATH; reinstall \
         kakemono or put `kakemono` on your PATH"
    )]
    CliNotFound,
    /// The CLI ran and exited non-zero; `message` is its stderr.
    #[error("`kakemono {action}` failed: {message}")]
    CliFailed { action: String, message: String },
    #[error("`kakemono {action}` printed unexpected output: {source}")]
    CliOutput {
        action: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot run `kakemono {action}`: {source}")]
    Io {
        action: String,
        #[source]
        source: io::Error,
    },
    #[error("start at login (kakemono-ui): {0}")]
    Autostart(String),
    #[error("the daemon started but does not answer on {0}")]
    NotAnswering(String),
    /// A step failed after an earlier one succeeded and the earlier one could not be undone.
    #[error("{error} (undoing the earlier step also failed: {undo})")]
    UndoFailed {
        error: Box<LifecycleError>,
        undo: Box<LifecycleError>,
    },
}

impl Kinded for LifecycleError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::CliNotFound => ErrorKind::CliNotFound,
            Self::CliFailed { .. } => ErrorKind::CliFailed,
            Self::CliOutput { .. } => ErrorKind::CliOutput,
            Self::Io { .. } => ErrorKind::Io,
            Self::Autostart(_) => ErrorKind::Autostart,
            Self::NotAnswering(_) => ErrorKind::Daemon,
            Self::UndoFailed { error, .. } => error.kind(),
        }
    }
}

serialize_as_wire_error!(LifecycleError);

type Result<T, E = LifecycleError> = std::result::Result<T, E>;

// ── Running the CLI ─────────────────────────────────────────────────────────

/// Finds `kakemono`: in `exe_dirs` (this executable's directory, then its resolved one), then
/// on `path_env`. Empty `PATH` entries (the cwd) are skipped.
pub fn locate_cli(
    exe_dirs: &[PathBuf],
    path_env: Option<&OsStr>,
    is_file: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let name = format!("kakemono{}", std::env::consts::EXE_SUFFIX);
    let on_path = path_env
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| !dir.as_os_str().is_empty());
    exe_dirs
        .iter()
        .cloned()
        .chain(on_path)
        .map(|dir| dir.join(&name))
        .find(|candidate| is_file(candidate))
}

/// [`locate_cli`] for this process.
pub fn find_cli() -> Option<PathBuf> {
    let mut exe_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let resolved = exe.canonicalize().ok();
        for dir in [Some(exe.as_path()), resolved.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(Path::parent)
        {
            if !exe_dirs.iter().any(|d| d == dir) {
                exe_dirs.push(dir.to_owned());
            }
        }
    }
    locate_cli(
        &exe_dirs,
        std::env::var_os("PATH").as_deref(),
        &Path::is_file,
    )
}

/// One CLI invocation: `program args…`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliCall {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl CliCall {
    fn new<I, S>(program: &Path, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            program: program.to_owned(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// The subcommand for messages: `service install`, `daemon`, … Flags, and the values of
    /// the global `--socket` / `--config`, are left out.
    fn action(&self) -> String {
        let mut words = Vec::new();
        let mut args = self.args.iter().map(|a| a.to_string_lossy());
        while let Some(arg) = args.next() {
            if arg == "--socket" || arg == "--config" {
                args.next();
            } else if !arg.starts_with('-') {
                words.push(arg);
            }
        }
        words.join(" ")
    }
}

/// What a finished CLI call reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs the CLI. [`ProcessRunner`] in production.
pub trait CliRunner: Sync {
    fn run(&self, call: CliCall) -> impl Future<Output = io::Result<CliOutput>> + Send;
}

/// Runs the CLI as a child process with stdin closed and stdout/stderr captured, killing it
/// after [`CLI_TIMEOUT`]. `kakemono daemon` exits once its detached daemon answers, so this
/// does not wait for the daemon itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessRunner;

impl CliRunner for ProcessRunner {
    async fn run(&self, call: CliCall) -> io::Result<CliOutput> {
        let mut cmd = tokio::process::Command::new(&call.program);
        cmd.args(&call.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // No console window flashing up from a GUI app.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let output = tokio::time::timeout(CLI_TIMEOUT, cmd.output())
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no answer after {} s", CLI_TIMEOUT.as_secs()),
                )
            })??;
        Ok(CliOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Runs `call` and turns a non-zero exit into [`LifecycleError::CliFailed`] carrying its stderr.
async fn run_checked(runner: &impl CliRunner, call: CliCall) -> Result<CliOutput> {
    let action = call.action();
    let output = runner
        .run(call)
        .await
        .map_err(|source| LifecycleError::Io {
            action: action.clone(),
            source,
        })?;
    if output.success {
        Ok(output)
    } else {
        let message = match output.stderr.trim() {
            "" => "it exited with an error and printed nothing".to_owned(),
            text => text.to_owned(),
        };
        Err(LifecycleError::CliFailed { action, message })
    }
}

// ── Starting the daemon ─────────────────────────────────────────────────────

/// Whether the daemon answers IPC. [`DaemonLink`] in production.
pub trait DaemonProbe: Sync {
    fn running(&self) -> impl Future<Output = bool> + Send;
}

impl DaemonProbe for DaemonLink {
    async fn running(&self) -> bool {
        self.status().await.is_ok()
    }
}

/// What [`ensure_running`] found or did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonStart {
    /// It answered already, or another start won the race and it answers now.
    AlreadyRunning,
    /// This call started it.
    Started,
}

/// Starts the daemon unless it answers: runs `kakemono --socket <socket> daemon`, so the daemon
/// binds where this UI connects. `kakemono daemon` refuses while another daemon holds the lock,
/// so a race with launchd, systemd or a second UI cannot produce two daemons.
pub async fn ensure_running(
    probe: &impl DaemonProbe,
    runner: &impl CliRunner,
    cli: Option<&Path>,
    socket: &Path,
) -> Result<DaemonStart> {
    if probe.running().await {
        return Ok(DaemonStart::AlreadyRunning);
    }
    let cli = cli.ok_or(LifecycleError::CliNotFound)?;
    let call = CliCall::new(
        cli,
        [
            OsStr::new("--socket"),
            socket.as_os_str(),
            OsStr::new("daemon"),
        ],
    );
    match run_checked(runner, call).await {
        Ok(_) if probe.running().await => Ok(DaemonStart::Started),
        Ok(_) => Err(LifecycleError::NotAnswering(socket.display().to_string())),
        // Refused because one started meanwhile: that is what we wanted.
        Err(_) if probe.running().await => Ok(DaemonStart::AlreadyRunning),
        Err(err) => Err(err),
    }
}

/// Startup hook: starts the daemon in the background if it does not answer. Failures are
/// logged; the settings window shows "Daemon not running" with a Start button.
pub fn spawn_at_startup(link: Arc<DaemonLink>) {
    tauri::async_runtime::spawn(async move {
        let socket = link.endpoint().path().to_owned();
        let cli = find_cli();
        match ensure_running(link.as_ref(), &ProcessRunner, cli.as_deref(), &socket).await {
            Ok(DaemonStart::AlreadyRunning) => tracing::debug!("daemon already running"),
            Ok(DaemonStart::Started) => {
                tracing::info!(socket = %socket.display(), "started the daemon")
            }
            Err(err) => tracing::warn!(error = %err, "cannot start the daemon"),
        }
    });
}

/// `ensure_daemon` → `true` once the daemon answers. Rejects with the CLI's stderr when it
/// could not be started.
#[tauri::command]
pub async fn ensure_daemon(link: State<'_, Arc<DaemonLink>>) -> Result<bool> {
    let link = link.inner().as_ref();
    let socket = link.endpoint().path().to_owned();
    let cli = find_cli();
    ensure_running(link, &ProcessRunner, cli.as_deref(), &socket).await?;
    Ok(true)
}

// ── Start at login ──────────────────────────────────────────────────────────

/// `kakemono service status --json` (the service contract): exit 0 and this object on stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct ServiceStatus {
    pub installed: bool,
    pub running: bool,
    /// Where the unit or plist lives (set even when not installed); `null` where there is no file (Windows).
    pub path: Option<String>,
}

/// Parses `service status --json` output. Unknown extra fields are ignored.
pub fn parse_service_status(stdout: &str) -> Result<ServiceStatus, serde_json::Error> {
    serde_json::from_str(stdout.trim())
}

/// The UI's own autostart entry. [`AutoLaunchManager`] (the plugin) in production.
pub trait Autostart: Sync {
    fn is_enabled(&self) -> Result<bool, String>;
    fn enable(&self) -> Result<(), String>;
    fn disable(&self) -> Result<(), String>;
}

impl Autostart for AutoLaunchManager {
    fn is_enabled(&self) -> Result<bool, String> {
        AutoLaunchManager::is_enabled(self).map_err(|e| e.to_string())
    }
    fn enable(&self) -> Result<(), String> {
        AutoLaunchManager::enable(self).map_err(|e| e.to_string())
    }
    fn disable(&self) -> Result<(), String> {
        AutoLaunchManager::disable(self).map_err(|e| e.to_string())
    }
}

/// `start_at_login` → both halves and the combined switch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ts_rs::TS)]
pub struct LoginItems {
    /// The switch: the daemon service is installed and the UI starts at login.
    pub enabled: bool,
    pub service: ServiceStatus,
    pub ui_autostart: bool,
}

async fn service_status(runner: &impl CliRunner, cli: &Path) -> Result<ServiceStatus> {
    let call = CliCall::new(cli, ["service", "status", "--json"]);
    let action = call.action();
    let output = run_checked(runner, call).await?;
    parse_service_status(&output.stdout)
        .map_err(|source| LifecycleError::CliOutput { action, source })
}

async fn service(runner: &impl CliRunner, cli: &Path, verb: &str) -> Result<()> {
    run_checked(runner, CliCall::new(cli, ["service", verb]))
        .await
        .map(drop)
}

fn autostart(result: Result<(), String>) -> Result<()> {
    result.map_err(LifecycleError::Autostart)
}

/// Reads both halves.
pub async fn login_items(
    runner: &impl CliRunner,
    cli: Option<&Path>,
    ui: &impl Autostart,
) -> Result<LoginItems> {
    let cli = cli.ok_or(LifecycleError::CliNotFound)?;
    let service = service_status(runner, cli).await?;
    let ui_autostart = ui.is_enabled().map_err(LifecycleError::Autostart)?;
    Ok(LoginItems {
        enabled: service.installed && ui_autostart,
        service,
        ui_autostart,
    })
}

/// Turns both halves on or off, skipping a half already in that state. On: install the service,
/// then enable the UI entry. Off: disable the UI entry, then uninstall the service. When the
/// second step fails, the first is undone. Returns the state read back afterwards.
pub async fn set_login_items(
    runner: &impl CliRunner,
    cli: Option<&Path>,
    ui: &impl Autostart,
    enabled: bool,
) -> Result<LoginItems> {
    let cli = cli.ok_or(LifecycleError::CliNotFound)?;
    let before = login_items(runner, Some(cli), ui).await?;
    let service_changes = before.service.installed != enabled;
    let ui_changes = before.ui_autostart != enabled;

    if enabled {
        if service_changes {
            service(runner, cli, "install").await?;
        }
        if ui_changes && let Err(error) = autostart(ui.enable()) {
            if service_changes {
                return Err(undo(error, service(runner, cli, "uninstall").await));
            }
            return Err(error);
        }
    } else {
        if ui_changes {
            autostart(ui.disable())?;
        }
        if service_changes && let Err(error) = service(runner, cli, "uninstall").await {
            if ui_changes {
                return Err(undo(error, autostart(ui.enable())));
            }
            return Err(error);
        }
    }
    login_items(runner, Some(cli), ui).await
}

/// `error` itself when the undo worked, else both.
fn undo(error: LifecycleError, undone: Result<()>) -> LifecycleError {
    match undone {
        Ok(()) => error,
        Err(undo) => LifecycleError::UndoFailed {
            error: Box::new(error),
            undo: Box::new(undo),
        },
    }
}

/// `start_at_login` → [`LoginItems`].
#[tauri::command]
pub async fn start_at_login<R: Runtime>(app: AppHandle<R>) -> Result<LoginItems> {
    let cli = find_cli();
    login_items(&ProcessRunner, cli.as_deref(), app.autolaunch().inner()).await
}

/// `set_start_at_login { enabled }` → the [`LoginItems`] read back afterwards.
#[tauri::command]
pub async fn set_start_at_login<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<LoginItems> {
    let cli = find_cli();
    set_login_items(
        &ProcessRunner,
        cli.as_deref(),
        app.autolaunch().inner(),
        enabled,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::collections::{HashSet, VecDeque};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    const CLI: &str = "/opt/k/kakemono";
    const SOCKET: &str = "/run/k.sock";

    /// Fixture: the service contract's `service status --json` output.
    const STATUS_INSTALLED: &str = r#"{"installed":true,"running":true,"path":"/Users/me/Library/LaunchAgents/dev.kakemono.daemon.plist"}"#;
    const STATUS_NOT_INSTALLED: &str = r#"{"installed":false,"running":false,"path":null}"#;

    /// Answers each call with the next scripted reply and records the argv.
    #[derive(Default)]
    struct FakeRunner {
        replies: Mutex<VecDeque<io::Result<CliOutput>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn with(replies: Vec<io::Result<CliOutput>>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                calls: Mutex::default(),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("calls").clone()
        }
    }

    impl CliRunner for FakeRunner {
        async fn run(&self, call: CliCall) -> io::Result<CliOutput> {
            let mut argv = vec![call.program.display().to_string()];
            argv.extend(call.args.iter().map(|a| a.to_string_lossy().into_owned()));
            self.calls.lock().expect("calls").push(argv.join(" "));
            self.replies
                .lock()
                .expect("replies")
                .pop_front()
                .expect("an unexpected CLI call")
        }
    }

    fn ok(stdout: &str) -> io::Result<CliOutput> {
        Ok(CliOutput {
            success: true,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    }

    fn fail(stderr: &str) -> io::Result<CliOutput> {
        Ok(CliOutput {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_owned(),
        })
    }

    /// Answers `running()` from a script; repeats the last answer.
    struct FakeProbe(Mutex<VecDeque<bool>>);

    impl FakeProbe {
        fn new(answers: &[bool]) -> Self {
            Self(Mutex::new(answers.iter().copied().collect()))
        }
    }

    impl DaemonProbe for FakeProbe {
        async fn running(&self) -> bool {
            let mut q = self.0.lock().expect("probe");
            if q.len() > 1 {
                q.pop_front().expect("answer")
            } else {
                *q.front().expect("answer")
            }
        }
    }

    struct FakeAutostart {
        enabled: AtomicBool,
        fail_enable: Option<&'static str>,
        fail_disable: Option<&'static str>,
    }

    impl FakeAutostart {
        fn new(enabled: bool) -> Self {
            Self {
                enabled: AtomicBool::new(enabled),
                fail_enable: None,
                fail_disable: None,
            }
        }
        fn get(&self) -> bool {
            self.enabled.load(Ordering::SeqCst)
        }
    }

    impl Autostart for FakeAutostart {
        fn is_enabled(&self) -> Result<bool, String> {
            Ok(self.get())
        }
        fn enable(&self) -> Result<(), String> {
            if let Some(msg) = self.fail_enable {
                return Err(msg.to_owned());
            }
            self.enabled.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn disable(&self) -> Result<(), String> {
            if let Some(msg) = self.fail_disable {
                return Err(msg.to_owned());
            }
            self.enabled.store(false, Ordering::SeqCst);
            Ok(())
        }
    }

    fn cli() -> Option<&'static Path> {
        Some(Path::new(CLI))
    }

    // ── locate_cli ──

    fn exe(dir: &str) -> PathBuf {
        PathBuf::from(dir).join(format!("kakemono{}", std::env::consts::EXE_SUFFIX))
    }

    fn files<const N: usize>(paths: [PathBuf; N]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = paths.into_iter().collect();
        move |p: &Path| set.contains(p)
    }

    #[test]
    fn cli_beside_the_ui_wins_over_path() {
        let dirs = [PathBuf::from("/app/MacOS"), PathBuf::from("/real/MacOS")];
        let path = std::env::join_paths(["/usr/local/bin", "/usr/bin"]).expect("PATH");
        let fs = files([exe("/real/MacOS"), exe("/usr/bin"), exe("/app/MacOS")]);
        assert_eq!(locate_cli(&dirs, Some(&path), &fs), Some(exe("/app/MacOS")));
        let fs = files([exe("/real/MacOS"), exe("/usr/bin")]);
        assert_eq!(
            locate_cli(&dirs, Some(&path), &fs),
            Some(exe("/real/MacOS"))
        );
        let fs = files([exe("/usr/bin")]);
        assert_eq!(locate_cli(&dirs, Some(&path), &fs), Some(exe("/usr/bin")));
        assert_eq!(locate_cli(&dirs, Some(&path), &files([])), None);
    }

    #[test]
    fn empty_path_entries_are_not_the_cwd() {
        let path = std::env::join_paths(["", "/usr/bin"]).expect("PATH");
        let fs = files([exe(""), exe("/usr/bin")]);
        assert_eq!(locate_cli(&[], Some(&path), &fs), Some(exe("/usr/bin")));
    }

    // ── ensure_daemon ──

    #[tokio::test]
    async fn a_running_daemon_is_left_alone() {
        let runner = FakeRunner::default();
        let got = ensure_running(&FakeProbe::new(&[true]), &runner, cli(), Path::new(SOCKET)).await;
        assert_eq!(got.expect("ok"), DaemonStart::AlreadyRunning);
        assert!(runner.calls().is_empty(), "no CLI call");
    }

    #[tokio::test]
    async fn starts_the_daemon_on_this_uis_socket() {
        let runner = FakeRunner::with(vec![ok("kakemono daemon started (pid 7)\n")]);
        let probe = FakeProbe::new(&[false, true]);
        let got = ensure_running(&probe, &runner, cli(), Path::new(SOCKET)).await;
        assert_eq!(got.expect("ok"), DaemonStart::Started);
        assert_eq!(runner.calls(), [format!("{CLI} --socket {SOCKET} daemon")]);
    }

    #[tokio::test]
    async fn a_refusal_because_one_started_meanwhile_is_success() {
        let runner = FakeRunner::with(vec![fail(
            "error: daemon is already running (pid 9, lock: /run/daemon.lock)\n",
        )]);
        let probe = FakeProbe::new(&[false, true]);
        let got = ensure_running(&probe, &runner, cli(), Path::new(SOCKET)).await;
        assert_eq!(got.expect("ok"), DaemonStart::AlreadyRunning);
    }

    #[tokio::test]
    async fn a_refusal_with_nothing_answering_reports_stderr() {
        let runner = FakeRunner::with(vec![fail(
            "error: daemon is already running (pid 9, lock: /run/daemon.lock)\n",
        )]);
        let err = ensure_running(&FakeProbe::new(&[false]), &runner, cli(), Path::new(SOCKET))
            .await
            .expect_err("refused");
        assert_eq!(err.kind(), ErrorKind::CliFailed);
        assert_eq!(
            err.to_string(),
            "`kakemono daemon` failed: error: daemon is already running (pid 9, lock: \
             /run/daemon.lock)"
        );
    }

    #[tokio::test]
    async fn success_without_an_answer_is_an_error() {
        let runner = FakeRunner::with(vec![ok("")]);
        let err = ensure_running(&FakeProbe::new(&[false]), &runner, cli(), Path::new(SOCKET))
            .await
            .expect_err("silent daemon");
        assert_eq!(err.kind(), ErrorKind::Daemon);
    }

    #[tokio::test]
    async fn no_cli_no_spawn() {
        let runner = FakeRunner::default();
        let err = ensure_running(&FakeProbe::new(&[false]), &runner, None, Path::new(SOCKET))
            .await
            .expect_err("no CLI");
        assert_eq!(err.kind(), ErrorKind::CliNotFound);
        assert!(runner.calls().is_empty());
    }

    #[tokio::test]
    async fn a_spawn_error_names_the_action() {
        let runner = FakeRunner::with(vec![Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "denied",
        ))]);
        let err = ensure_running(&FakeProbe::new(&[false]), &runner, cli(), Path::new(SOCKET))
            .await
            .expect_err("spawn fails");
        assert_eq!(err.to_string(), "cannot run `kakemono daemon`: denied");
        assert_eq!(err.kind(), ErrorKind::Io);
    }

    // ── service status parsing ──

    #[test]
    fn parses_the_service_status_fixture() {
        assert_eq!(
            parse_service_status(&format!("{STATUS_INSTALLED}\n")).expect("parse"),
            ServiceStatus {
                installed: true,
                running: true,
                path: Some("/Users/me/Library/LaunchAgents/dev.kakemono.daemon.plist".into()),
            }
        );
        assert_eq!(
            parse_service_status(STATUS_NOT_INSTALLED).expect("parse"),
            ServiceStatus {
                installed: false,
                running: false,
                path: None,
            }
        );
    }

    /// The lines lane-service's `service status --json` prints (its insta snapshot): `path` is
    /// set even when not installed and `null` on Windows.
    #[test]
    fn parses_every_line_the_service_command_prints() {
        let lines = [
            (
                r#"{"installed":false,"running":false,"path":"/h/.config/systemd/user/kakemono.service"}"#,
                false,
                false,
                true,
            ),
            (
                r#"{"installed":true,"running":true,"path":"/h/.config/systemd/user/kakemono.service"}"#,
                true,
                true,
                true,
            ),
            (
                r#"{"installed":true,"running":false,"path":"/h/.config/systemd/user/kakemono.service"}"#,
                true,
                false,
                true,
            ),
            (
                r#"{"installed":true,"running":true,"path":null}"#,
                true,
                true,
                false,
            ),
        ];
        for (line, installed, running, has_path) in lines {
            let status = parse_service_status(line).expect(line);
            assert_eq!(
                (status.installed, status.running, status.path.is_some()),
                (installed, running, has_path),
                "{line}"
            );
        }
    }

    #[test]
    fn extra_fields_are_ignored_but_missing_ones_are_errors() {
        let extra = r#"{"installed":false,"running":false,"path":null,"manager":"launchd"}"#;
        assert!(parse_service_status(extra).is_ok());
        assert!(parse_service_status(r#"{"installed":true}"#).is_err());
        assert!(parse_service_status("installed").is_err());
    }

    // ── login items ──

    #[tokio::test]
    async fn on_only_when_both_halves_are_on() {
        for (status, ui, want) in [
            (STATUS_INSTALLED, true, true),
            (STATUS_INSTALLED, false, false),
            (STATUS_NOT_INSTALLED, true, false),
        ] {
            let runner = FakeRunner::with(vec![ok(status)]);
            let items = login_items(&runner, cli(), &FakeAutostart::new(ui))
                .await
                .expect("read");
            assert_eq!(items.enabled, want, "{status} ui={ui}");
            assert_eq!(runner.calls(), [format!("{CLI} service status --json")]);
        }
    }

    #[tokio::test]
    async fn unreadable_status_output_is_reported() {
        let runner = FakeRunner::with(vec![ok("not json")]);
        let err = login_items(&runner, cli(), &FakeAutostart::new(false))
            .await
            .expect_err("bad output");
        assert_eq!(err.kind(), ErrorKind::CliOutput);
        assert!(
            err.to_string()
                .starts_with("`kakemono service status` printed unexpected output")
        );
    }

    #[tokio::test]
    async fn enabling_installs_then_enables() {
        let runner = FakeRunner::with(vec![ok(STATUS_NOT_INSTALLED), ok(""), ok(STATUS_INSTALLED)]);
        let ui = FakeAutostart::new(false);
        let items = set_login_items(&runner, cli(), &ui, true)
            .await
            .expect("on");
        assert!(items.enabled);
        assert!(ui.get());
        assert_eq!(
            runner.calls(),
            [
                format!("{CLI} service status --json"),
                format!("{CLI} service install"),
                format!("{CLI} service status --json"),
            ]
        );
    }

    #[tokio::test]
    async fn enabling_skips_a_half_already_on() {
        let runner = FakeRunner::with(vec![ok(STATUS_INSTALLED), ok(STATUS_INSTALLED)]);
        let ui = FakeAutostart::new(false);
        let items = set_login_items(&runner, cli(), &ui, true)
            .await
            .expect("on");
        assert!(items.enabled);
        assert_eq!(runner.calls().len(), 2, "no install: {:?}", runner.calls());
    }

    #[tokio::test]
    async fn a_failed_install_changes_nothing() {
        let runner = FakeRunner::with(vec![
            ok(STATUS_NOT_INSTALLED),
            fail("error: launchctl bootstrap failed: 5\n"),
        ]);
        let ui = FakeAutostart::new(false);
        let err = set_login_items(&runner, cli(), &ui, true)
            .await
            .expect_err("install fails");
        assert_eq!(
            err.to_string(),
            "`kakemono service install` failed: error: launchctl bootstrap failed: 5"
        );
        assert!(!ui.get(), "UI autostart untouched");
    }

    #[tokio::test]
    async fn a_failed_ui_enable_uninstalls_the_service_again() {
        let runner = FakeRunner::with(vec![ok(STATUS_NOT_INSTALLED), ok(""), ok("")]);
        let ui = FakeAutostart {
            fail_enable: Some("LaunchAgents is not writable"),
            ..FakeAutostart::new(false)
        };
        let err = set_login_items(&runner, cli(), &ui, true)
            .await
            .expect_err("enable fails");
        assert_eq!(
            err.to_string(),
            "start at login (kakemono-ui): LaunchAgents is not writable"
        );
        assert_eq!(
            runner.calls(),
            [
                format!("{CLI} service status --json"),
                format!("{CLI} service install"),
                format!("{CLI} service uninstall"),
            ]
        );
    }

    #[tokio::test]
    async fn a_service_installed_before_is_not_undone() {
        let runner = FakeRunner::with(vec![ok(STATUS_INSTALLED)]);
        let ui = FakeAutostart {
            fail_enable: Some("nope"),
            ..FakeAutostart::new(false)
        };
        set_login_items(&runner, cli(), &ui, true)
            .await
            .expect_err("enable fails");
        assert_eq!(runner.calls(), [format!("{CLI} service status --json")]);
    }

    #[tokio::test]
    async fn a_failed_undo_reports_both() {
        let runner = FakeRunner::with(vec![
            ok(STATUS_NOT_INSTALLED),
            ok(""),
            fail("error: launchctl bootout failed\n"),
        ]);
        let ui = FakeAutostart {
            fail_enable: Some("nope"),
            ..FakeAutostart::new(false)
        };
        let err = set_login_items(&runner, cli(), &ui, true)
            .await
            .expect_err("both fail");
        assert_eq!(err.kind(), ErrorKind::Autostart);
        assert_eq!(
            err.to_string(),
            "start at login (kakemono-ui): nope (undoing the earlier step also failed: \
             `kakemono service uninstall` failed: error: launchctl bootout failed)"
        );
    }

    #[tokio::test]
    async fn disabling_turns_off_both() {
        let runner = FakeRunner::with(vec![ok(STATUS_INSTALLED), ok(""), ok(STATUS_NOT_INSTALLED)]);
        let ui = FakeAutostart::new(true);
        let items = set_login_items(&runner, cli(), &ui, false)
            .await
            .expect("off");
        assert!(!items.enabled);
        assert!(!ui.get());
        assert_eq!(runner.calls()[1], format!("{CLI} service uninstall"));
    }

    #[tokio::test]
    async fn a_failed_uninstall_re_enables_the_ui() {
        let runner = FakeRunner::with(vec![ok(STATUS_INSTALLED), fail("permission denied\n")]);
        let ui = FakeAutostart::new(true);
        let err = set_login_items(&runner, cli(), &ui, false)
            .await
            .expect_err("uninstall fails");
        assert_eq!(
            err.to_string(),
            "`kakemono service uninstall` failed: permission denied"
        );
        assert!(ui.get(), "UI autostart restored");
    }

    #[tokio::test]
    async fn an_empty_stderr_still_explains() {
        let runner = FakeRunner::with(vec![ok(STATUS_NOT_INSTALLED), fail("")]);
        let err = set_login_items(&runner, cli(), &FakeAutostart::new(false), true)
            .await
            .expect_err("install fails");
        assert_eq!(
            err.to_string(),
            "`kakemono service install` failed: it exited with an error and printed nothing"
        );
    }

    /// The real runner against a stand-in script: argv passed directly (no shell around it),
    /// both streams captured, the exit status kept.
    #[cfg(unix)]
    #[tokio::test]
    async fn process_runner_captures_output_and_status() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("kakemono");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"out $*\"\necho 'err line' >&2\nexit 3\n",
        )
        .expect("write");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let out = ProcessRunner
            .run(CliCall::new(&script, ["service", "status", "--json"]))
            .await
            .expect("ran");
        assert_eq!(
            out,
            CliOutput {
                success: false,
                stdout: "out service status --json\n".into(),
                stderr: "err line\n".into(),
            }
        );
    }

    #[test]
    fn errors_serialise_as_kind_and_message() {
        let json = serde_json::to_value(LifecycleError::CliNotFound).expect("json");
        assert_eq!(json["kind"], "cli_not_found");
        assert!(json["message"].as_str().expect("message").contains("PATH"));
    }
}
