//! Engine core ops (next/prev/set/status) against fakes: `RecordingSetter`, `ManualSignals`,
//! `StaticProvider` in a fake `SourceSet` and a local wiremock image server. Ports the relevant Go
//! `internal/wallpaper/manager_test.go` cases. No network beyond 127.0.0.1, temp dirs only.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use kakemono_config::Config;
use kakemono_config::schema::SourceType;
use kakemono_engine::downloader::DownloadEnv;
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine, EngineError, EngineEvent};
use kakemono_platform::api::{Fit, Platform, Theme};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_sources::fake::StaticProvider;
use kakemono_sources::{Provider, ProviderError, SourceSet, WallpaperResult};
use tokio::sync::Notify;
use wiremock::MockServer;

use crate::common::{Builder, Fixture, image_server, now, row, settle};

/// A one-source set: the online row `bing` backed by `provider`.
fn bing_set(provider: Arc<dyn Provider>) -> SourceSet {
    SourceSet::fake(vec![("bing", provider, "")])
}

/// The config edit that adds the `bing` row [`bing_set`] serves.
fn bing_row(c: &mut Config) {
    c.sources = vec![row("bing", SourceType::Bing, "")];
}

/// An engine with Internet access and `interval = "45m"` over the fixed `sources`.
fn fixture_with(sources: SourceSet, theme: Theme, edit: impl FnOnce(&mut Config)) -> Fixture {
    Builder::new()
        .internet(true)
        .interval("45m")
        .theme(theme)
        .sources(sources)
        .build(|c, _| edit(c))
}

fn fixture(sources: SourceSet) -> Fixture {
    fixture_with(sources, Theme::Light, |_| {})
}

// ─── construction ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn new_creates_download_trash_and_favorites_dirs() {
    let f = fixture(SourceSet::default());
    for d in ["downloads", "trash", "favorites"] {
        assert!(f.root.join(d).is_dir(), "{d} not created");
    }
}

#[tokio::test]
async fn new_reports_a_dir_it_cannot_create() {
    let base = Builder::new().build(|_, _| {});
    let blocker = base.image("file");
    let mut cfg = (*base.engine.config()).clone();
    cfg.daemon.download_dir = blocker.join("sub");
    let platform = Platform {
        setter: Arc::new(RecordingSetter::new()),
        signals: Arc::new(ManualSignals::new(Theme::Light)),
    };
    let deps = Deps::new(platform, reqwest::Client::new()).with_sources(SourceSet::default());
    let err = Engine::new(cfg, State::new(), base.root.join("s.json"), deps).unwrap_err();
    assert!(matches!(err, EngineError::CreateDir { .. }), "{err:?}");
    assert!(err.to_string().starts_with("manager: create dir "), "{err}");
}

// ─── next ─────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn next_on_an_empty_pool_downloads_sets_and_pushes_history() {
    let server = image_server(&["photo.jpg"]).await;
    let url = format!("{}/img/photo.jpg", server.uri());
    let bing = Arc::new(StaticProvider::new(
        "bing",
        vec![StaticProvider::result("bing", &url)],
    ));
    let f = fixture_with(bing_set(bing.clone()), Theme::Light, bing_row);
    let mut events = f.engine.subscribe();

    let info = f.engine.next().await.unwrap();

    let expected_path = f.downloads().join("bing").join("photo.jpg");
    assert_eq!(info.path, expected_path.to_str().unwrap());
    assert_eq!(std::fs::read(&expected_path).unwrap().len(), 12 * 1024);
    assert_eq!(info.source, "bing");
    assert_eq!(info.title, "photo.jpg");
    assert_eq!(info.set_at, "2026-03-04T05:06:07+08:00");
    assert_eq!((info.width, info.height), (0, 0));
    assert_eq!(bing.queries(), vec![""]);
    assert_eq!(f.setter.calls(), vec![(expected_path, Fit::Center)]);

    let status = f.engine.status();
    assert_eq!(status.current.as_ref(), Some(&info));
    assert_eq!((status.history_count, status.history_index), (1, 0));
    assert_eq!(
        events.try_recv().unwrap(),
        EngineEvent::Changed(info.clone())
    );

    // Persisted, with last_changed from the injected clock.
    let saved = State::load(&f.state_path()).unwrap();
    assert_eq!(saved.current(), Some(&info));
    assert_eq!(saved.last_changed(), Some(now()));
}

