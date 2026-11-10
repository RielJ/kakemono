//! The picker list: history, then images from the source folders, then favorites. The
//! listing also admits each path for `thumb://` ([`crate::thumbs::Thumbs::admit`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_config::Config;
use kakemono_config::schema::SourceType;
use kakemono_protocol::types::WallpaperInfo;
use serde::{Deserialize, Serialize};
use tauri::State;
use ts_rs::TS;

use super::{CmdResult, blocking, locate_config};
use crate::thumbs::{ListKind, Thumbs};

/// The picker shows at most this many entries, as the Go picker did.
pub const MAX_THUMBNAILS: usize = 100;

// ── Picker entries ──────────────────────────────────────────────────────────

/// One picker entry, in the Go picker's JSON shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
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

/// `thumbnails` → history (newest first), then the folders of enabled sources (online
/// downloads, then `manual_dir`, then local sources), then the favourites dir
/// ([`thumbnail_dirs`]); de-duplicated, missing files skipped, at most [`MAX_THUMBNAILS`].
/// The listed files become the `thumb://` allowlist.
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
        let dirs = thumbnail_dirs(&cfg);
        let mut list = list_thumbnails(&state, &dirs);
        let mut roots = vec![cfg.daemon.download_dir.clone(), cfg.favorites.dir];
        roots.extend(dirs.into_iter().map(|d| d.path));
        let paths: Vec<&str> = list.iter().map(|t| t.path.as_str()).collect();
        let urls = thumbs.admit(ListKind::Legacy, &roots, &paths);
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

/// Pure listing logic behind [`thumbnails`]: history, then the images in `dirs` in order.
pub fn list_thumbnails(state: &StateView, dirs: &[ThumbDir]) -> Vec<Thumbnail> {
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
    for dir in dirs {
        if out.len() >= MAX_THUMBNAILS {
            break;
        }
        for (path, stem) in list_images(dir, MAX_THUMBNAILS) {
            if !seen.insert(path.clone()) {
                continue;
            }
            out.push(Thumbnail {
                is_current: path == current,
                path,
                source: dir.source.clone(),
                title: stem,
                image_url: String::new(),
            });
        }
    }
    out.truncate(MAX_THUMBNAILS);
    out
}

/// Image extensions the picker lists.
const IMAGE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// A recursive listing stops after visiting this many directory entries.
const MAX_WALK_ENTRIES: usize = 10_000;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands_sources::fixtures::*;
    use serde_json::json;

    fn info(path: &Path, source: &str) -> WallpaperInfo {
        WallpaperInfo {
            path: path.display().to_string(),
            source: source.into(),
            title: format!("{source} title"),
            ..Default::default()
        }
    }

    #[test]
    fn thumbnails_list_history_then_source_dirs_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let wh = dir.path().join("dl/wh-1");
        let manual = dir.path().join("dl/manual");
        let pics = dir.path().join("pics");
        let favs = dir.path().join("favs");
        for d in [&wh, &manual, &pics.join("sub"), &favs] {
            std::fs::create_dir_all(d).unwrap();
        }
        let a = wh.join("a.jpg");
        let b = wh.join("b.PNG");
        let gone = wh.join("gone.jpg");
        let dropped = manual.join("m.jpg");
        let nested = pics.join("sub/n.webp");
        let fav = favs.join("f.webp");
        for p in [&a, &b, &dropped, &nested, &fav] {
            std::fs::write(p, b"x").unwrap();
        }
        std::fs::write(wh.join("notes.txt"), b"x").unwrap();

        let state = StateView {
            current: Some(info(&b, "wallhaven")),
            history: Some(vec![
                info(&a, "reddit"),
                info(&gone, "x"),
                info(&b, "wallhaven"),
            ]),
        };
        let td = |path: &Path, source: &str, listing| ThumbDir {
            path: path.to_owned(),
            source: source.to_owned(),
            listing,
        };
        let dirs = [
            td(&wh, "wallhaven", Listing::Flat),
            td(&manual, "manual", Listing::Flat),
            td(&pics, "folder", Listing::Recursive),
            td(&favs, "favorites", Listing::Flat),
        ];
        let got = list_thumbnails(&state, &dirs);
        let summary: Vec<(String, &str, bool)> = got
            .iter()
            .map(|t| (t.title.clone(), t.source.as_str(), t.is_current))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("wallhaven title".to_owned(), "wallhaven", true),
                ("reddit title".to_owned(), "reddit", false),
                ("m".to_owned(), "manual", false),
                ("n".to_owned(), "folder", false),
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
}
