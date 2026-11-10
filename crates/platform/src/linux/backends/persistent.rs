//! Long-running setters (swaybg, wbg, `persistent = true` commands), plan §E.
//!
//! A persistent setter shows the wallpaper for as long as it runs. Each `set` starts a new
//! instance in its own process group, waits [`SETTLE`] so the new surface is up (no black flash),
//! then terminates the previous one. The current PID and argv are kept in
//! `<runtime_dir>/setter.pid`, and a new [`Supervisor`] adopts that process on daemon start.
//! Before signalling a recorded PID, the supervisor checks the process name still matches argv0,
//! so a reused PID is never killed.
//!
//! All OS access goes through [`ProcessControl`]. The logic is tested on every OS with a fake.
//! [`NativeProcesses`] is the real implementation: `/proc` and `nix` on Linux. On other Unixes it
//! is test-only plumbing on `std::process::Child`; it never ships, because Linux backends only run
//! on Linux.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kakemono_config::schema::Fit;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::api::{BackendInfo, CommandStatus, SetError, Theme, WallpaperSetter};
use crate::linux::backends::command::{Template, ThemeSource, shell_words_join};
use crate::linux::detect::resolve::Resolution;

/// How long the new instance runs before the old one is terminated.
pub const SETTLE: Duration = Duration::from_millis(300);
/// How long a terminated setter gets after SIGTERM before SIGKILL.
pub const TERM_GRACE: Duration = Duration::from_secs(2);
/// The PID file's name inside the runtime dir.
pub const PID_FILE: &str = "setter.pid";
/// The first restart delay after a setter dies; doubles per consecutive restart.
pub const RESTART_BASE: Duration = Duration::from_secs(1);
/// The longest restart delay.
pub const RESTART_MAX: Duration = Duration::from_secs(60);
/// A restarted setter that stays up this long resets the backoff.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);
/// How often [`Supervisor::watch`] checks the setter.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(2);
/// Linux keeps the first 15 bytes of the executable name as the process name (`TASK_COMM_LEN - 1`).
pub const COMM_LEN: usize = 15;

/// The process operations the supervisor needs.
#[async_trait::async_trait]
pub trait ProcessControl: Send + Sync + fmt::Debug + 'static {
    /// Start `argv` (no shell) in its own process group, detached from our stdio, and reap it
    /// when it exits. Returns its PID.
    fn spawn(&self, argv: &[String]) -> io::Result<u32>;

    /// The process name (Linux `/proc/<pid>/comm`), or `None` if `pid` is gone or a zombie.
    fn comm(&self, pid: u32) -> Option<String>;

    /// SIGTERM `pid` (its whole group if it leads one), wait up to `grace` for it to exit, then
    /// SIGKILL. Never fails: a process that is already gone is fine.
    async fn terminate(&self, pid: u32, grace: Duration);
}

/// The name Linux reports for a process started from `argv0`: its file name, cut to
/// [`COMM_LEN`] bytes.
pub fn expected_comm(argv0: &str) -> String {
    let name = Path::new(argv0).file_name().map_or_else(
        || argv0.as_bytes().to_vec(),
        |n| n.as_encoded_bytes().to_vec(),
    );
    let cut = &name[..name.len().min(COMM_LEN)];
    String::from_utf8_lossy(cut).into_owned()
}

/// What `setter.pid` holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub pid: u32,
    /// The argv it was started with. `argv[0]` gives the expected process name; the whole argv
    /// is what a restart runs again.
    pub argv: Vec<String>,
}

impl Record {
    fn argv0(&self) -> &str {
        self.argv.first().map_or("", String::as_str)
    }
}

/// What [`Supervisor::ensure_running`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    /// Nothing is supervised (no `set` yet, nothing adopted, or [`Supervisor::stop`]ped).
    Idle,
    /// The setter is running.
    Running { pid: u32 },
    /// The setter died and the next restart is not due yet.
    Waiting { until: Instant },
    /// The setter died and was started again.
    Restarted { pid: u32, attempt: u32 },
    /// The setter died and starting it again failed; it will be retried after the backoff.
    RestartFailed { attempt: u32, error: String },
}

