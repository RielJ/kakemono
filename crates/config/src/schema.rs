//! `config.toml` schema.
//!
//! Starts from Go's `internal/config.Config`, plus the new optional `[desktop]`, `[hooks]` and
//! `[ui]` sections, the new `[daemon]`/`[picker]` keys, `[[sources]]` and `[filters]`.
//! Every struct is `#[serde(default)]`, so a missing key takes its value from
//! [`crate::defaults::defaults`], and unknown keys are ignored, as BurntSushi/toml did.
//!
//! The legacy keys (`[light]`, `[dark]`, `daemon.cache_size`, each provider's `enabled`,
//! `[reddit]`, `[local]`) are not part of the schema: [`crate::migrate`] reads them into
//! `[[sources]]` through its own copies, and any left in a file are ignored like other unknown
//! keys.

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
    pub bing: BingConfig,
    pub apod: ApodConfig,
    /// `[[sources]]`: one row per source instance. A missing key gives the default rows; an
    /// explicit `sources = []` stays empty.
    pub sources: Vec<SourceEntry>,
    /// `[filters]`: soft filters applied when picking and downloading images.
    pub filters: FiltersConfig,
    pub favorites: FavoritesConfig,
    pub picker: PickerConfig,
    /// New in the Rust rewrite. Go ignores it.
    pub desktop: DesktopConfig,
    /// New in the Rust rewrite. Go ignores it.
    pub hooks: HooksConfig,
    /// New in the Rust rewrite. Settings only the UI reads.
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
    /// Trash directory. `~` is expanded at load.
    #[ts(type = "string")]
    pub trash_dir: PathBuf,
    /// IPC socket path. `None` means the platform default (see `protocol::endpoint`).
    /// `~` is expanded at load. Ignored on Windows (named pipe).
    #[ts(type = "string | null")]
    pub socket_path: Option<PathBuf>,
    /// Rotate when the system theme flips light/dark. New; default `true`.
    pub rotate_on_theme_change: bool,
    /// Re-set the current image when the macOS Space changes. New; default `false`.
    pub reapply_on_space_change: bool,
    /// Change the wallpaper once when the daemon starts. Default `false`.
    pub change_on_start: bool,
    /// Whether online sources may be used at all (downloads and on-demand fetches). Default `true`.
    pub internet: bool,
    /// Whether the background downloader runs. Default `true`.
    pub download_enabled: bool,
    /// Go duration between background downloads, e.g. `"10m"`; use
    /// [`DaemonConfig::download_interval_duration`] to parse.
    pub download_interval: String,
    /// Whether [`DaemonConfig::download_limit_mb`] is enforced on `download_dir`. Default `true`.
    pub download_limit_mb_enabled: bool,
    /// Size limit for downloaded images in MB; the oldest downloads are deleted first. Default 500.
    pub download_limit_mb: u32,
    /// Folder for dropped and fetched images. `None` means `<download_dir>/manual`; use
    /// [`DaemonConfig::manual_dir_resolved`]. `~` is expanded at load.
    #[ts(type = "string | null")]
    pub manual_dir: Option<PathBuf>,
}

impl DaemonConfig {
    /// Parses [`DaemonConfig::interval`] with Go `time.ParseDuration` grammar.
    pub fn interval_duration(&self) -> Result<Duration, DurationError> {
        duration::parse_go(&self.interval)
    }

    /// Parses [`DaemonConfig::download_interval`] with Go `time.ParseDuration` grammar.
    pub fn download_interval_duration(&self) -> Result<Duration, DurationError> {
        duration::parse_go(&self.download_interval)
    }

    /// [`DaemonConfig::manual_dir`], or `<download_dir>/manual` when unset.
    pub fn manual_dir_resolved(&self) -> PathBuf {
        self.manual_dir
            .clone()
            .unwrap_or_else(|| self.download_dir.join("manual"))
    }
}

/// `[wallhaven]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct WallhavenConfig {
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
    /// Optional access key; empty means not configured.
    pub access_key: String,
}

/// `[bing]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct BingConfig {
    /// Market code, e.g. `"en-US"`.
    pub market: String,
}

/// `[apod]` (Go `APODConfig`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct ApodConfig {
    /// NASA API key; `"DEMO_KEY"` works for light use.
    pub api_key: String,
}

/// One `[[sources]]` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct SourceEntry {
    /// `[a-z0-9-]{1,32}`; names the download folder `downloads/<id>/`.
    pub id: String,
    pub enabled: bool,
    #[serde(rename = "type")]
    pub kind: SourceType,
    /// Keywords, URL, subreddit or path; empty for `bing` and `apod`. `~` is expanded at load for
    /// local types.
    pub location: String,
}

impl Default for SourceEntry {
    /// A row missing `enabled` is on: the user wrote it to use it. A row missing `type` is
    /// [`SourceType::Unsupported`], which the engine skips, never a guessed source.
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            kind: SourceType::Unsupported,
            location: String::new(),
        }
    }
}

/// `[[sources]] type`. A string this version doesn't know loads as `unsupported` instead of failing
/// the whole load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SourceType {
    Wallhaven,
    Unsplash,
    Reddit,
    #[default]
    Bing,
    Apod,
    /// A folder of images, picked at random, recursive.
    Folder,
    /// Individual image files.
    Images,
    /// A folder shown in file-name order.
    AlbumName,
    /// A folder shown in file-date order.
    AlbumDate,
    /// A type this version doesn't know. The engine skips it; the UI shows "Unknown".
    Unsupported,
}

