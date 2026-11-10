//! Favorites, trash and download-folder trimming (plan §D). Port of Go `Manager.Favorite`,
//! `Manager.Trash` and `copyFile` (`internal/wallpaper/manager.go`).
//!
//! Differences from Go:
//! - `trash` never overwrites a file already in the trash. It uses the same collision name as
//!   `favorite` (`<stem>_<unix nanos>.<ext>`). Go's `os.Rename` replaced the older file.
//! - A copy that fails halfway removes the partial destination file. Go left it behind.
//! - An empty favorites or trash dir is an error. Go would have written into the working dir.
//!
//! [`safe_delete`] and [`trim_to_mb`] (with [`Protect`]) replace Go's count-based `trimCache`
//! for the per-source download layout (`docs/plans/variety-sources-impl.md` Task 11): every
//! delete is checked against the download dir and the protected dirs and files.
//!
//! The file work runs on tokio's blocking pool (`spawn_blocking`) with `std::fs`. Copies go
//! through [`kakemono_fs::copy_unique`] (Go's `<stem>_<unix nanos>.<ext>` collision name).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kakemono_config::Config;
use kakemono_config::schema::SourceType;
use kakemono_fs::{CopyError, collision_name};
use kakemono_sources::is_image_file;
use tokio::task::JoinError;

use crate::engine::EngineError;
use crate::gotext::go_reason;
use crate::state::State;

