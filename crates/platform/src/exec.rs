//! Safe external command execution.
//!
//! - Commands are argv vectors, never run through a shell.
//! - [`render`] substitutes `{path}`, `{uri}`, `{fit}` and `{theme}` inside each token and never
//!   re-splits, so a path with spaces, quotes, `$()` or `;` stays one argv element.
//! - [`validate`] rejects unknown `{name}` placeholders (call it when loading config).
//! - [`CommandRunner`] runs an [`Invocation`] with a timeout and captured output. [`TokioRunner`]
//!   is the real one; `RecordingRunner` (feature `fake`) records argv/env for tests.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::api::{CommandStatus, Fit, SetError, Theme};

#[cfg(any(test, feature = "fake"))]
mod recording;
#[cfg(any(test, feature = "fake"))]
pub use recording::{FAKE_PID_BASE, RecordingRunner};

/// The placeholders `render` substitutes.
pub const PLACEHOLDERS: [&str; 4] = ["path", "uri", "fit", "theme"];

/// Timeout for non-persistent setter commands.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Max bytes kept per output stream; the rest is drained and dropped.
const MAX_CAPTURE: usize = 1024 * 1024;

/// Max bytes of stderr kept in `SetError::Command` (the tail is kept).
const MAX_ERROR_STDERR: usize = 4096;

/// How long to keep reading pipes after the child exited (a background grandchild may hold them).
const PIPE_DRAIN: Duration = Duration::from_millis(500);

/// Errors from building placeholder values or validating a command template.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error("command is empty")]
    EmptyCommand,
    #[error(
        "unknown placeholder {{{name}}} in {token:?} (known: {{path}}, {{uri}}, {{fit}}, {{theme}})"
    )]
    UnknownPlaceholder { token: String, name: String },
    #[error("image path must be absolute: {0}")]
    RelativePath(PathBuf),
    #[error("image path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
}

/// Errors from starting a command. A command that starts and then fails is an `Ok(Output)`.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("command is empty")]
    EmptyCommand,
    #[error("{argv0}: {source}")]
    Spawn {
        argv0: String,
        #[source]
        source: std::io::Error,
    },
}

impl From<RunError> for SetError {
    fn from(err: RunError) -> Self {
        match err {
            RunError::EmptyCommand => SetError::Unconfigured {
                hint: "`[desktop] set_command` is empty".to_owned(),
            },
            RunError::Spawn { argv0, source } => SetError::Command {
                argv0,
                status: CommandStatus::FailedToStart,
                stderr: source.to_string(),
            },
        }
    }
}

/// Placeholder values. Build with [`Vars::new`], which checks the path and computes `{uri}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vars {
    path: String,
    uri: String,
    fit: Fit,
    theme: Theme,
}

