//! Platform paths (etcetera `choose_base_strategy`: XDG on macOS/Linux, Known Folders on Windows),
//! `config init`, and config lookup.
//!
//! Path computation is split into an env snapshot ([`BaseDirs::from_env`]) and a pure function
//! ([`Paths::from_base`]) so it can be tested for every OS without touching the environment.

use std::io::Write;
use std::path::{Path, PathBuf};

use etcetera::BaseStrategy;

use crate::error::ConfigError;
use crate::load::{Loaded, load_or_default_report};
use crate::schema::Config;

/// App directory name used under every base directory.
pub const APP_NAME: &str = "kakemono";
/// Env var that overrides the config file path (below `--config`, above the default).
pub const CONFIG_ENV: &str = "KAKEMONO_CONFIG";
/// Config file name.
pub const CONFIG_FILE_NAME: &str = "config.toml";
/// State file name.
pub const STATE_FILE_NAME: &str = "state.json";

/// The documented default config written by [`init`].
pub const TEMPLATE: &str = include_str!("template.toml");

/// Which OS's layout to compute. [`Os::current`] for real use; tests pass others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    MacOs,
    /// Linux and other Unix-likes.
    Linux,
    Windows,
}

impl Os {
    /// The OS this binary was built for.
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

/// Snapshot of the base directories and env overrides that paths derive from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseDirs {
    pub home: PathBuf,
    /// XDG: `$XDG_CONFIG_HOME` or `~/.config`. Windows: `%APPDATA%`.
    pub config_home: PathBuf,
    /// XDG: `$XDG_CACHE_HOME` or `~/.cache`. Windows: `%LOCALAPPDATA%`.
    pub cache_home: PathBuf,
    /// XDG: `$XDG_STATE_HOME` or `~/.local/state`. Windows: `None`.
    pub state_home: Option<PathBuf>,
    /// `$XDG_RUNTIME_DIR` (used on Linux only).
    pub runtime_home: Option<PathBuf>,
    /// `$KAKEMONO_CONFIG`, if set and non-empty.
    pub config_override: Option<PathBuf>,
}

impl BaseDirs {
    /// Reads the base directories from etcetera and the environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let s = etcetera::choose_base_strategy().map_err(|_| ConfigError::NoHome)?;
        Ok(Self {
            home: s.home_dir().to_owned(),
            config_home: s.config_dir(),
            cache_home: s.cache_dir(),
            state_home: s.state_dir(),
            runtime_home: s.runtime_dir(),
            config_override: std::env::var_os(CONFIG_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        })
    }
}

/// Every path the app uses. Fields are public; [`Paths::detect`] builds them for this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    /// `<config_home>/kakemono`.
    pub config_dir: PathBuf,
    /// `<config_dir>/config.toml` (ignores `KAKEMONO_CONFIG`; see [`Paths::locate_config`]).
    pub config_file: PathBuf,
    /// `$KAKEMONO_CONFIG`, if set.
    pub config_override: Option<PathBuf>,
    /// `<config_dir>/state.json`.
    pub state_file: PathBuf,
    /// Lock file, setter PIDs and (on Unix) the default socket. Linux: `$XDG_RUNTIME_DIR/kakemono`,
    /// else `<config_dir>`. macOS: `<config_dir>`. Windows: `%LOCALAPPDATA%\kakemono\run`.
    pub runtime_dir: PathBuf,
    /// XDG: `<state_home>/kakemono`. Windows: `%LOCALAPPDATA%\kakemono\logs`.
    pub log_dir: PathBuf,
    /// XDG: `<cache_home>/kakemono`. Windows: `%LOCALAPPDATA%\kakemono\cache`.
    pub cache_dir: PathBuf,
}

impl Paths {
    /// Paths for this machine and environment.
    pub fn detect() -> Result<Self, ConfigError> {
        Ok(Self::from_base(&BaseDirs::from_env()?, Os::current()))
    }

    /// Pure path layout for `os`.
    pub fn from_base(base: &BaseDirs, os: Os) -> Self {
        let config_dir = base.config_home.join(APP_NAME);
        let local_app = base.cache_home.join(APP_NAME);
        let (runtime_dir, log_dir, cache_dir) = match os {
            Os::Windows => (
                local_app.join("run"),
                local_app.join("logs"),
                local_app.join("cache"),
            ),
            Os::MacOs | Os::Linux => {
                let runtime = match (os, &base.runtime_home) {
                    (Os::Linux, Some(rt)) => rt.join(APP_NAME),
                    _ => config_dir.clone(),
                };
                let state_home = base
                    .state_home
                    .clone()
                    .unwrap_or_else(|| base.home.join(".local").join("state"));
                (runtime, state_home.join(APP_NAME), local_app)
            }
        };
        Self {
            home: base.home.clone(),
            config_file: config_dir.join(CONFIG_FILE_NAME),
            state_file: config_dir.join(STATE_FILE_NAME),
            config_dir,
            config_override: base.config_override.clone(),
            runtime_dir,
            log_dir,
            cache_dir,
        }
    }