/// Why `favorite` or `trash` failed. Shown after `manager: ` ([`EngineError::Files`]); the
/// texts are Go's.
#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    /// `[daemon] favorites_dir` or `trash_dir` is empty.
    #[error("{what} dir is not configured")]
    NotConfigured { what: &'static str },
    /// Creating the favorites or trash dir failed.
    #[error("create {what} dir: mkdir {}: {}", dir.display(), go_reason(source))]
    CreateDir {
        what: &'static str,
        dir: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Copying into the favorites dir failed.
    #[error("copy to favorites: {0}")]
    Favorite(#[source] CopyFailure),
    /// The current wallpaper has no file name to trash under.
    #[error("move to trash: {0}")]
    TrashTarget(#[source] CopyFailure),
    /// Both the rename into the trash and the copy fallback failed.
    #[error(
        "move to trash: rename {} {}: {} (copy fallback: {copy})",
        src.display(),
        dst.display(),
        go_reason(rename)
    )]
    Trash {
        src: PathBuf,
        dst: PathBuf,
        rename: io::Error,
        #[source]
        copy: CopyFailure,
    },
    /// The file task panicked or was cancelled.
    #[error("file task failed: {0}")]
    Task(#[source] JoinError),
}

/// One copy's failure, in Go's `*PathError` form (`open <path>: <reason>`).
#[derive(Debug, thiserror::Error)]
pub enum CopyFailure {
    /// The source path has no file name (`/`, `..`).
    #[error("{}: not a file path", .0.display())]
    NoFileName(PathBuf),
    /// [`kakemono_fs::copy_unique`] failed.
    #[error("{} {}: {}", .0.op(), .0.path().display(), copy_reason(.0))]
    Copy(#[source] CopyError),
}

/// The reason part of a [`CopyError`]: Go's errno text, or the message of an error built from
/// text (such as running out of collision names).
fn copy_reason(err: &CopyError) -> String {
    let io = err.io_error();
    if io.get_ref().is_some() {
        io.to_string()
    } else {
        go_reason(io)
    }
}

/// Copies `src` into `favorites_dir` and returns the new file's path. Errors are
/// [`EngineError::Files`] (rendered as `manager: <message>`).
pub(crate) async fn favorite(src: &Path, favorites_dir: &Path) -> Result<PathBuf, EngineError> {
    let (src, dir) = (src.to_path_buf(), favorites_dir.to_path_buf());
    Ok(blocking(move || favorite_blocking(&src, &dir)).await?)
}

/// Moves `src` into `trash_dir` and returns its new path. The engine then runs `next`.
pub(crate) async fn trash(src: &Path, trash_dir: &Path) -> Result<PathBuf, EngineError> {
    let (src, dir) = (src.to_path_buf(), trash_dir.to_path_buf());
    Ok(blocking(move || trash_blocking(&src, &dir, |from, to| fs::rename(from, to))).await?)
}

/// File name of the per-source seen ledger the downloader keeps in `downloads/<id>/`.
/// [`trim_to_mb`] neither counts nor deletes it.
pub const SEEN_LEDGER: &str = ".kakemono-seen";

/// Paths no delete may touch: protected dirs (manual dir, favorites, trash, local-source dirs)
/// and protected files (the current wallpaper and history entries). Paths are stored as given
/// and compared both as given and canonicalised when a delete is checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Protect {
    dirs: Vec<PathBuf>,
    files: HashSet<PathBuf>,
}

impl Protect {
    /// Protects nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Protects everything under `dir`. An empty path (an unset setting) is ignored.
    #[must_use]
    pub fn dir(mut self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        if !dir.as_os_str().is_empty() {
            self.dirs.push(dir);
        }
        self
    }

    /// Protects the file `file`. An empty path is ignored.
    #[must_use]
    pub fn file(mut self, file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        if !file.as_os_str().is_empty() {
            self.files.insert(file);
        }
        self
    }

    /// Protects the dirs and files `cfg` names that downloads must never touch: `manual_dir`,
    /// the favorites and trash dirs, and every local source's location (an `images` row's
    /// location is a file), enabled or not.
    #[must_use]
    pub fn config(self, cfg: &Config) -> Self {
        let protect = self
            .dir(cfg.daemon.manual_dir_resolved())
            .dir(&cfg.favorites.dir)
            .dir(&cfg.daemon.trash_dir);
        cfg.sources
            .iter()
            .filter(|e| e.kind.is_local())
            .fold(protect, |protect, entry| match entry.kind {
                SourceType::Images => protect.file(&entry.location),
                _ => protect.dir(&entry.location),
            })
    }

    /// Protects the current wallpaper and every history entry of `state`.
    #[must_use]
    pub fn state(self, state: &State) -> Self {
        state
            .current()
            .into_iter()
            .chain(state.history())
            .fold(self, |protect, info| protect.file(&info.path))
    }
}

/// A [`Protect`] resolved against one download dir: every path as given plus its canonical
/// form, so one trim canonicalises the protected paths once.
struct Guard {
    /// The canonical download dir.
    root: PathBuf,
    dirs: Vec<PathBuf>,
    files: HashSet<PathBuf>,
}

/// Marks a refused delete inside its `io::Error`, so the trim can tell refusals from failures.
#[derive(Debug)]
struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

fn refused(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        Refused(format!("refusing to delete {}: {reason}", path.display())),
    )
}

fn is_refusal(err: &io::Error) -> bool {
    err.get_ref().is_some_and(|inner| inner.is::<Refused>())
}

/// `path` as given, then canonicalised when it resolves.
fn with_canonical(path: &Path) -> impl Iterator<Item = PathBuf> {
    std::iter::once(path.to_path_buf()).chain(fs::canonicalize(path).ok())
}

impl Guard {
    fn new(download_dir: &Path, protect: &Protect) -> io::Result<Self> {
        if download_dir.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "download dir is not configured",
            ));
        }
        Ok(Self {
            root: fs::canonicalize(download_dir)?,
            dirs: protect
                .dirs
                .iter()
                .flat_map(|d| with_canonical(d))
                .collect(),
            files: protect
                .files
                .iter()
                .flat_map(|f| with_canonical(f))
                .collect(),
        })
    }

    /// The protected dir that contains the download dir, if any (then nothing is deletable).
    fn dir_containing_root(&self) -> Option<&Path> {
        self.dirs
            .iter()
            .find(|d| self.root.starts_with(d))
            .map(PathBuf::as_path)
    }

    /// Whether the walk must not enter `dir`: it is under a protected dir, or can't be resolved.
    fn skips_dir(&self, dir: &Path) -> bool {
        let Ok(real) = fs::canonicalize(dir) else {
            return true;
        };
        [dir, real.as_path()]
            .iter()
            .any(|p| self.dirs.iter().any(|d| p.starts_with(d)))
    }

    /// Refuses `path` unless every spelling of it is a deletable download: the path as given,
    /// the entry itself (parent resolved) and the file it resolves to (symlinks followed).
    fn check(&self, path: &Path) -> io::Result<()> {
        if fs::symlink_metadata(path)?.is_dir() {
            return Err(refused(path, "is a directory"));
        }
        let name = path
            .file_name()
            .ok_or_else(|| refused(path, "not a file path"))?;
        let parent = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let entry = fs::canonicalize(parent)?.join(name);
        let target = fs::canonicalize(path)?;
        for resolved in [&entry, &target] {
            if !resolved.starts_with(&self.root) || *resolved == self.root {
                return Err(refused(path, "outside the download dir"));
            }
        }
        for spelling in [path, &entry, &target] {
            if let Some(dir) = self.dirs.iter().find(|d| spelling.starts_with(d)) {
                return Err(refused(
                    path,
                    &format!("inside protected dir {}", dir.display()),
                ));
            }
            if self.files.contains(spelling) {
                return Err(refused(path, "protected file"));
            }
        }
        Ok(())
    }

    fn delete(&self, path: &Path) -> io::Result<()> {
        self.check(path)?;
        fs::remove_file(path)
    }
}

