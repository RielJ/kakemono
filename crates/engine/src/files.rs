//! Favorites, trash and download-cache trimming (plan §D). Port of Go `Manager.Favorite`,
//! `Manager.Trash`, `trimCache` and `copyFile` (`internal/wallpaper/manager.go`).
//!
//! Differences from Go:
//! - `trash` never overwrites a file already in the trash. It uses the same collision name as
//!   `favorite` (`<stem>_<unix nanos>.<ext>`). Go's `os.Rename` replaced the older file.
//! - A copy that fails halfway removes the partial destination file. Go left it behind.
//! - `trim_cache` also keeps the image of the previous history entry, not only the current one.
//!   GNOME's dark-mode key can still point at it (plan Task 20).
//! - An empty favorites or trash dir is an error. Go would have written into the working dir.
//!
//! The file work runs on tokio's blocking pool (`spawn_blocking`) with `std::fs`.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kakemono_sources::is_image_file;

use crate::engine::EngineError;
use crate::state::State;

/// How many history entries (newest first) `trim_cache` never deletes, besides `current`.
const PROTECTED_HISTORY: usize = 2;

/// How many collision names `create_unique` tries before it gives up.
const MAX_NAME_ATTEMPTS: u32 = 1000;

/// Copies `src` into `favorites_dir` and returns the new file's path. Errors use
/// `EngineError::Files` (rendered as `manager: <message>`).
pub(crate) async fn favorite(src: &Path, favorites_dir: &Path) -> Result<PathBuf, EngineError> {
    let (src, dir) = (src.to_path_buf(), favorites_dir.to_path_buf());
    blocking(move || favorite_blocking(&src, &dir)).await
}

/// Moves `src` into `trash_dir` and returns its new path. The engine then runs `next`.
pub(crate) async fn trash(src: &Path, trash_dir: &Path) -> Result<PathBuf, EngineError> {
    let (src, dir) = (src.to_path_buf(), trash_dir.to_path_buf());
    blocking(move || trash_blocking(&src, &dir, |from, to| fs::rename(from, to))).await
}

/// Deletes the oldest images in `download_dir` beyond `cache_size` (0 = unlimited). `state` is
/// the state after the new image was pushed (its `current` and history are protected). Failures
/// are logged, never returned.
pub(crate) async fn trim_cache(download_dir: &Path, cache_size: u32, state: &State) {
    if cache_size == 0 || download_dir.as_os_str().is_empty() {
        return;
    }
    let dir = download_dir.to_path_buf();
    let protected = protected_paths(state);
    let joined = tokio::task::spawn_blocking(move || trim_blocking(&dir, cache_size, &protected));
    match joined.await {
        Ok(deleted) if !deleted.is_empty() => {
            tracing::debug!(count = deleted.len(), "trimmed the download cache");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!("trim cache: {err}"),
    }
}

async fn blocking<T, F>(f: F) -> Result<T, EngineError>
where
    F: FnOnce() -> Result<T, EngineError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|err| EngineError::Files(format!("file task failed: {err}")))?
}

fn favorite_blocking(src: &Path, dir: &Path) -> Result<PathBuf, EngineError> {
    ensure_dir(dir, "favorites")?;
    let dst =
        target_path(src, dir).map_err(|e| EngineError::Files(format!("copy to favorites: {e}")))?;
    copy_unique(src, &dst).map_err(|e| EngineError::Files(format!("copy to favorites: {e}")))
}

