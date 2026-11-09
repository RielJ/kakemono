//! The platform seam: the only types the engine sees (plan §B).
//!
//! `WallpaperSetter` applies an image to every display; `SystemSignals` reports the theme and
//! streams OS events. They are separate because Linux combines them independently (theme from the
//! portal, wallpaper from e.g. awww). `Platform` bundles one of each.

use std::fmt;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The system colour scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    /// The lowercase name used in config, `{theme}` placeholders and hook env.
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

impl Theme {
    /// The `[desktop] theme` override, if it pins a theme (`auto` returns `None`).
    pub fn from_override(theme: kakemono_config::schema::ThemeOverride) -> Option<Self> {
        use kakemono_config::schema::ThemeOverride;
        match theme {
            ThemeOverride::Auto => None,
            ThemeOverride::Light => Some(Theme::Light),
            ThemeOverride::Dark => Some(Theme::Dark),
        }
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Theme {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "light" => Ok(Theme::Light),
            "dark" => Ok(Theme::Dark),
            _ => Err(ParseEnumError {
                kind: "theme",
                value: s.to_owned(),
            }),
        }
    }
}

/// How the image is scaled onto each display; the only per-set option (no per-monitor images).
/// Defined in `kakemono-config` because `[desktop] fit` uses it; re-exported here as the seam type.
pub use kakemono_config::schema::Fit;

/// Returned by `Theme::from_str` for an unknown name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} {value:?}")]
pub struct ParseEnumError {
    kind: &'static str,
    value: String,
}

/// An OS event the engine reacts to (plan §D).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// The system theme flipped. Emitted only on change (deduplicated by the platform).
    ThemeChanged(Theme),
    /// A display was added, removed or reconfigured.
    DisplaysChanged,
    /// The active macOS Space changed.
    SpaceChanged,
    /// The machine woke from sleep.
    Resumed,
}

/// Which backend sets the wallpaper, for `status`, `doctor` and logs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackendInfo {
    /// Short backend id, e.g. `"macos"`, `"gnome"`, `"awww"`, `"unconfigured"`.
    pub name: String,
    /// Human-readable detail, e.g. the rendered argv or the API used.
    pub detail: String,
}

/// How an external command ended, for `SetError::Command`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandStatus {
    /// The process exited with this code.
    Exited(i32),
    /// The process was killed by a signal (Unix) or ended without an exit code.
    Signaled,
    /// The process ran past its timeout and was killed.
    TimedOut(Duration),
    /// The process could not be started (not found, not executable, ...).
    FailedToStart,
}

impl CommandStatus {
    /// True only for `Exited(0)`.
    pub fn success(self) -> bool {
        self == CommandStatus::Exited(0)
    }
}

impl fmt::Display for CommandStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandStatus::Exited(code) => write!(f, "exit code {code}"),
            CommandStatus::Signaled => f.write_str("killed by a signal"),
            CommandStatus::TimedOut(after) => write!(f, "timed out after {}s", after.as_secs_f64()),
            CommandStatus::FailedToStart => f.write_str("failed to start"),
        }
    }
}

/// Why setting the wallpaper failed. Crosses IPC as its `Display` string.
#[derive(Debug, thiserror::Error)]
pub enum SetError {
    /// No usable backend. `hint` says what to do (run `kakemono doctor`, set `set_command`, ...).
    #[error("{hint}")]
    Unconfigured { hint: String },
    /// An external setter command failed. `stderr` is trimmed and capped.
    #[error("{argv0}: {status}{}", if stderr.is_empty() { String::new() } else { format!(": {stderr}") })]
    Command {
        argv0: String,
        status: CommandStatus,
        stderr: String,
    },
    /// A D-Bus call (KDE, XFCE, portal) failed.
    #[error("D-Bus: {0}")]
    Dbus(String),
    /// A native OS API (NSWorkspace, IDesktopWallpaper, ...) failed.
    #[error("{0}")]
    Os(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The hint used when no backend is available.
pub const UNCONFIGURED_HINT: &str =
    "No wallpaper backend detected. Run `kakemono doctor`, or set `[desktop] set_command`.";

/// Sets the wallpaper on every display.
#[async_trait::async_trait]
pub trait WallpaperSetter: Send + Sync + 'static {
    /// Set `image` on every display. Must be idempotent (the engine re-applies on display change and wake,
    /// and on Space change when `reapply_on_space_change` is on).
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError>;
    fn info(&self) -> BackendInfo;

    /// The setter for a changed `[desktop]` (the daemon's `reload`, Task 37b). The engine calls
    /// it under its op lock, so no `set` on this setter is running or starts meanwhile, and only
    /// when `[desktop]` differs from the one in use.
    ///
    /// - `Ok(None)`: this setter does not depend on `[desktop]` (beyond `fit`, which every `set`
    ///   receives). Keep it. The default, and what macOS and Windows return.
    /// - `Ok(Some(new))`: use `new` from now on. Before returning, this setter has released what
    ///   `new` does not take over (e.g. stopped a persistent setter process), so dropping it
    ///   leaks nothing.
    /// - `Err`: `desktop` cannot be used. Nothing was stopped or changed; this setter stays in
    ///   use.
    async fn rebuild(
        &self,
        desktop: &kakemono_config::schema::DesktopConfig,
    ) -> Result<Option<Arc<dyn WallpaperSetter>>, SetError> {
        let _ = desktop;
        Ok(None)
    }
}

/// Reports the current theme and streams OS events.
pub trait SystemSignals: Send + Sync + 'static {
    fn theme(&self) -> Theme;
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Signal>;
}

