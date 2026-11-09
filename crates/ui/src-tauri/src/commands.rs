//! Tauri commands for the settings window and the picker (plan §H).
//!
//! Every command is a thin wrapper: daemon calls go through the shared [`DaemonLink`], config
//! reads and writes through `kakemono-config` (writes use its `toml_edit` writer, so comments
//! and formatting survive). The file-system helpers are plain functions so tests can run them
//! against temp dirs.
//!
//! Errors reach the frontend as `{ kind, message }` ([`CommandError`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_config::paths::{ConfigSource, Paths};
use kakemono_config::{Config, ConfigError};
use kakemono_protocol::types::{Request, StatusData, WallpaperInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_opener::OpenerExt;

use crate::daemon_link::{DaemonLink, LinkError};
use crate::thumbs::Thumbs;

/// The picker shows at most this many entries, as the Go picker did.
pub const MAX_THUMBNAILS: usize = 100;

/// Image extensions the picker lists from the cache and favourites dirs (Go's set).
const IMAGE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// A failed command, serialised for the frontend as `{ "kind": "...", "message": "..." }`.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Link(#[from] LinkError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Bad input from the frontend (empty path, invalid interval, non-object patch).
    #[error("{0}")]
    Invalid(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Opener(String),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

impl CommandError {
    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Stable machine-readable kind. `daemon_not_running` lets the UI show "Daemon not
    /// running" instead of an error.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Link(err) if err.is_not_running() => "daemon_not_running",
            Self::Link(_) => "daemon",
            Self::Config(_) => "config",
            Self::Invalid(_) => "invalid",
            Self::Io { .. } => "io",
            Self::Opener(_) => "opener",
            Self::Tauri(_) => "internal",
        }
    }
}

impl Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("CommandError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

type CmdResult<T> = Result<T, CommandError>;

/// Where the config lives right now: `$KAKEMONO_CONFIG`, the kakemono file, the Go
/// `wallpaperd` file, or (missing) the kakemono path. Edits target the same file.
fn locate_config() -> CmdResult<(Paths, ConfigSource)> {
    let paths = Paths::detect()?;
    let source = paths.locate_config(None);
    Ok((paths, source))
}

fn load_config() -> CmdResult<Config> {
    let (_, source) = locate_config()?;
    Ok(kakemono_config::load_or_default(source.path())?)
}

// ── Config ──────────────────────────────────────────────────────────────────

/// `get_config` → the effective config (defaults filled in, `~` expanded).
#[tauri::command]
pub async fn get_config() -> CmdResult<Config> {
    blocking(load_config).await
}

/// Result of [`save_config`]. The file is written either way; `reloaded` says whether the
/// daemon picked it up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveOutcome {
    /// The daemon answered IPC `reload`.
    pub reloaded: bool,
    /// Why the reload failed. `None` when it worked or the daemon isn't running.
    pub reload_error: Option<String>,
}

/// `save_config { patch }`: merges a partial config (only the changed keys; `null` removes a
/// key) into `config.toml` with the comment-preserving writer, then sends IPC `reload`.
#[tauri::command]
pub async fn save_config(link: State<'_, Arc<DaemonLink>>, patch: Json) -> CmdResult<SaveOutcome> {
    let (_, source) = locate_config()?;
    let path = source.path().to_owned();
    blocking(move || write_patch(&path, &patch)).await?;
    Ok(reload(&link).await)
}

/// Validates `patch` and applies it to `path`. Split out for tests.
pub fn write_patch(path: &Path, patch: &Json) -> CmdResult<()> {
    validate_patch(patch)?;
    kakemono_config::edit::apply_patch(path, patch)?;
    tracing::info!(config = %path.display(), "config saved");
    Ok(())
}

/// Checks what the schema can't: the patch is an object and a new `daemon.interval` is a
/// positive Go duration.
pub fn validate_patch(patch: &Json) -> CmdResult<()> {
    let Json::Object(obj) = patch else {
        return Err(CommandError::Invalid(
            "config patch must be a JSON object".into(),
        ));
    };
    if let Some(interval) = obj.get("daemon").and_then(|d| d.get("interval")) {
        let Some(text) = interval.as_str() else {
            return Err(CommandError::Invalid(
                "daemon.interval must be a string".into(),
            ));
        };
        let parsed = kakemono_config::parse_go(text)
            .map_err(|e| CommandError::Invalid(format!("daemon.interval: {e}")))?;
        if parsed.is_zero() {
            return Err(CommandError::Invalid(
                "daemon.interval must be greater than zero".into(),
            ));
        }
    }
    Ok(())
}