/// Go `Trash`: rename into the trash, else (another device, or any rename error) copy and
/// remove the source. `rename` is injectable so tests can simulate a cross-device move.
fn trash_blocking(
    src: &Path,
    dir: &Path,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, EngineError> {
    ensure_dir(dir, "trash")?;
    let dst =
        target_path(src, dir).map_err(|e| EngineError::Files(format!("move to trash: {e}")))?;
    let dst = if dst.exists() {
        collision_free(&dst)
    } else {
        dst
    };
    let rename_err = match rename(src, &dst) {
        Ok(()) => return Ok(dst),
        Err(err) => format!(
            "rename {} {}: {}",
            src.display(),
            dst.display(),
            go_reason(&err)
        ),
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
        Err(copy_err) => Err(EngineError::Files(format!(
            "move to trash: {rename_err} (copy fallback: {copy_err})"
        ))),
    }
}

/// `create_dir_all`, with Go's `create <what> dir: mkdir <dir>: <reason>` message.
fn ensure_dir(dir: &Path, what: &str) -> Result<(), EngineError> {
    if dir.as_os_str().is_empty() {
        return Err(EngineError::Files(format!("{what} dir is not configured")));
    }
    fs::create_dir_all(dir).map_err(|err| {
        EngineError::Files(format!(
            "create {what} dir: mkdir {}: {}",
            dir.display(),
            go_reason(&err)
        ))
    })
}

/// `dir/<file name of src>` (Go `filepath.Join(dir, filepath.Base(src))`).
fn target_path(src: &Path, dir: &Path) -> Result<PathBuf, String> {
    src.file_name()
        .map(|name| dir.join(name))
        .ok_or_else(|| format!("{}: not a file path", src.display()))
}

/// Go `copyFile`: copies `src` to `dst`, or to a collision-free name next to it when `dst`
/// exists. Returns the path written. Never overwrites a file. Errors are Go-style
/// (`open <path>: <reason>`), without a prefix.
fn copy_unique(src: &Path, dst: &Path) -> Result<PathBuf, String> {
    let mut input = File::open(src).map_err(|e| go_path_error("open", src, &e))?;
    let (mut output, written) = create_unique(dst)?;
    let copied = io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    if let Err(err) = copied {
        drop(output);
        let _ = fs::remove_file(&written);
        return Err(go_path_error("write", &written, &err));
    }
    Ok(written)
}

/// Creates `dst` exclusively. If it exists, tries `<stem>_<unix nanos>.<ext>` (Go's collision
/// name), counting up from the first timestamp until a name is free.
fn create_unique(dst: &Path) -> Result<(File, PathBuf), String> {
    let mut candidate = dst.to_path_buf();
    let mut nanos = unix_nanos();
    for _ in 0..=MAX_NAME_ATTEMPTS {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((file, candidate)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                candidate = suffixed(dst, nanos);
                nanos = nanos.saturating_add(1);
            }
            Err(err) => return Err(go_path_error("open", &candidate, &err)),
        }
    }
    Err(format!(
        "open {}: no free file name after {MAX_NAME_ATTEMPTS} attempts",
        dst.display()
    ))
}

/// A name for `dst` that doesn't exist yet (checked, not reserved; the engine's op lock keeps
/// other moves out).
fn collision_free(dst: &Path) -> PathBuf {
    let mut nanos = unix_nanos();
    loop {
        let candidate = suffixed(dst, nanos);
        if !candidate.exists() {
            return candidate;
        }
        nanos = nanos.saturating_add(1);
    }
}