#[derive(Debug)]
struct State {
    current: Option<Record>,
    /// Consecutive restarts since the last `replace` or stable run.
    restarts: u32,
    /// When the current instance started (for [`STABLE_AFTER`]).
    started_at: Instant,
    /// No restart before this.
    next_restart: Option<Instant>,
}

/// Owns at most one running persistent setter.
#[derive(Debug)]
pub struct Supervisor {
    control: Arc<dyn ProcessControl>,
    pid_file: PathBuf,
    settle: Duration,
    grace: Duration,
    state: tokio::sync::Mutex<State>,
}

/// The restart delay after `attempt` consecutive restarts (1-based): 1 s, 2 s, 4 s, ... up to
/// [`RESTART_MAX`].
pub fn backoff(attempt: u32) -> Duration {
    let factor = 1u32
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(u32::MAX);
    RESTART_BASE.saturating_mul(factor).min(RESTART_MAX)
}

impl Supervisor {
    /// A supervisor recording into `<runtime_dir>/setter.pid`. Adopts the recorded process if it
    /// is still running under the recorded name; otherwise starts with nothing.
    pub fn new(control: Arc<dyn ProcessControl>, runtime_dir: &Path) -> Self {
        let pid_file = runtime_dir.join(PID_FILE);
        let adopted = read_record(&pid_file).filter(|record| {
            let matches = alive_as(control.as_ref(), record);
            if !matches {
                tracing::debug!(
                    pid = record.pid,
                    "not adopting recorded setter: gone or PID reused"
                );
            }
            matches
        });
        if let Some(record) = &adopted {
            tracing::info!(
                pid = record.pid,
                argv0 = record.argv0(),
                "adopted running setter"
            );
        }
        Self {
            control,
            pid_file,
            settle: SETTLE,
            grace: TERM_GRACE,
            state: tokio::sync::Mutex::new(State {
                current: adopted,
                restarts: 0,
                started_at: Instant::now(),
                next_restart: None,
            }),
        }
    }

