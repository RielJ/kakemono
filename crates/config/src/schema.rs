//! `config.toml` schema.
//!
//! Mirrors Go's `internal/config.Config` key for key, plus the new optional `[desktop]`, `[hooks]` and
//! `[ui]` sections and the new `[daemon]`/`[picker]` keys (plan §I). Every struct is
//! `#[serde(default)]`, so a missing key takes its value from [`crate::defaults::defaults`], and
//! unknown keys are ignored, as BurntSushi/toml did.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

use crate::duration::{self, DurationError};

/// The whole `config.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub wallhaven: WallhavenConfig,
    pub unsplash: UnsplashConfig,
    pub reddit: RedditConfig,
    pub bing: BingConfig,
    pub apod: ApodConfig,
    pub local: LocalConfig,
    /// Keys missing inside a present `[light]` keep the light defaults, as Go's decode-over-defaults did.
    #[serde(with = "light_theme_serde")]
    #[ts(as = "ThemeConfig")]
    pub light: ThemeConfig,
    /// Keys missing inside a present `[dark]` keep the dark defaults.
    #[serde(with = "dark_theme_serde")]
    #[ts(as = "ThemeConfig")]
    pub dark: ThemeConfig,
    pub favorites: FavoritesConfig,
    pub picker: PickerConfig,
    /// New in the Rust rewrite (plan §E). Go ignores it.
    pub desktop: DesktopConfig,
    /// New in the Rust rewrite (plan §I). Go ignores it.
    pub hooks: HooksConfig,
    /// New in the Rust rewrite. Reserved for UI-only settings; currently empty.
    pub ui: UiConfig,
}

impl Default for Config {
    fn default() -> Self {
        crate::defaults::defaults()
    }
}

/// `[daemon]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct DaemonConfig {
    /// Whether automatic rotation is active.
    pub rotation_enabled: bool,
    /// Go duration string, e.g. `"30m"`, `"1h30m"`, `"1.5h"`. Kept as text so the UI round-trips it;
    /// use [`DaemonConfig::interval_duration`] to parse.
    pub interval: String,
    /// Download cache directory. `~` is expanded at load.
    #[ts(type = "string")]
    pub download_dir: PathBuf,
    /// Maximum number of images kept in `download_dir`.
    pub cache_size: u32,
    /// Trash directory. `~` is expanded at load.
    #[ts(type = "string")]
    pub trash_dir: PathBuf,
    /// IPC socket path. `None` means the platform default (plan §I, owned by `protocol::endpoint`).
    /// `~` is expanded at load. Ignored on Windows (named pipe).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | null")]
    pub socket_path: Option<PathBuf>,
    /// Rotate when the system theme flips light/dark. New; default `true`.
    pub rotate_on_theme_change: bool,
    /// Re-set the current image when the macOS Space changes. New; default `false`.
    pub reapply_on_space_change: bool,
}

impl DaemonConfig {
    /// Parses [`DaemonConfig::interval`] with Go `time.ParseDuration` grammar.
    pub fn interval_duration(&self) -> Result<Duration, DurationError> {
        duration::parse_go(&self.interval)
    }
}

/// `[wallhaven]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct WallhavenConfig {
    pub enabled: bool,
    /// Optional; empty means HTML-scrape mode.
    pub api_key: String,
    /// `"100"` = SFW, `"110"` = SFW+Sketchy, `"111"` = all.
    pub purity: String,
    /// `"random"`, `"toplist"`, `"date_added"`, `"relevance"`, `"views"`, `"favorites"`, `"hot"`.
    pub sorting: String,
    /// Three bits general/anime/people, `"111"` = all.
    pub categories: String,
}

/// `[unsplash]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct UnsplashConfig {
    pub enabled: bool,
    /// Optional access key; empty means not configured.
    pub access_key: String,
}

/// `[reddit]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct RedditConfig {
    pub enabled: bool,
    pub subreddits: Vec<String>,
}

/// `[bing]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct BingConfig {
    pub enabled: bool,
    /// Market code, e.g. `"en-US"`.
    pub market: String,
}

/// `[apod]` (Go `APODConfig`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct ApodConfig {
    pub enabled: bool,
    /// NASA API key; `"DEMO_KEY"` works for light use.
    pub api_key: String,
}

/// `[local]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct LocalConfig {
    pub enabled: bool,
    /// Directories to scan. `~` is expanded at load.
    #[ts(type = "Array<string>")]
    pub dirs: Vec<PathBuf>,
    pub recursive: bool,
}

/// `[light]` and `[dark]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct ThemeConfig {
    /// Provider names, e.g. `["bing", "wallhaven"]`.
    pub sources: Vec<String>,
    /// Search terms for providers that support them.
    pub queries: Vec<String>,
}

/// `[favorites]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct FavoritesConfig {
    /// `~` is expanded at load.
    #[ts(type = "string")]
    pub dir: PathBuf,
}

