//! Platform paths (etcetera `choose_base_strategy`: XDG on macOS/Linux, Known Folders on Windows),
//! `config init`, and lookup of the legacy Go `wallpaperd` config.
//!
//! Path computation is split into an env snapshot ([`BaseDirs::from_env`]) and a pure function
//! ([`Paths::from_base`]) so it can be tested for every OS without touching the environment.

use std::io::Write;
use std::path::{Path, PathBuf};

use etcetera::BaseStrategy;

use crate::error::ConfigError;
use crate::load::load_or_default;
use crate::schema::Config;

/// App directory name used under every base directory.
pub const APP_NAME: &str = "kakemono";
/// The Go app's directory name. Only read, never written, by this crate.
pub const LEGACY_APP_NAME: &str = "wallpaperd";
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
    /// The Go app's config: `($XDG_CONFIG_HOME | ~/.config)/wallpaperd/config.toml`.
    pub legacy_config_file: PathBuf,
    /// `<config_dir>/state.json`.
    pub state_file: PathBuf,
    /// The Go app's state: `~/.config/wallpaperd/state.json` (Go ignored `XDG_CONFIG_HOME` here).
    pub legacy_state_file: PathBuf,
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
        let dot_config = base.home.join(".config");
        let legacy_config_home = match os {
            Os::Windows => &dot_config,
            Os::MacOs | Os::Linux => &base.config_home,
        };
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
            legacy_config_file: legacy_config_home
                .join(LEGACY_APP_NAME)
                .join(CONFIG_FILE_NAME),
            legacy_state_file: dot_config.join(LEGACY_APP_NAME).join(STATE_FILE_NAME),
            runtime_dir,
            log_dir,
            cache_dir,
        }
    }

    /// Decides which config file to read. Order: `explicit` (`--config`), then `$KAKEMONO_CONFIG`,
    /// then the kakemono file if it exists, then the Go `wallpaperd` file if it exists, else
    /// [`ConfigSource::Missing`] with the kakemono path. Never moves or copies files.
    pub fn locate_config(&self, explicit: Option<&Path>) -> ConfigSource {
        if let Some(p) = explicit.or(self.config_override.as_deref()) {
            return ConfigSource::Explicit(p.to_owned());
        }
        if self.config_file.exists() {
            return ConfigSource::Primary(self.config_file.clone());
        }
        if self.legacy_config_file.exists() {
            return ConfigSource::Legacy(self.legacy_config_file.clone());
        }
        ConfigSource::Missing(self.config_file.clone())
    }

    /// [`Paths::locate_config`] then [`load_or_default`]. A missing explicit file also yields the
    /// defaults, as Go's `LoadOrDefault` did.
    pub fn load_config(
        &self,
        explicit: Option<&Path>,
    ) -> Result<(Config, ConfigSource), ConfigError> {
        let source = self.locate_config(explicit);
        let cfg = load_or_default(source.path())?;
        Ok((cfg, source))
    }
}

/// Where the config was (or will be) read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    /// `--config` or `$KAKEMONO_CONFIG`. The file may not exist.
    Explicit(PathBuf),
    /// The kakemono config file exists.
    Primary(PathBuf),
    /// No kakemono config, but the Go app's `wallpaperd/config.toml` exists and is used. The CLI
    /// should tell the user and offer [`copy_legacy_config`].
    Legacy(PathBuf),
    /// No file anywhere; defaults apply. The path is the kakemono file to create.
    Missing(PathBuf),
}

impl ConfigSource {
    /// The file that was read, or (for `Missing`) would be. Edits should target this path, so a
    /// legacy config keeps working until the user copies it.
    pub fn path(&self) -> &Path {
        match self {
            ConfigSource::Explicit(p)
            | ConfigSource::Primary(p)
            | ConfigSource::Legacy(p)
            | ConfigSource::Missing(p) => p,
        }
    }

    pub fn is_legacy(&self) -> bool {
        matches!(self, ConfigSource::Legacy(_))
    }
}

