//! `kakemono service install|uninstall|status`: start the daemon at login.
//!
//! | OS | Mechanism | Definition |
//! |---|---|---|
//! | macOS | launchd LaunchAgent, label [`launchd::LABEL`] | `~/Library/LaunchAgents/dev.rielj.kakemono.daemon.plist` |
//! | Linux | systemd user unit [`systemd::UNIT_NAME`] | `$XDG_CONFIG_HOME/systemd/user/kakemono.service` |
//! | Windows | HKCU `Run` value [`windows::RUN_VALUE_NAME`] | registry, no file |
//!
//! The daemon's names are distinct from the Tauri UI's own autostart entry (tauri-plugin-autostart
//! registers `kakemono` as the LaunchAgent label / Run value name).
//!
//! The file contents are pure functions of [`ServiceEnv`] (snapshot-tested on every host). Every
//! external command goes through a [`CommandRunner`] and the Windows registry through a
//! [`windows::RunKey`], so tests never touch the real service manager.

pub mod launchd;
pub mod systemd;
pub mod windows;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_config::ConfigError;
use kakemono_config::paths::{BaseDirs, Paths};

pub use kakemono_config::paths::Os;

use crate::api::CommandStatus;
use crate::exec::{CommandRunner, Invocation, Output, RunError, TokioRunner, tidy_stderr};