/// `[picker]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct PickerConfig {
    /// Screen edge for `placement = "edge"`.
    pub position: PickerPosition,
    /// Relaunch the picker with `GDK_BACKEND=x11` on Wayland. New; default `false`.
    pub force_x11: bool,
    /// Close the picker when it loses focus. New; default `true`.
    pub close_on_blur: bool,
    /// `edge` (default) or `custom` (user dragged it; see `custom`). New.
    pub placement: PickerPlacement,
    /// Saved user position, used when `placement = "custom"`. New.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<PickerCustom>,
}

/// `[picker] position`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum PickerPosition {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

/// `[picker] placement`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum PickerPlacement {
    #[default]
    Edge,
    Custom,
}

/// `[picker] custom = { x, y, monitor }`. `monitor` is the monitor name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct PickerCustom {
    pub x: i32,
    pub y: i32,
    pub monitor: String,
}

/// `[desktop]` (plan §E). Selects and remembers the wallpaper backend.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct DesktopConfig {
    /// `auto` (default) resolves at runtime; anything else is pinned.
    pub backend: Backend,
    /// Argv for `command` and preset backends. Accepts a TOML array or a string split with shell-words.
    /// Empty array/string loads as `None`.
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_command_serde")]
    #[ts(type = "Array<string> | null")]
    pub set_command: Option<CommandSpec>,
    /// `true` = the command stays running (swaybg-style) and the previous instance is replaced.
    pub persistent: bool,
    pub fit: Fit,
    /// Theme override when the platform can't report one (e.g. bare X11 WMs).
    pub theme: ThemeOverride,
    /// `detect` = re-detection may overwrite `[desktop]`; `user` = never touch.
    pub managed_by: ManagedBy,
    /// Session key from the last detection, e.g. `"hyprland:wayland"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_session: Option<String>,
    /// RFC 3339 time of the last detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | null")]
    pub detected_at: Option<DateTime<Utc>>,
}

/// `[desktop] backend`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    #[default]
    Auto,
    /// macOS NSWorkspace / Windows IDesktopWallpaper.
    Native,
    Gnome,
    Kde,
    Xfce,
    Cinnamon,
    Mate,
    Sway,
    Hyprpaper,
    Awww,
    Swww,
    Swaybg,
    Wbg,
    Xwallpaper,
    Feh,
    Nitrogen,
    Hsetroot,
    Pcmanfm,
    PcmanfmQt,
    Command,
}

impl Backend {
    /// Every variant, in declaration order.
    pub const ALL: [Backend; 20] = [
        Backend::Auto,
        Backend::Native,
        Backend::Gnome,
        Backend::Kde,
        Backend::Xfce,
        Backend::Cinnamon,
        Backend::Mate,
        Backend::Sway,
        Backend::Hyprpaper,
        Backend::Awww,
        Backend::Swww,
        Backend::Swaybg,
        Backend::Wbg,
        Backend::Xwallpaper,
        Backend::Feh,
        Backend::Nitrogen,
        Backend::Hsetroot,
        Backend::Pcmanfm,
        Backend::PcmanfmQt,
        Backend::Command,
    ];

    /// The config string, e.g. `"pcmanfm-qt"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Auto => "auto",
            Backend::Native => "native",
            Backend::Gnome => "gnome",
            Backend::Kde => "kde",
            Backend::Xfce => "xfce",
            Backend::Cinnamon => "cinnamon",
            Backend::Mate => "mate",
            Backend::Sway => "sway",
            Backend::Hyprpaper => "hyprpaper",
            Backend::Awww => "awww",
            Backend::Swww => "swww",
            Backend::Swaybg => "swaybg",
            Backend::Wbg => "wbg",
            Backend::Xwallpaper => "xwallpaper",
            Backend::Feh => "feh",
            Backend::Nitrogen => "nitrogen",
            Backend::Hsetroot => "hsetroot",
            Backend::Pcmanfm => "pcmanfm",
            Backend::PcmanfmQt => "pcmanfm-qt",
            Backend::Command => "command",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How the image is scaled. Defined here (not in `platform`) because `[desktop] fit` needs it and
/// `platform` depends on `config`; `platform` should re-export this type (plan §B shape).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    #[default]
    Fill,
    Fit,
    Center,
    Stretch,
    Tile,
}

impl Fit {
    /// The config string, e.g. `"fill"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Fit::Fill => "fill",
            Fit::Fit => "fit",
            Fit::Center => "center",
            Fit::Stretch => "stretch",
            Fit::Tile => "tile",
        }
    }
}

impl std::fmt::Display for Fit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `[desktop] theme`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum ThemeOverride {
    #[default]
    Auto,
    Light,
    Dark,
}

/// `[desktop] managed_by`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum ManagedBy {
    /// Re-detection may overwrite `[desktop]`.
    #[default]
    Detect,
    /// The user edited it; never overwrite without `--force`.
    User,
}

