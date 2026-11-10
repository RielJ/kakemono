//! Wallpaper changes pick from disk (`docs/plans/variety-sources-impl.md` Task 14): `next` over
//! the local pool, the on-demand download for an empty pool, `NoImages`, `change_on_start`, the
//! downloader wake on `reload`, the filter context and what deletes must spare. Fakes only
//! (`RecordingSetter`, `ManualSignals`, `StaticProvider`) plus a local wiremock server.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use image::{Rgb, RgbImage};
use kakemono_config::Config;
use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_engine::downloader::DownloadEnv;
use kakemono_engine::files::safe_delete;
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine, EngineError, EngineEvent};
use kakemono_platform::api::{Fit, Platform, Theme};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_protocol::types::WallpaperInfo;
use kakemono_sources::fake::StaticProvider;
use kakemono_sources::{Provider, SourceSet};
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NOW: &str = "2026-03-04T05:06:07.890+08:00";
const MIN: Duration = Duration::from_secs(60);
const NO_IMAGES: &str = "No images yet. Add a source or turn on Internet access.";

/// Online rows and the fake providers behind them, by row id.
type Fakes = Vec<(&'static str, Arc<dyn Provider>)>;

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    setter: RecordingSetter,
    signals: ManualSignals,
    engine: Engine,
}

impl Fixture {
    fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }
}

fn row(id: &str, kind: SourceType, location: &str) -> SourceEntry {
    SourceEntry {
        id: id.to_owned(),
        enabled: true,
        kind,
        location: location.to_owned(),
    }
}

/// A config rooted in `root` with no source and no Internet access.
fn config(root: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.daemon.download_dir = root.join("downloads");
    cfg.daemon.trash_dir = root.join("trash");
    cfg.favorites.dir = root.join("favorites");
    cfg.daemon.interval = "30m".to_owned();
    cfg.daemon.internet = false;
    cfg.daemon.download_limit_mb_enabled = false;
    cfg.desktop.fit = Fit::Center;
    cfg.sources = Vec::new();
    cfg
}

/// The `SourceSet` for `cfg`: one fake per enabled row whose id is in `fakes`.
fn fake_set(cfg: &Config, fakes: &Fakes) -> SourceSet {
    SourceSet::fake(
        cfg.sources
            .iter()
            .filter(|e| e.enabled)
            .filter_map(|e| {
                fakes
                    .iter()
                    .find(|(id, _)| *id == e.id)
                    .map(|(id, p)| (*id, Arc::clone(p), ""))
            })
            .collect(),
    )
}

/// An engine over `cfg` (edited by `edit`, which gets the temp root) whose online sources
/// come from `fakes`.
fn fixture(fakes: Fakes, edit: impl FnOnce(&mut Config, &Path)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut cfg = config(&root);
    edit(&mut cfg, &root);
    let setter = RecordingSetter::new();
    let signals = ManualSignals::new(Theme::Light);
    let platform = Platform {
        setter: Arc::new(setter.clone()),
        signals: Arc::new(signals.clone()),
    };
    let now: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(NOW).unwrap();
    let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
        .with_sources_factory(move |cfg, _| fake_set(cfg, &fakes))
        .with_clock(move || now);
    let engine = Engine::new(cfg, State::new(), root.join("state.json"), deps).unwrap();
    Fixture {
        _dir: dir,
        root,
        setter,
        signals,
        engine,
    }
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    path.to_path_buf()
}

/// A real PNG of `w`×`h` pixels (the filters read its facts).
fn png(path: &Path, w: u32, h: u32) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    RgbImage::from_pixel(w, h, Rgb([90, 120, 160]))
        .save(path)
        .unwrap();
    path.to_path_buf()
}

/// A 12 KiB JPEG-typed body at `/img/<name>` (downloads want ≥ 10 KiB and an image type).
async fn image_server(name: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/img/{name}")))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(vec![0xAB; 12 * 1024]),
        )
        .mount(&server)
        .await;
    server
}

fn bing_at(url: &str) -> Arc<StaticProvider> {
    Arc::new(StaticProvider::new(
        "bing",
        vec![StaticProvider::result("bing", url)],
    ))
}