/// Service errors. Messages read as CLI output (`kakemono service install` prints them as is).
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("locating the home directory: {0}")]
    Env(#[from] ConfigError),
    #[error("finding the kakemono executable: {0}")]
    Exe(#[source] io::Error),
    #[error("{action} {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("`{command}` failed ({status}){}", stderr_suffix(.stderr))]
    Command {
        command: String,
        status: CommandStatus,
        stderr: String,
    },
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("login registry entry: {0}")]
    Registry(#[source] io::Error),
    /// A path the service definition must contain is not valid UTF-8, so writing it would point
    /// the service at a file that does not exist.
    #[error("cannot install the service: path is not valid UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),
}

fn stderr_suffix(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

/// Everything the service definitions depend on. Pure data so tests can inject temp dirs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceEnv {
    pub os: Os,
    /// `~` (macOS: `~/Library/LaunchAgents`).
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME` or `~/.config` (Linux: `systemd/user`).
    pub config_home: PathBuf,
    /// `Paths::log_dir` (macOS: launchd's stdout/stderr file).
    pub log_dir: PathBuf,
    /// Absolute path of the `kakemono` binary the service runs.
    pub exe: PathBuf,
    /// The user's id (macOS: the `gui/<uid>` launchd domain). `getuid()` on Unix; 0 on Windows,
    /// where nothing reads it.
    pub uid: u32,
}

/// The real user id of this process (no command spawned, unlike `id -u`).
#[cfg(unix)]
fn current_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

impl ServiceEnv {
    /// The current user's dirs and `std::env::current_exe()`, made absolute. Symlinks are kept
    /// where the OS reports them (macOS: `/opt/homebrew/bin/kakemono` stays valid across
    /// upgrades, its Cellar target does not); Linux's `/proc/self/exe` is already resolved.
    pub fn detect() -> Result<Self, ServiceError> {
        let base = BaseDirs::from_env()?;
        let os = Os::current();
        let paths = Paths::from_base(&base, os);
        let exe = std::env::current_exe().map_err(ServiceError::Exe)?;
        let exe = std::path::absolute(&exe).map_err(ServiceError::Exe)?;
        Ok(Self {
            os,
            home: base.home,
            config_home: base.config_home,
            log_dir: paths.log_dir,
            exe,
            uid: current_uid(),
        })
    }

    /// Every path the definition embeds is UTF-8 (the definitions are text; a lossy path would
    /// name a file that does not exist).
    fn check_utf8(&self) -> Result<(), ServiceError> {
        let log_dir = (self.os == Os::MacOs).then_some(&self.log_dir);
        match std::iter::once(&self.exe)
            .chain(log_dir)
            .find(|path| path.to_str().is_none())
        {
            Some(path) => Err(ServiceError::NonUtf8Path(path.clone())),
            None => Ok(()),
        }
    }

    /// The service definition file, if this OS uses one (not Windows).
    pub fn definition_path(&self) -> Option<PathBuf> {
        match self.os {
            Os::MacOs => Some(launchd::plist_path(&self.home)),
            Os::Linux => Some(systemd::unit_path(&self.config_home)),
            Os::Windows => None,
        }
    }

    /// Where the service is registered, for messages: the file, or the registry value.
    pub fn location(&self) -> String {
        match self.definition_path() {
            Some(path) => path.display().to_string(),
            None => windows::run_value_location(),
        }
    }

    /// The generated definition: plist, unit file, or the Run value's command line.
    pub fn definition(&self) -> String {
        match self.os {
            Os::MacOs => launchd::plist(self),
            Os::Linux => systemd::unit(&self.exe),
            Os::Windows => windows::run_command(&self.exe),
        }
    }
}

/// Whether the service manager runs the daemon now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Running {
    /// launchd / systemd report it running (`pid` when known).
    Yes { pid: Option<u32> },
    /// Loaded but stopped, failed, or not loaded at all. `state` is the manager's word for it
    /// (`inactive`, `failed`, `not loaded`, ...), when there is one.
    No { state: Option<String> },
    /// The manager can't say (Windows has none; or `launchctl`/`systemctl` is missing). Callers
    /// may ask the daemon's socket instead.
    Unknown,
}

/// The result of [`ServiceManager::status`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceStatus {
    /// The plist / unit file exists, or the Run value is set (and not disabled in Task Manager).
    pub installed: bool,
    pub running: Running,
    /// The definition file (`None` on Windows).
    pub path: Option<PathBuf>,
    /// The file path, or the registry value on Windows.
    pub location: String,
}

/// The result of [`ServiceManager::install`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The definition file written (`None` on Windows).
    pub path: Option<PathBuf>,
    /// The file path, or the registry value on Windows.
    pub location: String,
    /// The command line the service runs.
    pub command: String,
    /// The service manager started the daemon (macOS, Linux). On Windows the Run value only
    /// applies at the next login; the caller starts the daemon itself.
    pub started: bool,
}

/// The result of [`ServiceManager::uninstall`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uninstalled {
    /// `false`: nothing was installed (uninstall is idempotent).
    pub removed: bool,
    pub location: String,
}

/// Installs, removes and inspects the daemon's login service.
pub struct ServiceManager {
    env: ServiceEnv,
    runner: Arc<dyn CommandRunner>,
    run_key: Arc<dyn windows::RunKey>,
}

impl std::fmt::Debug for ServiceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceManager")
            .field("env", &self.env)
            .finish_non_exhaustive()
    }
}

impl ServiceManager {
    /// The real thing: detected dirs, `TokioRunner`, the HKCU registry on Windows.
    pub fn system() -> Result<Self, ServiceError> {
        Ok(Self::with_parts(
            ServiceEnv::detect()?,
            Arc::new(TokioRunner),
            windows::system_run_key(),
        ))
    }

    /// Explicit parts, for tests (`RecordingRunner`, `windows::MemoryRunKey`, temp dirs).
    pub fn with_parts(
        env: ServiceEnv,
        runner: Arc<dyn CommandRunner>,
        run_key: Arc<dyn windows::RunKey>,
    ) -> Self {
        Self {
            env,
            runner,
            run_key,
        }
    }

    pub fn env(&self) -> &ServiceEnv {
        &self.env
    }

    /// Writes the definition and starts the service. Re-running it rewrites the definition and
    /// restarts the service, so it also upgrades an old install.
    pub async fn install(&self) -> Result<Installed, ServiceError> {
        self.env.check_utf8()?;
        match self.env.os {
            Os::MacOs => launchd::install(&self.env, self.runner.as_ref()).await,
            Os::Linux => systemd::install(&self.env, self.runner.as_ref()).await,
            Os::Windows => windows::install(&self.env, self.run_key.as_ref()),
        }
    }

    /// Stops the service and removes the definition. Succeeds when nothing is installed.
    pub async fn uninstall(&self) -> Result<Uninstalled, ServiceError> {
        match self.env.os {
            Os::MacOs => launchd::uninstall(&self.env, self.runner.as_ref()).await,
            Os::Linux => systemd::uninstall(&self.env, self.runner.as_ref()).await,
            Os::Windows => windows::uninstall(self.run_key.as_ref()),
        }
    }

    /// Installed? Running? Never changes anything.
    pub async fn status(&self) -> Result<ServiceStatus, ServiceError> {
        match self.env.os {
            Os::MacOs => launchd::status(&self.env, self.runner.as_ref()).await,
            Os::Linux => systemd::status(&self.env, self.runner.as_ref()).await,
            Os::Windows => windows::status(self.run_key.as_ref()),
        }
    }
}

// ---- helpers shared by the backends ----

/// Runs `argv`; a non-zero exit or timeout is `ServiceError::Command`.
async fn run_checked(
    runner: &dyn CommandRunner,
    argv: Vec<String>,
) -> Result<Output, ServiceError> {
    let invocation = Invocation::new(argv);
    let output = runner.run(&invocation).await?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_error(&invocation.argv, &output))
    }
}

/// Runs `argv` and returns the output whatever the exit status (spawn failures are still `Err`).
async fn run_any(runner: &dyn CommandRunner, argv: Vec<String>) -> Result<Output, RunError> {
    runner.run(&Invocation::new(argv)).await
}

/// `ServiceError::Command` for a failed `argv`, with stderr (or stdout when stderr is empty)
/// trimmed and capped like `exec::Output::check`'s.
fn command_error(argv: &[String], output: &Output) -> ServiceError {
    let stderr = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    ServiceError::Command {
        command: argv.join(" "),
        status: output.status,
        stderr: tidy_stderr(stderr),
    }
}

fn write_file(path: &Path, contents: &str) -> Result<(), ServiceError> {
    if let Some(dir) = path.parent() {
        create_dir(dir)?;
    }
    std::fs::write(path, contents).map_err(|source| ServiceError::Io {
        action: "writing",
        path: path.to_path_buf(),
        source,
    })
}

fn create_dir(dir: &Path) -> Result<(), ServiceError> {
    std::fs::create_dir_all(dir).map_err(|source| ServiceError::Io {
        action: "creating",
        path: dir.to_path_buf(),
        source,
    })
}

/// Removes `path`. `Ok(false)` if it wasn't there.
fn remove_file(path: &Path) -> Result<bool, ServiceError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ServiceError::Io {
            action: "removing",
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Escapes `&`, `<`, `>`, `"` and `'` for XML text.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// POSIX shell quoting: bare when only safe characters, else single quotes.
fn shell_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c));
    if safe {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(xml_escape(r#"a&b<c>"d'"#), "a&amp;b&lt;c&gt;&quot;d&apos;");
        assert_eq!(shell_quote("/usr/bin/kakemono"), "/usr/bin/kakemono");
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}