#[tokio::test]
async fn single_failing_source_keeps_go_message() {
    let bad = Arc::new(StaticProvider::failing("bing", "bing: boom"));
    let f = fixture_with(bing_set(bad), Theme::Light, bing_row);
    let err = f.engine.next().await.unwrap_err();
    assert_eq!(err.to_string(), "manager: provider bing fetch: bing: boom");
}

#[tokio::test]
async fn download_error_redacts_signed_url() {
    let server = MockServer::start().await; // every path 404s
    let url = format!(
        "{}/img/x.jpg?X-Amz-Signature=S3CRETSIG&X-Amz-Credential=CRED&sig=S1G&w=1",
        server.uri()
    );
    let p = Arc::new(StaticProvider::new(
        "bing",
        vec![StaticProvider::result("bing", &url)],
    ));
    let f = fixture_with(bing_set(p), Theme::Light, bing_row);
    let msg = f.engine.next().await.unwrap_err().to_string();
    assert!(msg.starts_with("manager: download image: "), "{msg}");
    assert!(msg.contains("404"), "{msg}");
    for secret in ["S3CRETSIG", "CRED", "S1G"] {
        assert!(!msg.contains(secret), "{secret} leaked: {msg}");
    }
    assert!(msg.contains("X-Amz-Signature=REDACTED"), "{msg}");
    assert!(msg.contains("w=1"), "{msg}");
}

#[tokio::test]
async fn setter_failure_leaves_history_unchanged() {
    let f = fixture(SourceSet::default());
    let img = f.image("a.png");
    f.setter.fail_next(1);
    let err = f.engine.set_file(&img).await.unwrap_err();
    assert_eq!(err.to_string(), "manager: set wallpaper: injected failure");
    assert_eq!(f.engine.status().history_count, 0);
    assert!(!f.state_path().exists());
}

// ─── prev / history navigation ────────────────────────────────────────────────────────────

/// Go TestManagerPrev_EmptyHistory.
#[tokio::test]
async fn prev_on_empty_history_errors() {
    let f = fixture(SourceSet::default());
    let err = f.engine.prev().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "manager: already at the beginning of history"
    );
    assert!(f.setter.calls().is_empty());
}

#[tokio::test]
async fn prev_then_next_walks_history_without_fetching() {
    let bing = Arc::new(StaticProvider::new("bing", vec![]));
    let f = fixture_with(bing_set(bing.clone()), Theme::Light, bing_row);
    let fetched = f.downloads().join("bing").join("fetched.png");
    std::fs::create_dir_all(fetched.parent().unwrap()).unwrap();
    std::fs::write(&fetched, b"f").unwrap();
    let a = f.engine.set_file(&f.image("a.png")).await.unwrap();
    let b = f.engine.set_file(&f.image("b.png")).await.unwrap();
    let c = f.engine.set_file(&f.image("c.png")).await.unwrap();

    assert_eq!(f.engine.prev().await.unwrap(), b);
    assert_eq!(f.engine.prev().await.unwrap(), a);
    assert_eq!(
        f.engine.prev().await.unwrap_err().to_string(),
        "manager: already at the beginning of history"
    );
    let st = f.engine.status();
    assert_eq!((st.history_count, st.history_index), (3, 0));
    assert_eq!(st.current.as_ref(), Some(&a));

    assert_eq!(f.engine.next().await.unwrap(), b);
    assert_eq!(f.engine.next().await.unwrap(), c);
    assert_eq!(f.engine.status().history_index, 2);

    // At the end of history, next picks from disk.
    let d = f.engine.next().await.unwrap();
    assert_eq!(d.source, "bing");
    assert_eq!(Path::new(&d.path), fetched);
    assert_eq!(
        bing.calls(),
        0,
        "nothing is fetched while the pool has images"
    );
    assert_eq!(f.engine.status().history_count, 4);

    let set_paths: Vec<_> = f.setter.calls().into_iter().map(|(p, _)| p).collect();
    let expect: Vec<PathBuf> = [&a, &b, &c, &b, &a, &b, &c, &d]
        .iter()
        .map(|i| PathBuf::from(&i.path))
        .collect();
    assert_eq!(set_paths, expect);

    let saved = State::load(&f.state_path()).unwrap();
    assert_eq!(saved.history_index(), 3);
}

