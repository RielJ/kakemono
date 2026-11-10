//! The file manager and static app facts: `reveal`, `open_folder`, `app_info`.

use super::control::require_listed;
use super::{CmdResult, CommandError, blocking, locate_config};
use crate::daemon_link::DaemonLink;
use crate::thumbs::Thumbs;
use kakemono_config::paths::Paths;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_opener::OpenerExt;
use ts_rs::TS;

// ── Files, URLs, platform ───────────────────────────────────────────────────

/// `reveal { path }`: shows the file in Finder / Explorer / the file manager. Only a picker
/// entry ([`require_listed`]), the config file, or the daemon's current wallpaper.
#[tauri::command]
pub async fn reveal<R: Runtime>(
    app: AppHandle<R>,
    link: State<'_, Arc<DaemonLink>>,
    thumbs: State<'_, Arc<Thumbs>>,
    path: String,
) -> CmdResult<()> {
    if path.is_empty() {
        return Err(CommandError::Invalid("reveal: path is empty".into()));
    }
    let current = link
        .status()
        .await
        .ok()
        .and_then(|s| s.current)
        .map(|c| c.path);
    let thumbs = Arc::clone(&thumbs);
    let path = blocking(move || {
        let (_, source) = locate_config()?;
        let known = [Some(source.path().to_owned()), current.map(PathBuf::from)];
        if !known
            .iter()
            .flatten()
            .any(|p| same_file(p, Path::new(&path)))
        {
            require_listed(&thumbs, "reveal", &path)?;
        }
        CmdResult::Ok(path)
    })
    .await?;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| CommandError::Opener(e.to_string()))
}

/// Both paths name the same existing file (symlinks resolved).
fn same_file(a: &Path, b: &Path) -> bool {
    matches!(
        (std::fs::canonicalize(a), std::fs::canonicalize(b)),
        (Ok(a), Ok(b)) if a == b
    )
}

/// `open_folder { path }`: opens a folder in the file manager. Only an existing folder is
/// opened (a leading `~` is expanded), so the page can't launch a file this way.
#[tauri::command]
pub async fn open_folder<R: Runtime>(app: AppHandle<R>, path: String) -> CmdResult<()> {
    let dir = blocking(move || {
        let home = Paths::detect()?.home;
        folder_to_open(&path, &home)
    })
    .await?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| CommandError::Opener(e.to_string()))
}

/// The folder `open_folder` opens: `path` with `~` expanded, absolute and an existing directory
/// (symlinks followed).
pub fn folder_to_open(path: &str, home: &Path) -> CmdResult<PathBuf> {
    let path = Path::new(path);
    let path = kakemono_config::expand_tilde(path, home).unwrap_or_else(|| path.to_owned());
    if !path.is_absolute() {
        return Err(CommandError::Invalid(format!(
            "open_folder: not an absolute path: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(CommandError::Invalid(format!(
            "open_folder: not a folder: {}",
            path.display()
        )));
    }
    Ok(path)
}

/// Static facts the settings window needs.
#[derive(Debug, Clone, Serialize, TS)]
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
pub async fn app_info() -> CmdResult<AppInfo> {
    let (_, source) = blocking(locate_config).await?;
    Ok(AppInfo {
        os: std::env::consts::OS,
        config_path: source.path().display().to_string(),
        legacy_config: source.is_legacy(),
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorKind, Kinded};

    #[test]
    fn open_folder_accepts_only_existing_folders() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("pics");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(home.path().join("run.command"), b"#!/bin/sh").unwrap();

        assert_eq!(
            folder_to_open(&dir.to_string_lossy(), home.path()).unwrap(),
            dir
        );
        assert_eq!(folder_to_open("~/pics", home.path()).unwrap(), dir);
        for refused in [
            String::new(),
            "pics".to_owned(),
            home.path()
                .join("run.command")
                .to_string_lossy()
                .into_owned(),
            home.path().join("missing").to_string_lossy().into_owned(),
        ] {
            let err = folder_to_open(&refused, home.path()).expect_err(&refused);
            assert_eq!(err.kind(), ErrorKind::Invalid, "{refused}");
        }
    }
}