/// What `host()` hands to the daemon.
#[derive(Clone)]
pub struct Platform {
    pub setter: Arc<dyn WallpaperSetter>,
    pub signals: Arc<dyn SystemSignals>,
}

impl fmt::Debug for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Platform")
            .field("setter", &self.setter.info())
            .field("theme", &self.signals.theme())
            .finish()
    }
}

/// A setter that always fails with `SetError::Unconfigured`. Used when detection finds nothing,
/// and by the per-OS stubs until their backends land.
#[derive(Clone, Debug)]
pub struct UnconfiguredSetter {
    hint: String,
}

impl UnconfiguredSetter {
    pub fn new(hint: impl Into<String>) -> Self {
        Self { hint: hint.into() }
    }
}

impl Default for UnconfiguredSetter {
    fn default() -> Self {
        Self::new(UNCONFIGURED_HINT)
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for UnconfiguredSetter {
    async fn set(&self, _image: &Path, _fit: Fit) -> Result<(), SetError> {
        Err(SetError::Unconfigured {
            hint: self.hint.clone(),
        })
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "unconfigured".to_owned(),
            detail: self.hint.clone(),
        }
    }
}

/// Signals with a fixed theme that never emit. Used when no theme source exists, and by stubs.
#[derive(Debug)]
pub struct StaticSignals {
    theme: Theme,
    tx: tokio::sync::broadcast::Sender<Signal>,
}

impl StaticSignals {
    pub fn new(theme: Theme) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(1);
        Self { theme, tx }
    }
}

impl SystemSignals for StaticSignals {
    fn theme(&self) -> Theme {
        self.theme
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Signal> {
        self.tx.subscribe()
    }
}

impl Platform {
    /// A platform whose setter fails with `Unconfigured { hint }` and whose theme is fixed.
    pub fn unconfigured(hint: impl Into<String>, theme: Theme) -> Self {
        Self {
            setter: Arc::new(UnconfiguredSetter::new(hint)),
            signals: Arc::new(StaticSignals::new(theme)),
        }
    }

    /// [`Platform::unconfigured`] with [`UNCONFIGURED_HINT`] and the `[desktop] theme` override
    /// (light when `auto`). The fallback when no backend is usable.
    pub fn unconfigured_for(cfg: &kakemono_config::schema::DesktopConfig) -> Self {
        let theme = Theme::from_override(cfg.theme).unwrap_or(Theme::Light);
        Self::unconfigured(UNCONFIGURED_HINT, theme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_round_trip() {
        for theme in [Theme::Light, Theme::Dark] {
            assert_eq!(theme.to_string().parse::<Theme>(), Ok(theme));
            assert_eq!(
                serde_json::to_string(&theme).unwrap(),
                format!("\"{}\"", theme.as_str())
            );
        }
        assert!("Dark".parse::<Theme>().is_err());
        assert_eq!(Fit::default(), Fit::Fill);
        assert_eq!(Fit::Tile.as_str(), "tile");
    }

    #[test]
    fn theme_from_override() {
        use kakemono_config::schema::ThemeOverride;
        assert_eq!(Theme::from_override(ThemeOverride::Auto), None);
        assert_eq!(Theme::from_override(ThemeOverride::Dark), Some(Theme::Dark));
        assert_eq!(
            Theme::from_override(ThemeOverride::Light),
            Some(Theme::Light)
        );
    }

    #[test]
    fn set_error_messages() {
        let err = SetError::Command {
            argv0: "awww".into(),
            status: CommandStatus::Exited(1),
            stderr: "no daemon".into(),
        };
        assert_eq!(err.to_string(), "awww: exit code 1: no daemon");
        let err = SetError::Command {
            argv0: "feh".into(),
            status: CommandStatus::TimedOut(Duration::from_secs(15)),
            stderr: String::new(),
        };
        assert_eq!(err.to_string(), "feh: timed out after 15s");
    }

    #[tokio::test]
    async fn unconfigured_platform_fails_with_hint() {
        let p = Platform::unconfigured(UNCONFIGURED_HINT, Theme::Dark);
        let err = p
            .setter
            .set(Path::new("/tmp/x.jpg"), Fit::Fill)
            .await
            .unwrap_err();
        assert!(matches!(err, SetError::Unconfigured { .. }));
        assert_eq!(err.to_string(), UNCONFIGURED_HINT);
        assert_eq!(p.setter.info().name, "unconfigured");
        assert_eq!(p.signals.theme(), Theme::Dark);
    }
}