/// IPC `reload`. A stopped daemon is not an error: it reads the file when it starts.
pub async fn reload(link: &DaemonLink) -> SaveOutcome {
    match link.call(Request::Reload).await {
        Ok(_) => SaveOutcome {
            reloaded: true,
            reload_error: None,
        },
        Err(err) if err.is_not_running() => SaveOutcome {
            reloaded: false,
            reload_error: None,
        },
        Err(err) => {
            tracing::warn!(error = %err, "config saved but the daemon did not reload it");
            SaveOutcome {
                reloaded: false,
                reload_error: Some(err.to_string()),
            }
        }
    }
}

// ── Daemon control ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn status(link: State<'_, Arc<DaemonLink>>) -> CmdResult<StatusData> {
    Ok(link.status().await?)
}

async fn simple(link: &DaemonLink, request: Request) -> CmdResult<()> {
    link.call(request).await?;
    Ok(())
}

#[tauri::command]
pub async fn next(link: State<'_, Arc<DaemonLink>>) -> CmdResult<()> {
    simple(&link, Request::Next).await
}

#[tauri::command]
pub async fn prev(link: State<'_, Arc<DaemonLink>>) -> CmdResult<()> {
    simple(&link, Request::Prev).await
}

#[tauri::command]
pub async fn pause<R: Runtime>(
    app: AppHandle<R>,
    link: State<'_, Arc<DaemonLink>>,
) -> CmdResult<()> {
    simple(&link, Request::Pause).await?;
    crate::tray::set_pause_label(&app, true);
    Ok(())
}

#[tauri::command]
pub async fn resume<R: Runtime>(
    app: AppHandle<R>,
    link: State<'_, Arc<DaemonLink>>,
) -> CmdResult<()> {
    simple(&link, Request::Resume).await?;
    crate::tray::set_pause_label(&app, false);
    Ok(())
}

/// `trash { expected }`: with a path, the daemon trashes only if that is still the current
/// wallpaper (checked under its op lock), else it fails and trashes nothing.
#[tauri::command]
pub async fn trash(link: State<'_, Arc<DaemonLink>>, expected: Option<String>) -> CmdResult<()> {
    simple(&link, Request::Trash { expected }).await
}

/// `favorite { path }`: `null` favourites the current wallpaper through the daemon; a path
/// (from the picker) is copied into `[favorites] dir`, as the Go picker did.
#[tauri::command]
pub async fn favorite(link: State<'_, Arc<DaemonLink>>, path: Option<String>) -> CmdResult<()> {
    match path.filter(|p| !p.is_empty()) {
        None => simple(&link, Request::Favorite).await,
        Some(path) => {
            blocking(move || {
                let cfg = load_config()?;
                copy_to_favorites(Path::new(&path), &cfg.favorites.dir).map(|_| ())
            })
            .await
        }
    }
}

/// Copies `src` into `dir` (created if missing) under its file name. Returns the new path.
pub fn copy_to_favorites(src: &Path, dir: &Path) -> CmdResult<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| CommandError::Invalid(format!("not a file: {}", src.display())))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| CommandError::io(format!("creating {}", dir.display()), e))?;
    let dest = dir.join(name);
    if dest != src {
        std::fs::copy(src, &dest)
            .map_err(|e| CommandError::io(format!("copying {}", src.display()), e))?;
    }
    Ok(dest)
}

/// `set_file { path }`: IPC `set`.
#[tauri::command]
pub async fn set_file(link: State<'_, Arc<DaemonLink>>, path: String) -> CmdResult<()> {
    if path.is_empty() {
        return Err(CommandError::Invalid("set_file: path is empty".into()));
    }
    simple(&link, Request::Set { path }).await
}

/// `doctor` → the daemon's diagnostics report (IPC `doctor` data), as JSON.
#[tauri::command]
pub async fn doctor(link: State<'_, Arc<DaemonLink>>) -> CmdResult<Json> {
    let response = link.call(Request::Doctor { apply: false }).await?;
    let data = response
        .data_as::<Json>()
        .map_err(|source| LinkError::Decode {
            what: "doctor",
            source,
        })?;
    Ok(data.unwrap_or(Json::Null))
}

/// `daemon_running` → whether the daemon answers `status`.
#[tauri::command]
pub async fn daemon_running(link: State<'_, Arc<DaemonLink>>) -> CmdResult<bool> {
    Ok(link.status().await.is_ok())
}

