//! `kakemono daemon [-f]` (port of Go `cmd/daemon.go`).
//!
//! - `-f/--foreground`: runs the daemon in this process ([`crate::daemon::run`]), logging to
//!   stderr.
//! - Without `-f`: refuses if a daemon already runs, then re-executes itself as
//!   `daemon --foreground --detached` in a new session (`setsid`) on Unix, or with
//!   `DETACHED_PROCESS | CREATE_NO_WINDOW` on Windows, stdio closed. The child logs to the rolling
//!   file in `Paths::log_dir`. Unlike Go, the parent waits up to [`READY_TIMEOUT`] for the socket
//!   to answer, so a child that fails at start-up (bad config, socket in use) is reported here.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use kakemono_config::paths::Paths;
use kakemono_protocol::client::Client;
use kakemono_protocol::endpoint::Endpoint;

use crate::cli::Context;
use crate::daemon::{self, Options, lock};
use crate::logging::{self, LogTarget};

/// How long `daemon` (without `-f`) waits for the new daemon to answer on its socket.
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);
const READY_POLL: Duration = Duration::from_millis(50);

/// Runs `kakemono daemon`. `detached` is the hidden flag the background re-exec passes: log to
/// the file instead of stderr.
pub fn run(
    ctx: &Context,
    out: &mut dyn Write,
    foreground: bool,
    detached: bool,
) -> anyhow::Result<()> {
    let paths = Paths::detect()?;
    let options = Options {
        config: ctx.global.config.clone(),
        socket: ctx.global.socket.clone(),
    };
    if !foreground {
        return spawn_background(ctx, out, &paths, &options);
    }
    let target = if detached {
        LogTarget::Dir(paths.log_dir.clone())
    } else {
        LogTarget::Stderr
    };
    let _guard = logging::init(&target)?;
    let result = daemon::run(&paths, &options);
    if detached && let Err(e) = &result {
        // stderr is /dev/null here; the log is the only place the reason shows up.
        tracing::error!("daemon failed: {e:#}");
    }
    result
}

fn spawn_background(
    ctx: &Context,
    out: &mut dyn Write,
    paths: &Paths,
    options: &Options,
) -> anyhow::Result<()> {
    // Refuse a second daemon before spawning (Go checked the socket only).
    drop(lock::acquire(&paths.runtime_dir)?);
    let endpoint = ctx.endpoint()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    if runtime.block_on(answers(&endpoint)) {
        bail!("daemon is already running (socket: {endpoint})");
    }

    let mut child = background_command(options)?
        .spawn()
        .context("spawning daemon")?;
    let pid = child.id();
    runtime.block_on(wait_until_ready(&mut child, &endpoint, &paths.log_dir))?;
    writeln!(out, "kakemono daemon started (pid {pid})")?;
    writeln!(out, "Socket: {endpoint}")?;
    Ok(())
}

/// `<this exe> daemon --foreground --detached [--config C] [--socket S]`, detached, stdio null.
/// `--config` and `--socket` are forwarded only when given (made absolute); otherwise the child
/// resolves them from the same environment.
fn background_command(options: &Options) -> anyhow::Result<Command> {
    let exe = std::env::current_exe().context("resolving executable path")?;
    let mut cmd = Command::new(exe);
    cmd.args(["daemon", "--foreground", "--detached"]);
    if let Some(config) = &options.config {
        cmd.arg("--config").arg(absolute(config)?);
    }
    if let Some(socket) = options
        .socket
        .as_ref()
        .filter(|s| !s.as_os_str().is_empty())
    {
        cmd.arg("--socket").arg(absolute(socket)?);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut cmd);
    Ok(cmd)
}

fn absolute(path: &std::path::Path) -> anyhow::Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))
}

#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    // SAFETY: `setsid` is async-signal-safe and touches no memory of the parent.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid()
                .map(drop)
                .map_err(std::io::Error::from)
        });
    }
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

/// Polls until the endpoint answers. Fails if the child exits first or the timeout passes.
async fn wait_until_ready(
    child: &mut Child,
    endpoint: &Endpoint,
    log_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().context("checking the daemon process")? {
            bail!(
                "the daemon exited during start-up ({status}); see the log in {}",
                log_dir.display()
            );
        }
        if answers(endpoint).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "the daemon (pid {}) did not answer on {endpoint} within {}s; see the log in {}",
                child.id(),
                READY_TIMEOUT.as_secs(),
                log_dir.display()
            );
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

async fn answers(endpoint: &Endpoint) -> bool {
    Client::connect(endpoint).await.is_ok()
}
