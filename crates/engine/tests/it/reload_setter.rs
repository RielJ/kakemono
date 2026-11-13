//! Task 37b: `Engine::reload` rebuilds the wallpaper setter when `[desktop]` changed, under the
//! op lock, and keeps the old one when the new `[desktop]` cannot be used. Fakes only:
//! `ArgvSetter` records the argv it would run; temp dirs only.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use kakemono_config::Config;
use kakemono_config::schema::{CommandSpec, DesktopConfig};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine, EngineError};
use kakemono_platform::api::{BackendInfo, Fit, Platform, SetError, Theme, WallpaperSetter};
use kakemono_platform::fake::{ArgvSetter, ManualSignals};
use kakemono_sources::SourceSet;
use tempfile::TempDir;
use tokio::sync::Notify;

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    engine: Engine,
}

impl Fixture {
    fn image(&self, name: &str) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, b"fake image data").unwrap();
        p
    }

    fn path(&self, name: &str) -> String {
        self.root.join(name).to_str().unwrap().to_owned()
    }
}

fn command(argv: &[&str]) -> Option<CommandSpec> {
    Some(CommandSpec::new(argv.iter().map(|s| (*s).to_owned()).collect()).unwrap())
}

fn config(root: &Path, argv: &[&str]) -> Config {
    let mut cfg = Config::default();
    cfg.daemon.download_dir = root.join("downloads");
    cfg.daemon.trash_dir = root.join("trash");
    cfg.favorites.dir = root.join("favorites");
    cfg.desktop.set_command = command(argv);
    cfg
}

fn engine(root: &Path, cfg: Config, setter: Arc<dyn WallpaperSetter>) -> Engine {
    let platform = Platform {
        setter,
        signals: Arc::new(ManualSignals::new(Theme::Light)),
    };
    let deps = Deps::new(
        platform,
        kakemono_sources::http::client_builder().build().unwrap(),
    )
    .with_sources(SourceSet::default());
    Engine::new(cfg, State::new(), root.join("state.json"), deps).unwrap()
}

fn fixture(argv: &[&str]) -> (Fixture, ArgvSetter) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cfg = config(&root, argv);
    let setter = ArgvSetter::from_desktop(&cfg.desktop).unwrap();
    let engine = engine(&root, cfg, Arc::new(setter.clone()));
    (
        Fixture {
            _dir: dir,
            root,
            engine,
        },
        setter,
    )
}

#[tokio::test]
async fn a_changed_set_command_is_used_by_the_next_apply() {
    let (f, log) = fixture(&["old-setter", "{path}"]);
    f.engine.set_file(&f.image("a.jpg")).await.unwrap();

    let mut cfg = (*f.engine.config()).clone();
    cfg.desktop.set_command = command(&["new-setter", "--fit", "{fit}", "{path}"]);
    f.engine.reload(cfg).await.unwrap();
    assert_eq!(log.rebuilds(), 1);
    assert_eq!(
        f.engine.platform().setter.info().detail,
        "new-setter --fit {fit} {path}",
        "doctor reports the new backend"
    );

    // `reapply` is what `doctor --apply` runs; rotation goes through the same setter.
    f.engine
        .reapply()
        .await
        .unwrap()
        .expect("a current wallpaper");
    f.engine.set_file(&f.image("b.jpg")).await.unwrap();
    assert_eq!(
        log.runs(),
        [
            vec!["old-setter".to_owned(), f.path("a.jpg")],
            vec![
                "new-setter".to_owned(),
                "--fit".to_owned(),
                "fill".to_owned(),
                f.path("a.jpg")
            ],
            vec![
                "new-setter".to_owned(),
                "--fit".to_owned(),
                "fill".to_owned(),
                f.path("b.jpg")
            ],
        ]
    );
}