// ─── next picks from disk ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn next_with_images_on_disk_sets_one_without_any_http_request() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let bing = bing_at(&format!("{}/img/new.jpg", server.uri()));
    let fakes: Fakes = vec![("bing", bing.clone())];
    let f = fixture(fakes, |c, _| {
        c.daemon.internet = true;
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });
    let on_disk = write(&f.downloads().join("bing/a.jpg"), b"fake image data");
    let mut events = f.engine.subscribe();

    let info = f.engine.next().await.unwrap();

    assert_eq!(Path::new(&info.path), on_disk);
    assert_eq!(info.source, "bing");
    assert_eq!(info.source_id, "bing", "the row id");
    assert_eq!(info.title, "a.jpg");
    assert_eq!(info.set_at, "2026-03-04T05:06:07+08:00");
    assert_eq!(f.setter.calls(), vec![(on_disk, Fit::Center)]);
    assert_eq!(bing.calls(), 0, "no fetch while the pool has images");
    assert_eq!(f.engine.status().current.as_ref(), Some(&info));
    assert_eq!(events.try_recv().unwrap(), EngineEvent::Changed(info));
    server.verify().await;
}

#[tokio::test]
async fn next_uses_the_row_type_as_source_for_local_rows_and_manual() {
    let f = fixture(Vec::new(), |c, root| {
        c.sources = vec![row(
            "pics",
            SourceType::Folder,
            root.join("pics").to_str().unwrap(),
        )];
    });
    let pic = write(&f.root.join("pics/sub/p.png"), b"p");
    let info = f.engine.next().await.unwrap();
    assert_eq!(
        (Path::new(&info.path), info.source.as_str()),
        (&*pic, "folder")
    );
    assert_eq!(info.source_id, "pics");

    // Only the manual folder has images now.
    std::fs::remove_file(&pic).unwrap();
    let manual = write(&f.downloads().join("manual/m.png"), b"m");
    let info = f.engine.next().await.unwrap();
    assert_eq!(
        (Path::new(&info.path), info.source.as_str()),
        (&*manual, "manual")
    );
    assert_eq!(info.source_id, "manual");
    assert_eq!(
        f.engine.state().history().last(),
        Some(&info),
        "stored in history"
    );
}

