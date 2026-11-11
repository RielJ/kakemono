//! The picker's on-disk lists. Each listing also admits its paths for `thumb://`
//! ([`crate::thumbs::Thumbs::admit`]).
//!
//! - `tab_thumbnails { tab, offset }`: one [`PickerTab`], paged ([`TAB_PAGE`]).
//! - `current_thumbnail`: the current wallpaper alone, for the Settings window's "Now
//!   showing" card ([`ListKind::Current`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use kakemono_config::Config;
use kakemono_config::schema::SourceType;
use kakemono_protocol::types::WallpaperInfo;
use serde::{Deserialize, Serialize};
use tauri::State;
use ts_rs::TS;

use super::{CmdResult, blocking, locate_config};
use crate::thumbs::{ListKind, Thumbs};

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

/// `current_thumbnail` → the current wallpaper as a picker entry flagged current, admitted
/// for `thumb://` under [`ListKind::Current`] (which replaces only the last current; the
/// picker tabs keep theirs). `null` when nothing is showing or the file is gone.
#[tauri::command]
pub async fn current_thumbnail(thumbs: State<'_, Arc<Thumbs>>) -> CmdResult<Option<Thumbnail>> {
    let thumbs = Arc::clone(&thumbs);
    blocking(move || {
        let (cfg, state) = load_listing_inputs()?;
        Ok(current_entry(&thumbs, &cfg, &state))
    })
    .await
}

/// The config, and the daemon's state file.
fn load_listing_inputs() -> CmdResult<(Config, StateView)> {
    let (paths, source) = locate_config()?;
    let cfg = kakemono_config::load_or_default(source.path())?;
    Ok((cfg, read_state(&paths.state_file)))
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

/// [`current_thumbnail`] after the config and state are read. The roots are every picker
/// tab's folders, so the card shows any wallpaper the picker could; a current file outside
/// them gets an empty `image_url`.
pub fn current_entry(thumbs: &Thumbs, cfg: &Config, state: &StateView) -> Option<Thumbnail> {
    let current = state.current.as_ref()?;
    if current.path.is_empty() || !Path::new(&current.path).is_file() {
        thumbs.admit::<&Path>(ListKind::Current, &[], &[]);
        return None;
    }
    let roots: Vec<PathBuf> = PickerTab::ALL
        .into_iter()
        .flat_map(|tab| tab_dirs(cfg, tab))
        .map(|dir| dir.path)
        .collect();
    let image_url = thumbs
        .admit(ListKind::Current, &roots, &[current.path.as_str()])
        .pop()
        .unwrap_or_default();
    Some(Thumbnail {
        path: current.path.clone(),
        source: current.source.clone(),
        title: current.title.clone(),
        is_current: true,
        image_url,
    })
}

// ── Picker tabs ─────────────────────────────────────────────────────────────

/// A `tab_thumbnails` page holds at most this many entries.
pub const TAB_PAGE: usize = 40;

/// An on-disk picker tab. Wire: `"downloaded"`, `"local"`, `"favorites"` or `"cache"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum PickerTab {
    /// Enabled online sources' download folders, then `manual_dir`.
    Downloaded,
    /// Enabled folder, album and images sources.
    Local,
    /// The favorites dir.
    Favorites,
    /// Loose files at the root of `download_dir`, then `download_dir/<id>/` folders whose id
    /// is not an enabled online source.
    Cache,
}

/// One page of a [`PickerTab`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TabListing {
    pub items: Vec<Thumbnail>,
    /// `0` when the current wallpaper is in this tab and pinned first on this page (only on
    /// offset 0); else `null`.
    pub current_index: Option<u32>,
    /// More entries follow this page.
    pub has_more: bool,
}