#[tokio::test]
async fn set_after_prev_truncates_forward_history() {
    let f = fixture(SourceSet::default());
    f.engine.set_file(&f.image("a.png")).await.unwrap();
    f.engine.set_file(&f.image("b.png")).await.unwrap();
    f.engine.prev().await.unwrap();
    let c = f.engine.set_file(&f.image("c.png")).await.unwrap();
    let st = f.engine.state();
    let titles: Vec<_> = st.history().iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["a.png", "c.png"]);
    assert_eq!(st.current(), Some(&c));
}

#[tokio::test]
async fn failed_prev_set_keeps_position() {
    let f = fixture(SourceSet::default());
    f.engine.set_file(&f.image("a.png")).await.unwrap();
    let b = f.engine.set_file(&f.image("b.png")).await.unwrap();
    f.setter.fail_next(1);
    assert!(f.engine.prev().await.is_err());
    assert_eq!(f.engine.status().current, Some(b));
    assert_eq!(f.engine.status().history_index, 1);
}

// ─── set_file ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_file_pushes_local_entry() {
    let f = fixture(SourceSet::default());
    let img = f.image("Mount Fuji.jpg");
    let mut events = f.engine.subscribe();
    let info = f.engine.set_file(&img).await.unwrap();
    assert_eq!(info.path, img.to_str().unwrap());
    assert_eq!(info.source, "local");
    assert_eq!(info.source_id, "", "no [[sources]] row");
    assert_eq!(info.title, "Mount Fuji.jpg");
    assert_eq!(info.set_at, "2026-03-04T05:06:07+08:00");
    assert_eq!(
        (
            info.source_url.as_str(),
            info.image_url.as_str(),
            info.author.as_str()
        ),
        ("", "", "")
    );
    assert_eq!(f.setter.calls(), vec![(img, Fit::Center)]);
    assert_eq!(
        events.try_recv().unwrap(),
        EngineEvent::Changed(info.clone())
    );
    assert_eq!(State::load(&f.state_path()).unwrap().current(), Some(&info));
}

#[tokio::test]
async fn set_file_missing_has_go_stat_message() {
    let f = fixture(SourceSet::default());
    let err = f
        .engine
        .set_file(Path::new("/nonexistent/kakemono-golden.png"))
        .await
        .unwrap_err();
    // Same text as the Go wire fixture 13_set_nonexistent.
    assert_eq!(
        err.to_string(),
        "manager: file not found: stat /nonexistent/kakemono-golden.png: no such file or directory"
    );
    assert_eq!(f.setter.attempts(), 0);
}

// ─── status / pause / resume ──────────────────────────────────────────────────────────────

/// Go TestManagerStatus.
#[tokio::test]
async fn status_of_fresh_engine() {
    let f = fixture_with(SourceSet::default(), Theme::Dark, |_| {});
    let st = f.engine.status();
    assert_eq!(st.current, None);
    assert!(!st.paused);
    assert_eq!(st.interval, "45m");
    assert_eq!(st.theme, "dark");
    assert_eq!((st.history_count, st.history_index), (0, -1));

    f.signals.set_theme(Theme::Light);
    assert_eq!(f.engine.status().theme, "light");
}

