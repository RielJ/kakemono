//! The folder a source owns (`download_dir/<id>`, or a local source's own folder), and
//! deleting or opening it.

use super::{SourceError, SrcResult};
use crate::commands::CommandError;
use crate::commands::{blocking, load_config};
use kakemono_config::Config;
use kakemono_config::schema::{SourceEntry, SourceType};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;

/// Valid-looking ids that name folders a source must never own.
const RESERVED_IDS: [&str; 3] = ["manual", "favorites", "trash"];

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

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::*;

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
}