#[tokio::test]
async fn an_unchanged_desktop_rebuilds_nothing() {
    let (f, log) = fixture(&["setter", "{path}"]);
    let before = f.engine.setter();

    // Other sections change, `[desktop]` does not (a fresh `detected_at` alone is no change).
    let mut cfg = (*f.engine.config()).clone();
    cfg.daemon.interval = "10m".to_owned();
    cfg.desktop.detected_at = Some(chrono::Utc::now());
    f.engine.reload(cfg).await.unwrap();
    f.engine.reload((*f.engine.config()).clone()).await.unwrap();

    assert_eq!(log.rebuilds(), 0);
    assert!(Arc::ptr_eq(&before, &f.engine.setter()), "same setter");
    assert_eq!(f.engine.config().daemon.interval, "10m");
}

#[tokio::test]
async fn a_desktop_that_cannot_be_used_keeps_the_old_setter_and_config() {
    let (f, log) = fixture(&["old-setter", "{path}"]);
    let before = f.engine.setter();
    let old_interval = f.engine.config().daemon.interval.clone();

    let mut cfg = (*f.engine.config()).clone();
    cfg.daemon.interval = "10m".to_owned();
    cfg.desktop.set_command = None;
    let err = f.engine.reload(cfg).await.unwrap_err();
    assert!(matches!(err, EngineError::Desktop(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "[desktop]: `[desktop] set_command` is empty"
    );

    assert!(Arc::ptr_eq(&before, &f.engine.setter()));
    let cfg = f.engine.config();
    assert_eq!(
        cfg.daemon.interval, old_interval,
        "all or nothing: the old config stays"
    );
    assert!(cfg.desktop.set_command.is_some());
    f.engine.set_file(&f.image("a.jpg")).await.unwrap();
    assert_eq!(log.runs(), [vec!["old-setter".to_owned(), f.path("a.jpg")]]);
    assert_eq!(log.rebuilds(), 0);
}

/// Blocks in `set` until released; `rebuild` returns a fresh `Gated`.
struct Gated {
    name: &'static str,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    log: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl WallpaperSetter for Gated {
    async fn set(&self, _image: &Path, _fit: Fit) -> Result<(), SetError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("{} set start", self.name));
        self.entered.notify_one();
        self.release.notified().await;
        self.log
            .lock()
            .unwrap()
            .push(format!("{} set end", self.name));
        Ok(())
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: self.name.to_owned(),
            detail: String::new(),
        }
    }

    async fn rebuild(
        &self,
        _desktop: &DesktopConfig,
    ) -> Result<Option<Arc<dyn WallpaperSetter>>, SetError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("{} rebuild", self.name));
        Ok(Some(Arc::new(Self {
            name: "new",
            entered: Arc::clone(&self.entered),
            release: Arc::clone(&self.release),
            log: Arc::clone(&self.log),
        })))
    }
}

#[tokio::test]
async fn reload_waits_for_an_apply_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let setter = Arc::new(Gated {
        name: "old",
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
        log: Arc::clone(&log),
    });
    let engine = engine(&root, config(&root, &["x", "{path}"]), setter);
    let image = root.join("a.jpg");
    std::fs::write(&image, b"x").unwrap();

    let applying = tokio::spawn({
        let engine = engine.clone();
        async move { engine.set_file(&image).await }
    });
    entered.notified().await;

    let mut cfg = (*engine.config()).clone();
    cfg.desktop.set_command = command(&["y", "{path}"]);
    let reloading = tokio::spawn({
        let engine = engine.clone();
        async move { engine.reload(cfg).await }
    });
    crate::common::settle().await;
    assert!(!reloading.is_finished(), "reload waits for the op lock");
    assert_eq!(engine.platform().setter.info().name, "old");

    release.notify_one();
    applying.await.unwrap().unwrap();
    reloading.await.unwrap().unwrap();
    assert_eq!(engine.platform().setter.info().name, "new");
    assert_eq!(
        *log.lock().unwrap(),
        ["old set start", "old set end", "old rebuild"],
        "the apply in flight finished on the old setter before the swap"
    );
}