    /// Decides which config file to read. Order: `explicit` (`--config`), then `$KAKEMONO_CONFIG`,
    /// then the kakemono file if it exists, else [`ConfigSource::Missing`] with the kakemono path.
    /// Never moves or copies files.
    pub fn locate_config(&self, explicit: Option<&Path>) -> ConfigSource {
        if let Some(p) = explicit.or(self.config_override.as_deref()) {
            return ConfigSource::Explicit(p.to_owned());
        }
        if self.config_file.exists() {
            return ConfigSource::Primary(self.config_file.clone());
        }
        ConfigSource::Missing(self.config_file.clone())
    }

    /// [`Paths::locate_config`] then [`load_or_default`](crate::load_or_default). A missing explicit file also yields the
    /// defaults, as Go's `LoadOrDefault` did.
    pub fn load_config(
        &self,
        explicit: Option<&Path>,
    ) -> Result<(Config, ConfigSource), ConfigError> {
        self.load_config_report(explicit)
            .map(|(loaded, source)| (loaded.config, source))
    }

    /// [`Paths::load_config`] with the load warnings ([`Loaded`]).
    pub fn load_config_report(
        &self,
        explicit: Option<&Path>,
    ) -> Result<(Loaded, ConfigSource), ConfigError> {
        let source = self.locate_config(explicit);
        let loaded = load_or_default_report(source.path())?;
        Ok((loaded, source))
    }
}

/// Where the config was (or will be) read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    /// `--config` or `$KAKEMONO_CONFIG`. The file may not exist.
    Explicit(PathBuf),
    /// The kakemono config file exists.
    Primary(PathBuf),
    /// No file anywhere; defaults apply. The path is the kakemono file to create.
    Missing(PathBuf),
}

impl ConfigSource {
    /// The file that was read, or (for `Missing`) would be. Edits should target this path.
    pub fn path(&self) -> &Path {
        match self {
            ConfigSource::Explicit(p) | ConfigSource::Primary(p) | ConfigSource::Missing(p) => p,
        }
    }
}

impl Config {
    /// The one rule the daemon, the CLI client and the UI use to pick the socket from the config:
    /// `daemon.socket_path` when it is set and non-empty, else `None`. Pass the result and
    /// [`Paths::runtime_dir`] to `kakemono_protocol::endpoint::default_endpoint`, which then
    /// falls back to `$KAKEMONO_SOCKET` and the platform default.
    pub fn socket_override(&self) -> Option<&Path> {
        self.daemon
            .socket_path
            .as_deref()
            .filter(|p| !p.as_os_str().is_empty())
    }
}

/// Writes [`TEMPLATE`] to `path`. Refuses to overwrite (Go's message) and creates the parent
/// directory. The file is created 0600 on Unix because it may hold API keys.
pub fn init(path: &Path) -> Result<(), ConfigError> {
    write_new(path, TEMPLATE.as_bytes())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    if path.exists() {
        return Err(ConfigError::AlreadyExists {
            path: path.to_owned(),
        });
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|cause| ConfigError::CreateDir {
            path: parent.to_owned(),
            cause,
        })?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write_err = |cause| ConfigError::Write {
        path: path.to_owned(),
        cause,
    };
    let mut file = opts.open(path).map_err(|source: std::io::Error| {
        if source.kind() == std::io::ErrorKind::AlreadyExists {
            ConfigError::AlreadyExists {
                path: path.to_owned(),
            }
        } else {
            write_err(source)
        }
    })?;
    file.write_all(bytes).map_err(write_err)?;
    file.sync_all().map_err(write_err)
}

fn detect_join(f: impl FnOnce(Paths) -> PathBuf) -> Result<PathBuf, ConfigError> {
    Paths::detect().map(f)
}

/// Default kakemono config file, honouring `$KAKEMONO_CONFIG` and `$XDG_CONFIG_HOME`. Use
/// [`Paths::locate_config`] for reading.
pub fn config_file() -> Result<PathBuf, ConfigError> {
    detect_join(|p| p.config_override.unwrap_or(p.config_file))
}

/// See [`Paths::state_file`].
pub fn state_file() -> Result<PathBuf, ConfigError> {
    detect_join(|p| p.state_file)
}

/// See [`Paths::runtime_dir`].
pub fn runtime_dir() -> Result<PathBuf, ConfigError> {
    detect_join(|p| p.runtime_dir)
}

/// See [`Paths::log_dir`].
pub fn log_dir() -> Result<PathBuf, ConfigError> {
    detect_join(|p| p.log_dir)
}

/// See [`Paths::cache_dir`].
pub fn cache_dir() -> Result<PathBuf, ConfigError> {
    detect_join(|p| p.cache_dir)
}