/// Deletes the file `path`, refusing (`ErrorKind::PermissionDenied`, file kept) when:
/// - it isn't inside `download_dir`, as given or with symlinks resolved (`../` escapes and
///   symlinks pointing outside are refused);
/// - it is, or resolves to a path, under a protected dir of `protect`;
/// - it is, or resolves to, a protected file of `protect`;
/// - it is a directory.
///
/// Every delete of downloaded files goes through this.
///
/// # Errors
///
/// `PermissionDenied` for a refused path (see above), or the error of the delete.
pub fn safe_delete(path: &Path, download_dir: &Path, protect: &Protect) -> io::Result<()> {
    Guard::new(download_dir, protect)?.delete(path)
}

/// Deletes the oldest image files (by modification time) under `download_dir`, recursively and
/// including legacy flat files at its root, until their total size is at or under
/// `limit_bytes`. Deletes only through [`safe_delete`]. Protected dirs are not walked or
/// counted. Protected files count but are skipped. [`SEEN_LEDGER`] files, hidden files
/// (including partial downloads) and symlinks are neither counted nor deleted. Returns the
/// deleted paths. Failures are logged, never returned.
pub async fn trim_to_mb(download_dir: &Path, limit_bytes: u64, protect: &Protect) -> Vec<PathBuf> {
    let (dir, protect) = (download_dir.to_path_buf(), protect.clone());
    let joined =
        tokio::task::spawn_blocking(move || trim_to_mb_blocking(&dir, limit_bytes, &protect));
    match joined.await {
        Ok(deleted) => {
            if !deleted.is_empty() {
                tracing::debug!(count = deleted.len(), "trimmed the download folder");
            }
            deleted
        }
        Err(err) => {
            tracing::warn!("trim download folder: {err}");
            Vec::new()
        }
    }
}

/// Total size in bytes of the files [`trim_to_mb`] counts against the limit under
/// `download_dir` (same walk: protected dirs, hidden files, ledgers and symlinks are left out).
/// `None` when `download_dir` is unset or can't be resolved (for example, it doesn't exist yet).
/// Blocking.
#[must_use]
pub fn counted_bytes(download_dir: &Path, protect: &Protect) -> Option<u64> {
    let guard = Guard::new(download_dir, protect).ok()?;
    let mut files = Vec::new();
    collect_cached(download_dir, &guard, &mut files);
    Some(files.iter().map(|f| f.size).sum())
}

/// One file [`trim_to_mb`] counts.
struct CachedFile {
    modified: SystemTime,
    size: u64,
    path: PathBuf,
}

fn trim_to_mb_blocking(download_dir: &Path, limit_bytes: u64, protect: &Protect) -> Vec<PathBuf> {
    if download_dir.as_os_str().is_empty() {
        return Vec::new();
    }
    let guard = match Guard::new(download_dir, protect) {
        Ok(guard) => guard,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!(
                    "trim download folder: {}: {}",
                    download_dir.display(),
                    go_reason(&err)
                );
            }
            return Vec::new();
        }
    };
    if let Some(dir) = guard.dir_containing_root() {
        tracing::warn!(
            "trim download folder: protected dir {} contains the download dir {}; \
             not deleting anything",
            dir.display(),
            download_dir.display()
        );
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_cached(download_dir, &guard, &mut files);
    let mut total: u64 = files.iter().map(|f| f.size).sum();
    if total <= limit_bytes {
        return Vec::new();
    }
    files.sort_by(|a, b| (a.modified, &a.path).cmp(&(b.modified, &b.path)));
    let mut deleted = Vec::new();
    for file in files {
        if total <= limit_bytes {
            break;
        }
        match guard.delete(&file.path) {
            Ok(()) => {
                total = total.saturating_sub(file.size);
                deleted.push(file.path);
            }
            Err(err) if is_refusal(&err) => tracing::debug!("trim download folder: {err}"),
            Err(err) => tracing::warn!(
                "trim download folder: remove {}: {}",
                file.path.display(),
                go_reason(&err)
            ),
        }
    }
    if total > limit_bytes {
        tracing::info!(
            total,
            limit_bytes,
            "download folder is still over its limit; the rest is protected"
        );
    }
    deleted
}

/// Whether the trim counts and may delete the file `path`: a visible image, not a ledger.
fn is_cached_image(path: &Path) -> bool {
    let visible = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n != SEEN_LEDGER && !n.starts_with('.'));
    visible && is_image_file(path)
}

