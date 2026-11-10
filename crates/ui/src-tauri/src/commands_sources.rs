//! Tauri commands for the Sources table and for images dropped on a window.
//!
//! - `check_source { kind, location }` → [`CheckSourceData`]: starts the daemon when it doesn't
//!   answer, then sends IPC `check_source`.
//! - `download_now { id }` → [`DownloadNowData`]: IPC `download_now`.
//! - `delete_source_files { id }`: deletes `download_dir/<id>`, and nothing else.
//! - `open_source_folder { id }`: opens a source's folder in the file manager.
//! - `fetch_dropped_url { url }` → [`WallpaperInfo`]: IPC `fetch`.
//! - `import_dropped_file` (raw body, name in the `x-file-name` header) → [`WallpaperInfo`]:
//!   writes the bytes to a new file in `manual_dir`, then sends IPC `fetch` with its path.
//!
//! Errors reach the frontend as `{ kind, message }`, like [`CommandError`]. When the daemon
//! can't be reached (it isn't running and can't be started, or the connection fails or times
//! out) the kind is `daemon_unreachable` ([`SourceError::Unreachable`]).
//!
//! The file-system helpers are plain functions so tests run them against temp dirs.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_config::{Config, ConfigError};
use kakemono_protocol::types::{CheckSourceData, DownloadNowData, Request, WallpaperInfo};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_opener::OpenerExt;

use crate::commands::CommandError;
use crate::daemon_link::{DaemonLink, LinkError};
use crate::lifecycle::{self, CliRunner, LifecycleError, ProcessRunner};

/// The largest image `import_dropped_file` accepts, as the daemon's `fetch`.
pub const MAX_DROP_BYTES: usize = 64 * 1024 * 1024;

/// The header carrying the dropped file's name, percent-encoded (`encodeURIComponent`).
pub const FILE_NAME_HEADER: &str = "x-file-name";

/// Valid-looking ids that name folders a source must never own.
const RESERVED_IDS: [&str; 3] = ["manual", "favorites", "trash"];

/// File-name extensions a dropped file may have.
const DROP_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// Image extensions the picker lists.
const IMAGE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// A recursive listing stops after visiting this many directory entries.
const MAX_WALK_ENTRIES: usize = 10_000;

/// A failed source command, serialised as `{ "kind": "...", "message": "..." }`.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The daemon isn't running and couldn't be started, or the connection failed or timed
    /// out. Kind `daemon_unreachable`; the UI offers "Add anyway" only for this one.
    #[error("cannot reach the kakemono daemon: {0}")]
    Unreachable(String),
    #[error(transparent)]
    Command(#[from] CommandError),
}

impl SourceError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unreachable(_) => "daemon_unreachable",
            Self::Command(err) => err.kind(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::Command(CommandError::Invalid(message.into()))
    }

    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Command(CommandError::Io {
            context: context.into(),
            source,
        })
    }
}

impl From<LinkError> for SourceError {
    fn from(err: LinkError) -> Self {
        match err {
            LinkError::Client(err) => Self::Unreachable(err.to_string()),
            other => Self::Command(CommandError::Link(other)),
        }
    }
}

impl From<LifecycleError> for SourceError {
    fn from(err: LifecycleError) -> Self {
        Self::Unreachable(err.to_string())
    }
}

impl From<ConfigError> for SourceError {
    fn from(err: ConfigError) -> Self {
        Self::Command(CommandError::Config(err))
    }
}

impl Serialize for SourceError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("SourceError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

type SrcResult<T> = Result<T, SourceError>;

fn load_config() -> SrcResult<Config> {
    let paths = kakemono_config::paths::Paths::detect()?;
    let source = paths.locate_config(None);
    Ok(kakemono_config::load_or_default(source.path())?)
}

/// Runs blocking file work off the async runtime.
async fn blocking<T, F>(f: F) -> SrcResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> SrcResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| SourceError::Command(CommandError::Tauri(e)))?
}

// ── Daemon requests ─────────────────────────────────────────────────────────

/// Sends `request` and decodes the answer's `data` (missing data is a decode error).
async fn call_for<T: DeserializeOwned>(
    link: &DaemonLink,
    request: Request,
    what: &'static str,
) -> SrcResult<T> {
    let response = link.call(request).await?;
    let data = response
        .data_as::<T>()
        .map_err(|source| LinkError::Decode { what, source })?;
    data.ok_or_else(|| {
        SourceError::Command(CommandError::Link(LinkError::Decode {
            what,
            source: serde::de::Error::custom("the daemon sent no data"),
        }))
    })
}

/// [`check_source`] with the CLI runner and path passed in, for tests.
pub async fn check_source_with(
    link: &DaemonLink,
    runner: &impl CliRunner,
    cli: Option<&Path>,
    kind: String,
    location: String,
) -> SrcResult<CheckSourceData> {
    let socket = link.endpoint().path().to_owned();
    lifecycle::ensure_running(link, runner, cli, &socket).await?;
    let request = Request::CheckSource { kind, location };
    call_for(link, request, "check_source").await
}

/// `check_source { kind, location }` → `{ count }`. Starts the daemon first when it doesn't
/// answer. A source that gives no images fails with kind `daemon` and the daemon's text.
#[tauri::command]
pub async fn check_source(
    link: State<'_, Arc<DaemonLink>>,
    kind: String,
    location: String,
) -> SrcResult<CheckSourceData> {
    let cli = lifecycle::find_cli();
    check_source_with(&link, &ProcessRunner, cli.as_deref(), kind, location).await
}