/// Go TestManagerPauseResume, plus persistence and events.
#[tokio::test]
async fn pause_resume_persist_and_emit_on_change() {
    let f = fixture(SourceSet::default());
    let mut events = f.engine.subscribe();
    f.engine.pause();
    assert!(f.engine.is_paused() && f.engine.status().paused);
    f.engine.save_state().await;
    assert!(State::load(&f.state_path()).unwrap().paused());
    f.engine.pause(); // no change, no event
    f.engine.resume();
    assert!(!f.engine.status().paused);
    f.engine.save_state().await;
    assert!(!State::load(&f.state_path()).unwrap().paused());
    assert_eq!(events.try_recv().unwrap(), EngineEvent::Paused(true));
    assert_eq!(events.try_recv().unwrap(), EngineEvent::Paused(false));
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn engine_starts_from_loaded_state() {
    let f = fixture(SourceSet::default());
    let a = f.engine.set_file(&f.image("a.png")).await.unwrap();
    f.engine.pause();
    f.engine.save_state().await;
    let loaded = State::load(&f.state_path()).unwrap();
    let platform = Platform {
        setter: Arc::new(RecordingSetter::new()),
        signals: Arc::new(ManualSignals::new(Theme::Light)),
    };
    let deps = Deps::new(platform, reqwest::Client::new()).with_sources(SourceSet::default());
    let again = Engine::new((*f.engine.config()).clone(), loaded, f.state_path(), deps).unwrap();
    let st = again.status();
    assert_eq!(st.current, Some(a));
    assert!(st.paused);
    assert_eq!((st.history_count, st.history_index), (1, 0));
}

#[tokio::test]
async fn reload_replaces_config_and_rebuilds_sources() {
    let dir = tempfile::tempdir().unwrap();
    let pics = dir.path().join("pics");
    std::fs::create_dir(&pics).unwrap();
    std::fs::write(pics.join("x.png"), b"x").unwrap();
    let builds = Arc::new(AtomicUsize::new(0));
    let f = {
        let builds = builds.clone();
        // The sources follow the config: one fake per online row.
        Builder::new()
            .sources_factory(move |cfg, _| {
                builds.fetch_add(1, Ordering::SeqCst);
                SourceSet::fake(
                    cfg.sources
                        .iter()
                        .filter(|e| e.kind.is_online())
                        .map(|e| {
                            let p: Arc<dyn Provider> =
                                Arc::new(StaticProvider::new("bing", vec![]));
                            (e.id.as_str(), p, "")
                        })
                        .collect(),
                )
            })
            .build(|_, _| {})
    };
    assert!(matches!(
        f.engine.next().await.unwrap_err(),
        EngineError::NoImages
    ));
    assert!(f.engine.sources().is_empty());

    let mut cfg = (*f.engine.config()).clone();
    cfg.daemon.interval = "2h".to_owned();
    cfg.sources = vec![
        row("pics", SourceType::Folder, pics.to_str().unwrap()),
        row("bing", SourceType::Bing, ""),
    ];
    cfg.favorites.dir = f.root.join("new-favs");
    f.engine.reload(cfg).await.unwrap();

    assert_eq!(builds.load(Ordering::SeqCst), 2);
    assert!(f.engine.sources().get("bing").is_some());
    assert_eq!(f.engine.status().interval, "2h");
    assert!(f.root.join("new-favs").is_dir());
    assert_eq!(f.engine.next().await.unwrap().source, "folder");
    assert_eq!(f.engine.config().daemon.interval, "2h");
}

// ─── favorite / trash call sites (bodies land in Task 20) ──────────────────────────────────

#[tokio::test]
async fn favorite_and_trash_need_a_current_wallpaper() {
    let f = fixture(SourceSet::default());
    assert_eq!(
        f.engine.favorite().await.unwrap_err().to_string(),
        "manager: no current wallpaper"
    );
    assert_eq!(
        f.engine.trash().await.unwrap_err().to_string(),
        "manager: no current wallpaper"
    );
}

// ─── concurrency ──────────────────────────────────────────────────────────────────────────

/// A provider that blocks in `fetch` until released.
struct GatedProvider {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    url: String,
}

#[async_trait]
impl Provider for GatedProvider {
    fn name(&self) -> &'static str {
        "bing"
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(vec![StaticProvider::result("bing", &self.url)])
    }
}

#[tokio::test]
async fn status_does_not_block_while_next_awaits_a_slow_provider() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let server = image_server(&["slow.jpg"]).await;
    let slow = Arc::new(GatedProvider {
        entered: entered.clone(),
        release: release.clone(),
        url: format!("{}/img/slow.jpg", server.uri()),
    });
    // An empty pool: `next` downloads on demand through the gated provider.
    let f = fixture_with(bing_set(slow), Theme::Light, bing_row);
    let a = f.engine.set_file(&f.image("a.png")).await.unwrap();

    let engine = f.engine.clone();
    let next = tokio::spawn(async move { engine.next().await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("provider never called");

    // `next` holds the op lock and awaits the provider; these must return right away.
    let engine = f.engine.clone();
    let st = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || {
            let st = engine.status();
            engine.pause();
            engine.resume();
            st
        }),
    )
    .await
    .expect("status blocked behind next")
    .unwrap();
    assert_eq!(st.current, Some(a));
    assert_eq!(st.history_count, 1);

    // A second mutating op waits for the first.
    let engine = f.engine.clone();
    let prev = tokio::spawn(async move { engine.prev().await });
    settle().await;
    assert!(!prev.is_finished(), "prev must wait for the op lock");

    release.notify_one();
    let info = next.await.unwrap().unwrap();
    assert_eq!(info.source, "bing");
    // prev ran after next completed: back to `a`.
    let back = prev.await.unwrap().unwrap();
    assert_eq!(back.title, "a.png");
    assert_eq!(f.engine.status().history_count, 2);
}
