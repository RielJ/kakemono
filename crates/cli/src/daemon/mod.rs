//! The daemon process (port of Go `cmd/daemon.go` `runForeground` and `internal/daemon/daemon.go`).
//!
//! [`run`] does, in order (contract `.pi/contracts/daemon.md`):
//! 1. Load the config (`--config`, else `Paths::locate_config`). A missing file is created from
//!    the commented template first, as Go did.
//! 2. Resolve the endpoint: `--socket`, else the config's `socket_path`, else `$KAKEMONO_SOCKET`,
//!    else the platform default.
//! 3. Take the single-instance lock, then write `<runtime_dir>/daemon.pid` (removed on exit).
//! 4. [`kakemono_platform::host`] owns the main thread; inside its runtime: bind the socket, build
//!    the engine, then run the IPC server and the rotation loop until SIGINT/SIGTERM (Ctrl-C,
//!    Ctrl-Break, console close or shutdown on Windows).
//! 5. Stop the server (in-flight requests get `SHUTDOWN_GRACE`) and the rotation loop, then save
//!    `state.json`.
//!
//! Test builds with the `fake` feature replace the platform with
//! `kakemono_platform::fake` when `KAKEMONO_FAKE_PLATFORM=1`.

pub mod lock;
pub mod server;

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use kakemono_config::paths::{ConfigSource, Paths};
use kakemono_config::{Config, load_or_default};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine};
use kakemono_platform::Platform;
use kakemono_protocol::endpoint::{self, Endpoint};
use tokio_util::sync::CancellationToken;

use self::lock::LockGuard;
use self::server::Server;

/// PID file name inside the runtime dir (Go wrote `daemon.pid` next to its socket).
pub const PID_FILE_NAME: &str = "daemon.pid";

/// Set to `1` in a build with the `fake` feature to run on the fake platform.
pub const FAKE_PLATFORM_ENV: &str = "KAKEMONO_FAKE_PLATFORM";

/// The command-line inputs of `daemon -f`.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--socket`; wins over the config's `socket_path`.
    pub socket: Option<PathBuf>,
}

/// The config file, the config read from it, and the endpoint the daemon listens on.
#[derive(Debug)]
pub struct Resolved {
    pub config_path: PathBuf,
    pub config: Config,
    pub endpoint: Endpoint,
}

/// Steps 1 and 2: loads (creating if missing) the config and resolves the endpoint.
pub fn resolve(paths: &Paths, options: &Options) -> anyhow::Result<Resolved> {
    let source = paths.locate_config(options.config.as_deref());
    create_missing_config(&source);
    if let ConfigSource::Legacy(path) = &source {
        tracing::info!(
            path = %path.display(),
            "using the wallpaperd config; `kakemono config init` creates a kakemono one"
        );
    }
    let config_path = source.path().to_owned();
    let config = load_or_default(&config_path)
        .with_context(|| format!("loading config {}", config_path.display()))?;

    let endpoint = match options
        .socket
        .as_deref()
        .filter(|s| !s.as_os_str().is_empty())
    {
        Some(socket) => endpoint::default_endpoint(Some(socket))?,
        None => {
            let socket = config.effective_socket_path(&source, &paths.home);
            if let Some(notice) = socket.notice() {
                tracing::info!("{notice}");
            }
            endpoint::default_endpoint(socket.override_path())?
        }
    };
    Ok(Resolved {
        config_path,
        config,
        endpoint,
    })
}

/// Go `runForeground`: a missing config file (default location or `--config`) gets the
/// commented template. Failure is only a warning; the defaults apply either way.
fn create_missing_config(source: &ConfigSource) {
    let path = match source {
        ConfigSource::Missing(path) => path,
        ConfigSource::Explicit(path) if !path.exists() => path,
        _ => return,
    };
    match kakemono_config::paths::init(path) {
        Ok(()) => {
            eprintln!("Created default config at {}", path.display());
            tracing::info!(path = %path.display(), "created default config");
        }
        Err(e) => {
            eprintln!("warning: could not write default config: {e}");
            tracing::warn!(error = %e, "could not write default config");
        }
    }
}