/// `download_now { id }` → `{ path }`: one image now for source `id`, or with `id` `null` for
/// every enabled online source that has none yet.
#[tauri::command]
pub async fn download_now(
    link: State<'_, Arc<DaemonLink>>,
    id: Option<String>,
) -> SrcResult<DownloadNowData> {
    download_now_on(&link, id).await
}

pub async fn download_now_on(link: &DaemonLink, id: Option<String>) -> SrcResult<DownloadNowData> {
    call_for(link, Request::DownloadNow { id }, "download_now").await
}

/// `fetch_dropped_url { url }` → the new wallpaper. The daemon accepts http(s) URLs, `file://`
/// URLs and absolute paths, saves the image into `manual_dir` and sets it.
#[tauri::command]
pub async fn fetch_dropped_url(
    link: State<'_, Arc<DaemonLink>>,
    url: String,
) -> SrcResult<WallpaperInfo> {
    fetch_on(&link, url).await
}

pub async fn fetch_on(link: &DaemonLink, url: String) -> SrcResult<WallpaperInfo> {
    let url = url.trim();
    if url.is_empty() {
        return Err(SourceError::invalid("fetch_dropped_url: the url is empty"));
    }
    let request = Request::Fetch {
        url: url.to_owned(),
    };
    call_for(link, request, "fetch").await
}

/// `import_dropped_file`: the request body is the file's bytes (raw, not JSON) and the
/// `x-file-name` header its percent-encoded name. Writes a new file into `manual_dir`, then
/// sets it through IPC `fetch` → the new wallpaper.
#[tauri::command]
pub async fn import_dropped_file(
    link: State<'_, Arc<DaemonLink>>,
    request: tauri::ipc::Request<'_>,
) -> SrcResult<WallpaperInfo> {
    let name = request
        .headers()
        .get(FILE_NAME_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(percent_decode)
        .unwrap_or_default();
    let bytes = body_bytes(request.body())?;
    let manual_dir = load_config()?.daemon.manual_dir_resolved();
    import_on(&link, manual_dir, name, bytes).await
}

/// The bytes of a raw request body. The postMessage IPC fallback sends a `Uint8Array` as a JSON
/// array of numbers, so that is accepted too.
fn body_bytes(body: &tauri::ipc::InvokeBody) -> SrcResult<Vec<u8>> {
    match body {
        tauri::ipc::InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        tauri::ipc::InvokeBody::Json(json @ serde_json::Value::Array(_)) => {
            serde_json::from_value(json.clone()).map_err(|_| {
                SourceError::invalid("import_dropped_file: the body is not a byte array")
            })
        }
        tauri::ipc::InvokeBody::Json(_) => Err(SourceError::invalid(
            "import_dropped_file: expected the file's bytes as the request body",
        )),
    }
}

/// [`import_dropped_file`] after the request is unpacked.
pub async fn import_on(
    link: &DaemonLink,
    manual_dir: PathBuf,
    name: String,
    bytes: Vec<u8>,
) -> SrcResult<WallpaperInfo> {
    let path = blocking(move || write_dropped_file(&manual_dir, &name, &bytes)).await?;
    fetch_on(link, path.to_string_lossy().into_owned()).await
}

// ── Source folders ──────────────────────────────────────────────────────────

/// `delete_source_files { id }`: deletes `download_dir/<id>` and everything in it. A folder
/// that doesn't exist is fine. Refuses anything [`resolve_source_dir`] refuses, a symlink, and
/// a path that isn't a direct child of `download_dir` once resolved.
#[tauri::command]
pub async fn delete_source_files(id: String) -> SrcResult<()> {
    blocking(move || {
        let cfg = load_config()?;
        remove_source_dir(&cfg, &id).map(|_| ())
    })
    .await
}

/// `open_source_folder { id }`: opens the folder of source `id` (see [`open_target`]) in the
/// file manager. An online source's download folder is created first.
#[tauri::command]
pub async fn open_source_folder<R: Runtime>(app: AppHandle<R>, id: String) -> SrcResult<()> {
    let dir = blocking(move || {
        let cfg = load_config()?;
        let entry = cfg
            .sources
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| SourceError::invalid(format!("no source \"{id}\"")))?;
        let dir = open_target(entry, &cfg).map_err(SourceError::invalid)?;
        if entry.kind.is_online() {
            std::fs::create_dir_all(&dir)
                .map_err(|e| SourceError::io(format!("create {}", dir.display()), e))?;
        } else if !dir.is_dir() {
            return Err(SourceError::invalid(format!(
                "folder not found: {}",
                dir.display()
            )));
        }
        Ok(dir)
    })
    .await?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| SourceError::Command(CommandError::Opener(e.to_string())))
}