/// File name of the Go daemon's socket.
pub const LEGACY_SOCKET_FILE_NAME: &str = "daemon.sock";

/// The Go daemon's default socket, `~/.config/wallpaperd/daemon.sock` (Go `DefaultSocketPath`,
/// which ignored `XDG_CONFIG_HOME`). The Go config template wrote this value to `socket_path`.
pub fn legacy_default_socket(home: &Path) -> PathBuf {
    home.join(".config")
        .join(LEGACY_APP_NAME)
        .join(LEGACY_SOCKET_FILE_NAME)
}

/// How `daemon.socket_path` resolves for a config read from a given [`ConfigSource`]. Returned by
/// [`Config::effective_socket_path`]; pass [`SocketPath::override_path`] to
/// `kakemono_protocol::endpoint::default_endpoint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketPath<'a> {
    /// `socket_path` is unset or empty: use the endpoint precedence (`$KAKEMONO_SOCKET`, then the
    /// platform default).
    Default,
    /// `socket_path` as configured.
    Configured(&'a Path),
    /// The config came from the legacy Go location and `socket_path` is the Go default. It is
    /// ignored so kakemono and a still-running Go daemon don't fight over one socket. Treated like
    /// [`SocketPath::Default`]. Callers log [`SocketPath::notice`] once at info level.
    IgnoredLegacy(&'a Path),
}

impl<'a> SocketPath<'a> {
    /// The path to pass as the endpoint override, if any.
    pub fn override_path(&self) -> Option<&'a Path> {
        match self {
            SocketPath::Configured(p) => Some(p),
            SocketPath::Default | SocketPath::IgnoredLegacy(_) => None,
        }
    }

    /// The info line to log when the legacy socket was ignored; `None` otherwise.
    pub fn notice(&self) -> Option<String> {
        match self {
            SocketPath::IgnoredLegacy(p) => Some(format!(
                "ignoring socket_path {} from the legacy wallpaperd config; using the kakemono default",
                p.display()
            )),
            SocketPath::Default | SocketPath::Configured(_) => None,
        }
    }
}

impl Config {
    /// The one rule the daemon, the CLI client and the UI use to pick the socket from the config.
    ///
    /// A config loaded through the legacy fallback ([`ConfigSource::Legacy`]) usually still has
    /// the Go template's `socket_path = "~/.config/wallpaperd/daemon.sock"`. That value is ignored
    /// ([`SocketPath::IgnoredLegacy`]). Any other `socket_path`, or the Go default in a config from
    /// any other source, is honoured. `home` is the home directory the config was expanded with
    /// (`Paths::home`).
    pub fn effective_socket_path(&self, source: &ConfigSource, home: &Path) -> SocketPath<'_> {
        let Some(path) = self
            .daemon
            .socket_path
            .as_deref()
            .filter(|p| !p.as_os_str().is_empty())
        else {
            return SocketPath::Default;
        };
        let go_default = legacy_default_socket(home);
        let unexpanded = Path::new("~/.config")
            .join(LEGACY_APP_NAME)
            .join(LEGACY_SOCKET_FILE_NAME);
        if source.is_legacy() && (path == go_default || path == unexpanded) {
            SocketPath::IgnoredLegacy(path)
        } else {
            SocketPath::Configured(path)
        }
    }
}

/// Copies the legacy Go config to the kakemono path. Refuses to overwrite `to`. Leaves `from` in
/// place. Only call this when the user agreed.
pub fn copy_legacy_config(from: &Path, to: &Path) -> Result<(), ConfigError> {
    let text = std::fs::read(from).map_err(|source| ConfigError::Read {
        path: from.to_owned(),
        source,
    })?;
    write_new(to, &text)
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
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::CreateDir {
            path: parent.to_owned(),
            source,
        })?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write_err = |source| ConfigError::Write {
        path: path.to_owned(),
        source,
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

/// Default kakemono config file, honouring `$KAKEMONO_CONFIG` and `$XDG_CONFIG_HOME`. Does not
/// consider the legacy file; use [`Paths::locate_config`] for reading.
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