/// `tab_thumbnails { tab, offset }` → one page of `tab`: the current wallpaper first when the
/// tab holds it (only at offset 0), then the tab's images newest first by mtime, at most
/// [`TAB_PAGE`] from `offset`. Admits the tab's entries up to the end of this page for
/// `thumb://` under the tab's [`ListKind`], so earlier pages stay servable and pickable.
#[tauri::command]
pub async fn tab_thumbnails(
    thumbs: State<'_, Arc<Thumbs>>,
    tab: PickerTab,
    offset: u32,
) -> CmdResult<TabListing> {
    let thumbs = Arc::clone(&thumbs);
    blocking(move || {
        let (cfg, state) = load_listing_inputs()?;
        Ok(tab_listing(&thumbs, &cfg, &state, tab, offset))
    })
    .await
}

impl PickerTab {
    /// Every tab, in strip order.
    pub const ALL: [Self; 4] = [Self::Downloaded, Self::Local, Self::Favorites, Self::Cache];

    /// The `thumb://` allowlist this tab's listing replaces.
    pub fn list_kind(self) -> ListKind {
        match self {
            Self::Downloaded => ListKind::Downloaded,
            Self::Local => ListKind::Local,
            Self::Favorites => ListKind::Favorites,
            Self::Cache => ListKind::Cache,
        }
    }
}

/// Where `tab` lists images from, in order; a path already listed is skipped. Cache reads
/// `download_dir` to find the orphan folders.
pub fn tab_dirs(cfg: &Config, tab: PickerTab) -> Vec<ThumbDir> {
    let dirs = match tab {
        PickerTab::Downloaded => {
            let manual = thumb_dir(cfg.daemon.manual_dir_resolved(), "manual", Listing::Flat);
            online_dirs(cfg).chain(std::iter::once(manual)).collect()
        }
        PickerTab::Local => local_dirs(cfg).collect(),
        PickerTab::Favorites => vec![favorites_dir(cfg)],
        PickerTab::Cache => {
            let root = thumb_dir(cfg.daemon.download_dir.clone(), "cache", Listing::Flat);
            std::iter::once(root).chain(orphan_dirs(cfg)).collect()
        }
    };
    distinct(dirs)
}

/// The folders in `download_dir` that hold files of no enabled online source: valid source
/// ids with no enabled online row, sorted by name. `manual_dir`, the favorites and trash dirs
/// and dot-dirs are not orphans. Symlinked folders are skipped.
fn orphan_dirs(cfg: &Config) -> impl Iterator<Item = ThumbDir> {
    let download_dir = &cfg.daemon.download_dir;
    let live: HashSet<&str> = cfg
        .sources
        .iter()
        .filter(|e| e.enabled && e.kind.is_online())
        .map(|e| e.id.as_str())
        .collect();
    let owned = [
        cfg.daemon.manual_dir_resolved(),
        cfg.favorites.dir.clone(),
        cfg.daemon.trash_dir.clone(),
    ];
    let mut names: Vec<String> = std::fs::read_dir(download_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| kakemono_config::valid_id(name) && !live.contains(name.as_str()))
        .filter(|name| !owned.contains(&download_dir.join(name)))
        .collect();
    names.sort_unstable();
    names.into_iter().map(move |name| {
        let path = download_dir.join(&name);
        thumb_dir(path, &name, Listing::Flat)
    })
}

/// One tab entry and the mtime it is ordered by.
struct TabEntry {
    thumb: Thumbnail,
    modified: SystemTime,
}

/// The whole of a tab: the images in `dirs` de-duplicated, newest first by mtime (ties by
/// path), with the `current` path moved first and flagged. Files that vanish while listed
/// are skipped.
fn tab_entries(dirs: &[ThumbDir], current: Option<&str>) -> Vec<Thumbnail> {
    let mut seen = HashSet::new();
    let mut entries: Vec<TabEntry> = Vec::new();
    for dir in dirs {
        for (path, stem) in list_images(dir, MAX_WALK_ENTRIES) {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            entries.push(TabEntry {
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                thumb: Thumbnail {
                    is_current: current == Some(path.as_str()),
                    path,
                    source: dir.source.clone(),
                    title: stem,
                    image_url: String::new(),
                },
            });
        }
    }
    entries.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.thumb.path.cmp(&b.thumb.path))
    });
    let mut list: Vec<Thumbnail> = entries.into_iter().map(|e| e.thumb).collect();
    if let Some(at) = list.iter().position(|t| t.is_current) {
        list[..=at].rotate_right(1);
    }
    list
}