/// Runs the daemon until a stop signal. Call from `main`'s thread with no tokio runtime running
/// (the platform host needs the main thread on macOS). Logging must already be set up.
pub fn run(paths: &Paths, options: &Options) -> anyhow::Result<()> {
    let Resolved {
        config_path,
        mut config,
        endpoint,
    } = resolve(paths, options)?;

    let lock = lock::acquire(&paths.runtime_dir)?;
    // First-run detection (Task 37): under the lock, so a refused second daemon writes nothing;
    // before `host`, so the platform resolves from the saved `[desktop]`.
    if !fake_platform_requested() {
        let legacy = paths.locate_config(options.config.as_deref()).is_legacy();
        crate::firstrun::ensure_desktop(&config_path, legacy, &mut config);
    }
    let _pid_file = PidFile::write(paths.runtime_dir.join(PID_FILE_NAME));

    let state_file = paths.state_file.clone();
    let state = State::load_from_paths(paths)
        .context("loading state")?
        .state;
    let desktop = config.desktop.clone();
    let daemon = Daemon {
        config,
        config_path,
        endpoint,
        state,
        state_file,
        lock,
    };
    host(&desktop, paths.runtime_dir.clone(), move |platform| {
        daemon.serve(platform)
    })
}

/// True when the fake platform will be used (test builds only).
fn fake_platform_requested() -> bool {
    cfg!(feature = "fake") && std::env::var_os(FAKE_PLATFORM_ENV).is_some_and(|v| v == "1")
}

/// The platform host for `desktop`. On Linux the persistent-setter PID file lives in the same
/// runtime dir as the lock and socket (`open_platform_in`), so a `--config`/env override of the
/// runtime dir moves them together.
fn host<F, Fut>(
    desktop: &kakemono_config::DesktopConfig,
    runtime_dir: PathBuf,
    f: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Platform) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send,
{
    #[cfg(feature = "fake")]
    if fake_platform_requested() {
        use kakemono_platform::fake::{ManualSignals, RecordingSetter};
        tracing::warn!("{FAKE_PLATFORM_ENV}=1: using the fake platform; wallpapers are not set");
        let platform = Platform {
            setter: std::sync::Arc::new(RecordingSetter::new()),
            signals: std::sync::Arc::new(ManualSignals::default()),
        };
        return kakemono_platform::default_host(std::future::ready(platform), f);
    }
    #[cfg(target_os = "linux")]
    {
        kakemono_platform::default_host(
            kakemono_platform::linux::open_platform_in(desktop, Some(runtime_dir)),
            f,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = runtime_dir;
        kakemono_platform::host(desktop, f)
    }
}

/// Everything the async part needs; moved into the host closure.
struct Daemon {
    config: Config,
    config_path: PathBuf,
    endpoint: Endpoint,
    state: State,
    state_file: PathBuf,
    /// Held until the server has stopped and the state is saved.
    lock: LockGuard,
}

impl Daemon {
    async fn serve(self, platform: Platform) -> anyhow::Result<()> {
        let Self {
            config,
            config_path,
            endpoint,
            state,
            state_file,
            lock,
        } = self;
        let listener = server::bind(&endpoint, &lock).await?;

        let rotation = rotation_summary(&config);
        let http = kakemono_sources::http::client().context("building the HTTP client")?;
        let engine = Engine::new(config, state, state_file.clone(), Deps::new(platform, http))
            .context("initialising the engine")?;
        let backend = engine.platform().setter.info();
        let server = Server::new(engine.clone(), move || load_or_default(&config_path));
        // `doctor` reports the daemon's own session env (systemd units often lack it; Task 37).
        #[cfg(target_os = "linux")]
        let server = server.with_env_probe(|| {
            Box::pin(kakemono_platform::linux::detect::env::capture_with_dbus())
        });

        let shutdown = CancellationToken::new();
        let serving = tokio::spawn(server.serve(listener, shutdown.clone()));
        let rotating = tokio::spawn({
            let engine = engine.clone();
            let shutdown = shutdown.clone();
            async move { engine.run(shutdown).await }
        });
        tracing::info!(
            pid = std::process::id(),
            socket = %endpoint,
            backend = %backend.name,
            "daemon started ({rotation})"
        );

        let signal = wait_for_stop_signal().await?;
        tracing::info!("daemon shutting down ({signal})");
        shutdown.cancel();
        if let Err(e) = serving.await {
            tracing::warn!(error = %e, "IPC server task failed");
        }
        if let Err(e) = rotating.await {
            tracing::warn!(error = %e, "rotation task failed");
        }
        if let Err(e) = engine.state().save(&state_file) {
            tracing::warn!(error = %e, "saving state on shutdown");
        }
        drop(lock);
        tracing::info!("daemon stopped");
        Ok(())
    }
}

fn rotation_summary(config: &Config) -> String {
    if config.daemon.rotation_enabled {
        format!("interval {}, rotation enabled", config.daemon.interval)
    } else {
        "rotation disabled".to_owned()
    }
}

/// Waits for SIGINT or SIGTERM (Unix) or a console stop event (Windows) and names it.
#[cfg(unix)]
async fn wait_for_stop_signal() -> io::Result<&'static str> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(tokio::select! {
        _ = interrupt.recv() => "SIGINT",
        _ = terminate.recv() => "SIGTERM",
    })
}

