//! Engine core ops (next/prev/set/status) against fakes: `RecordingSetter`, `ManualSignals`,
//! `StaticProvider` and a local wiremock image server. Ports the relevant Go
//! `internal/wallpaper/manager_test.go` cases. No network beyond 127.0.0.1, temp dirs only.

// In a module so `cargo test -p kakemono-engine engine_core` (the plan's filter) selects them.
mod engine_core {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::{DateTime, FixedOffset};
    use kakemono_config::Config;
    use kakemono_engine::state::State;
    use kakemono_engine::{Deps, Engine, EngineError, EngineEvent};
    use kakemono_platform::api::{Fit, Platform, Theme};
    use kakemono_platform::fake::{ManualSignals, RecordingSetter};
    use kakemono_sources::fake::StaticProvider;
    use kakemono_sources::registry::Registry;
    use kakemono_sources::{Provider, ProviderError, WallpaperResult};
    use tempfile::TempDir;
    use tokio::sync::Notify;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const NOW: &str = "2026-03-04T05:06:07.890+08:00";

    struct Fixture {
        _dir: TempDir,
        root: PathBuf,
        setter: RecordingSetter,
        signals: ManualSignals,
        engine: Engine,
    }

    impl Fixture {
        fn state_path(&self) -> PathBuf {
            self.root.join("state.json")
        }

        fn download_dir(&self) -> PathBuf {
            self.root.join("downloads")
        }

        /// An image file outside the download dir.
        fn image(&self, name: &str) -> PathBuf {
            let p = self.root.join(name);
            std::fs::write(&p, b"fake image data").unwrap();
            p
        }
    }

    fn config(root: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.daemon.download_dir = root.join("downloads");
        cfg.daemon.trash_dir = root.join("trash");
        cfg.favorites.dir = root.join("favorites");
        cfg.daemon.interval = "45m".to_owned();
        cfg.desktop.fit = Fit::Center;
        cfg.light.sources = vec!["bing".to_owned()];
        cfg.light.queries = vec!["day".to_owned()];
        cfg.dark.sources = vec!["apod".to_owned()];
        cfg.dark.queries = vec!["night".to_owned()];
        cfg
    }

    fn fixture_with(registry: Registry, theme: Theme, edit: impl FnOnce(&mut Config)) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let mut cfg = config(&root);
        edit(&mut cfg);
        let setter = RecordingSetter::new();
        let signals = ManualSignals::new(theme);
        let platform = Platform {
            setter: Arc::new(setter.clone()),
            signals: Arc::new(signals.clone()),
        };
        let now: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(NOW).unwrap();
        let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
            .with_registry(registry)
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

    fn fixture(registry: Registry) -> Fixture {
        fixture_with(registry, Theme::Light, |_| {})
    }

    fn local_result(name: &str, image: &Path) -> WallpaperResult {
        StaticProvider::result(name, image.to_str().unwrap())
    }

    /// A 12 KiB JPEG-typed body at `/img/<name>` (the downloader wants ≥ 10 KiB and an image type).
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