/// [`tab_thumbnails`] after the config and state are read.
pub fn tab_listing(
    thumbs: &Thumbs,
    cfg: &Config,
    state: &StateView,
    tab: PickerTab,
    offset: u32,
) -> TabListing {
    let dirs = tab_dirs(cfg, tab);
    let current = state.current.as_ref().map(|c| c.path.as_str());
    let mut list = tab_entries(&dirs, current);
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let end = offset.saturating_add(TAB_PAGE).min(list.len());
    let start = offset.min(end);
    let has_more = end < list.len();
    list.truncate(end);

    let roots: Vec<&Path> = dirs.iter().map(|d| d.path.as_path()).collect();
    let paths: Vec<&str> = list.iter().map(|t| t.path.as_str()).collect();
    let urls = thumbs.admit(tab.list_kind(), &roots, &paths);
    let items: Vec<Thumbnail> = list
        .into_iter()
        .zip(urls)
        .skip(start)
        .map(|(thumb, image_url)| Thumbnail { image_url, ..thumb })
        .collect();
    let current_index = (start == 0 && items.first().is_some_and(|t| t.is_current)).then_some(0);
    TabListing {
        items,
        current_index,
        has_more,
    }
}

/// Image extensions the picker lists.
const IMAGE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// A recursive listing stops after visiting this many directory entries.
const MAX_WALK_ENTRIES: usize = 10_000;

// ── Picker listing ──────────────────────────────────────────────────────────

/// How [`list_images`] lists a [`ThumbDir`].
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

fn thumb_dir(path: PathBuf, source: &str, listing: Listing) -> ThumbDir {
    ThumbDir {
        path,
        source: source.to_owned(),
        listing,
    }
}

/// `download_dir/<id>` of each enabled online source, in config order.
fn online_dirs(cfg: &Config) -> impl Iterator<Item = ThumbDir> {
    cfg.sources
        .iter()
        .filter(|e| e.enabled && e.kind.is_online() && kakemono_config::valid_id(&e.id))
        .map(|e| {
            let path = cfg.daemon.download_dir.join(&e.id);
            thumb_dir(path, e.kind.as_str(), Listing::Flat)
        })
}

/// The location of each enabled folder, album and images source, in config order.
fn local_dirs(cfg: &Config) -> impl Iterator<Item = ThumbDir> {
    cfg.sources
        .iter()
        .filter(|e| e.enabled && !e.location.trim().is_empty())
        .filter_map(|e| {
            let listing = match e.kind {
                SourceType::Folder => Listing::Recursive,
                SourceType::AlbumName | SourceType::AlbumDate => Listing::Flat,
                SourceType::Images => Listing::File,
                _ => return None,
            };
            let path = PathBuf::from(e.location.trim());
            Some(thumb_dir(path, e.kind.as_str(), listing))
        })
}

fn favorites_dir(cfg: &Config) -> ThumbDir {
    thumb_dir(cfg.favorites.dir.clone(), "favorites", Listing::Flat)
}

