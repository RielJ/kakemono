//! Engine favorite and trash against fakes (`RecordingSetter`, `ManualSignals`) and a local
//! folder source. Ports Go `TestManagerFavorite_CopiesFile`. Temp dirs only. Download-folder
//! trimming moved to the downloader (`trim_to_mb`, covered in `downloader.rs`); a change no
//! longer trims (see `pick_from_disk.rs`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::DateTime;
use kakemono_config::Config;
use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine};
use kakemono_platform::api::{Platform, Theme};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_protocol::types::WallpaperInfo;
use kakemono_sources::SourceSet;
use tempfile::TempDir;

const NOW: &str = "2026-03-04T05:06:07+08:00";

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    setter: RecordingSetter,
    engine: Engine,
}

impl Fixture {
    fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    fn favorites(&self) -> PathBuf {
        self.root.join("favorites")
    }

    fn trash(&self) -> PathBuf {
        self.root.join("trash")
    }
}

fn info(path: &Path) -> WallpaperInfo {
    WallpaperInfo {
        path: path.to_str().unwrap().to_owned(),
        source: "local".to_owned(),
        ..WallpaperInfo::default()
    }
}

/// An engine rooted in a temp dir, internet off. With `pics`, a folder source at that dir
/// is the pool `next` picks from. `prepare` gets the root before the engine exists and
/// returns the initial state (so tests can pre-populate files and history).
fn fixture(pics: Option<&Path>, prepare: impl FnOnce(&Path) -> State) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut cfg = Config::default();
    cfg.daemon.download_dir = root.join("downloads");
    cfg.daemon.trash_dir = root.join("trash");
    cfg.favorites.dir = root.join("favorites");
    cfg.daemon.internet = false;
    cfg.sources = pics
        .map(|p| SourceEntry {
            id: "pics".to_owned(),
            enabled: true,
            kind: SourceType::Folder,
            location: p.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .into_iter()
        .collect();
    fs::create_dir_all(root.join("downloads")).unwrap();
    let state = prepare(&root);

    let setter = RecordingSetter::new();
    let platform = Platform {
        setter: Arc::new(setter.clone()),
        signals: Arc::new(ManualSignals::new(Theme::Light)),
    };
    let now = DateTime::parse_from_rfc3339(NOW).unwrap();
    let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
        .with_sources(SourceSet::default())
        .with_clock(move || now);
    let engine = Engine::new(cfg, state, root.join("state.json"), deps).unwrap();
    Fixture {
        _dir: dir,
        root,
        setter,
        engine,
    }
}

/// A state whose current wallpaper is `path`.
fn state_with(paths: &[&Path]) -> State {
    let mut state = State::new();
    for p in paths {
        state.push(info(p), DateTime::parse_from_rfc3339(NOW).unwrap());
    }
    state
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn is_collision_name(name: &str, stem: &str, ext: &str) -> bool {
    name.strip_prefix(&format!("{stem}_"))
        .and_then(|rest| rest.strip_suffix(&format!(".{ext}")))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

// ─── favorite ─────────────────────────────────────────────────────────────────────────────

/// Go `TestManagerFavorite_CopiesFile`.
#[tokio::test]
async fn favorite_copies_the_current_file() {
    let f = fixture(None, |root| {
        let src = root.join("wall.jpg");
        fs::write(&src, b"fake image data").unwrap();
        state_with(&[&src])
    });
    f.engine.favorite().await.unwrap();

    let copied = f.favorites().join("wall.jpg");
    assert_eq!(fs::read(&copied).unwrap(), b"fake image data");
    assert!(
        f.root.join("wall.jpg").exists(),
        "favorite must copy, not move"
    );
}

#[tokio::test]
async fn favorite_twice_adds_a_collision_suffix_and_keeps_the_first_copy() {
    let f = fixture(None, |root| {
        let src = root.join("wall.jpg");
        fs::write(&src, b"v1").unwrap();
        state_with(&[&src])
    });
    f.engine.favorite().await.unwrap();
    fs::write(f.root.join("wall.jpg"), b"v2").unwrap();
    f.engine.favorite().await.unwrap();

    let files = names(&f.favorites());
    assert_eq!(files.len(), 2, "{files:?}");
    assert_eq!(fs::read(f.favorites().join("wall.jpg")).unwrap(), b"v1");
    let second = files.iter().find(|n| *n != "wall.jpg").unwrap();
    assert!(is_collision_name(second, "wall", "jpg"), "{second}");
    assert_eq!(fs::read(f.favorites().join(second)).unwrap(), b"v2");
}

#[tokio::test]
async fn favorite_creates_the_favorites_dir() {
    let f = fixture(None, |root| {
        let src = root.join("wall.png");
        fs::write(&src, b"x").unwrap();
        state_with(&[&src])
    });
    assert!(f.favorites().is_dir());
    fs::remove_dir(f.favorites()).unwrap();
    f.engine.favorite().await.unwrap();
    assert!(f.favorites().join("wall.png").is_file());
}

#[tokio::test]
async fn favorite_of_a_missing_file_fails_with_go_message() {
    let f = fixture(None, |root| state_with(&[&root.join("gone.jpg")]));
    let err = f.engine.favorite().await.unwrap_err().to_string();
    let src = f.root.join("gone.jpg");
    assert_eq!(
        err,
        format!(
            "manager: copy to favorites: open {}: no such file or directory",
            src.display()
        )
    );
    assert!(
        names(&f.favorites()).is_empty(),
        "no empty file left behind"
    );
}

#[tokio::test]
async fn favorite_reports_a_favorites_dir_it_cannot_create() {
    let f = fixture(None, |root| {
        let src = root.join("wall.jpg");
        fs::write(&src, b"x").unwrap();
        state_with(&[&src])
    });
    fs::remove_dir(f.favorites()).unwrap();
    fs::write(f.favorites(), b"a file, not a dir").unwrap();
    let err = f.engine.favorite().await.unwrap_err().to_string();
    assert!(
        err.starts_with("manager: create favorites dir: mkdir "),
        "{err}"
    );
}

// ─── trash ────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn trash_moves_the_current_file_then_sets_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let next_image = dir.path().join("next.png");
    fs::write(&next_image, b"next").unwrap();
    let f = fixture(Some(dir.path()), |root| {
        let src = root.join("downloads").join("bad.jpg");
        fs::write(&src, b"bad").unwrap();
        state_with(&[&src])
    });

    let info = f.engine.trash().await.unwrap();

    assert!(!f.downloads().join("bad.jpg").exists(), "source must move");
    assert_eq!(fs::read(f.trash().join("bad.jpg")).unwrap(), b"bad");
    assert_eq!(Path::new(&info.path), next_image);
    assert_eq!(f.setter.last().unwrap().0, next_image);
    let status = f.engine.status();
    assert_eq!(status.current.as_ref(), Some(&info));
    assert_eq!(status.history_count, 2);
}

#[tokio::test]
async fn trash_keeps_an_older_file_with_the_same_name() {
    let dir = tempfile::tempdir().unwrap();
    let next_image = dir.path().join("next.png");
    fs::write(&next_image, b"next").unwrap();
    let f = fixture(Some(dir.path()), |root| {
        let src = root.join("downloads").join("bad.jpg");
        fs::write(&src, b"new").unwrap();
        fs::create_dir_all(root.join("trash")).unwrap();
        fs::write(root.join("trash").join("bad.jpg"), b"old").unwrap();
        state_with(&[&src])
    });

    f.engine.trash().await.unwrap();

    let files = names(&f.trash());
    assert_eq!(files.len(), 2, "{files:?}");
    assert_eq!(fs::read(f.trash().join("bad.jpg")).unwrap(), b"old");
    let moved = files.iter().find(|n| *n != "bad.jpg").unwrap();
    assert!(is_collision_name(moved, "bad", "jpg"), "{moved}");
    assert_eq!(fs::read(f.trash().join(moved)).unwrap(), b"new");
}

#[tokio::test]
async fn trash_expected_a_stale_path_fails_and_moves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let next_image = dir.path().join("next.png");
    fs::write(&next_image, b"next").unwrap();
    let f = fixture(Some(dir.path()), |root| {
        let src = root.join("downloads").join("bad.jpg");
        fs::write(&src, b"bad").unwrap();
        state_with(&[&src])
    });
    let current = f.downloads().join("bad.jpg");

    let err = f
        .engine
        .trash_expected(Some("/elsewhere/old.jpg"))
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "manager: current wallpaper changed: expected /elsewhere/old.jpg, current {}",
            current.display()
        )
    );
    assert!(current.exists(), "nothing may move");
    assert!(names(&f.trash()).is_empty());
    assert!(f.setter.calls().is_empty(), "no next after a refused trash");
    assert_eq!(f.engine.status().history_count, 1);
}