/// The `kakemono` CLI: beside the current executable (also the `.app` `Contents/MacOS`),
/// otherwise the bare name so `PATH` decides.
pub fn cli_path() -> PathBuf {
    let name = format!("kakemono{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

// ── Files, URLs, platform ───────────────────────────────────────────────────

/// `reveal { path }`: shows the file in Finder / Explorer / the file manager.
#[tauri::command]
pub fn reveal<R: Runtime>(app: AppHandle<R>, path: String) -> CmdResult<()> {
    if path.is_empty() {
        return Err(CommandError::Invalid("reveal: path is empty".into()));
    }
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| CommandError::Opener(e.to_string()))
}

/// `open_url { url }`: opens an `http(s)` URL in the default browser. Other schemes are
/// refused so a wallpaper's `source_url` can't launch anything else.
#[tauri::command]
pub fn open_url<R: Runtime>(app: AppHandle<R>, url: String) -> CmdResult<()> {
    if !is_web_url(&url) {
        return Err(CommandError::Invalid(format!(
            "open_url: not an http(s) URL: {url}"
        )));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| CommandError::Opener(e.to_string()))
}

pub fn is_web_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && !url.contains(char::is_whitespace)
}

/// Static facts the settings window needs.
#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    /// `std::env::consts::OS`: `"macos"`, `"windows"`, `"linux"`, …
    pub os: &'static str,
    /// The config file being read and edited.
    pub config_path: String,
    /// True when that file is the Go `wallpaperd` config.
    pub legacy_config: bool,
    pub version: &'static str,
}

#[tauri::command]
pub fn app_info() -> CmdResult<AppInfo> {
    let (_, source) = locate_config()?;
    Ok(AppInfo {
        os: std::env::consts::OS,
        config_path: source.path().display().to_string(),
        legacy_config: source.is_legacy(),
        version: env!("CARGO_PKG_VERSION"),
    })
}

// ── Picker entries ──────────────────────────────────────────────────────────

/// One picker entry, in the Go picker's JSON shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub path: String,
    pub source: String,
    pub title: String,
    pub is_current: bool,
    /// `thumb://` URL from [`crate::thumbs::Thumbs::admit`]; `""` when the file isn't
    /// inside the download, favourites or local dirs (the picker shows a placeholder).
    pub image_url: String,
}

/// `thumbnails` → history (newest first), then the cache dir, then the favourites dir;
/// de-duplicated, missing files skipped, at most [`MAX_THUMBNAILS`]. The listed files become
/// the `thumb://` allowlist.
#[tauri::command]
pub async fn thumbnails(thumbs: State<'_, Arc<Thumbs>>) -> CmdResult<Vec<Thumbnail>> {
    let thumbs = Arc::clone(&thumbs);
    blocking(move || {
        let (paths, source) = locate_config()?;
        let cfg = kakemono_config::load_or_default(source.path())?;
        let state_file = if paths.state_file.exists() {
            &paths.state_file
        } else {
            &paths.legacy_state_file
        };
        let state = read_state(state_file);
        let mut list = list_thumbnails(&state, &cfg.daemon.download_dir, &cfg.favorites.dir);
        let mut roots = vec![cfg.daemon.download_dir.clone(), cfg.favorites.dir.clone()];
        roots.extend(cfg.local.dirs.iter().cloned());
        let paths: Vec<&str> = list.iter().map(|t| t.path.as_str()).collect();
        let urls = thumbs.admit(&roots, &paths);
        for (thumb, url) in list.iter_mut().zip(urls) {
            thumb.image_url = url;
        }
        Ok(list)
    })
    .await
}

/// The parts of the daemon's `state.json` the picker reads. The daemon owns the file; a
/// missing or unreadable file is an empty history.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StateView {
    pub current: Option<WallpaperInfo>,
    /// Go writes `null` for an empty history.
    pub history: Option<Vec<WallpaperInfo>>,
}

pub fn read_state(path: &Path) -> StateView {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::warn!(state = %path.display(), error = %err, "cannot parse state file");
            StateView::default()
        }),
        Err(_) => StateView::default(),
    }
}

/// Pure listing logic behind [`thumbnails`].
pub fn list_thumbnails(
    state: &StateView,
    download_dir: &Path,
    favorites_dir: &Path,
) -> Vec<Thumbnail> {
    let current = state
        .current
        .as_ref()
        .map(|c| c.path.as_str())
        .unwrap_or("");
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for item in state.history.iter().flatten().rev() {
        if item.path.is_empty()
            || !Path::new(&item.path).exists()
            || !seen.insert(item.path.clone())
        {
            continue;
        }
        out.push(Thumbnail {
            path: item.path.clone(),
            source: item.source.clone(),
            title: item.title.clone(),
            is_current: item.path == current,
            image_url: String::new(),
        });
    }
    for (dir, source) in [(download_dir, "cache"), (favorites_dir, "favorites")] {
        for (path, stem) in images_in(dir) {
            if !seen.insert(path.clone()) {
                continue;
            }
            out.push(Thumbnail {
                is_current: path == current,
                path,
                source: source.to_owned(),
                title: stem,
                image_url: String::new(),
            });
        }
    }
    out.truncate(MAX_THUMBNAILS);
    out
}