/// `dirs` without empty paths and without a path listed earlier.
fn distinct(dirs: Vec<ThumbDir>) -> Vec<ThumbDir> {
    let mut seen = HashSet::new();
    dirs.into_iter()
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

    // ── current_thumbnail ──

    fn current(path: &Path) -> StateView {
        StateView {
            current: Some(info(path, "wallhaven")),
            history: None,
        }
    }

    #[test]
    fn current_thumbnail_admits_the_current_wallpaper() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, files) = favorites_with(root.path(), 1);
        let thumbs = Thumbs::new(root.path().join("thumbs"));

        let got = current_entry(&thumbs, &cfg, &current(&files[0])).expect("a current entry");

        assert_eq!(got.path, path_str(&files[0]));
        assert_eq!(
            serde_json::to_value(&got).unwrap()["isCurrent"],
            json!(true),
            "picker JSON shape"
        );
        assert!(!got.image_url.is_empty(), "inside the favorites root");
        assert!(thumbs.is_admitted(&files[0]));
    }

    #[test]
    fn current_thumbnail_is_none_without_a_current_wallpaper() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, _) = favorites_with(root.path(), 1);
        let thumbs = Thumbs::new(root.path().join("thumbs"));
        assert_eq!(current_entry(&thumbs, &cfg, &StateView::default()), None);
    }

    #[test]
    fn current_thumbnail_is_none_when_the_file_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, files) = favorites_with(root.path(), 1);
        std::fs::remove_file(&files[0]).unwrap();
        let thumbs = Thumbs::new(root.path().join("thumbs"));
        assert_eq!(current_entry(&thumbs, &cfg, &current(&files[0])), None);
    }

    #[test]
    fn current_thumbnail_leaves_the_picker_tabs_admitted() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, files) = favorites_with(root.path(), 3);
        let manual = cfg.daemon.manual_dir_resolved();
        std::fs::create_dir_all(&manual).unwrap();
        let elsewhere = manual.join("m.jpg");
        std::fs::write(&elsewhere, b"x").unwrap();
        let thumbs = Thumbs::new(root.path().join("thumbs"));
        let tab = tab_listing(
            &thumbs,
            &cfg,
            &StateView::default(),
            PickerTab::Favorites,
            0,
        );

        current_entry(&thumbs, &cfg, &current(&elsewhere)).expect("a current entry");

        for file in &files {
            assert!(thumbs.is_admitted(file), "{}", file.display());
        }
        for item in &tab.items {
            let id = item.image_url.rsplit('/').next().unwrap();
            assert_ne!(
                thumbs.respond(&format!("/{id}")).status(),
                403,
                "{}",
                item.path
            );
        }
    }

    #[test]
    fn current_thumbnail_replaces_only_the_previous_current() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, _) = favorites_with(root.path(), 0);
        let manual = cfg.daemon.manual_dir_resolved();
        std::fs::create_dir_all(&manual).unwrap();
        let (a, b) = (manual.join("a.jpg"), manual.join("b.jpg"));
        for file in [&a, &b] {
            std::fs::write(file, b"x").unwrap();
        }
        let thumbs = Thumbs::new(root.path().join("thumbs"));

        current_entry(&thumbs, &cfg, &current(&a)).expect("a");
        current_entry(&thumbs, &cfg, &current(&b)).expect("b");

        assert!(!thumbs.is_admitted(&a), "the old current is dropped");
        assert!(thumbs.is_admitted(&b));
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

    // ── Picker tabs ──

    fn dirs_of(cfg: &Config, tab: PickerTab) -> Vec<(PathBuf, String, Listing)> {
        tab_dirs(cfg, tab)
            .into_iter()
            .map(|d| (d.path, d.source, d.listing))
            .collect()
    }

    fn path_str(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    /// A config rooted at a temp dir with one row of each kind of source.
    fn tab_cfg(root: &Path) -> Config {
        let mut cfg = cfg_in(root);
        let pics = root.join("pics");
        cfg.sources = vec![
            row("wh-1", true, SourceType::Wallhaven, "forest"),
            row("dir-1", true, SourceType::Folder, &path_str(&pics)),
            row("off-1", false, SourceType::Unsplash, "x"),
            row(
                "img-1",
                true,
                SourceType::Images,
                &path_str(&pics.join("a.jpg")),
            ),
            row(
                "aln-1",
                true,
                SourceType::AlbumName,
                &path_str(&root.join("album")),
            ),
            row(
                "dir-off",
                false,
                SourceType::Folder,
                &path_str(&root.join("off")),
            ),
            row("bing", true, SourceType::Bing, ""),
        ];
        cfg
    }

    #[test]
    fn tab_dirs_downloaded_lists_enabled_online_folders_then_manual() {
        let root = tempfile::tempdir().unwrap();
        let cfg = tab_cfg(root.path());
        let dl = cfg.daemon.download_dir.clone();
        assert_eq!(
            dirs_of(&cfg, PickerTab::Downloaded),
            vec![
                (dl.join("wh-1"), "wallhaven".into(), Listing::Flat),
                (dl.join("bing"), "bing".into(), Listing::Flat),
                (dl.join("manual"), "manual".into(), Listing::Flat),
            ]
        );
    }

    #[test]
    fn tab_dirs_local_lists_enabled_folder_album_and_images_sources() {
        let root = tempfile::tempdir().unwrap();
        let cfg = tab_cfg(root.path());
        let pics = root.path().join("pics");
        assert_eq!(
            dirs_of(&cfg, PickerTab::Local),
            vec![
                (pics.clone(), "folder".into(), Listing::Recursive),
                (pics.join("a.jpg"), "images".into(), Listing::File),
                (
                    root.path().join("album"),
                    "album-name".into(),
                    Listing::Flat
                ),
            ]
        );
    }

    #[test]
    fn tab_dirs_favorites_lists_the_favorites_dir() {
        let root = tempfile::tempdir().unwrap();
        let cfg = tab_cfg(root.path());
        assert_eq!(
            dirs_of(&cfg, PickerTab::Favorites),
            vec![(cfg.favorites.dir.clone(), "favorites".into(), Listing::Flat)]
        );
    }

    #[test]
    fn tab_dirs_cache_lists_the_root_then_orphan_source_folders() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = tab_cfg(root.path());
        let dl = cfg.daemon.download_dir.clone();
        cfg.favorites.dir = dl.join("favs");
        cfg.daemon.trash_dir = dl.join("trash");
        for d in [
            "wh-1",      // enabled online: Downloaded
            "bing",      // enabled online: Downloaded
            "off-1",     // disabled online: orphan
            "wh-59f2",   // removed row: orphan
            "manual",    // manual_dir
            "favs",      // favorites inside download_dir
            "trash",     // trash inside download_dir
            ".hidden",   // dot-dir
            "Not_An_Id", // not a valid id
        ] {
            std::fs::create_dir_all(dl.join(d)).unwrap();
        }
        std::fs::write(dl.join(".kakemono-seen"), b"x").unwrap();
        std::fs::write(dl.join("loose-id"), b"a file, not a folder").unwrap();
        assert_eq!(
            dirs_of(&cfg, PickerTab::Cache),
            vec![
                (dl.clone(), "cache".into(), Listing::Flat),
                (dl.join("off-1"), "off-1".into(), Listing::Flat),
                (dl.join("wh-59f2"), "wh-59f2".into(), Listing::Flat),
            ]
        );
    }

    #[test]
    fn tab_cache_lists_no_seen_ledger_or_live_source_files() {
        let root = tempfile::tempdir().unwrap();
        let cfg = tab_cfg(root.path());
        let dl = cfg.daemon.download_dir.clone();
        for f in ["wh-1/live.jpg", "wh-59f2/old.jpg", "manual/m.jpg"] {
            std::fs::create_dir_all(dl.join(f).parent().unwrap()).unwrap();
            std::fs::write(dl.join(f), b"x").unwrap();
        }
        std::fs::write(dl.join("root.png"), b"x").unwrap();
        std::fs::write(dl.join(".kakemono-seen"), b"x").unwrap();
        std::fs::write(dl.join("wh-59f2/.kakemono-seen"), b"x").unwrap();
        let thumbs = Thumbs::new(root.path().join("thumbs"));
        let got = tab_listing(&thumbs, &cfg, &StateView::default(), PickerTab::Cache, 0);
        let mut titles: Vec<&str> = got.items.iter().map(|t| t.title.as_str()).collect();
        titles.sort_unstable();
        assert_eq!(titles, ["old", "root"]);
    }

    /// `n` images in the favorites dir; `i.jpg` is `i` seconds newer than `0.jpg`.
    fn favorites_with(root: &Path, n: u64) -> (Config, Vec<PathBuf>) {
        let mut cfg = cfg_in(root);
        cfg.sources.clear();
        let favs = cfg.favorites.dir.clone();
        std::fs::create_dir_all(&favs).unwrap();
        let base =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let files = (0..n)
            .map(|i| {
                let path = favs.join(format!("{i:03}.jpg"));
                let file = std::fs::File::create(&path).unwrap();
                file.set_modified(base + std::time::Duration::from_secs(i))
                    .unwrap();
                path
            })
            .collect();
        (cfg, files)
    }

    fn page(cfg: &Config, state: &StateView, offset: u32) -> TabListing {
        let thumbs = Thumbs::new(PathBuf::from("unused"));
        tab_listing(&thumbs, cfg, state, PickerTab::Favorites, offset)
    }

    #[test]
    fn tab_pages_hold_forty_without_overlap_and_flag_more() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, files) = favorites_with(root.path(), 100);
        let state = StateView::default();
        let pages: Vec<TabListing> = [0, 40, 80].map(|o| page(&cfg, &state, o)).into();
        let shape: Vec<(usize, bool)> = pages.iter().map(|p| (p.items.len(), p.has_more)).collect();
        assert_eq!(shape, [(40, true), (40, true), (20, false)]);
        let mut seen: Vec<String> = pages
            .iter()
            .flat_map(|p| p.items.iter().map(|t| t.path.clone()))
            .collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), files.len(), "every file once");
    }

    #[test]
    fn tab_page_past_the_end_is_empty() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, _files) = favorites_with(root.path(), 40);
        let full = page(&cfg, &StateView::default(), 0);
        assert_eq!((full.items.len(), full.has_more), (40, false));
        let past = page(&cfg, &StateView::default(), 40);
        assert_eq!((past.items.len(), past.has_more), (0, false));
    }

    #[test]
    fn tab_lists_newest_first_by_mtime() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, _files) = favorites_with(root.path(), 3);
        let got = page(&cfg, &StateView::default(), 0);
        let titles: Vec<&str> = got.items.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["002", "001", "000"]);
    }

    #[test]
    fn tab_pins_the_current_wallpaper_first_on_offset_zero() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, files) = favorites_with(root.path(), 50);
        let state = StateView {
            current: Some(info(&files[3], "favorites")),
            history: None,
        };
        let first = page(&cfg, &state, 0);
        assert_eq!(first.current_index, Some(0));
        let top: Vec<(&str, bool)> = first
            .items
            .iter()
            .take(3)
            .map(|t| (t.title.as_str(), t.is_current))
            .collect();
        assert_eq!(top, [("003", true), ("049", false), ("048", false)]);
    }

    #[test]
    fn tab_current_is_not_repeated_on_later_pages() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, files) = favorites_with(root.path(), 50);
        let state = StateView {
            current: Some(info(&files[3], "favorites")),
            history: None,
        };
        let second = page(&cfg, &state, 40);
        assert_eq!(second.current_index, None);
        let titles: Vec<&str> = second.items.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "010", "009", "008", "007", "006", "005", "004", "002", "001", "000"
            ],
            "page 0 held 003 and 049..=011"
        );
    }

    #[test]
    fn tab_without_the_current_wallpaper_pins_nothing() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, _files) = favorites_with(root.path(), 2);
        let elsewhere = root.path().join("elsewhere.jpg");
        std::fs::write(&elsewhere, b"x").unwrap();
        let state = StateView {
            current: Some(info(&elsewhere, "manual")),
            history: None,
        };
        let got = page(&cfg, &state, 0);
        assert_eq!(got.current_index, None);
        assert_eq!(got.items.len(), 2);
        assert!(got.items.iter().all(|t| !t.is_current));
    }

    #[test]
    fn tab_later_pages_keep_earlier_pages_admitted() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, _files) = favorites_with(root.path(), 50);
        let thumbs = Thumbs::new(root.path().join("thumbs"));
        let state = StateView::default();
        let first = tab_listing(&thumbs, &cfg, &state, PickerTab::Favorites, 0);
        let second = tab_listing(&thumbs, &cfg, &state, PickerTab::Favorites, 40);
        assert_eq!(first.items.len() + second.items.len(), 50);
        assert!(
            first
                .items
                .iter()
                .chain(&second.items)
                .all(|t| t.image_url.starts_with(&crate::thumbs::url_for(""))
                    && thumbs.is_admitted(Path::new(&t.path))),
            "every listed entry stays servable and pickable"
        );
    }
}