/// `download_dir/<id>` if a source may own it: `id` is valid and not reserved, `download_dir`
/// is absolute, and the folder neither is, contains nor sits inside `manual_dir`, the
/// favorites dir, the trash dir or any local source's location.
pub fn resolve_source_dir(cfg: &Config, id: &str) -> Result<PathBuf, String> {
    if !kakemono_config::valid_id(id) || RESERVED_IDS.contains(&id) {
        return Err(format!("invalid source id {id:?}"));
    }
    let download_dir = &cfg.daemon.download_dir;
    if !download_dir.is_absolute() {
        return Err(format!(
            "the download folder is not an absolute path: {}",
            download_dir.display()
        ));
    }
    let dir = download_dir.join(id);
    let local = cfg
        .sources
        .iter()
        .filter(|e| e.kind.is_local() && !e.location.trim().is_empty())
        .map(|e| PathBuf::from(e.location.trim()));
    let protected = [
        cfg.daemon.manual_dir_resolved(),
        cfg.favorites.dir.clone(),
        cfg.daemon.trash_dir.clone(),
    ]
    .into_iter()
    .chain(local);
    for keep in protected {
        if !keep.as_os_str().is_empty() && overlaps(&dir, &keep) {
            return Err(format!(
                "{} overlaps {}; not touching it",
                dir.display(),
                keep.display()
            ));
        }
    }
    Ok(dir)
}

/// Whether `a` and `b` are the same path or one contains the other, compared as written and,
/// where they resolve, through symlinks.
fn overlaps(a: &Path, b: &Path) -> bool {
    let near = |a: &Path, b: &Path| a.starts_with(b) || b.starts_with(a);
    if near(a, b) {
        return true;
    }
    match (resolve_existing(a), resolve_existing(b)) {
        (Some(a), Some(b)) => near(&a, &b),
        _ => false,
    }
}

/// `path` resolved through its nearest existing ancestor, so a path that doesn't exist yet
/// still compares by where it would be.
fn resolve_existing(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(cur) {
            return Some(rest.iter().rev().fold(real, |acc, c| acc.join(c)));
        }
        rest.push(cur.file_name()?.to_owned());
        cur = cur.parent()?;
    }
}

/// Deletes the folder [`resolve_source_dir`] gives for `id`. `Ok(false)` when it doesn't
/// exist. Refuses a symlink, a non-directory, and a folder whose resolved path isn't a direct
/// child of the resolved `download_dir`.
pub fn remove_source_dir(cfg: &Config, id: &str) -> SrcResult<bool> {
    let dir = resolve_source_dir(cfg, id).map_err(SourceError::invalid)?;
    let meta = match std::fs::symlink_metadata(&dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(SourceError::io(format!("read {}", dir.display()), e)),
    };
    if !meta.is_dir() {
        return Err(SourceError::invalid(format!(
            "{} is not a folder; not deleting it",
            dir.display()
        )));
    }
    let real_parent = std::fs::canonicalize(&dir)
        .ok()
        .and_then(|real| real.parent().map(Path::to_path_buf));
    let real_root = std::fs::canonicalize(&cfg.daemon.download_dir).ok();
    if real_parent.is_none() || real_parent != real_root {
        return Err(SourceError::invalid(format!(
            "{} is not directly inside the download folder; not deleting it",
            dir.display()
        )));
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|e| SourceError::io(format!("delete {}", dir.display()), e))?;
    tracing::info!(dir = %dir.display(), "deleted a source's downloads");
    Ok(true)
}

/// The folder "Open Folder" shows: a local folder or album's location, the folder holding an
/// `images` file, or an online source's `download_dir/<id>`.
pub fn open_target(entry: &SourceEntry, cfg: &Config) -> Result<PathBuf, String> {
    let location = || {
        let loc = entry.location.trim();
        if loc.is_empty() {
            Err(format!("source \"{}\" has no location", entry.id))
        } else {
            Ok(PathBuf::from(loc))
        }
    };
    match entry.kind {
        SourceType::Folder | SourceType::AlbumName | SourceType::AlbumDate => location(),
        SourceType::Images => location()?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("source \"{}\" has no parent folder", entry.id)),
        SourceType::Wallhaven
        | SourceType::Unsplash
        | SourceType::Reddit
        | SourceType::Bing
        | SourceType::Apod => {
            if kakemono_config::valid_id(&entry.id) {
                Ok(cfg.daemon.download_dir.join(&entry.id))
            } else {
                Err(format!("invalid source id {:?}", entry.id))
            }
        }
        SourceType::Unsupported => Err(format!(
            "source \"{}\" has an unsupported type and no folder",
            entry.id
        )),
    }
}

// ── Picker listing ──────────────────────────────────────────────────────────

/// How [`crate::commands::list_thumbnails`] lists a [`ThumbDir`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    /// Image files directly in the folder.
    Flat,
    /// Image files in the folder and its subfolders.
    Recursive,
    /// The path is one image file.
    File,
}

/// One place the picker lists images from, with the `source` label its entries get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbDir {
    pub path: PathBuf,
    pub source: String,
    pub listing: Listing,
}

/// Where the picker looks after history: enabled online sources' download folders (config
/// order), then `manual_dir`, then enabled local sources, then the favorites dir, then the
/// legacy flat files at the root of `download_dir`. A path that is already listed is skipped.
pub fn thumbnail_dirs(cfg: &Config) -> Vec<ThumbDir> {
    let dir = |path: PathBuf, source: &str, listing| ThumbDir {
        path,
        source: source.to_owned(),
        listing,
    };
    let enabled = || cfg.sources.iter().filter(|e| e.enabled);
    let online = enabled()
        .filter(|e| e.kind.is_online() && kakemono_config::valid_id(&e.id))
        .map(|e| {
            let path = cfg.daemon.download_dir.join(&e.id);
            dir(path, e.kind.as_str(), Listing::Flat)
        });
    let manual = dir(cfg.daemon.manual_dir_resolved(), "manual", Listing::Flat);
    let local = enabled()
        .filter(|e| !e.location.trim().is_empty())
        .filter_map(|e| {
            let listing = match e.kind {
                SourceType::Folder => Listing::Recursive,
                SourceType::AlbumName | SourceType::AlbumDate => Listing::Flat,
                SourceType::Images => Listing::File,
                _ => return None,
            };
            let path = PathBuf::from(e.location.trim());
            Some(dir(path, e.kind.as_str(), listing))
        });
    let favorites = dir(cfg.favorites.dir.clone(), "favorites", Listing::Flat);
    // Files saved before per-source folders stay at the root of `download_dir`.
    let legacy = dir(cfg.daemon.download_dir.clone(), "cache", Listing::Flat);

    let mut seen = HashSet::new();
    online
        .chain(std::iter::once(manual))
        .chain(local)
        .chain([favorites, legacy])
        .filter(|d| !d.path.as_os_str().is_empty() && seen.insert(d.path.clone()))
        .collect()
}