/// Image files directly in `dir`, sorted by name, as `(path, file stem)`.
fn images_in(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            if !IMAGE_EXTS.contains(&ext.as_str()) {
                return None;
            }
            let stem = path.file_stem()?.to_string_lossy().into_owned();
            Some((path.to_string_lossy().into_owned(), stem))
        })
        .collect();
    found.sort();
    found
}

/// Runs blocking file work off the async runtime.
async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const COMMENTED: &str = "\
# my kakemono config
[daemon]
interval = \"30m\" # rotate every half hour
cache_size = 50

# sources
[bing]
enabled = true
";

    #[test]
    fn patch_keeps_comments_and_untouched_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, COMMENTED).unwrap();

        write_patch(&path, &json!({ "daemon": { "interval": "1h30m" } })).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, COMMENTED.replace("\"30m\"", "\"1h30m\""));
    }

    #[test]
    fn null_removes_a_key_and_nested_objects_merge() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[daemon]\nsocket_path = \"/tmp/x.sock\"\n").unwrap();

        write_patch(
            &path,
            &json!({ "daemon": { "socket_path": null }, "picker": { "position": "top" } }),
        )
        .unwrap();

        let cfg = kakemono_config::load(&path).unwrap();
        assert_eq!(cfg.daemon.socket_path, None);
        assert_eq!(cfg.picker.position, kakemono_config::PickerPosition::Top);
    }

    #[test]
    fn bad_intervals_and_shapes_are_rejected_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, COMMENTED).unwrap();

        for patch in [
            json!({ "daemon": { "interval": "1d" } }),
            json!({ "daemon": { "interval": "0s" } }),
            json!({ "daemon": { "interval": 30 } }),
            json!(["not", "an", "object"]),
            json!({ "daemon": { "cache_size": "lots" } }),
        ] {
            let err = write_patch(&path, &patch).expect_err(&patch.to_string());
            assert!(matches!(err.kind(), "invalid" | "config"), "{err}");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
    }

    #[test]
    fn errors_serialise_with_a_kind() {
        let err = CommandError::Invalid("nope".into());
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({ "kind": "invalid", "message": "nope" })
        );
    }

    #[test]
    fn web_urls_only() {
        assert!(is_web_url("https://wallhaven.cc/w/abc"));
        assert!(is_web_url("HTTP://example.com"));
        assert!(!is_web_url("file:///etc/passwd"));
        assert!(!is_web_url("javascript:alert(1)"));
        assert!(!is_web_url("https://a b"));
    }

    fn info(path: &Path, source: &str) -> WallpaperInfo {
        WallpaperInfo {
            path: path.display().to_string(),
            source: source.into(),
            title: format!("{source} title"),
            ..Default::default()
        }
    }

    #[test]
    fn thumbnails_list_history_then_cache_then_favorites() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let favs = dir.path().join("favs");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&favs).unwrap();
        let a = cache.join("a.jpg");
        let b = cache.join("b.PNG");
        let gone = cache.join("gone.jpg");
        let fav = favs.join("f.webp");
        for p in [&a, &b, &fav] {
            std::fs::write(p, b"x").unwrap();
        }
        std::fs::write(cache.join("notes.txt"), b"x").unwrap();

        let state = StateView {
            current: Some(info(&b, "bing")),
            history: Some(vec![info(&a, "reddit"), info(&gone, "x"), info(&b, "bing")]),
        };
        let got = list_thumbnails(&state, &cache, &favs);
        let summary: Vec<(String, &str, bool)> = got
            .iter()
            .map(|t| (t.title.clone(), t.source.as_str(), t.is_current))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("bing title".to_owned(), "bing", true),
                ("reddit title".to_owned(), "reddit", false),
                ("f".to_owned(), "favorites", false),
            ]
        );
        assert_eq!(
            serde_json::to_value(&got[0]).unwrap()["isCurrent"],
            json!(true),
            "Go picker JSON shape"
        );
    }

    #[test]
    fn state_with_null_history_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"current":null,"history":null,"history_index":-1,"paused":false,"last_changed":"0001-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let state = read_state(&path);
        assert!(state.current.is_none());
        assert!(state.history.is_none());
        assert!(
            read_state(&dir.path().join("missing.json"))
                .history
                .is_none()
        );
    }

    #[test]
    fn favorites_copy_creates_the_dir() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("pic.jpg");
        std::fs::write(&src, b"img").unwrap();
        let dest = copy_to_favorites(&src, &dir.path().join("favs/deep")).unwrap();
        assert_eq!(std::fs::read(dest).unwrap(), b"img");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reload_reports_a_stopped_daemon_as_not_reloaded_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint =
            kakemono_protocol::endpoint::Endpoint::new(dir.path().join("k.sock")).unwrap();
        let link = DaemonLink::new(endpoint);
        assert_eq!(
            reload(&link).await,
            SaveOutcome {
                reloaded: false,
                reload_error: None
            }
        );
    }
}
