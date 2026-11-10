//! Daemon control: status, next/prev, pause/resume, favorite, trash and `set_file`.

use super::{CmdResult, CommandError, blocking, load_config};
use crate::daemon_link::DaemonLink;
use crate::thumbs::Thumbs;
use kakemono_protocol::types::{Request, StatusData};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Runtime, State};

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
/// (a picker entry, [`require_listed`]) is copied into `[favorites] dir`, as the Go picker did.
#[tauri::command]
pub async fn favorite(
    link: State<'_, Arc<DaemonLink>>,
    thumbs: State<'_, Arc<Thumbs>>,
    path: Option<String>,
) -> CmdResult<()> {
    match path.filter(|p| !p.is_empty()) {
        None => simple(&link, Request::Favorite).await,
        Some(path) => {
            let thumbs = Arc::clone(&thumbs);
            blocking(move || {
                require_listed(&thumbs, "favorite", &path)?;
                let cfg = load_config()?;
                copy_to_favorites(Path::new(&path), &cfg.favorites.dir).map(drop)
            })
            .await
        }
    }
}

/// Refuses a `path` that is not an entry of the last `thumbnails` listing
/// ([`Thumbs::is_admitted`]): the picker never sends anything else, so a page script can't
/// make `command` touch an arbitrary file. Does file I/O; call it off the async runtime.
pub fn require_listed(thumbs: &Thumbs, command: &str, path: &str) -> CmdResult<()> {
    if thumbs.is_admitted(Path::new(path)) {
        Ok(())
    } else {
        Err(CommandError::Invalid(format!(
            "{command}: not a picker entry: {path}"
        )))
    }
}

/// Copies `src` into `dir` (created if missing) under its file name, or a collision-free
/// variant of it ([`kakemono_fs::copy_unique_into`]), so an existing favourite is never
/// overwritten. A file already in `dir` is left alone. Returns the favourite's path.
pub fn copy_to_favorites(src: &Path, dir: &Path) -> CmdResult<PathBuf> {
    if src.file_name().is_none() {
        return Err(CommandError::Invalid(format!(
            "not a file: {}",
            src.display()
        )));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| CommandError::io(format!("creating {}", dir.display()), e))?;
    if src.parent() == Some(dir) {
        return Ok(src.to_owned());
    }
    Ok(kakemono_fs::copy_unique_into(src, dir)?)
}

/// `set_file { path }`: IPC `set` for a picker entry ([`require_listed`]).
#[tauri::command]
pub async fn set_file(
    link: State<'_, Arc<DaemonLink>>,
    thumbs: State<'_, Arc<Thumbs>>,
    path: String,
) -> CmdResult<()> {
    if path.is_empty() {
        return Err(CommandError::Invalid("set_file: path is empty".into()));
    }
    let thumbs = Arc::clone(&thumbs);
    let path = blocking(move || require_listed(&thumbs, "set_file", &path).map(|()| path)).await?;
    simple(&link, Request::Set { path }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorKind, Kinded};

    #[test]
    fn picker_paths_must_come_from_the_last_listing() {
        let dir = tempfile::tempdir().unwrap();
        let listed = dir.path().join("a.jpg");
        let secret = dir.path().join("id_ed25519");
        std::fs::write(&listed, b"img").unwrap();
        std::fs::write(&secret, b"key").unwrap();
        let thumbs = Thumbs::new(dir.path().join("cache"));
        thumbs.admit(
            crate::thumbs::ListKind::Legacy,
            &[dir.path()],
            &[listed.to_str().unwrap()],
        );

        assert!(require_listed(&thumbs, "favorite", &listed.to_string_lossy()).is_ok());
        let err =
            require_listed(&thumbs, "favorite", &secret.to_string_lossy()).expect_err("not listed");
        assert_eq!(err.kind(), ErrorKind::Invalid);
        assert!(
            err.to_string().starts_with("favorite: not a picker entry"),
            "{err}"
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

    #[test]
    fn favorites_copy_never_overwrites_a_favorite_with_the_same_name() {
        let dir = tempfile::tempdir().unwrap();
        let favs = dir.path().join("favs");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("1.jpg"), b"first").unwrap();
        std::fs::write(b.join("1.jpg"), b"second").unwrap();

        let first = copy_to_favorites(&a.join("1.jpg"), &favs).unwrap();
        let second = copy_to_favorites(&b.join("1.jpg"), &favs).unwrap();

        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
    }

    #[test]
    fn favorites_copy_of_a_file_already_in_favorites_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("pic.jpg");
        std::fs::write(&src, b"img").unwrap();
        assert_eq!(copy_to_favorites(&src, dir.path()).unwrap(), src);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