    /// Override [`SETTLE`] (tests).
    #[must_use]
    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }

    /// Override [`TERM_GRACE`] (tests).
    #[must_use]
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    pub fn pid_file(&self) -> &Path {
        &self.pid_file
    }

    /// The supervised setter, if any.
    pub async fn current(&self) -> Option<Record> {
        self.state.lock().await.current.clone()
    }

    /// Start `argv`, wait the settle time, record the new process, then terminate the previous
    /// setter.
    ///
    /// # Errors
    ///
    /// On any error the previous setter keeps running and stays recorded, and the new process
    /// (if one started) is stopped:
    /// - `SetError::Command` when the new process exits during the settle time, or runs under
    ///   another name than argv0's (a wrapper such as `env` or `sh -c exec`, which could never
    ///   be told apart from a reused PID later);
    /// - `SetError::Io` when `setter.pid` cannot be written.
    pub async fn replace(&self, argv: Vec<String>) -> Result<u32, SetError> {
        // Held across the settle sleep on purpose (a tokio mutex): concurrent sets queue, so two
        // replacements never race to terminate the same old instance.
        // pi-lens-ignore: rust-lock-held-across-await -- tokio mutex, held across the settle on purpose
        let mut state = self.state.lock().await;
        let pid = self.spawn(&argv)?;
        tokio::time::sleep(self.settle).await;
        let new = Record { pid, argv };
        let expected = expected_comm(new.argv0());
        let stderr = match self.control.comm(pid) {
            Some(comm) if comm == expected => None,
            Some(comm) => {
                // Our own fresh group leader, so stopping it cannot hit another program.
                self.control.terminate(pid, self.grace).await;
                Some(format!(
                    "runs as `{comm}`, expected `{expected}`; use the real binary as argv0"
                ))
            }
            None => Some(format!(
                "exited within {} ms; a persistent setter must keep running",
                self.settle.as_millis()
            )),
        };
        if let Some(stderr) = stderr {
            return Err(SetError::Command {
                argv0: new.argv0().to_owned(),
                status: CommandStatus::FailedToStart,
                stderr,
            });
        }
        // Record the new process before stopping the old one: if the record cannot be written,
        // the old setter is still running and still tracked.
        if let Err(err) = write_record(&self.pid_file, &new) {
            self.control.terminate(pid, self.grace).await;
            return Err(err.into());
        }
        if let Some(old) = state.current.take() {
            self.terminate_if_ours(&old, pid).await;
        }
        tracing::debug!(pid, argv = ?new.argv, "persistent setter replaced");
        state.current = Some(new);
        state.restarts = 0;
        state.next_restart = None;
        state.started_at = Instant::now();
        Ok(pid)
    }

    /// Check the setter and start it again if it died, with exponential backoff
    /// ([`backoff`]). A setter that stays up [`STABLE_AFTER`] resets the backoff.
    pub async fn ensure_running(&self) -> Health {
        let mut state = self.state.lock().await;
        let Some(record) = state.current.clone() else {
            return Health::Idle;
        };
        let now = Instant::now();
        if alive_as(self.control.as_ref(), &record) {
            if state.restarts > 0 && now.duration_since(state.started_at) >= STABLE_AFTER {
                state.restarts = 0;
                state.next_restart = None;
            }
            return Health::Running { pid: record.pid };
        }
        // The first death restarts at once; later ones wait out the backoff.
        if let Some(until) = state.next_restart
            && now < until
        {
            return Health::Waiting { until };
        }
        state.restarts += 1;
        let attempt = state.restarts;
        state.next_restart = Some(now + backoff(attempt));
        state.started_at = now;
        match self.spawn(&record.argv) {
            Ok(pid) => {
                let new = Record {
                    pid,
                    argv: record.argv,
                };
                if let Err(err) = write_record(&self.pid_file, &new) {
                    tracing::warn!(%err, "could not write {}", self.pid_file.display());
                }
                tracing::warn!(
                    old = record.pid,
                    pid,
                    attempt,
                    "persistent setter died; restarted"
                );
                state.current = Some(new);
                Health::Restarted { pid, attempt }
            }
            Err(err) => {
                tracing::warn!(%err, attempt, "persistent setter died; restart failed");
                Health::RestartFailed {
                    attempt,
                    error: err.to_string(),
                }
            }
        }
    }

    /// Call [`ensure_running`](Self::ensure_running) every `interval` until the task is aborted.
    pub fn watch(self: &Arc<Self>, interval: Duration) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                this.ensure_running().await;
            }
        })
    }

    /// Terminate the supervised setter and remove `setter.pid` (e.g. when switching to a
    /// non-persistent backend).
    pub async fn stop(&self) {
        let mut state = self.state.lock().await;
        if let Some(old) = state.current.take() {
            self.terminate_if_ours(&old, 0).await;
        }
        state.restarts = 0;
        state.next_restart = None;
        if let Err(err) = std::fs::remove_file(&self.pid_file)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(%err, "could not remove {}", self.pid_file.display());
        }
    }

    fn spawn(&self, argv: &[String]) -> Result<u32, SetError> {
        let argv0 = argv.first().ok_or_else(|| SetError::Unconfigured {
            hint: "`[desktop] set_command` is empty".to_owned(),
        })?;
        self.control
            .spawn(argv)
            .map_err(|source| SetError::Command {
                argv0: argv0.clone(),
                status: CommandStatus::FailedToStart,
                stderr: source.to_string(),
            })
    }

    /// Terminate `old` unless it is `keep` or its PID now belongs to another program.
    async fn terminate_if_ours(&self, old: &Record, keep: u32) {
        if old.pid == keep {
            return;
        }
        if alive_as(self.control.as_ref(), old) {
            self.control.terminate(old.pid, self.grace).await;
        } else {
            tracing::debug!(
                pid = old.pid,
                "previous setter already gone or PID reused; not signalling"
            );
        }
    }
}

/// True if `record.pid` is running under the name its argv0 gives.
fn alive_as(control: &dyn ProcessControl, record: &Record) -> bool {
    record.pid > 0
        && control
            .comm(record.pid)
            .is_some_and(|comm| comm == expected_comm(record.argv0()))
}

/// Read `setter.pid`. Missing or malformed files read as nothing.
pub fn read_record(path: &Path) -> Option<Record> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Record>(&text) {
        Ok(record) if record.pid > 0 && !record.argv.is_empty() => Some(record),
        Ok(_) => None,
        Err(err) => {
            tracing::debug!(%err, "ignoring malformed {}", path.display());
            None
        }
    }
}