/// Collects the trim's files under `dir`, recursively. Symlinks are not followed or counted,
/// and protected dirs are not entered.
fn collect_cached(dir: &Path, guard: &Guard, out: &mut Vec<CachedFile>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(
                "trim download folder: read {}: {}",
                dir.display(),
                go_reason(&err)
            );
            return;
        }
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !guard.skips_dir(&path) {
                collect_cached(&path, guard, out);
            }
        } else if kind.is_file() && is_cached_image(&path) {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            out.push(CachedFile {
                modified,
                size: meta.len(),
                path,
            });
        }
    }
}

async fn blocking<T, F>(f: F) -> Result<T, FilesError>
where
    F: FnOnce() -> Result<T, FilesError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(FilesError::Task)?
}

fn favorite_blocking(src: &Path, dir: &Path) -> Result<PathBuf, FilesError> {
    ensure_dir(dir, "favorites")?;
    let dst = target_path(src, dir).map_err(FilesError::Favorite)?;
    copy_unique(src, &dst).map_err(FilesError::Favorite)
}

/// Go `Trash`: rename into the trash, else (another device, or any rename error) copy and
/// remove the source. `rename` is injectable so tests can simulate a cross-device move.
fn trash_blocking(
    src: &Path,
    dir: &Path,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, FilesError> {
    ensure_dir(dir, "trash")?;
    let dst = target_path(src, dir).map_err(FilesError::TrashTarget)?;
    let dst = if dst.exists() {
        collision_free(&dst)
    } else {
        dst
    };
    let rename_err = match rename(src, &dst) {
        Ok(()) => return Ok(dst),
        Err(err) => err,
    };
    match copy_unique(src, &dst) {
        Ok(moved) => {
            if let Err(err) = fs::remove_file(src) {
                // Go ignored this error too: the trash has a copy, the original stays.
                tracing::warn!(
                    "trash: remove {} after copy: {}",
                    src.display(),
                    go_reason(&err)
                );
            }
            Ok(moved)
        }
        Err(copy) => Err(FilesError::Trash {
            src: src.to_path_buf(),
            dst,
            rename: rename_err,
            copy,
        }),
    }
}

/// `create_dir_all`, with Go's `create <what> dir: mkdir <dir>: <reason>` message.
fn ensure_dir(dir: &Path, what: &'static str) -> Result<(), FilesError> {
    if dir.as_os_str().is_empty() {
        return Err(FilesError::NotConfigured { what });
    }
    fs::create_dir_all(dir).map_err(|source| FilesError::CreateDir {
        what,
        dir: dir.to_path_buf(),
        source,
    })
}

/// `dir/<file name of src>` (Go `filepath.Join(dir, filepath.Base(src))`).
fn target_path(src: &Path, dir: &Path) -> Result<PathBuf, CopyFailure> {
    src.file_name()
        .map(|name| dir.join(name))
        .ok_or_else(|| CopyFailure::NoFileName(src.to_path_buf()))
}

/// Go `copyFile`: [`kakemono_fs::copy_unique`], with Go's error text.
fn copy_unique(src: &Path, dst: &Path) -> Result<PathBuf, CopyFailure> {
    kakemono_fs::copy_unique(src, dst).map_err(CopyFailure::Copy)
}

/// A name for `dst` that doesn't exist yet (checked, not reserved; the engine's op lock keeps
/// other moves out).
fn collision_free(dst: &Path) -> PathBuf {
    let mut nanos = unix_nanos();
    loop {
        let candidate = collision_name(dst, nanos);
        if !candidate.exists() {
            return candidate;
        }
        nanos = nanos.saturating_add(1);
    }
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
    }

    fn is_collision_name(path: &Path, stem: &str, ext: &str) -> bool {
        let name = path.file_name().unwrap().to_str().unwrap();
        name.strip_prefix(&format!("{stem}_"))
            .and_then(|rest| rest.strip_suffix(&format!(".{ext}")))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }

    /// The on-disk collision names must stay Go's.
    #[test]
    fn collision_names_keep_the_extension() {
        let p = Path::new("/d/photo.jpg");
        assert_eq!(collision_name(p, 42), Path::new("/d/photo_42.jpg"));
        assert_eq!(
            collision_name(Path::new("/d/a.tar.gz"), 7),
            Path::new("/d/a.tar_7.gz")
        );
        assert_eq!(
            collision_name(Path::new("/d/noext"), 7),
            Path::new("/d/noext_7")
        );
        assert_eq!(
            collision_name(Path::new("/d/.hidden"), 7),
            Path::new("/d/_7.hidden")
        );
    }

    #[test]
    fn copy_unique_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.png");
        write(&src, b"new");
        let dst = dir.path().join("a.png");
        write(&dst, b"old");
        let p1 = copy_unique(&src, &dst).unwrap();
        let p2 = copy_unique(&src, &dst).unwrap();
        assert_ne!(p1, dst);
        assert_ne!(p1, p2);
        assert!(is_collision_name(&p1, "a", "png"), "{}", p1.display());
        assert!(is_collision_name(&p2, "a", "png"), "{}", p2.display());
        assert_eq!(fs::read(&dst).unwrap(), b"old");
    }

    #[test]
    fn trash_falls_back_to_copy_and_remove_across_devices() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("wall.jpg");
        write(&src, b"image bytes");
        let trash = dir.path().join("trash");
        let exdev = |_: &Path, _: &Path| Err(io::Error::from(io::ErrorKind::CrossesDevices));
        let moved = trash_blocking(&src, &trash, exdev).unwrap();
        assert_eq!(moved, trash.join("wall.jpg"));
        assert_eq!(fs::read(&moved).unwrap(), b"image bytes");
        assert!(!src.exists(), "source must be removed after the copy");
    }

    #[test]
    fn trash_copy_fallback_keeps_existing_trash_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("wall.jpg");
        write(&src, b"new");
        let trash = dir.path().join("trash");
        fs::create_dir(&trash).unwrap();
        write(&trash.join("wall.jpg"), b"old");
        let exdev = |_: &Path, _: &Path| Err(io::Error::from(io::ErrorKind::CrossesDevices));
        let moved = trash_blocking(&src, &trash, exdev).unwrap();
        assert!(
            is_collision_name(&moved, "wall", "jpg"),
            "{}",
            moved.display()
        );
        assert_eq!(fs::read(&moved).unwrap(), b"new");
        assert_eq!(fs::read(trash.join("wall.jpg")).unwrap(), b"old");
        assert!(!src.exists());
    }

    /// `FilesError` replaced `EngineError::Files(String)`; the daemon's wire errors depend on
    /// this text.
    #[test]
    fn files_errors_keep_their_go_text() {
        let text = |e: FilesError| EngineError::from(e).to_string();
        assert_eq!(
            text(FilesError::NotConfigured { what: "favorites" }),
            "manager: favorites dir is not configured"
        );
        assert_eq!(
            text(FilesError::CreateDir {
                what: "trash",
                dir: "/t".into(),
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            }),
            "manager: create trash dir: mkdir /t: permission denied"
        );
        assert_eq!(
            text(FilesError::Favorite(CopyFailure::NoFileName("/".into()))),
            "manager: copy to favorites: /: not a file path"
        );
        assert_eq!(
            text(FilesError::TrashTarget(CopyFailure::NoFileName(
                "..".into()
            ))),
            "manager: move to trash: ..: not a file path"
        );
    }

    #[test]
    fn favorite_reports_copy_errors_like_go() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("gone.jpg");
        let favs = dir.path().join("favs");
        let err = EngineError::from(favorite_blocking(&src, &favs).unwrap_err()).to_string();
        assert_eq!(
            err,
            format!(
                "manager: copy to favorites: open {}: no such file or directory",
                src.display()
            )
        );
    }

    #[test]
    fn trash_reports_rename_and_copy_errors_like_go() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("gone.jpg");
        let trash = dir.path().join("trash");
        let exdev = |_: &Path, _: &Path| Err(io::Error::from(io::ErrorKind::CrossesDevices));
        let err = EngineError::from(trash_blocking(&src, &trash, exdev).unwrap_err()).to_string();
        let dst = trash.join("gone.jpg");
        assert_eq!(
            err,
            format!(
                "manager: move to trash: rename {} {}: cross-device link (copy fallback: open {}: no such file or directory)",
                src.display(),
                dst.display(),
                src.display()
            )
        );
        assert!(!dst.exists(), "no partial file in the trash");
    }

    // ---- safe_delete and trim_to_mb ----

    const KB: usize = 1000;
    const MB: u64 = 1_000_000;

    /// A tempdir holding `downloads/` (created) and room for dirs outside it.
    struct Tree {
        root: tempfile::TempDir,
    }

    impl Tree {
        fn new() -> Self {
            let tree = Self {
                root: tempfile::tempdir().unwrap(),
            };
            fs::create_dir_all(tree.downloads()).unwrap();
            tree
        }

        fn downloads(&self) -> PathBuf {
            self.root.path().join("downloads")
        }

        /// Writes `size` bytes at `rel` (under the tempdir root) with mtime `UNIX_EPOCH + age_s`.
        fn file(&self, rel: &str, size: usize, age_s: u64) -> PathBuf {
            let path = self.root.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, vec![0_u8; size]).unwrap();
            let mtime = UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + age_s);
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(mtime)
                .unwrap();
            path
        }
    }

    /// Asserts `safe_delete` refused `path` and left it in place.
    fn assert_refused(result: io::Result<()>, path: &Path, reason: &str) {
        let err = result.expect_err("safe_delete must refuse");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert!(is_refusal(&err), "{err}");
        let msg = err.to_string();
        assert!(
            msg.starts_with("refusing to delete ") && msg.contains(reason),
            "{msg}"
        );
        assert!(path.exists(), "{} must survive", path.display());
    }

    #[test]
    fn safe_delete_deletes_a_download() {
        let t = Tree::new();
        let p = t.file("downloads/bing/a.jpg", 1, 0);
        let legacy = t.file("downloads/legacy.jpg", 1, 0);
        safe_delete(&p, &t.downloads(), &Protect::new()).unwrap();
        safe_delete(&legacy, &t.downloads(), &Protect::new()).unwrap();
        assert!(!p.exists());
        assert!(!legacy.exists());
    }

    #[test]
    fn safe_delete_refuses_a_dotdot_escape() {
        let t = Tree::new();
        let outside = t.file("outside.jpg", 1, 0);
        let sneaky = t.downloads().join("..").join("outside.jpg");
        assert_refused(
            safe_delete(&sneaky, &t.downloads(), &Protect::new()),
            &outside,
            "outside the download dir",
        );
    }

    #[test]
    fn safe_delete_refuses_a_file_outside_the_download_dir() {
        let t = Tree::new();
        let outside = t.file("elsewhere/a.jpg", 1, 0);
        assert_refused(
            safe_delete(&outside, &t.downloads(), &Protect::new()),
            &outside,
            "outside the download dir",
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_delete_refuses_a_symlink_pointing_outside() {
        let t = Tree::new();
        let outside = t.file("elsewhere/a.jpg", 1, 0);
        let link = t.downloads().join("link.jpg");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_refused(
            safe_delete(&link, &t.downloads(), &Protect::new()),
            &link,
            "outside the download dir",
        );
        assert!(outside.exists());
    }

    #[cfg(unix)]
    #[test]
    fn safe_delete_refuses_a_path_through_a_symlinked_dir_pointing_outside() {
        let t = Tree::new();
        let outside = t.file("elsewhere/a.jpg", 1, 0);
        std::os::unix::fs::symlink(outside.parent().unwrap(), t.downloads().join("dir")).unwrap();
        let through = t.downloads().join("dir").join("a.jpg");
        assert_refused(
            safe_delete(&through, &t.downloads(), &Protect::new()),
            &outside,
            "outside the download dir",
        );
    }

    #[test]
    fn safe_delete_refuses_the_manual_dir() {
        let t = Tree::new();
        let p = t.file("downloads/manual/a.jpg", 1, 0);
        let protect = Protect::new().dir(t.downloads().join("manual"));
        assert_refused(
            safe_delete(&p, &t.downloads(), &protect),
            &p,
            "inside protected dir",
        );
    }

    #[test]
    fn safe_delete_refuses_a_favorites_dir_nested_in_downloads() {
        let t = Tree::new();
        let p = t.file("downloads/favorites/a.jpg", 1, 0);
        let protect = Protect::new().dir(t.downloads().join("favorites"));
        assert_refused(
            safe_delete(&p, &t.downloads(), &protect),
            &p,
            "inside protected dir",
        );
    }

    #[test]
    fn safe_delete_refuses_the_trash_dir() {
        let t = Tree::new();
        let p = t.file("downloads/.trash/a.jpg", 1, 0);
        let protect = Protect::new().dir(t.downloads().join(".trash"));
        assert_refused(
            safe_delete(&p, &t.downloads(), &protect),
            &p,
            "inside protected dir",
        );
    }

    #[test]
    fn safe_delete_refuses_a_local_source_dir_nested_in_downloads() {
        let t = Tree::new();
        let p = t.file("downloads/mine/deep/a.jpg", 1, 0);
        let protect = Protect::new().dir(t.downloads().join("mine"));
        assert_refused(
            safe_delete(&p, &t.downloads(), &protect),
            &p,
            "inside protected dir",
        );
    }

    #[test]
    fn safe_delete_refuses_everything_when_a_protected_dir_contains_downloads() {
        let t = Tree::new();
        let p = t.file("downloads/bing/a.jpg", 1, 0);
        let protect = Protect::new().dir(t.root.path());
        assert_refused(
            safe_delete(&p, &t.downloads(), &protect),
            &p,
            "inside protected dir",
        );
    }

    #[test]
    fn safe_delete_refuses_the_current_wallpaper() {
        let t = Tree::new();
        let p = t.file("downloads/bing/current.jpg", 1, 0);
        let mut state = State::new();
        state.push(info(&p), chrono::Utc::now());
        let protect = Protect::new().state(&state);
        assert_refused(
            safe_delete(&p, &t.downloads(), &protect),
            &p,
            "protected file",
        );
    }

    #[test]
    fn safe_delete_refuses_a_history_entry() {
        let t = Tree::new();
        let old = t.file("downloads/bing/old.jpg", 1, 0);
        let newer = t.file("downloads/bing/newer.jpg", 1, 0);
        let mut state = State::new();
        state.push(info(&old), chrono::Utc::now());
        state.push(info(&newer), chrono::Utc::now());
        assert_ne!(state.current().map(|i| i.path.as_str()), old.to_str());
        let protect = Protect::new().state(&state);
        assert_refused(
            safe_delete(&old, &t.downloads(), &protect),
            &old,
            "protected file",
        );
    }

    #[test]
    fn safe_delete_refuses_a_protected_file_given_by_another_spelling() {
        let t = Tree::new();
        let p = t.file("downloads/bing/a.jpg", 1, 0);
        let spelled = t
            .downloads()
            .join("bing")
            .join(".")
            .join("..")
            .join("bing")
            .join("a.jpg");
        let protect = Protect::new().file(&p);
        assert_refused(
            safe_delete(&spelled, &t.downloads(), &protect),
            &p,
            "protected file",
        );
    }

    #[test]
    fn safe_delete_refuses_a_directory() {
        let t = Tree::new();
        let dir = t.downloads().join("bing");
        fs::create_dir_all(&dir).unwrap();
        assert_refused(
            safe_delete(&dir, &t.downloads(), &Protect::new()),
            &dir,
            "is a directory",
        );
    }

    #[test]
    fn protect_ignores_empty_paths() {
        assert_eq!(Protect::new().dir("").file(""), Protect::new());
    }

    fn info(path: &Path) -> kakemono_protocol::types::WallpaperInfo {
        kakemono_protocol::types::WallpaperInfo {
            path: path.to_string_lossy().into_owned(),
            ..Default::default()
        }
    }

    fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths.sort();
        paths
    }

    #[tokio::test]
    async fn trim_to_mb_deletes_the_oldest_and_spares_manual_current_and_history() {
        let t = Tree::new();
        let manual = t.file("downloads/manual/m.jpg", 1_000 * KB, 0); // oldest of all
        let legacy = t.file("downloads/legacy.jpg", 400 * KB, 1);
        let a1 = t.file("downloads/a/1.jpg", 400 * KB, 2); // history
        let a2 = t.file("downloads/a/2.jpg", 400 * KB, 3);
        let a3 = t.file("downloads/a/3.jpg", 400 * KB, 4); // current
        let mut state = State::new();
        state.push(info(&a1), chrono::Utc::now());
        state.push(info(&a3), chrono::Utc::now());
        let protect = Protect::new()
            .dir(t.downloads().join("manual"))
            .state(&state);

        // Counted: 1.6 MB (manual/ is not). Delete legacy (1.2 MB), skip a1, delete a2 (0.8 MB).
        let deleted = trim_to_mb(&t.downloads(), MB, &protect).await;
        assert_eq!(sorted(deleted), sorted(vec![legacy.clone(), a2.clone()]));
        assert!(!legacy.exists() && !a2.exists());
        assert!(manual.exists() && a1.exists() && a3.exists());
    }

    #[tokio::test]
    async fn trim_to_mb_spares_a_favorites_dir_nested_in_downloads() {
        let t = Tree::new();
        let fav = t.file("downloads/favorites/f.jpg", 400 * KB, 0);
        let a1 = t.file("downloads/a/1.jpg", 400 * KB, 1);
        let a2 = t.file("downloads/a/2.jpg", 400 * KB, 2);
        let protect = Protect::new().dir(t.downloads().join("favorites"));
        let deleted = trim_to_mb(&t.downloads(), 0, &protect).await;
        assert_eq!(sorted(deleted), sorted(vec![a1, a2]));
        assert!(fav.exists());
    }

    #[test]
    fn counted_bytes_counts_what_the_trim_counts() {
        let t = Tree::new();
        t.file("downloads/a/1.jpg", 300 * KB, 0);
        t.file("downloads/legacy.png", 200 * KB, 0);
        t.file("downloads/favorites/f.jpg", 400 * KB, 0);
        t.file("downloads/a/.partial.jpg", 50 * KB, 0);
        t.file("downloads/a/notes.txt", 10 * KB, 0);
        t.file(&format!("downloads/a/{SEEN_LEDGER}"), 5 * KB, 0);
        let protect = Protect::new().dir(t.downloads().join("favorites"));
        assert_eq!(
            counted_bytes(&t.downloads(), &protect),
            Some(500 * KB as u64)
        );
        assert_eq!(counted_bytes(&t.root.path().join("nope"), &protect), None);
        assert_eq!(counted_bytes(Path::new(""), &protect), None);
    }

    #[test]
    fn protect_config_covers_manual_favorites_trash_and_local_rows() {
        let mut cfg = Config::default();
        cfg.daemon.download_dir = "/d".into();
        cfg.daemon.manual_dir = None;
        cfg.favorites.dir = "/fav".into();
        cfg.daemon.trash_dir = "/trash".into();
        let row = |kind, location: &str, enabled| kakemono_config::SourceEntry {
            id: "x".into(),
            enabled,
            kind,
            location: location.into(),
        };
        cfg.sources = vec![
            row(SourceType::Folder, "/pics", false),
            row(SourceType::Images, "/one.jpg", true),
            row(SourceType::Bing, "", true),
        ];
        let p = Protect::new().config(&cfg);
        assert_eq!(
            p.dirs,
            ["/d/manual", "/fav", "/trash", "/pics"].map(PathBuf::from)
        );
        assert_eq!(p.files, HashSet::from([PathBuf::from("/one.jpg")]));
    }

    #[tokio::test]
    async fn trim_to_mb_neither_counts_nor_deletes_seen_ledgers() {
        let t = Tree::new();
        let ledger = t.file("downloads/a/.kakemono-seen", 2_000 * KB, 0);
        let root_ledger = t.file("downloads/.kakemono-seen", 2_000 * KB, 0);
        let a1 = t.file("downloads/a/1.jpg", 400 * KB, 1);
        // Under the limit without the ledgers: nothing to do.
        assert!(
            trim_to_mb(&t.downloads(), MB, &Protect::new())
                .await
                .is_empty()
        );
        // Over a zero limit: the image goes, the ledgers stay.
        assert_eq!(
            trim_to_mb(&t.downloads(), 0, &Protect::new()).await,
            vec![a1]
        );
        assert!(ledger.exists() && root_ledger.exists());
    }

    #[tokio::test]
    async fn trim_to_mb_does_nothing_under_the_limit() {
        let t = Tree::new();
        let a1 = t.file("downloads/a/1.jpg", 400 * KB, 0);
        let a2 = t.file("downloads/b/2.jpg", 400 * KB, 1);
        assert!(
            trim_to_mb(&t.downloads(), MB, &Protect::new())
                .await
                .is_empty()
        );
        // Exactly at the limit is fine too.
        assert!(
            trim_to_mb(&t.downloads(), 800_000, &Protect::new())
                .await
                .is_empty()
        );
        assert!(a1.exists() && a2.exists());
    }

    #[tokio::test]
    async fn trim_to_mb_skips_hidden_and_partial_files() {
        let t = Tree::new();
        let part = t.file("downloads/a/.x.jpg.123-4.part", 400 * KB, 0);
        let hidden = t.file("downloads/a/.hidden.jpg", 400 * KB, 0);
        let txt = t.file("downloads/a/notes.txt", 400 * KB, 0);
        assert!(
            trim_to_mb(&t.downloads(), 0, &Protect::new())
                .await
                .is_empty()
        );
        assert!(part.exists() && hidden.exists() && txt.exists());
    }

    #[tokio::test]
    async fn trim_to_mb_deletes_nothing_when_a_protected_dir_contains_downloads() {
        let t = Tree::new();
        let a1 = t.file("downloads/a/1.jpg", 400 * KB, 0);
        let protect = Protect::new().dir(t.root.path());
        assert!(trim_to_mb(&t.downloads(), 0, &protect).await.is_empty());
        assert!(a1.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn trim_to_mb_does_not_follow_symlinks_out_of_downloads() {
        let t = Tree::new();
        let outside = t.file("elsewhere/a.jpg", 400 * KB, 0);
        std::os::unix::fs::symlink(outside.parent().unwrap(), t.downloads().join("dir")).unwrap();
        std::os::unix::fs::symlink(&outside, t.downloads().join("link.jpg")).unwrap();
        assert!(
            trim_to_mb(&t.downloads(), 0, &Protect::new())
                .await
                .is_empty()
        );
        assert!(outside.exists());
    }

    #[tokio::test]
    async fn trim_to_mb_ignores_a_missing_or_unset_dir() {
        let t = Tree::new();
        let gone = t.root.path().join("nope");
        assert!(trim_to_mb(&gone, 0, &Protect::new()).await.is_empty());
        assert!(
            trim_to_mb(Path::new(""), 0, &Protect::new())
                .await
                .is_empty()
        );
    }
}