/// Image files in `dir` as `(path, file stem)`, sorted by path, at most `limit`.
pub fn list_images(dir: &ThumbDir, limit: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut budget = MAX_WALK_ENTRIES;
    match dir.listing {
        Listing::File => {
            if dir.path.is_file() {
                push_image(&mut out, &dir.path);
            }
        }
        Listing::Flat => walk(&dir.path, false, limit, &mut budget, &mut out),
        Listing::Recursive => walk(&dir.path, true, limit, &mut budget, &mut out),
    }
    out.truncate(limit);
    out
}

/// Appends the images in `dir` sorted by name, then, when `recursive`, those in its
/// subfolders. Symlinked folders are not followed. Stops at `limit` images or after `budget`
/// directory entries.
fn walk(
    dir: &Path,
    recursive: bool,
    limit: usize,
    budget: &mut usize,
    out: &mut Vec<(String, String)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        match entry.file_type() {
            Ok(t) if t.is_file() => files.push(entry.path()),
            Ok(t) if t.is_dir() && recursive => dirs.push(entry.path()),
            _ => {}
        }
    }
    files.sort();
    dirs.sort();
    for file in files {
        if out.len() >= limit {
            return;
        }
        push_image(out, &file);
    }
    for sub in dirs {
        if out.len() >= limit || *budget == 0 {
            return;
        }
        walk(&sub, recursive, limit, budget, out);
    }
}

fn push_image(out: &mut Vec<(String, String)>, path: &Path) {
    let is_image = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()));
    if let (true, Some(stem)) = (is_image, path.file_stem()) {
        out.push((
            path.to_string_lossy().into_owned(),
            stem.to_string_lossy().into_owned(),
        ));
    }
}

// ── Dropped files ───────────────────────────────────────────────────────────

/// Writes `bytes` to a new file in `manual_dir` (created if missing) and returns its path.
///
/// `name` is reduced to its last component and to safe characters. Its extension must be
/// jpg, jpeg, png or webp, the bytes must start like a JPEG, PNG or WebP file, and there must
/// be at most [`MAX_DROP_BYTES`]. When the extension doesn't fit the content it is replaced
/// (`a.jpg` holding a PNG → `a.png`). A taken name gets `-2`, `-3`, ….
pub fn write_dropped_file(manual_dir: &Path, name: &str, bytes: &[u8]) -> SrcResult<PathBuf> {
    if bytes.len() > MAX_DROP_BYTES {
        return Err(SourceError::invalid(format!(
            "the image is larger than {} MB",
            MAX_DROP_BYTES / 1024 / 1024
        )));
    }
    let (stem, ext) = safe_file_name(name);
    let ext_lower = ext.to_ascii_lowercase();
    if !DROP_EXTS.contains(&ext_lower.as_str()) {
        return Err(SourceError::invalid(format!(
            "{name:?} is not a .jpg, .jpeg, .png or .webp file"
        )));
    }
    let sniffed =
        sniff(bytes).ok_or_else(|| SourceError::invalid("not a JPEG, PNG or WebP image"))?;
    let fits = match sniffed {
        "jpg" => matches!(ext_lower.as_str(), "jpg" | "jpeg"),
        other => ext_lower == other,
    };
    let ext = if fits { ext } else { sniffed.to_owned() };

    std::fs::create_dir_all(manual_dir)
        .map_err(|e| SourceError::io(format!("create {}", manual_dir.display()), e))?;
    for n in 1_u32.. {
        let file = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        let path = manual_dir.join(file);
        let mut out = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(SourceError::io(format!("save {}", path.display()), e)),
        };
        if let Err(e) = out.write_all(bytes).and_then(|()| out.sync_all()) {
            let _ = std::fs::remove_file(&path);
            return Err(SourceError::io(format!("save {}", path.display()), e));
        }
        return Ok(path);
    }
    Err(SourceError::invalid(
        "no free file name in the manual folder",
    ))
}

/// `(stem, extension)` of the last component of `name` (after `/` or `\`). Characters other
/// than letters, digits, space, `-`, `_` and `.` become `_`, leading dots and `..` are removed,
/// and the stem is cut to 100 characters. An empty stem becomes `image`.
fn safe_file_name(name: &str) -> (String, String) {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.');
    let (stem, ext) = clean.rsplit_once('.').unwrap_or((clean, ""));
    let stem: String = stem.replace("..", "_").chars().take(100).collect();
    let stem = stem.trim_end_matches('.').trim();
    let stem = if stem.is_empty() { "image" } else { stem };
    (stem.to_owned(), ext.to_owned())
}