/// Write `setter.pid` atomically (temp file + rename), creating the runtime dir if needed.
pub fn write_record(path: &Path, record: &Record) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string(record).map_err(io::Error::other)?;
    let tmp = path.with_extension("pid.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// A [`WallpaperSetter`] that renders its template per `set` and hands the argv to a
/// [`Supervisor`].
#[derive(Clone)]
pub struct PersistentSetter {
    name: String,
    template: Template,
    supervisor: Arc<Supervisor>,
    theme: ThemeSource,
}

impl fmt::Debug for PersistentSetter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersistentSetter")
            .field("name", &self.name)
            .field("template", &self.template)
            .field("supervisor", &self.supervisor)
            .finish_non_exhaustive()
    }
}

impl PersistentSetter {
    /// A setter for `template`, reported as `name`. Validates the template.
    pub fn new(
        name: impl Into<String>,
        template: Template,
        supervisor: Arc<Supervisor>,
    ) -> Result<Self, crate::exec::RenderError> {
        template.validate()?;
        Ok(Self {
            name: name.into(),
            template,
            supervisor,
            theme: Arc::new(|| Theme::Light),
        })
    }

    /// The setter for a persistent resolution (its `set_command`, else its preset). `Ok(None)`
    /// when it has no command; `Err` when `set_command` is invalid.
    pub fn for_resolution(
        resolution: &Resolution,
        supervisor: Arc<Supervisor>,
    ) -> Result<Option<Self>, crate::exec::RenderError> {
        Template::for_resolution(resolution)
            .map(|template| Self::new(resolution.backend.as_str(), template, supervisor))
            .transpose()
    }