/// `[hooks]` (plan §I).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct HooksConfig {
    /// Run after every wallpaper change. Array or shell-words string; empty loads as `None`.
    #[serde(skip_serializing_if = "Option::is_none", with = "opt_command_serde")]
    #[ts(type = "Array<string> | null")]
    pub on_change: Option<CommandSpec>,
}

/// `[ui]`. Reserved for UI-only settings; no keys yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct UiConfig {}

/// A non-empty argv vector. Never run through a shell (plan §E "Safe command execution").
///
/// Deserializes from a TOML array of strings, or from a string split with `shell-words`.
/// Serializes as an array.
#[derive(Clone, Debug, PartialEq, Eq, TS)]
#[ts(type = "Array<string>")]
pub struct CommandSpec(Vec<String>);

/// Why a [`CommandSpec`] could not be built.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommandSpecError {
    #[error("command must not be empty")]
    Empty,
    #[error("cannot split command {line:?}: {reason}")]
    Split { line: String, reason: String },
}

impl CommandSpec {
    /// Wraps an argv. Fails if it is empty.
    pub fn new(argv: Vec<String>) -> Result<Self, CommandSpecError> {
        if argv.is_empty() {
            return Err(CommandSpecError::Empty);
        }
        Ok(Self(argv))
    }

    /// Splits `line` with shell-words (quotes and escapes only; no shell runs). Fails if empty.
    pub fn parse(line: &str) -> Result<Self, CommandSpecError> {
        let argv = shell_words::split(line).map_err(|e| CommandSpecError::Split {
            line: line.to_owned(),
            reason: e.to_string(),
        })?;
        Self::new(argv)
    }

    /// The full argv, program first.
    pub fn argv(&self) -> &[String] {
        &self.0
    }

    /// `argv[0]`.
    pub fn program(&self) -> &str {
        // Invariant: never empty (checked in `new`).
        self.0.first().map(String::as_str).unwrap_or_default()
    }

    /// Arguments after the program.
    pub fn args(&self) -> &[String] {
        self.0.get(1..).unwrap_or_default()
    }

    /// Consumes into the argv.
    pub fn into_argv(self) -> Vec<String> {
        self.0
    }
}

// Hand-written (not `#[serde(try_from, into)]`) so ts-rs sees no serde attributes it can't parse.
impl Serialize for CommandSpec {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for CommandSpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        CommandRepr::deserialize(d)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl From<CommandSpec> for Vec<String> {
    fn from(c: CommandSpec) -> Self {
        c.0
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CommandRepr {
    Argv(Vec<String>),
    Line(String),
}

impl TryFrom<CommandRepr> for CommandSpec {
    type Error = CommandSpecError;

    fn try_from(r: CommandRepr) -> Result<Self, Self::Error> {
        match r {
            CommandRepr::Argv(v) => CommandSpec::new(v),
            CommandRepr::Line(s) => CommandSpec::parse(&s),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ThemeConfigPartial {
    sources: Option<Vec<String>>,
    queries: Option<Vec<String>>,
}

fn merge_theme(partial: ThemeConfigPartial, base: ThemeConfig) -> ThemeConfig {
    ThemeConfig {
        sources: partial.sources.unwrap_or(base.sources),
        queries: partial.queries.unwrap_or(base.queries),
    }
}

// `#[serde(with = ...)]` modules: ts-rs accepts `with` (unlike `deserialize_with`), so these
// keep the build free of ts-rs attribute warnings.

/// `[light]`: keys missing in a present table keep the light defaults.
mod light_theme_serde {
    use super::*;

    pub fn serialize<S: Serializer>(t: &ThemeConfig, s: S) -> Result<S::Ok, S::Error> {
        t.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ThemeConfig, D::Error> {
        let partial = ThemeConfigPartial::deserialize(d)?;
        Ok(merge_theme(partial, crate::defaults::light_theme()))
    }
}

/// `[dark]`: keys missing in a present table keep the dark defaults.
mod dark_theme_serde {
    use super::*;

    pub fn serialize<S: Serializer>(t: &ThemeConfig, s: S) -> Result<S::Ok, S::Error> {
        t.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ThemeConfig, D::Error> {
        let partial = ThemeConfigPartial::deserialize(d)?;
        Ok(merge_theme(partial, crate::defaults::dark_theme()))
    }
}

/// Optional command: empty array or blank string → `None`; otherwise a [`CommandSpec`].
mod opt_command_serde {
    use super::*;

    pub fn serialize<S: Serializer>(c: &Option<CommandSpec>, s: S) -> Result<S::Ok, S::Error> {
        c.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<CommandSpec>, D::Error> {
        let raw = Option::<CommandRepr>::deserialize(d)?;
        match raw {
            None => Ok(None),
            Some(CommandRepr::Argv(v)) if v.is_empty() => Ok(None),
            Some(CommandRepr::Line(s)) if s.trim().is_empty() => Ok(None),
            Some(r) => CommandSpec::try_from(r)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}