    // ─── construction ─────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn new_creates_download_trash_and_favorites_dirs() {
        let f = fixture(Registry::new());
        for d in ["downloads", "trash", "favorites"] {
            assert!(f.root.join(d).is_dir(), "{d} not created");
        }
    }

    #[tokio::test]
    async fn new_reports_a_dir_it_cannot_create() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        let mut cfg = config(dir.path());
        cfg.daemon.download_dir = blocker.join("sub");
        let platform = Platform {
            setter: Arc::new(RecordingSetter::new()),
            signals: Arc::new(ManualSignals::new(Theme::Light)),
        };
        let deps = Deps::new(platform, reqwest::Client::new()).with_registry(Registry::new());
        let err = Engine::new(cfg, State::new(), dir.path().join("s.json"), deps).unwrap_err();
        assert!(matches!(err, EngineError::CreateDir { .. }), "{err:?}");
        assert!(err.to_string().starts_with("manager: create dir "), "{err}");
    }

    // ─── next ─────────────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn next_downloads_sets_and_pushes_history() {
        let server = image_server("photo.jpg").await;
        let url = format!("{}/img/photo.jpg", server.uri());
        let bing = Arc::new(StaticProvider::new(
            "bing",
            vec![StaticProvider::result("bing", &url)],
        ));
        let f = fixture(Registry::new().with(bing.clone()));
        let mut events = f.engine.subscribe();

        let info = f.engine.next().await.unwrap();

        let expected_path = f.download_dir().join("photo.jpg");
        assert_eq!(info.path, expected_path.to_str().unwrap());
        assert_eq!(std::fs::read(&expected_path).unwrap().len(), 12 * 1024);
        assert_eq!(info.source, "bing");
        assert_eq!(info.image_url, url);
        assert_eq!(info.source_url, "https://example.test/bing");
        assert_eq!(info.title, "bing wallpaper");
        assert_eq!(info.set_at, "2026-03-04T05:06:07+08:00");
        assert_eq!((info.width, info.height), (0, 0));
        assert_eq!(bing.queries(), vec!["day"]);
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
        assert_eq!(
            saved.last_changed(),
            Some(DateTime::parse_from_rfc3339(NOW).unwrap())
        );
    }

    #[tokio::test]
    async fn next_uses_local_absolute_path_without_download() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("local.png");
        std::fs::write(&img, b"png").unwrap();
        let local = Arc::new(StaticProvider::new(
            "local",
            vec![local_result("local", &img)],
        ));
        let f = fixture_with(Registry::new().with(local), Theme::Light, |c| {
            c.light.sources = vec!["local".to_owned()];
        });

        let info = f.engine.next().await.unwrap();
        assert_eq!(info.path, img.to_str().unwrap());
        assert_eq!(f.setter.last().unwrap().0, img);
        assert_eq!(std::fs::read_dir(f.download_dir()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn next_accepts_file_url() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("with space.png");
        std::fs::write(&img, b"png").unwrap();
        // Go trimmed `file://` literally; the percent-encoded form works too.
        for url in [
            format!("file://{}", img.display()),
            reqwest::Url::from_file_path(&img).unwrap().to_string(),
        ] {
            let p = Arc::new(StaticProvider::new(
                "local",
                vec![StaticProvider::result("local", &url)],
            ));
            let f = fixture_with(Registry::new().with(p), Theme::Light, |c| {
                c.light.sources = vec!["local".to_owned()];
            });
            let info = f.engine.next().await.unwrap();
            assert_eq!(Path::new(&info.path), img, "url {url}");
        }
    }

    #[tokio::test]
    async fn next_with_missing_local_file_fails_with_go_message() {
        let p = Arc::new(StaticProvider::new(
            "local",
            vec![StaticProvider::result(
                "local",
                "/nonexistent/kakemono-x.png",
            )],
        ));
        let f = fixture_with(Registry::new().with(p), Theme::Light, |c| {
            c.light.sources = vec!["local".to_owned()];
        });
        let err = f.engine.next().await.unwrap_err().to_string();
        assert_eq!(
            err,
            "manager: local file not found \"/nonexistent/kakemono-x.png\": \
         stat /nonexistent/kakemono-x.png: no such file or directory"
        );
        assert!(f.setter.calls().is_empty());
        assert_eq!(f.engine.status().history_count, 0);
    }

    #[tokio::test]
    async fn next_uses_dark_sources_and_queries_for_dark_theme() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("day.png");
        let night = dir.path().join("night.png");
        std::fs::write(&day, b"d").unwrap();
        std::fs::write(&night, b"n").unwrap();
        let bing = Arc::new(StaticProvider::new(
            "bing",
            vec![local_result("bing", &day)],
        ));
        let apod = Arc::new(StaticProvider::new(
            "apod",
            vec![local_result("apod", &night)],
        ));
        let f = fixture_with(
            Registry::new().with(bing.clone()).with(apod.clone()),
            Theme::Dark,
            |_| {},
        );

        let info = f.engine.next().await.unwrap();
        assert_eq!(info.source, "apod");
        assert_eq!(apod.queries(), vec!["night"]);
        assert_eq!(bing.calls(), 0);

        f.signals.set_theme(Theme::Light);
        let info = f.engine.next().await.unwrap();
        assert_eq!(info.source, "bing");
        assert_eq!(bing.queries(), vec!["day"]);
    }

    #[tokio::test]
    async fn next_falls_back_to_another_source_when_one_fails() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("ok.png");
        std::fs::write(&img, b"ok").unwrap();
        // Candidate order is random; run enough times that both orders occur (p(miss) = 2^-20).
        for _ in 0..20 {
            let bad = Arc::new(StaticProvider::failing("bing", "bing: boom"));
            let good = Arc::new(StaticProvider::new(
                "apod",
                vec![local_result("apod", &img)],
            ));
            let f = fixture_with(
                Registry::new().with(bad.clone()).with(good.clone()),
                Theme::Light,
                |c| c.light.sources = vec!["bing".to_owned(), "apod".to_owned()],
            );
            let info = f.engine.next().await.unwrap();
            assert_eq!(info.source, "apod");
            assert_eq!(good.calls(), 1);
            assert!(bad.calls() <= 1);
        }
    }

    #[tokio::test]
    async fn next_falls_back_on_empty_results_and_reports_all_failures() {
        let empty = Arc::new(StaticProvider::new("bing", vec![]));
        let bad = Arc::new(StaticProvider::failing("apod", "apod: boom"));
        let f = fixture_with(
            Registry::new().with(empty.clone()).with(bad.clone()),
            Theme::Light,
            |c| c.light.sources = vec!["bing".to_owned(), "apod".to_owned()],
        );
        let err = f.engine.next().await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.starts_with("manager: "), "{msg}");
        assert!(
            msg.contains("provider bing returned no results for query \"day\""),
            "{msg}"
        );
        assert!(msg.contains("provider apod fetch: apod: boom"), "{msg}");
        assert_eq!((empty.calls(), bad.calls()), (1, 1));
        assert!(f.setter.calls().is_empty());
        assert_eq!(f.engine.status().history_index, -1);
    }

    #[tokio::test]
    async fn single_failing_source_keeps_go_message() {
        let bad = Arc::new(StaticProvider::failing("bing", "bing: boom"));
        let f = fixture(Registry::new().with(bad));
        let err = f.engine.next().await.unwrap_err();
        assert_eq!(err.to_string(), "manager: provider bing fetch: bing: boom");
    }

    #[tokio::test]
    async fn named_sources_do_not_fall_back_to_unlisted_providers() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("ok.png");
        std::fs::write(&img, b"ok").unwrap();
        let bad = Arc::new(StaticProvider::failing("bing", "bing: boom"));
        let other = Arc::new(StaticProvider::new(
            "apod",
            vec![local_result("apod", &img)],
        ));
        let f = fixture(Registry::new().with(bad).with(other.clone()));
        assert!(f.engine.next().await.is_err());
        assert_eq!(other.calls(), 0);
    }

    #[tokio::test]
    async fn next_without_enabled_provider_fails_with_go_message() {
        let off = Arc::new(StaticProvider::new("bing", vec![]).with_enabled(false));
        let f = fixture_with(Registry::new().with(off), Theme::Dark, |c| {
            c.dark.sources = vec!["bing".to_owned(), "nope".to_owned()];
        });
        let err = f.engine.next().await.unwrap_err();
        assert!(matches!(err, EngineError::NoProvider { .. }));
        assert_eq!(
            err.to_string(),
            "manager: no enabled provider available (theme: dark, sources: [bing nope])"
        );
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
        let f = fixture(Registry::new().with(p));
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
        let f = fixture(Registry::new());
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
        let f = fixture(Registry::new());
        let err = f.engine.prev().await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "manager: already at the beginning of history"
        );
        assert!(f.setter.calls().is_empty());
    }

    #[tokio::test]
    async fn prev_then_next_walks_history_without_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let fetched = dir.path().join("fetched.png");
        std::fs::write(&fetched, b"f").unwrap();
        let bing = Arc::new(StaticProvider::new(
            "bing",
            vec![local_result("bing", &fetched)],
        ));
        let f = fixture(Registry::new().with(bing.clone()));
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
        assert_eq!(bing.calls(), 0, "future history must not fetch");
        assert_eq!(f.engine.status().history_index, 2);

        // At the end of history, next fetches.
        let d = f.engine.next().await.unwrap();
        assert_eq!(d.source, "bing");
        assert_eq!(bing.calls(), 1);
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
        let f = fixture(Registry::new());
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
        let f = fixture(Registry::new());
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
        let f = fixture(Registry::new());
        let img = f.image("Mount Fuji.jpg");
        let mut events = f.engine.subscribe();
        let info = f.engine.set_file(&img).await.unwrap();
        assert_eq!(info.path, img.to_str().unwrap());
        assert_eq!(info.source, "local");
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
        let f = fixture(Registry::new());
        let err = f
            .engine
            .set_file(Path::new("/nonexistent/wallpaperd-golden.png"))
            .await
            .unwrap_err();
        // Same text as the Go wire fixture 13_set_nonexistent.
        assert_eq!(
            err.to_string(),
            "manager: file not found: stat /nonexistent/wallpaperd-golden.png: no such file or directory"
        );
        assert_eq!(f.setter.attempts(), 0);
    }

    // ─── status / pause / resume ──────────────────────────────────────────────────────────────

    /// Go TestManagerStatus.
    #[tokio::test]
    async fn status_of_fresh_engine() {
        let f = fixture_with(Registry::new(), Theme::Dark, |_| {});
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
        let f = fixture(Registry::new());
        let mut events = f.engine.subscribe();
        f.engine.pause();
        assert!(f.engine.is_paused() && f.engine.status().paused);
        assert!(State::load(&f.state_path()).unwrap().paused());
        f.engine.pause(); // no change, no event
        f.engine.resume();
        assert!(!f.engine.status().paused);
        assert!(!State::load(&f.state_path()).unwrap().paused());
        assert_eq!(events.try_recv().unwrap(), EngineEvent::Paused(true));
        assert_eq!(events.try_recv().unwrap(), EngineEvent::Paused(false));
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn engine_starts_from_loaded_state() {
        let f = fixture(Registry::new());
        let a = f.engine.set_file(&f.image("a.png")).await.unwrap();
        f.engine.pause();
        let loaded = State::load(&f.state_path()).unwrap();
        let platform = Platform {
            setter: Arc::new(RecordingSetter::new()),
            signals: Arc::new(ManualSignals::new(Theme::Light)),
        };
        let deps = Deps::new(platform, reqwest::Client::new()).with_registry(Registry::new());
        let again = Engine::new(config(&f.root), loaded, f.state_path(), deps).unwrap();
        let st = again.status();
        assert_eq!(st.current, Some(a));
        assert!(st.paused);
        assert_eq!((st.history_count, st.history_index), (1, 0));
    }

    #[tokio::test]
    async fn reload_replaces_config_and_rebuilds_registry() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("x.png");
        std::fs::write(&img, b"x").unwrap();
        let builds = Arc::new(AtomicUsize::new(0));
        let f = {
            let root = dir.path().join("root");
            std::fs::create_dir(&root).unwrap();
            let setter = RecordingSetter::new();
            let signals = ManualSignals::new(Theme::Light);
            let platform = Platform {
                setter: Arc::new(setter.clone()),
                signals: Arc::new(signals.clone()),
            };
            let img = img.clone();
            let builds = builds.clone();
            // The registry follows the config: only providers named in light.sources exist.
            let deps = Deps::new(platform, reqwest::Client::new()).with_registry_factory(
                move |cfg: &Config, _: &reqwest::Client| {
                    builds.fetch_add(1, Ordering::SeqCst);
                    let mut r = Registry::new();
                    if cfg.light.sources.iter().any(|s| s == "local") {
                        r.register(Arc::new(StaticProvider::new(
                            "local",
                            vec![local_result("local", &img)],
                        )));
                    }
                    r
                },
            );
            let engine =
                Engine::new(config(&root), State::new(), root.join("s.json"), deps).unwrap();
            Fixture {
                _dir: tempfile::tempdir().unwrap(),
                root,
                setter,
                signals,
                engine,
            }
        };
        assert!(matches!(
            f.engine.next().await.unwrap_err(),
            EngineError::NoProvider { .. }
        ));

        let mut cfg = config(&f.root);
        cfg.daemon.interval = "2h".to_owned();
        cfg.light.sources = vec!["local".to_owned()];
        cfg.favorites.dir = f.root.join("new-favs");
        f.engine.reload(cfg).await.unwrap();

        assert_eq!(builds.load(Ordering::SeqCst), 2);
        assert_eq!(f.engine.status().interval, "2h");
        assert!(f.root.join("new-favs").is_dir());
        assert_eq!(f.engine.next().await.unwrap().source, "local");
        assert_eq!(f.engine.config().daemon.interval, "2h");
    }

    // ─── favorite / trash call sites (bodies land in Task 20) ──────────────────────────────────

    #[tokio::test]
    async fn favorite_and_trash_need_a_current_wallpaper() {
        let f = fixture(Registry::new());
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
        image: PathBuf,
    }

    #[async_trait]
    impl Provider for GatedProvider {
        fn name(&self) -> &'static str {
            "bing"
        }

        fn enabled(&self) -> bool {
            true
        }

        async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(vec![local_result("bing", &self.image)])
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn status_does_not_block_while_next_awaits_a_slow_provider() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("slow.png");
        std::fs::write(&image, b"s").unwrap();
        let slow = Arc::new(GatedProvider {
            entered: entered.clone(),
            release: release.clone(),
            image,
        });
        let f = fixture(Registry::new().with(slow));
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
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!prev.is_finished(), "prev must wait for the op lock");

        release.notify_one();
        let info = next.await.unwrap().unwrap();
        assert_eq!(info.source, "bing");
        // prev ran after next completed: back to `a`.
        let back = prev.await.unwrap().unwrap();
        assert_eq!(back.title, "a.png");
        assert_eq!(f.engine.status().history_count, 2);
    }
}