    /// Where `{theme}` comes from (default: always light).
    #[must_use]
    pub fn with_theme(mut self, theme: ThemeSource) -> Self {
        self.theme = theme;
        self
    }

    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.supervisor
    }

    /// The exact argv `set(image, fit)` would start (for `doctor`'s dry run).
    pub fn render(&self, image: &Path, fit: Fit) -> Result<Vec<String>, SetError> {
        self.template.render(image, fit, (self.theme)())
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for PersistentSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        let argv = self.render(image, fit)?;
        self.supervisor.replace(argv).await?;
        Ok(())
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: self.name.clone(),
            detail: format!(
                "{} (kept running)",
                shell_words_join(&self.template.template(Fit::Fill))
            ),
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_impl::NativeProcesses;
#[cfg(not(target_os = "linux"))]
pub use std_impl::NativeProcesses;

#[cfg(target_os = "linux")]
mod linux_impl {
    use std::io;
    use std::time::Duration;

    use nix::sys::signal::{Signal, kill, killpg};
    use nix::unistd::{Pid, getpgid};

    use super::ProcessControl;
    use crate::exec::{CommandRunner, RunError, TokioRunner};

    /// The real process control: spawns through [`TokioRunner::spawn_detached`] (own process
    /// group, reaped on exit), reads `/proc/<pid>/stat`, signals with `nix`.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct NativeProcesses;

    impl NativeProcesses {
        pub fn new() -> Self {
            Self
        }

        fn signal(pid: u32, signal: Signal) {
            let Ok(raw) = i32::try_from(pid) else { return };
            let pid = Pid::from_raw(raw);
            // Signal the whole group when the setter leads one (wrappers like `sh -c`), else
            // just the process (an adopted PID from an older layout).
            let result = if getpgid(Some(pid)) == Ok(pid) {
                killpg(pid, signal)
            } else {
                kill(pid, signal)
            };
            if let Err(err) = result {
                tracing::debug!(%err, %pid, ?signal, "signal failed");
            }
        }
    }

    #[async_trait::async_trait]
    impl ProcessControl for NativeProcesses {
        fn spawn(&self, argv: &[String]) -> io::Result<u32> {
            TokioRunner.spawn_detached(argv).map_err(|err| match err {
                RunError::Spawn { source, .. } => source,
                RunError::EmptyCommand => io::Error::new(io::ErrorKind::InvalidInput, "empty argv"),
            })
        }

        fn comm(&self, pid: u32) -> Option<String> {
            let stat = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
            super::parse_stat(&stat)
        }

        async fn terminate(&self, pid: u32, grace: Duration) {
            Self::signal(pid, Signal::SIGTERM);
            let deadline = tokio::time::Instant::now() + grace;
            while tokio::time::Instant::now() < deadline {
                if self.comm(pid).is_none() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            if self.comm(pid).is_some() {
                tracing::warn!(pid, "setter ignored SIGTERM; sending SIGKILL");
                Self::signal(pid, Signal::SIGKILL);
            }
        }
    }
}

/// Parse `/proc/<pid>/stat` (`pid (comm) S ...`): the comm, or `None` for a zombie or dead task.
/// The comm can itself contain `)` and spaces, so it ends at the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat(stat: &[u8]) -> Option<String> {
    let open = stat.iter().position(|&b| b == b'(')?;
    let close = stat.iter().rposition(|&b| b == b')')?;
    let comm = stat.get(open + 1..close)?;
    let state = stat.get(close + 2).copied()?;
    if matches!(state, b'Z' | b'X' | b'x') {
        return None;
    }
    Some(String::from_utf8_lossy(comm).into_owned())
}

/// Test-only plumbing for non-Linux Unix hosts (developer Macs). Never ships: Linux backends only
/// run on Linux. It knows only the processes it spawned, kills with `Child::kill` (SIGKILL to the
/// direct child), and reports argv0's [`expected_comm`] as the name.
#[cfg(not(target_os = "linux"))]
mod std_impl {
    use std::collections::HashMap;
    use std::io;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use super::{ProcessControl, expected_comm};

    #[derive(Debug, Default)]
    pub struct NativeProcesses {
        children: Mutex<HashMap<u32, (Child, String)>>,
    }

    impl NativeProcesses {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait::async_trait]
    impl ProcessControl for NativeProcesses {
        fn spawn(&self, argv: &[String]) -> io::Result<u32> {
            let (argv0, args) = argv
                .split_first()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty argv"))?;
            let mut cmd = Command::new(argv0);
            cmd.args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
            let child = cmd.spawn()?;
            let pid = child.id();
            self.children
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(pid, (child, expected_comm(argv0)));
            Ok(pid)
        }

        fn comm(&self, pid: u32) -> Option<String> {
            let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
            let (child, name) = children.get_mut(&pid)?;
            match child.try_wait() {
                Ok(None) => Some(name.clone()),
                _ => None,
            }
        }

        async fn terminate(&self, pid: u32, _grace: Duration) {
            let entry = self
                .children
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&pid);
            if let Some((mut child, _)) = entry {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_comm_is_basename_cut_to_15_bytes() {
        assert_eq!(expected_comm("/usr/bin/swaybg"), "swaybg");
        assert_eq!(expected_comm("swaybg"), "swaybg");
        assert_eq!(
            expected_comm("/opt/x/a-very-long-setter-name"),
            "a-very-long-set"
        );
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
        assert_eq!(backoff(3), Duration::from_secs(4));
        assert_eq!(backoff(7), Duration::from_secs(60));
        assert_eq!(backoff(40), RESTART_MAX);
    }

    #[test]
    fn parse_stat_reads_comm_and_skips_zombies() {
        assert_eq!(
            parse_stat(b"42 (swaybg) S 1 42 42 0").as_deref(),
            Some("swaybg")
        );
        assert_eq!(parse_stat(b"42 (a) b) R 1 42").as_deref(), Some("a) b"));
        assert_eq!(parse_stat(b"42 (swaybg) Z 1 42"), None);
        assert_eq!(parse_stat(b"garbage"), None);
    }

    #[test]
    fn record_round_trips_and_rejects_junk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join(PID_FILE);
        let record = Record {
            pid: 7,
            argv: vec!["swaybg".into(), "-i".into(), "/a b".into()],
        };
        write_record(&path, &record).unwrap();
        assert_eq!(read_record(&path), Some(record));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_record(&path), None);
        std::fs::write(&path, r#"{"pid":0,"argv":["swaybg"]}"#).unwrap();
        assert_eq!(read_record(&path), None);
    }
}