#[tokio::test]
async fn next_does_not_trim_the_download_folder() {
    let f = fixture(Vec::new(), |c, _| {
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        write(&f.downloads().join("bing").join(name), b"x");
    }
    f.engine.next().await.unwrap();
    assert_eq!(
        std::fs::read_dir(f.downloads().join("bing"))
            .unwrap()
            .count(),
        3
    );
}

// ─── empty pool ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn next_with_an_empty_pool_and_internet_downloads_on_demand() {
    let server = image_server("photo.jpg").await;
    let url = format!("{}/img/photo.jpg", server.uri());
    let bing = bing_at(&url);
    let fakes: Fakes = vec![("bing", bing.clone())];
    let f = fixture(fakes, |c, _| {
        c.daemon.internet = true;
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });

    let info = f.engine.next().await.unwrap();

    let expected = f.downloads().join("bing/photo.jpg");
    assert_eq!(Path::new(&info.path), expected);
    assert_eq!(std::fs::read(&expected).unwrap().len(), 12 * 1024);
    assert_eq!(info.source, "bing");
    assert_eq!(info.title, "photo.jpg");
    assert_eq!(f.setter.calls(), vec![(expected.clone(), Fit::Center)]);
    assert_eq!(bing.calls(), 1);
    assert_eq!(f.engine.status().history_count, 1);

    // The pool has an image now: the next change makes no request.
    f.engine.next().await.unwrap();
    assert_eq!(bing.calls(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn on_demand_download_falls_back_to_the_next_source() {
    let server = image_server("w.jpg").await;
    let bad = Arc::new(StaticProvider::failing("bing", "bing: boom"));
    let good = Arc::new(StaticProvider::new(
        "wallhaven",
        vec![StaticProvider::result(
            "wallhaven",
            &format!("{}/img/w.jpg", server.uri()),
        )],
    ));
    let fakes: Fakes = vec![("bing", bad.clone()), ("wh-1", good.clone())];
    let f = fixture(fakes, |c, _| {
        c.daemon.internet = true;
        c.sources = vec![
            row("bing", SourceType::Bing, ""),
            row("wh-1", SourceType::Wallhaven, "forest"),
        ];
    });

    let info = f.engine.next().await.unwrap();

    assert_eq!(info.source, "wallhaven");
    assert_eq!(info.source_id, "wh-1", "the row id, not the type");
    assert_eq!(Path::new(&info.path), f.downloads().join("wh-1/w.jpg"));
    assert_eq!((bad.calls(), good.calls()), (1, 1));
}

#[tokio::test]
async fn on_demand_download_reports_every_failure_in_the_fetch_format() {
    let fakes: Fakes = vec![
        ("bing", Arc::new(StaticProvider::new("bing", vec![]))),
        (
            "apod",
            Arc::new(StaticProvider::failing("apod", "apod: boom")),
        ),
    ];
    let f = fixture(fakes, |c, _| {
        c.daemon.internet = true;
        c.sources = vec![
            row("bing", SourceType::Bing, ""),
            row("apod", SourceType::Apod, ""),
        ];
    });
    let err = f.engine.next().await.unwrap_err();
    assert!(matches!(err, EngineError::Fetch(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "manager: provider bing returned no results for query \"\"; \
             provider apod fetch: apod: boom"
    );
    assert!(f.setter.calls().is_empty());
    assert_eq!(f.engine.status().history_index, -1);
}

#[tokio::test]
async fn next_with_internet_off_and_an_empty_pool_returns_no_images() {
    let bing = Arc::new(StaticProvider::new("bing", vec![]));
    let fakes: Fakes = vec![("bing", bing.clone())];
    let f = fixture(fakes, |c, _| {
        c.daemon.internet = false;
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });
    let err = f.engine.next().await.unwrap_err();
    assert!(matches!(err, EngineError::NoImages), "{err:?}");
    assert_eq!(err.to_string(), NO_IMAGES);
    assert_eq!(bing.calls(), 0);
    assert!(f.setter.calls().is_empty());
}

#[tokio::test]
async fn next_without_an_online_source_and_an_empty_pool_returns_no_images() {
    let f = fixture(Vec::new(), |c, root| {
        c.daemon.internet = true;
        c.sources = vec![row(
            "pics",
            SourceType::Folder,
            root.join("empty").to_str().unwrap(),
        )];
    });
    let err = f.engine.next().await.unwrap_err();
    assert!(matches!(err, EngineError::NoImages), "{err:?}");
}

// ─── download_one ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn download_one_by_id_fills_that_source_folder() {
    let server = image_server("d.jpg").await;
    let fakes: Fakes = vec![("bing", bing_at(&format!("{}/img/d.jpg", server.uri())))];
    let f = fixture(fakes, |c, _| {
        c.daemon.internet = true;
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });
    let path = f.engine.download_one("bing").await.unwrap();
    assert_eq!(path, f.downloads().join("bing/d.jpg"));
    assert!(f.setter.calls().is_empty(), "download_one sets nothing");

    let err = f.engine.download_one("nope").await.unwrap_err();
    assert_eq!(err, "no enabled online source \"nope\"");
}

#[tokio::test]
async fn download_one_refuses_while_internet_is_off() {
    let bing = Arc::new(StaticProvider::new("bing", vec![]));
    let fakes: Fakes = vec![("bing", bing.clone())];
    let f = fixture(fakes, |c, _| {
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });
    let err = f.engine.download_one("bing").await.unwrap_err();
    assert_eq!(err, "Internet access is off");
    assert_eq!(bing.calls(), 0);
}

// ─── filters ──────────────────────────────────────────────────────────────────────────────

/// One `next` on a fresh engine over the folder `pics` with a 100 % minimum-size filter.
async fn pick_min_size(pics: &Path, screen: Option<(u32, u32)>) -> PathBuf {
    let pics = pics.to_str().unwrap().to_owned();
    let f = fixture(Vec::new(), move |c, _| {
        c.sources = vec![row("pics", SourceType::Folder, &pics)];
        c.filters.min_size_enabled = true;
        c.filters.min_size_percent = 100;
    });
    f.signals.set_screen_size(screen);
    PathBuf::from(f.engine.next().await.unwrap().path)
}

#[tokio::test]
async fn filter_context_uses_the_screen_size_and_falls_back_to_1080p() {
    let f = fixture(Vec::new(), |_, _| {});
    let ctx = f.engine.filter_ctx();
    assert_eq!((ctx.screen, ctx.theme), ((1920, 1080), Theme::Light));
    f.signals.set_screen_size(Some((5120, 2880)));
    f.signals.set_theme(Theme::Dark);
    let ctx = f.engine.filter_ctx();
    assert_eq!((ctx.screen, ctx.theme), ((5120, 2880), Theme::Dark));

    // And `next` judges the images against it.
    let dir = tempfile::tempdir().unwrap();
    let pics = dir.path().join("pics");
    let big = png(&pics.join("big.png"), 2000, 1200);
    png(&pics.join("small.png"), 1000, 600);
    for _ in 0..10 {
        // No screen size: 1920×1080, which only `big` covers.
        assert_eq!(pick_min_size(&pics, None).await, big);
    }
    let huge = png(&pics.join("huge.png"), 2600, 1500);
    for _ in 0..10 {
        // 2560×1440: only `huge` covers it (`big` would pass at 1080p).
        assert_eq!(pick_min_size(&pics, Some((2560, 1440))).await, huge);
    }
}

#[tokio::test]
async fn reload_recompiles_the_filters() {
    let dir = tempfile::tempdir().unwrap();
    let pics = dir.path().join("pics");
    let wide = png(&pics.join("wide.png"), 64, 32);
    for i in 0..3 {
        png(&pics.join(format!("tall-{i}.png")), 32, 64);
    }
    let location = pics.to_str().unwrap().to_owned();
    for _ in 0..10 {
        // A fresh engine each time: a second `next` would avoid the current image.
        let f = fixture(Vec::new(), |c, _| {
            c.sources = vec![row("pics", SourceType::Folder, &location)];
        });
        let mut cfg = (*f.engine.config()).clone();
        cfg.filters.landscape = true;
        f.engine.reload(cfg).await.unwrap();
        assert_eq!(PathBuf::from(f.engine.next().await.unwrap().path), wide);
    }
}

// ─── protect ──────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn protect_spares_manual_favorites_trash_local_rows_current_and_history() {
    let f = fixture(Vec::new(), |c, root| {
        let d = root.join("downloads");
        c.favorites.dir = d.join("favs");
        c.daemon.trash_dir = d.join("bin");
        let mut off = row("mine", SourceType::Folder, d.join("mine").to_str().unwrap());
        off.enabled = false;
        c.sources = vec![
            off,
            row(
                "one",
                SourceType::Images,
                d.join("one.jpg").to_str().unwrap(),
            ),
            row("bing", SourceType::Bing, ""),
        ];
    });
    let d = f.downloads();
    let old = write(&d.join("bing/old.jpg"), b"o");
    let cur = write(&d.join("bing/cur.jpg"), b"c");
    f.engine.set_file(&old).await.unwrap();
    f.engine.set_file(&cur).await.unwrap();
    let protect = f.engine.protect();

    for kept in [
        write(&d.join("manual/m.jpg"), b"m"),
        write(&d.join("favs/f.jpg"), b"f"),
        write(&d.join("bin/t.jpg"), b"t"),
        write(&d.join("mine/x.jpg"), b"x"),
        write(&d.join("one.jpg"), b"1"),
        cur,
        old,
    ] {
        assert!(
            safe_delete(&kept, &d, &protect).is_err(),
            "deleted {kept:?}"
        );
        assert!(kept.exists(), "{kept:?}");
    }
    let free = write(&d.join("bing/free.jpg"), b"x");
    safe_delete(&free, &d, &protect).unwrap();
    assert!(!free.exists());
}

// ─── run: change_on_start, downloader, facts ──────────────────────────────────────────────

struct Running {
    shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

impl Running {
    async fn stop(self) {
        self.shutdown.cancel();
        timeout(Duration::from_secs(5), self.handle)
            .await
            .expect("run did not stop on shutdown")
            .unwrap();
    }
}

async fn run(engine: &Engine) -> Running {
    let shutdown = CancellationToken::new();
    let (engine, token) = (engine.clone(), shutdown.clone());
    let handle = tokio::spawn(async move { engine.run(token).await });
    tokio::task::yield_now().await;
    Running { shutdown, handle }
}

/// The next `Changed` event within `within` (other events are skipped), or `None`.
async fn changed_within(
    rx: &mut broadcast::Receiver<EngineEvent>,
    within: Duration,
) -> Option<WallpaperInfo> {
    timeout(within, async {
        loop {
            if let EngineEvent::Changed(info) = rx.recv().await.unwrap() {
                return info;
            }
        }
    })
    .await
    .ok()
}

fn with_pics(c: &mut Config, root: &Path) {
    let pics = root.join("pics");
    for name in ["a.png", "b.png"] {
        write(&pics.join(name), b"image");
    }
    c.sources = vec![row("pics", SourceType::Folder, pics.to_str().unwrap())];
}

#[tokio::test(start_paused = true)]
async fn change_on_start_rotates_exactly_once_when_run_starts() {
    let f = fixture(Vec::new(), |c, root| {
        with_pics(c, root);
        c.daemon.change_on_start = true;
    });
    let mut rx = f.engine.subscribe();
    let running = run(&f.engine).await;

    let info = changed_within(&mut rx, Duration::from_secs(1))
        .await
        .expect("a change at start");
    assert_eq!(info.source, "folder");
    // The interval restarted at the start change: nothing more until 30m.
    assert_eq!(
        changed_within(&mut rx, 30 * MIN - Duration::from_secs(1)).await,
        None
    );
    assert_eq!(f.engine.status().history_count, 1);
    assert_eq!(f.setter.calls().len(), 1);
    changed_within(&mut rx, 2 * MIN).await.expect("tick at 30m");
    running.stop().await;
}

#[tokio::test(start_paused = true)]
async fn change_on_start_off_or_paused_does_not_rotate_at_start() {
    for paused in [false, true] {
        let f = fixture(Vec::new(), |c, root| {
            with_pics(c, root);
            c.daemon.change_on_start = paused;
        });
        if paused {
            f.engine.pause();
        }
        let mut rx = f.engine.subscribe();
        let running = run(&f.engine).await;
        assert_eq!(
            changed_within(&mut rx, 29 * MIN).await,
            None,
            "paused {paused}"
        );
        assert!(f.setter.calls().is_empty());
        running.stop().await;
    }
}

/// Polls for an image in `dir` for up to 5 s (real time).
async fn wait_for_image(dir: &Path) -> bool {
    for _ in 0..250 {
        if kakemono_sources::local::list_images(dir, false).len() == 1 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn reload_with_a_new_source_wakes_the_downloader() {
    let server = image_server("bg.jpg").await;
    let bing = bing_at(&format!("{}/img/bg.jpg", server.uri()));
    let fakes: Fakes = vec![("bing", bing.clone())];
    let f = fixture(fakes, |c, _| {
        c.daemon.internet = true;
        c.daemon.download_enabled = true;
        c.daemon.download_interval = "10m".to_owned();
    });
    let running = run(&f.engine).await;
    // Let the downloader finish its start pass (nothing to fill) and wait for its timer.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(bing.calls(), 0);

    let mut cfg = (*f.engine.config()).clone();
    cfg.sources = vec![row("bing", SourceType::Bing, "")];
    f.engine.reload(cfg).await.unwrap();

    // Long before the 10m download interval.
    assert!(
        wait_for_image(&f.downloads().join("bing")).await,
        "no download after reload"
    );
    assert_eq!(bing.calls(), 1);
    assert!(f.setter.calls().is_empty(), "the downloader sets nothing");
    running.stop().await;
}

#[tokio::test]
async fn run_saves_the_facts_cache_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let pics = dir.path().join("pics");
    png(&pics.join("wide.png"), 64, 32);
    let location = pics.to_str().unwrap().to_owned();
    let f = fixture(Vec::new(), |c, _| {
        c.sources = vec![row("pics", SourceType::Folder, &location)];
        c.filters.landscape = true;
    });
    let facts = f.root.join("facts.json");
    let running = run(&f.engine).await;
    f.engine.next().await.unwrap();
    // The debounced save waits 5 s; only the shutdown save can have written it this soon.
    running.stop().await;
    let text = std::fs::read_to_string(&facts).expect("facts.json written at shutdown");
    assert!(text.contains("wide.png"), "{text}");
}