#[cfg(windows)]
async fn wait_for_stop_signal() -> io::Result<&'static str> {
    use tokio::signal::windows;
    let mut ctrl_c = windows::ctrl_c()?;
    let mut ctrl_break = windows::ctrl_break()?;
    let mut ctrl_close = windows::ctrl_close()?;
    let mut ctrl_shutdown = windows::ctrl_shutdown()?;
    Ok(tokio::select! {
        _ = ctrl_c.recv() => "Ctrl-C",
        _ = ctrl_break.recv() => "Ctrl-Break",
        _ = ctrl_close.recv() => "console closed",
        _ = ctrl_shutdown.recv() => "system shutdown",
    })
}

/// `daemon.pid`, holding this process's PID; removed on drop. Written only after the lock is
/// taken, so it never overwrites a running daemon's file. A write failure is only logged.
#[derive(Debug)]
pub struct PidFile {
    path: Option<PathBuf>,
}

impl PidFile {
    pub fn write(path: PathBuf) -> Self {
        match std::fs::write(&path, std::process::id().to_string()) {
            Ok(()) => Self { path: Some(path) },
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not write the PID file");
                Self { path: None }
            }
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use kakemono_config::paths::BaseDirs;

    use super::*;

    fn paths(root: &Path) -> Paths {
        let base = BaseDirs {
            home: root.to_owned(),
            config_home: root.join("cfg"),
            cache_home: root.join("cache"),
            state_home: Some(root.join("state")),
            runtime_home: Some(root.join("run")),
            config_override: None,
        };
        Paths::from_base(&base, kakemono_config::paths::Os::Linux)
    }

    #[test]
    fn pid_file_holds_the_pid_and_is_removed_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PID_FILE_NAME);
        let pid = PidFile::write(path.clone());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            std::process::id().to_string()
        );
        drop(pid);
        assert!(!path.exists());
    }

    #[test]
    fn resolve_creates_a_missing_config_and_prefers_the_socket_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(tmp.path());
        let socket = tmp.path().join("flag.sock");
        let resolved = resolve(
            &paths,
            &Options {
                config: None,
                socket: Some(socket.clone()),
            },
        )
        .unwrap();
        assert_eq!(resolved.config_path, paths.config_file);
        assert!(paths.config_file.exists(), "template written");
        assert_eq!(resolved.endpoint.path(), socket);
    }

    #[test]
    fn resolve_uses_the_configs_socket_path() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths(tmp.path());
        let config = tmp.path().join("explicit.toml");
        let socket = tmp.path().join("cfg.sock");
        std::fs::write(
            &config,
            format!(
                "[daemon]\nsocket_path = {:?}\n",
                socket.display().to_string()
            ),
        )
        .unwrap();
        let resolved = resolve(
            &paths,
            &Options {
                config: Some(config.clone()),
                socket: None,
            },
        )
        .unwrap();
        assert_eq!(resolved.config_path, config);
        assert_eq!(resolved.endpoint.path(), socket);
    }

    #[test]
    fn resolve_reports_a_broken_config() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("bad.toml");
        std::fs::write(&config, "[daemon\n").unwrap();
        let err = resolve(
            &paths(tmp.path()),
            &Options {
                config: Some(config),
                socket: None,
            },
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("loading config"), "{err:#}");
    }
}