#[tokio::test]
async fn trash_expected_the_current_path_trashes_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let next_image = dir.path().join("next.png");
    fs::write(&next_image, b"next").unwrap();
    let f = fixture(Some(dir.path()), |root| {
        let src = root.join("downloads").join("bad.jpg");
        fs::write(&src, b"bad").unwrap();
        state_with(&[&src])
    });
    let current = f.downloads().join("bad.jpg");

    let info = f
        .engine
        .trash_expected(Some(current.to_str().unwrap()))
        .await
        .unwrap();

    assert!(!current.exists(), "source must move");
    assert_eq!(fs::read(f.trash().join("bad.jpg")).unwrap(), b"bad");
    assert_eq!(Path::new(&info.path), next_image);
}

#[tokio::test]
async fn trash_of_a_missing_file_fails_and_does_not_rotate() {
    let dir = tempfile::tempdir().unwrap();
    let next_image = dir.path().join("next.png");
    fs::write(&next_image, b"next").unwrap();
    let f = fixture(Some(dir.path()), |root| {
        state_with(&[&root.join("gone.jpg")])
    });

    let err = f.engine.trash().await.unwrap_err().to_string();

    let src = f.root.join("gone.jpg");
    let dst = f.trash().join("gone.jpg");
    assert_eq!(
        err,
        format!(
            "manager: move to trash: rename {} {}: no such file or directory \
                 (copy fallback: open {}: no such file or directory)",
            src.display(),
            dst.display(),
            src.display()
        )
    );
    assert!(f.setter.calls().is_empty(), "no next after a failed trash");
    assert_eq!(f.engine.status().history_count, 1);
    assert!(names(&f.trash()).is_empty());
}