impl SourceType {
    /// Every variant, in declaration order.
    pub const ALL: [SourceType; 10] = [
        SourceType::Wallhaven,
        SourceType::Unsplash,
        SourceType::Reddit,
        SourceType::Bing,
        SourceType::Apod,
        SourceType::Folder,
        SourceType::Images,
        SourceType::AlbumName,
        SourceType::AlbumDate,
        SourceType::Unsupported,
    ];

    /// The config string, e.g. `"album-name"`.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceType::Wallhaven => "wallhaven",
            SourceType::Unsplash => "unsplash",
            SourceType::Reddit => "reddit",
            SourceType::Bing => "bing",
            SourceType::Apod => "apod",
            SourceType::Folder => "folder",
            SourceType::Images => "images",
            SourceType::AlbumName => "album-name",
            SourceType::AlbumDate => "album-date",
            SourceType::Unsupported => "unsupported",
        }
    }

    /// Parses a config string. Anything unrecognised is [`SourceType::Unsupported`].
    pub fn from_config_str(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|t| *t != SourceType::Unsupported && t.as_str() == s)
            .unwrap_or(SourceType::Unsupported)
    }

    /// Whether `location` is a filesystem path (`~` is expanded at load).
    pub fn is_local(self) -> bool {
        matches!(
            self,
            SourceType::Folder | SourceType::Images | SourceType::AlbumName | SourceType::AlbumDate
        )
    }
}

impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// Hand-written (not `#[serde(other)]`) so ts-rs sees no serde attribute it can't parse.
impl<'de> Deserialize<'de> for SourceType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::from_config_str(&s))
    }
}

/// `[filters]`. Every filter is off by default. Filters are soft: when no image passes, the picker
/// takes the one that passes the most.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct FiltersConfig {
    /// Only landscape images (width > height).
    pub landscape: bool,
    pub min_size_enabled: bool,
    /// Minimum image size as a percentage of the screen size: 50, 80 or 100. Default 80.
    pub min_size_percent: u32,
    pub brightness: BrightnessFilter,
    pub color_enabled: bool,
    /// Target colour as `"#rrggbb"`. Default `"#3b82f6"`.
    pub color: String,
    pub rating_enabled: bool,
    /// Minimum EXIF/XMP rating, 1–5. Default 4.
    pub min_rating: u8,
    pub name_regex_enabled: bool,
    /// Regular expression the file name must match (Rust `regex` syntax).
    pub name_regex: String,
}

/// `[filters] brightness`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum BrightnessFilter {
    #[default]
    Off,
    /// Only dark images.
    Dark,
    /// Only light images.
    Light,
    /// Dark images in dark mode, light images in light mode.
    System,
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
    /// Strip direction. `auto` follows `position` (vertical on left/right). New.
    pub orientation: PickerOrientation,
    /// Relaunch the picker with `GDK_BACKEND=x11` on Wayland. New; default `false`.
    pub force_x11: bool,
    /// Close the picker when it loses focus. New; default `true`.
    pub close_on_blur: bool,
    /// Blur NSFW browse previews in the picker until hover or focus. New; default `true`.
    pub blur_nsfw: bool,
    /// `edge` (default) or `custom` (user dragged it; see `custom`). New.
    pub placement: PickerPlacement,
    /// Saved user position, used when `placement = "custom"`. New.
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

/// `[picker] orientation`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum PickerOrientation {
    /// Vertical on the left and right edges, horizontal on top and bottom.
    #[default]
    Auto,
    Horizontal,
    Vertical,
}

impl PickerOrientation {
    /// Whether the strip runs vertically when docked at `position`.
    pub fn is_vertical(self, position: PickerPosition) -> bool {
        match self {
            Self::Auto => matches!(position, PickerPosition::Left | PickerPosition::Right),
            Self::Horizontal => false,
            Self::Vertical => true,
        }
    }
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

/// `[desktop]`. Selects and remembers the wallpaper backend.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct DesktopConfig {
    /// `auto` (default) resolves at runtime; anything else is pinned.
    pub backend: Backend,
    /// Argv for `command` and preset backends. Accepts a TOML array or a string split with shell-words.
    /// Empty array/string loads as `None`.
    #[serde(with = "opt_command_serde")]
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
    pub detected_session: Option<String>,
    /// RFC 3339 time of the last detection.
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
/// `platform` depends on `config`; `platform` should re-export this type.
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

/// `[hooks]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct HooksConfig {
    /// Run after every wallpaper change. Array or shell-words string; empty loads as `None`.
    #[serde(with = "opt_command_serde")]
    #[ts(type = "Array<string> | null")]
    pub on_change: Option<CommandSpec>,
}

/// `[ui]`. Settings only the UI reads; the daemon ignores them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct UiConfig {
    /// Colour scheme of the settings window. New; default `light`.
    pub appearance: Appearance,
}

/// `[ui] appearance`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    Light,
    Dark,
    /// Follow the system light/dark setting.
    System,
}

/// A non-empty argv vector. Never run through a shell.
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

// `#[serde(with = ...)]` modules: ts-rs accepts `with` (unlike `deserialize_with`), so these
// keep the build free of ts-rs attribute warnings.

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