impl Vars {
    /// `path` must be absolute (so it can never be read as a flag) and UTF-8.
    pub fn new(path: &Path, fit: Fit, theme: Theme) -> Result<Self, RenderError> {
        if !path.is_absolute() {
            return Err(RenderError::RelativePath(path.to_path_buf()));
        }
        let path_str = path
            .to_str()
            .ok_or_else(|| RenderError::NonUtf8Path(path.to_path_buf()))?;
        let uri = url::Url::from_file_path(path)
            .map_err(|()| RenderError::RelativePath(path.to_path_buf()))?;
        Ok(Self {
            path: path_str.to_owned(),
            uri: uri.into(),
            fit,
            theme,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Percent-encoded `file://` URI of the path.
    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn fit(&self) -> Fit {
        self.fit
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }

    fn lookup(&self, name: &str) -> Option<&str> {
        match name {
            "path" => Some(&self.path),
            "uri" => Some(&self.uri),
            "fit" => Some(self.fit.as_str()),
            "theme" => Some(self.theme.as_str()),
            _ => None,
        }
    }
}

/// A `{name}` placeholder: braces around one or more ASCII letters, digits or `_`.
/// Anything else (`{}`, `{print $1}`) is literal text.
fn placeholders(token: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        while let Some(open) = token[pos..].find('{').map(|i| pos + i) {
            let rest = &token[open + 1..];
            let name_len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if name_len > 0 && rest[name_len..].starts_with('}') {
                let end = open + 1 + name_len + 1;
                pos = end;
                return Some((open, end, &rest[..name_len]));
            }
            pos = open + 1;
        }
        None
    })
}

/// Check a command template: non-empty, and every `{name}` is a known placeholder.
pub fn validate(spec: &[String]) -> Result<(), RenderError> {
    if spec.is_empty() {
        return Err(RenderError::EmptyCommand);
    }
    for token in spec {
        for (_, _, name) in placeholders(token) {
            if !PLACEHOLDERS.contains(&name) {
                return Err(RenderError::UnknownPlaceholder {
                    token: token.clone(),
                    name: name.to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Substitute placeholders inside each token. One input token always yields one output token.
/// Unknown placeholders are left as-is (`validate` reports them at config load).
pub fn render(spec: &[String], vars: &Vars) -> Vec<String> {
    spec.iter()
        .map(|token| {
            let mut out = String::with_capacity(token.len());
            let mut last = 0;
            for (start, end, name) in placeholders(token) {
                if let Some(value) = vars.lookup(name) {
                    out.push_str(&token[last..start]);
                    out.push_str(value);
                    last = end;
                }
            }
            out.push_str(&token[last..]);
            out
        })
        .collect()
}

/// One command run: argv, environment and timeout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub argv: Vec<String>,
    /// Variables set on the child (after clearing, if `clear_env`).
    pub env: Vec<(String, String)>,
    /// Start from an empty environment instead of inheriting the daemon's.
    pub clear_env: bool,
    pub timeout: Duration,
}

impl Invocation {
    /// Inherit the environment, with [`DEFAULT_TIMEOUT`].
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            env: Vec::new(),
            clear_env: false,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn envs<K: Into<String>, V: Into<String>>(
        mut self,
        vars: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.env
            .extend(vars.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    pub fn clear_env(mut self) -> Self {
        self.clear_env = true;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `argv[0]`, or `""` for an empty command.
    pub fn argv0(&self) -> &str {
        self.argv.first().map_or("", String::as_str)
    }
}

/// Options a setter applies to every command it runs: extra variables and the timeout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOptions {
    /// Variables set on each child, on top of the inherited environment.
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

impl Default for RunOptions {
    /// No extra variables, [`DEFAULT_TIMEOUT`].
    fn default() -> Self {
        Self {
            env: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl RunOptions {
    /// Set a variable on every run (e.g. `PATH` in tests).
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `invocation` with these variables added and this timeout.
    pub fn apply(&self, invocation: Invocation) -> Invocation {
        invocation
            .envs(self.env.iter().cloned())
            .timeout(self.timeout)
    }
}

/// The result of a command that started. `status` is never `FailedToStart`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub status: CommandStatus,
    /// Lossy UTF-8, capped at 1 MiB.
    pub stdout: String,
    /// Lossy UTF-8, capped at 1 MiB.
    pub stderr: String,
}

impl Output {
    /// Exit code 0 with no output.
    pub fn success() -> Self {
        Self {
            status: CommandStatus::Exited(0),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// Exit `code` with `stderr`.
    pub fn failure(code: i32, stderr: impl Into<String>) -> Self {
        Self {
            status: CommandStatus::Exited(code),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    /// `Ok(self)` on exit code 0, else `SetError::Command` with trimmed, capped stderr.
    pub fn check(self, argv0: &str) -> Result<Self, SetError> {
        if self.status.success() {
            return Ok(self);
        }
        Err(SetError::Command {
            argv0: argv0.to_owned(),
            status: self.status,
            stderr: tidy_stderr(&self.stderr),
        })
    }
}

/// `parts` as an owned argv.
pub(crate) fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

/// Trim and keep the last `MAX_ERROR_STDERR` bytes (on a char boundary).
pub(crate) fn tidy_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim();
    let mut start = trimmed.len().saturating_sub(MAX_ERROR_STDERR);
    while !trimmed.is_char_boundary(start) {
        start += 1;
    }
    trimmed[start..].to_owned()
}

/// Runs external commands. Object-safe: hold it as `Arc<dyn CommandRunner>`.
#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync + 'static {
    /// Run to completion or timeout (the child is killed on timeout and the status is
    /// `TimedOut`). `Err` only if the command could not be started.
    async fn run(&self, invocation: &Invocation) -> Result<Output, RunError>;

    /// Start a long-running command (swaybg-style) with null stdio, in its own process group on
    /// Unix (detached on Windows), inheriting the environment. Returns its PID. The child is
    /// reaped in the background when it exits.
    fn spawn_detached(&self, argv: &[String]) -> Result<u32, RunError>;
}

/// Run and turn a non-zero exit, timeout or spawn failure into `SetError::Command`.
pub async fn run_checked(
    runner: &dyn CommandRunner,
    invocation: &Invocation,
) -> Result<Output, SetError> {
    let output = runner.run(invocation).await?;
    output.check(invocation.argv0())
}

/// The real runner, on `tokio::process`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioRunner;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// A pipe being drained in the background into a shared, capped buffer.
struct Capture {
    buf: Arc<Mutex<Vec<u8>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Capture {
    fn start<R: AsyncRead + Unpin + Send + 'static>(reader: Option<R>) -> Self {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&buf);
        let task = tokio::spawn(async move {
            let Some(mut reader) = reader else { return };
            let mut chunk = [0u8; 8192];
            // Read errors just end the capture; the exit status is what matters.
            while let Ok(n @ 1..) = reader.read(&mut chunk).await {
                let mut buf = sink.lock().unwrap_or_else(PoisonError::into_inner);
                let room = MAX_CAPTURE.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(room)]);
            }
        });
        Self { buf, task }
    }

    /// Wait briefly for EOF (a background grandchild may hold the pipe open), then return what
    /// was read so far.
    async fn finish(self) -> String {
        let abort = self.task.abort_handle();
        if tokio::time::timeout(PIPE_DRAIN, self.task).await.is_err() {
            abort.abort();
        }
        let buf = self.buf.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&buf).into_owned()
    }
}

/// The process group a [`TokioRunner::run`] child leads (Unix). Dropping it while armed SIGKILLs
/// the group. Disarm it once the leader is reaped, because then its ID may be reused. Windows
/// has no groups here; there only the child itself is killed.
struct GroupKill(Option<u32>);

impl GroupKill {
    fn disarm(&mut self) {
        self.0 = None;
    }

    fn kill(&mut self) {
        if let Some(pgid) = self.0.take() {
            kill_group(pgid);
        }
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(unix)]
fn kill_group(pgid: u32) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    let Ok(raw) = i32::try_from(pgid) else { return };
    if let Err(err) = killpg(Pid::from_raw(raw), Signal::SIGKILL) {
        tracing::debug!(%err, pgid, "killpg failed");
    }
}

#[cfg(not(unix))]
fn kill_group(_pgid: u32) {}

fn status_of(status: std::process::ExitStatus) -> CommandStatus {
    status
        .code()
        .map_or(CommandStatus::Signaled, CommandStatus::Exited)
}

#[async_trait::async_trait]
impl CommandRunner for TokioRunner {
    async fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
        let (argv0, args) = invocation
            .argv
            .split_first()
            .ok_or(RunError::EmptyCommand)?;
        let mut cmd = tokio::process::Command::new(argv0);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if invocation.clear_env {
            cmd.env_clear();
        }
        cmd.envs(invocation.env.iter().map(|(k, v)| (k, v)));
        // Its own process group, so a timeout can kill wrapper commands' children too.
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd.spawn().map_err(|source| RunError::Spawn {
            argv0: argv0.clone(),
            source,
        })?;
        // Declared after `child`, so it drops first: a cancelled run kills the whole group while
        // the unreaped leader still holds the group ID, then `kill_on_drop` kills the leader.
        let mut group = GroupKill(child.id());
        let stdout = Capture::start(child.stdout.take());
        let stderr = Capture::start(child.stderr.take());

        let status = match tokio::time::timeout(invocation.timeout, child.wait()).await {
            Ok(Ok(status)) => {
                // Reaped: the group ID may be reused now. Background children it left on
                // purpose keep running.
                group.disarm();
                status_of(status)
            }
            // Waiting failed: the child is in an unknown state, so make sure it is gone.
            Ok(Err(_)) => {
                group.kill();
                let _ = child.kill().await;
                CommandStatus::Signaled
            }
            Err(_) => {
                // SIGKILL the group (Unix), then `kill` sends SIGKILL / TerminateProcess to
                // the child and reaps it.
                group.kill();
                let _ = child.kill().await;
                tracing::debug!(argv0 = %argv0, timeout = ?invocation.timeout, "command timed out; killed");
                CommandStatus::TimedOut(invocation.timeout)
            }
        };

        Ok(Output {
            status,
            stdout: stdout.finish().await,
            stderr: stderr.finish().await,
        })
    }

    fn spawn_detached(&self, argv: &[String]) -> Result<u32, RunError> {
        let (argv0, args) = argv.split_first().ok_or(RunError::EmptyCommand)?;
        let mut cmd = std::process::Command::new(argv0);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        let child = cmd.spawn().map_err(|source| RunError::Spawn {
            argv0: argv0.clone(),
            source,
        })?;
        let pid = child.id();
        // Reap it when it exits so replaced setters don't linger as zombies. If the thread
        // cannot start, dropping the unused closure drops `reaper`, which kills and reaps the
        // child instead of leaving it running untracked.
        let reaper = Reaper(Some(child));
        std::thread::Builder::new()
            .name(format!("reap-{pid}"))
            .spawn(move || reaper.wait())
            .map_err(|source| RunError::Spawn {
                argv0: argv0.clone(),
                source,
            })?;
        Ok(pid)
    }
}

/// A detached child that is reaped by [`Reaper::wait`], or killed and reaped when dropped
/// without it.
struct Reaper(Option<std::process::Child>);

impl Reaper {
    /// Block until the child exits, then reap it.
    fn wait(mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.wait();
        }
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|t| (*t).to_owned()).collect()
    }

    #[test]
    fn placeholder_scanner() {
        let found: Vec<_> = placeholders("a{path}b{}c{x y}{fit}{")
            .map(|(_, _, n)| n)
            .collect();
        assert_eq!(found, ["path", "fit"]);
    }

    #[test]
    fn tidy_stderr_keeps_tail_on_char_boundary() {
        let long = format!("{}é{}", "x".repeat(10_000), "end");
        let tidy = tidy_stderr(&long);
        assert!(tidy.len() <= MAX_ERROR_STDERR);
        assert!(tidy.ends_with("end"));
        assert_eq!(tidy_stderr("  oops\n"), "oops");
    }

    #[tokio::test]
    async fn recording_runner_scripts_outputs() {
        let runner = RecordingRunner::new();
        runner.push_output(Output::failure(2, "bad"));
        runner.push_spawn_error("no such file");
        let inv = Invocation::new(s(&["feh", "--bg-fill", "/a b.jpg"]));
        let err = run_checked(&runner, &inv).await.unwrap_err();
        assert_eq!(err.to_string(), "feh: exit code 2: bad");
        let err = run_checked(&runner, &inv).await.unwrap_err();
        assert!(matches!(
            err,
            SetError::Command {
                status: CommandStatus::FailedToStart,
                ..
            }
        ));
        assert!(run_checked(&runner, &inv).await.is_ok());
        assert_eq!(runner.argvs().len(), 3);
        assert_eq!(
            runner.spawn_detached(&s(&["swaybg"])).unwrap(),
            FAKE_PID_BASE
        );
        assert_eq!(
            runner.spawn_detached(&s(&["swaybg"])).unwrap(),
            FAKE_PID_BASE + 1
        );
        assert_eq!(runner.detached().len(), 2);
    }

    /// L1: a reaper dropped without running (its thread failed to spawn) kills and reaps the
    /// child instead of leaving it running untracked.
    #[cfg(unix)]
    #[test]
    fn an_unused_reaper_kills_its_child() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
        drop(Reaper(Some(child)));
        // Reaped: the pid no longer names a process (not even a zombie).
        assert_eq!(
            nix::sys::signal::kill(pid, None),
            Err(nix::errno::Errno::ESRCH)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_reaper_waits_for_its_child() {
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
        Reaper(Some(child)).wait();
        assert_eq!(
            nix::sys::signal::kill(pid, None),
            Err(nix::errno::Errno::ESRCH)
        );
    }
}