/// Go's `<base>_<n><ext>`, where `ext` is `filepath.Ext` (from the last `.` of the file name,
/// so `.hidden` becomes `_<n>.hidden`).
fn suffixed(dst: &Path, n: u128) -> PathBuf {
    let name = dst
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (base, ext) = name
        .rfind('.')
        .map_or((name.as_str(), ""), |i| name.split_at(i));
    dst.with_file_name(format!("{base}_{n}{ext}"))
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

/// The paths `trim_cache` must keep: `current` and the newest `PROTECTED_HISTORY` history
/// entries, each as given and canonicalised.
fn protected_paths(state: &State) -> HashSet<PathBuf> {
    let history = state.history();
    let recent = &history[history.len().saturating_sub(PROTECTED_HISTORY)..];
    state
        .current()
        .into_iter()
        .chain(recent)
        .map(|info| PathBuf::from(&info.path))
        .filter(|p| !p.as_os_str().is_empty())
        .flat_map(|p| {
            let canonical = fs::canonicalize(&p).ok();
            std::iter::once(p).chain(canonical)
        })
        .collect()
}

/// Go `trimCache`: lists the images directly in `dir` (no subdirs, no other files), oldest
/// modification time first, and deletes the oldest until `cache_size` remain. Protected files
/// are skipped and the next oldest is deleted instead. Returns the paths it removed.
fn trim_blocking(dir: &Path, cache_size: u32, protected: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!("trim cache: read {}: {}", dir.display(), go_reason(&err));
            }
            return Vec::new();
        }
    };
    let canonical_dir = fs::canonicalize(dir).ok();
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let path = entry.path();
            if meta.is_dir() || !is_image_file(&path) {
                return None;
            }
            Some((meta.modified().ok()?, path))
        })
        .collect();

    let keep = usize::try_from(cache_size).unwrap_or(usize::MAX);
    if files.len() <= keep {
        return Vec::new();
    }
    files.sort();

    let is_protected = |path: &Path| {
        protected.contains(path)
            || canonical_dir
                .as_ref()
                .zip(path.file_name())
                .is_some_and(|(d, name)| protected.contains(&d.join(name)))
    };
    let excess = files.len() - keep;
    let mut deleted = Vec::new();
    let mut attempts = 0;
    for (_, path) in files {
        if attempts >= excess {
            break;
        }
        if is_protected(&path) {
            continue;
        }
        // Go counted a failed remove too, so a stuck file doesn't take a newer one with it.
        attempts += 1;
        match fs::remove_file(&path) {
            Ok(()) => deleted.push(path),
            Err(err) => {
                tracing::warn!("trim cache: remove {}: {}", path.display(), go_reason(&err))
            }
        }
    }
    deleted
}

/// Go `*PathError` text: `<op> <path>: <reason>`.
fn go_path_error(op: &str, path: &Path, err: &io::Error) -> String {
    format!("{op} {}: {}", path.display(), go_reason(err))
}

/// Go's lower-case `syscall.Errno` text for the common cases, else Rust's message.
fn go_reason(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "no such file or directory".to_owned(),
        io::ErrorKind::PermissionDenied => "permission denied".to_owned(),
        io::ErrorKind::NotADirectory => "not a directory".to_owned(),
        io::ErrorKind::IsADirectory => "is a directory".to_owned(),
        io::ErrorKind::AlreadyExists => "file exists".to_owned(),
        io::ErrorKind::CrossesDevices => "cross-device link".to_owned(),
        _ => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn suffixed_keeps_the_extension() {
        let p = Path::new("/d/photo.jpg");
        assert_eq!(suffixed(p, 42), Path::new("/d/photo_42.jpg"));
        assert_eq!(
            suffixed(Path::new("/d/a.tar.gz"), 7),
            Path::new("/d/a.tar_7.gz")
        );
        assert_eq!(suffixed(Path::new("/d/noext"), 7), Path::new("/d/noext_7"));
        assert_eq!(
            suffixed(Path::new("/d/.hidden"), 7),
            Path::new("/d/_7.hidden")
        );
    }

    #[test]
    fn create_unique_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("a.png");
        write(&dst, b"old");
        let (_f1, p1) = create_unique(&dst).unwrap();
        let (_f2, p2) = create_unique(&dst).unwrap();
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

    #[test]
    fn trash_reports_rename_and_copy_errors_like_go() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("gone.jpg");
        let trash = dir.path().join("trash");
        let exdev = |_: &Path, _: &Path| Err(io::Error::from(io::ErrorKind::CrossesDevices));
        let err = trash_blocking(&src, &trash, exdev).unwrap_err().to_string();
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

    #[test]
    fn trim_ignores_a_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("nope");
        assert!(trim_blocking(&gone, 1, &HashSet::new()).is_empty());
    }
}