/// The extension for the image type whose magic bytes `bytes` start with.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Decodes `%XX` escapes (as `encodeURIComponent` writes them). `None` for a bad escape or
/// bytes that aren't UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use interprocess::local_socket::tokio::prelude::*;
    use kakemono_protocol::endpoint::Endpoint;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n rest";
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
    const WEBP: &[u8] = b"RIFF\0\0\0\0WEBPVP8 ";

    fn cfg_in(root: &Path) -> Config {
        kakemono_config::defaults_for_home(root)
    }

    fn row(id: &str, enabled: bool, kind: SourceType, location: &str) -> SourceEntry {
        SourceEntry {
            id: id.to_owned(),
            enabled,
            kind,
            location: location.to_owned(),
        }
    }

    // ── resolve_source_dir ──

    #[test]
    fn a_source_owns_its_download_subfolder() {
        let root = tempfile::tempdir().unwrap();
        let cfg = cfg_in(root.path());
        assert_eq!(
            resolve_source_dir(&cfg, "wh-1a2b"),
            Ok(cfg.daemon.download_dir.join("wh-1a2b"))
        );
    }

    #[test]
    fn bad_and_reserved_ids_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let cfg = cfg_in(root.path());
        let long = "x".repeat(33);
        for id in [
            "",
            ".",
            "..",
            "/",
            "a/b",
            "../x",
            "a\\b",
            "Wh-1",
            " wh",
            long.as_str(),
            "manual",
            "favorites",
            "trash",
        ] {
            assert!(resolve_source_dir(&cfg, id).is_err(), "{id:?} accepted");
        }
    }

    #[test]
    fn protected_folders_inside_download_dir_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = cfg_in(root.path());
        let dl = cfg.daemon.download_dir.clone();
        cfg.daemon.manual_dir = Some(dl.join("man-1"));
        cfg.favorites.dir = dl.join("fav-1");
        cfg.daemon.trash_dir = dl.join("tr-1").join("deep");
        cfg.sources = vec![
            row(
                "dir-1",
                false,
                SourceType::Folder,
                &dl.join("dir-1").to_string_lossy(),
            ),
            row(
                "img-1",
                true,
                SourceType::Images,
                &dl.join("img-1").join("a.jpg").to_string_lossy(),
            ),
        ];
        for id in ["man-1", "fav-1", "tr-1", "dir-1", "img-1"] {
            assert!(resolve_source_dir(&cfg, id).is_err(), "{id} accepted");
        }
        assert!(resolve_source_dir(&cfg, "wh-1").is_ok());
    }

    #[test]
    fn a_download_dir_inside_a_local_folder_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = cfg_in(root.path());
        let parent = cfg
            .daemon
            .download_dir
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cfg.sources = vec![row("dir-1", true, SourceType::Folder, &parent)];
        assert!(resolve_source_dir(&cfg, "wh-1").is_err());
    }

    #[test]
    fn a_relative_download_dir_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = cfg_in(root.path());
        cfg.daemon.download_dir = PathBuf::from("cache");
        assert!(resolve_source_dir(&cfg, "wh-1").is_err());
    }

    // ── remove_source_dir ──

    #[test]
    fn removing_deletes_only_the_source_folder() {
        let root = tempfile::tempdir().unwrap();
        let cfg = cfg_in(root.path());
        let dl = &cfg.daemon.download_dir;
        std::fs::create_dir_all(dl.join("wh-1/sub")).unwrap();
        std::fs::write(dl.join("wh-1/a.jpg"), b"x").unwrap();
        std::fs::write(dl.join("wh-1/sub/b.jpg"), b"x").unwrap();
        std::fs::create_dir_all(dl.join("manual")).unwrap();
        std::fs::write(dl.join("manual/keep.jpg"), b"x").unwrap();
        std::fs::write(dl.join("legacy.jpg"), b"x").unwrap();

        assert!(remove_source_dir(&cfg, "wh-1").unwrap());
        assert!(!dl.join("wh-1").exists());
        assert!(dl.join("manual/keep.jpg").exists());
        assert!(dl.join("legacy.jpg").exists());
        assert!(!remove_source_dir(&cfg, "wh-1").unwrap(), "missing is fine");
        assert!(remove_source_dir(&cfg, "manual").is_err());
        assert!(remove_source_dir(&cfg, "..").is_err());
        assert!(dl.exists());
    }

    #[test]
    fn removing_refuses_a_file_named_like_a_source() {
        let root = tempfile::tempdir().unwrap();
        let cfg = cfg_in(root.path());
        let dl = &cfg.daemon.download_dir;
        std::fs::create_dir_all(dl).unwrap();
        std::fs::write(dl.join("wh-1"), b"x").unwrap();
        assert!(remove_source_dir(&cfg, "wh-1").is_err());
        assert!(dl.join("wh-1").exists());
    }

    #[cfg(unix)]
    #[test]
    fn removing_refuses_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let cfg = cfg_in(root.path());
        let dl = &cfg.daemon.download_dir;
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(dl).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("a.jpg"), b"x").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dl.join("wh-1")).unwrap();
        assert!(remove_source_dir(&cfg, "wh-1").is_err());
        assert!(elsewhere.join("a.jpg").exists());
    }

    // ── open_target ──

    #[test]
    fn open_target_per_source_type() {
        let root = tempfile::tempdir().unwrap();
        let cfg = cfg_in(root.path());
        let pics = root.path().join("pics");
        let file = pics.join("one.jpg");
        let dl = &cfg.daemon.download_dir;
        let p = |s: &Path| s.to_string_lossy().into_owned();
        assert_eq!(
            open_target(&row("dir-1", true, SourceType::Folder, &p(&pics)), &cfg),
            Ok(pics.clone())
        );
        assert_eq!(
            open_target(&row("aln-1", true, SourceType::AlbumName, &p(&pics)), &cfg),
            Ok(pics.clone())
        );
        assert_eq!(
            open_target(&row("ald-1", true, SourceType::AlbumDate, &p(&pics)), &cfg),
            Ok(pics.clone())
        );
        assert_eq!(
            open_target(&row("img-1", true, SourceType::Images, &p(&file)), &cfg),
            Ok(pics.clone())
        );
        assert_eq!(
            open_target(&row("wh-1", true, SourceType::Wallhaven, "forest"), &cfg),
            Ok(dl.join("wh-1"))
        );
        assert_eq!(
            open_target(&row("bing", false, SourceType::Bing, ""), &cfg),
            Ok(dl.join("bing"))
        );
        assert!(open_target(&row("unk-1", true, SourceType::Unsupported, "x"), &cfg).is_err());
        assert!(open_target(&row("dir-2", true, SourceType::Folder, ""), &cfg).is_err());
        assert!(open_target(&row("..", true, SourceType::Bing, ""), &cfg).is_err());
    }

    // ── thumbnail_dirs ──

    #[test]
    fn thumbnail_dirs_list_online_then_manual_then_local_then_favorites() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = cfg_in(root.path());
        let dl = cfg.daemon.download_dir.clone();
        let pics = root.path().join("pics");
        let favs = cfg.favorites.dir.clone();
        let p = |s: &Path| s.to_string_lossy().into_owned();
        cfg.sources = vec![
            row("dir-1", true, SourceType::Folder, &p(&pics)),
            row("wh-1", true, SourceType::Wallhaven, "forest"),
            row("off-1", false, SourceType::Unsplash, "x"),
            row("img-1", true, SourceType::Images, &p(&pics.join("a.jpg"))),
            row(
                "aln-1",
                true,
                SourceType::AlbumName,
                &p(&root.path().join("album")),
            ),
            row(
                "dir-off",
                false,
                SourceType::Folder,
                &p(&root.path().join("off")),
            ),
            row("unk-1", true, SourceType::Unsupported, "x"),
            row("bing", true, SourceType::Bing, ""),
            row("favorites", true, SourceType::Folder, &p(&favs)),
            row("dir-dup", true, SourceType::Folder, &p(&pics)),
        ];
        let got: Vec<(PathBuf, &str, Listing)> = thumbnail_dirs(&cfg)
            .iter()
            .map(|d| (d.path.clone(), leak(&d.source), d.listing))
            .collect();
        assert_eq!(
            got,
            vec![
                (dl.join("wh-1"), "wallhaven", Listing::Flat),
                (dl.join("bing"), "bing", Listing::Flat),
                (dl.join("manual"), "manual", Listing::Flat),
                (pics.clone(), "folder", Listing::Recursive),
                (pics.join("a.jpg"), "images", Listing::File),
                (root.path().join("album"), "album-name", Listing::Flat),
                (favs, "folder", Listing::Recursive),
                (dl, "cache", Listing::Flat),
            ]
        );
    }

    fn leak(s: &str) -> &'static str {
        Box::leak(s.to_owned().into_boxed_str())
    }

    #[test]
    fn list_images_flat_recursive_and_file() {
        let root = tempfile::tempdir().unwrap();
        let top = root.path().join("top");
        std::fs::create_dir_all(top.join("sub")).unwrap();
        for f in ["b.JPG", "a.png", "notes.txt", "sub/c.webp"] {
            std::fs::write(top.join(f), b"x").unwrap();
        }
        let dir = |listing| ThumbDir {
            path: top.clone(),
            source: "folder".into(),
            listing,
        };
        let stems = |v: Vec<(String, String)>| v.into_iter().map(|(_, s)| s).collect::<Vec<_>>();
        assert_eq!(stems(list_images(&dir(Listing::Flat), 100)), ["a", "b"]);
        assert_eq!(
            stems(list_images(&dir(Listing::Recursive), 100)),
            ["a", "b", "c"]
        );
        assert_eq!(stems(list_images(&dir(Listing::Recursive), 2)), ["a", "b"]);
        let one = ThumbDir {
            path: top.join("a.png"),
            source: "images".into(),
            listing: Listing::File,
        };
        assert_eq!(stems(list_images(&one, 100)), ["a"]);
        let gone = ThumbDir {
            path: top.join("gone.png"),
            ..one
        };
        assert!(list_images(&gone, 100).is_empty());
    }

    // ── write_dropped_file ──

    #[test]
    fn dropped_files_get_unique_names_in_manual_dir() {
        let root = tempfile::tempdir().unwrap();
        let manual = root.path().join("manual");
        let first = write_dropped_file(&manual, "photo.jpg", JPEG).unwrap();
        let second = write_dropped_file(&manual, "photo.jpg", JPEG).unwrap();
        assert_eq!(first, manual.join("photo.jpg"));
        assert_eq!(second, manual.join("photo-2.jpg"));
        assert_eq!(std::fs::read(&second).unwrap(), JPEG);
        assert_eq!(
            write_dropped_file(&manual, "x.webp", WEBP).unwrap(),
            manual.join("x.webp")
        );
        assert_eq!(
            write_dropped_file(&manual, "y.JPEG", JPEG).unwrap(),
            manual.join("y.JPEG")
        );
    }

    #[test]
    fn dropped_file_names_cannot_leave_manual_dir() {
        let root = tempfile::tempdir().unwrap();
        let manual = root.path().join("manual");
        for name in [
            "../../evil.png",
            "/etc/evil.png",
            "..\\evil.png",
            ".evil.png",
        ] {
            let path = write_dropped_file(&manual, name, PNG).unwrap();
            assert_eq!(path.parent(), Some(manual.as_path()), "{name}");
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(!file.starts_with('.') && !file.contains(".."), "{file}");
        }
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            1,
            "only manual/ was created"
        );
    }

    #[test]
    fn dropped_files_must_be_jpeg_png_or_webp() {
        let root = tempfile::tempdir().unwrap();
        let manual = root.path().join("manual");
        for (name, bytes) in [
            ("a.gif", PNG),
            ("noext", PNG),
            ("a.png", b"GIF89a".as_slice()),
            ("a.png", b"".as_slice()),
        ] {
            let err = write_dropped_file(&manual, name, bytes).unwrap_err();
            assert_eq!(err.kind(), "invalid", "{name}");
        }
        assert!(!manual.exists() || std::fs::read_dir(&manual).unwrap().count() == 0);
        assert_eq!(
            write_dropped_file(&manual, "a.jpg", PNG).unwrap(),
            manual.join("a.png"),
            "extension fixed to the content"
        );
    }

    #[test]
    fn dropped_files_over_64_mb_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut big = vec![0_u8; MAX_DROP_BYTES + 1];
        big[..PNG.len()].copy_from_slice(PNG);
        let err = write_dropped_file(root.path(), "big.png", &big).unwrap_err();
        assert_eq!(err.kind(), "invalid");
        assert!(err.to_string().contains("64 MB"), "{err}");
    }

    #[test]
    fn percent_decoding_reads_encode_uri_component() {
        assert_eq!(
            percent_decode("na%C3%AFve%20photo.jpg").as_deref(),
            Some("naïve photo.jpg")
        );
        assert_eq!(percent_decode("plain.png").as_deref(), Some("plain.png"));
        assert_eq!(percent_decode("bad%2"), None);
        assert_eq!(percent_decode("bad%zz"), None);
        assert_eq!(percent_decode("%FF"), None);
    }

    #[test]
    fn dropped_bytes_come_raw_or_as_a_json_array() {
        use tauri::ipc::InvokeBody;
        assert_eq!(body_bytes(&InvokeBody::Raw(vec![1, 2])).unwrap(), [1, 2]);
        let json = InvokeBody::Json(serde_json::json!([255, 0]));
        assert_eq!(body_bytes(&json).unwrap(), [255, 0]);
        for bad in [serde_json::json!([256]), serde_json::json!({"a": 1})] {
            let err = body_bytes(&InvokeBody::Json(bad)).unwrap_err();
            assert_eq!(err.kind(), "invalid");
        }
    }

    // ── Errors ──

    #[test]
    fn transport_failures_are_daemon_unreachable() {
        let not_running = LinkError::Client(kakemono_protocol::client::ClientError::Closed);
        let err = SourceError::from(not_running);
        assert_eq!(err.kind(), "daemon_unreachable");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "daemon_unreachable");
        assert!(
            json["message"]
                .as_str()
                .unwrap()
                .starts_with("cannot reach")
        );

        let refused = SourceError::from(LinkError::Daemon("no images found".into()));
        assert_eq!(refused.kind(), "daemon");
        assert_eq!(refused.to_string(), "no images found");

        let no_cli = SourceError::from(LifecycleError::CliNotFound);
        assert_eq!(no_cli.kind(), "daemon_unreachable");
    }

    // ── Daemon requests against a fake daemon ──

    fn temp_endpoint() -> (tempfile::TempDir, Endpoint) {
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoint = Endpoint::new(dir.path().join("run/k.sock")).expect("endpoint");
        (dir, endpoint)
    }

    /// Answers each request line on one connection with the next reply. Returns the requests
    /// it read, parsed as JSON.
    fn serve(
        endpoint: &Endpoint,
        replies: Vec<String>,
    ) -> tokio::task::JoinHandle<Vec<serde_json::Value>> {
        let listener = endpoint.bind().expect("bind");
        tokio::spawn(async move {
            let conn = listener.accept().await.expect("accept");
            let mut reader = BufReader::new(conn);
            let mut seen = Vec::new();
            for reply in replies {
                let mut line = String::new();
                if reader.read_line(&mut line).await.expect("read") == 0 {
                    break;
                }
                seen.push(serde_json::from_str(&line).expect("request json"));
                let reply = format!("{reply}\n");
                reader
                    .get_mut()
                    .write_all(reply.as_bytes())
                    .await
                    .expect("write");
            }
            seen
        })
    }

    const STATUS: &str = concat!(
        "{\"ok\":true,\"data\":{\"current\":null,\"paused\":false,\"interval\":\"30m\",",
        "\"theme\":\"dark\",\"history_count\":0,\"history_index\":-1}}"
    );

    struct NoCli;
    impl CliRunner for NoCli {
        async fn run(&self, _call: lifecycle::CliCall) -> std::io::Result<lifecycle::CliOutput> {
            panic!("the daemon is running; no CLI call expected")
        }
    }

    #[tokio::test]
    async fn check_source_asks_the_running_daemon() {
        let (_dir, endpoint) = temp_endpoint();
        let server = serve(
            &endpoint,
            vec![
                STATUS.into(),
                "{\"ok\":true,\"data\":{\"count\":24}}".into(),
            ],
        );
        let link = DaemonLink::new(endpoint);
        let got = check_source_with(&link, &NoCli, None, "wallhaven".into(), "forest".into())
            .await
            .expect("count");
        assert_eq!(got, CheckSourceData { count: 24 });
        let seen = server.await.unwrap();
        assert_eq!(
            seen[1],
            serde_json::json!({"cmd":"check_source","args":{"type":"wallhaven","location":"forest"}})
        );
    }

    #[tokio::test]
    async fn check_source_reports_the_daemons_refusal() {
        let (_dir, endpoint) = temp_endpoint();
        let _server = serve(
            &endpoint,
            vec![
                STATUS.into(),
                "{\"ok\":false,\"error\":\"no images found\"}".into(),
            ],
        );
        let link = DaemonLink::new(endpoint);
        let err = check_source_with(&link, &NoCli, None, "bing".into(), String::new())
            .await
            .expect_err("refused");
        assert_eq!(err.kind(), "daemon");
        assert_eq!(err.to_string(), "no images found");
    }

    #[tokio::test]
    async fn check_source_without_a_daemon_or_cli_is_unreachable() {
        let (_dir, endpoint) = temp_endpoint();
        let link = DaemonLink::new(endpoint);
        let err = check_source_with(&link, &NoCli, None, "bing".into(), String::new())
            .await
            .expect_err("unreachable");
        assert_eq!(err.kind(), "daemon_unreachable");
    }

    #[tokio::test]
    async fn requests_without_a_daemon_are_unreachable() {
        let (_dir, endpoint) = temp_endpoint();
        let link = DaemonLink::new(endpoint);
        let err = download_now_on(&link, None).await.expect_err("no daemon");
        assert_eq!(err.kind(), "daemon_unreachable");
        let err = fetch_on(&link, "https://x/y.jpg".into())
            .await
            .expect_err("no daemon");
        assert_eq!(err.kind(), "daemon_unreachable");
    }

    #[tokio::test]
    async fn download_now_sends_the_id_and_decodes_the_path() {
        let (_dir, endpoint) = temp_endpoint();
        let server = serve(
            &endpoint,
            vec![
                "{\"ok\":true,\"data\":{\"path\":\"/d/wh-1/a.jpg\"}}".into(),
                "{\"ok\":true,\"data\":{\"path\":null}}".into(),
            ],
        );
        let link = DaemonLink::new(endpoint);
        let one = download_now_on(&link, Some("wh-1".into())).await.unwrap();
        assert_eq!(one.path.as_deref(), Some("/d/wh-1/a.jpg"));
        let none = download_now_on(&link, None).await.unwrap();
        assert_eq!(none.path, None);
        let seen = server.await.unwrap();
        assert_eq!(
            seen,
            vec![
                serde_json::json!({"cmd":"download_now","args":{"id":"wh-1"}}),
                serde_json::json!({"cmd":"download_now"}),
            ]
        );
    }

    fn info_reply(path: &str) -> String {
        let info = WallpaperInfo {
            path: path.to_owned(),
            source: "manual".into(),
            ..Default::default()
        };
        format!(
            "{{\"ok\":true,\"data\":{}}}",
            serde_json::to_string(&info).unwrap()
        )
    }

    #[tokio::test]
    async fn fetch_sends_the_url_and_refuses_an_empty_one() {
        let (_dir, endpoint) = temp_endpoint();
        let server = serve(&endpoint, vec![info_reply("/m/y.jpg")]);
        let link = DaemonLink::new(endpoint);
        let err = fetch_on(&link, "  ".into()).await.expect_err("empty");
        assert_eq!(err.kind(), "invalid");
        let info = fetch_on(&link, " https://x/y.jpg ".into()).await.unwrap();
        assert_eq!(info.path, "/m/y.jpg");
        let seen = server.await.unwrap();
        assert_eq!(
            seen,
            vec![serde_json::json!({"cmd":"fetch","args":{"url":"https://x/y.jpg"}})]
        );
    }

    #[tokio::test]
    async fn import_writes_into_manual_dir_then_fetches_that_path() {
        let (dir, endpoint) = temp_endpoint();
        let manual = dir.path().join("manual");
        let expected = manual.join("drop.png");
        let server = serve(&endpoint, vec![info_reply(&expected.to_string_lossy())]);
        let link = DaemonLink::new(endpoint);
        let info = import_on(&link, manual.clone(), "drop.png".into(), PNG.to_vec())
            .await
            .unwrap();
        assert_eq!(info.path, expected.to_string_lossy());
        assert_eq!(std::fs::read(&expected).unwrap(), PNG);
        let seen = server.await.unwrap();
        assert_eq!(
            seen,
            vec![serde_json::json!({"cmd":"fetch","args":{"url": expected.to_string_lossy()}})]
        );
    }

    #[tokio::test]
    async fn import_refuses_a_non_image_without_calling_the_daemon() {
        let (dir, endpoint) = temp_endpoint();
        let link = DaemonLink::new(endpoint);
        let err = import_on(
            &link,
            dir.path().join("m"),
            "a.png".into(),
            b"nope".to_vec(),
        )
        .await
        .expect_err("not an image");
        assert_eq!(err.kind(), "invalid");
    }
}
