//! Engine favorite, trash and download-cache trimming against fakes (`RecordingSetter`,
//! `ManualSignals`, `StaticProvider`) and a local wiremock image server. Ports Go
//! `TestManagerFavorite_CopiesFile` and covers `trimCache`. Temp dirs only.

// In a module so `cargo test -p kakemono-engine files` (the plan's filter) selects them.
mod files {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use chrono::DateTime;
    use kakemono_config::Config;
    use kakemono_engine::state::State;
    use kakemono_engine::{Deps, Engine};
    use kakemono_platform::api::{Platform, Theme};
    use kakemono_platform::fake::{ManualSignals, RecordingSetter};
    use kakemono_protocol::types::WallpaperInfo;
    use kakemono_sources::fake::StaticProvider;
    use kakemono_sources::registry::Registry;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    /// An engine rooted in a temp dir. `prepare` gets the root before the engine exists and
    /// returns the initial state (so tests can pre-populate files and history).
    fn fixture(
        registry: Registry,
        cache_size: u32,
        prepare: impl FnOnce(&Path) -> State,
    ) -> Fixture {
        fixture_with(registry, cache_size, |_, _| {}, prepare)
    }

    /// [`fixture`] plus a hook that edits the config (it gets the temp root) before `prepare`.
    fn fixture_with(
        registry: Registry,
        cache_size: u32,
        configure: impl FnOnce(&mut Config, &Path),
        prepare: impl FnOnce(&Path) -> State,
    ) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let mut cfg = Config::default();
        cfg.daemon.download_dir = root.join("downloads");
        cfg.daemon.trash_dir = root.join("trash");
        cfg.favorites.dir = root.join("favorites");
        cfg.daemon.cache_size = cache_size;
        cfg.light.sources = vec!["bing".to_owned()];
        configure(&mut cfg, &root);
        fs::create_dir_all(root.join("downloads")).unwrap();
        let state = prepare(&root);

        let setter = RecordingSetter::new();
        let platform = Platform {
            setter: Arc::new(setter.clone()),
            signals: Arc::new(ManualSignals::new(Theme::Light)),
        };
        let now = DateTime::parse_from_rfc3339(NOW).unwrap();
        let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
            .with_registry(registry)
            .with_clock(move || now);
        let engine = Engine::new(cfg, state, root.join("state.json"), deps).unwrap();
        Fixture {
            _dir: dir,
            root,
            setter,
            engine,
        }
    }

    /// One provider (`bing`) returning `image` as a local path.
    fn local_registry(image: &Path) -> Registry {
        Registry::new().with(Arc::new(StaticProvider::new(
            "bing",
            vec![StaticProvider::result("bing", image.to_str().unwrap())],
        )))
    }

    /// A state whose current wallpaper is `path`.
    fn state_with(paths: &[&Path]) -> State {
        let mut state = State::new();
        for p in paths {
            state.push(info(p), DateTime::parse_from_rfc3339(NOW).unwrap());
        }
        state
    }

    /// Writes `bytes` to `path` with a modification time `age_secs` in the past.
    fn write_aged(path: &Path, bytes: &[u8], age_secs: u64) {
        fs::write(path, bytes).unwrap();
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
            .unwrap();
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
        let f = fixture(Registry::new(), 0, |root| {
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
        let f = fixture(Registry::new(), 0, |root| {
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
        let f = fixture(Registry::new(), 0, |root| {
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
        let f = fixture(Registry::new(), 0, |root| {
            state_with(&[&root.join("gone.jpg")])
        });
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
        let f = fixture(Registry::new(), 0, |root| {
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
        let f = fixture(local_registry(&next_image), 0, |root| {
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
        let f = fixture(local_registry(&next_image), 0, |root| {
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
        let f = fixture(local_registry(&next_image), 0, |root| {
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
        let f = fixture(local_registry(&next_image), 0, |root| {
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
        let f = fixture(local_registry(&next_image), 0, |root| {
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

    // ─── trim_cache ───────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn next_trims_the_oldest_downloads_beyond_cache_size() {
        let server = image_server("new.jpg").await;
        let url = format!("{}/img/new.jpg", server.uri());
        let registry = Registry::new().with(Arc::new(StaticProvider::new(
            "bing",
            vec![StaticProvider::result("bing", &url)],
        )));
        let f = fixture(registry, 2, |root| {
            let d = root.join("downloads");
            write_aged(&d.join("a.jpg"), b"a", 400);
            write_aged(&d.join("b.png"), b"b", 300);
            write_aged(&d.join("c.webp"), b"c", 200);
            State::new()
        });

        f.engine.next().await.unwrap();

        // 4 images, keep 2: the two oldest go.
        assert_eq!(names(&f.downloads()), vec!["c.webp", "new.jpg"]);
    }

    #[tokio::test]
    async fn trim_never_deletes_current_or_the_last_two_history_entries() {
        let server = image_server("new.jpg").await;
        let url = format!("{}/img/new.jpg", server.uri());
        let registry = Registry::new().with(Arc::new(StaticProvider::new(
            "bing",
            vec![StaticProvider::result("bing", &url)],
        )));
        let f = fixture(registry, 1, |root| {
            let d = root.join("downloads");
            write_aged(&d.join("h1.jpg"), b"1", 600);
            write_aged(&d.join("h2.jpg"), b"2", 500);
            write_aged(&d.join("free.jpg"), b"f", 400);
            // History after `next`: [h1, h2, new]. h1 is three back, so it is not protected.
            state_with(&[&d.join("h1.jpg"), &d.join("h2.jpg")])
        });

        f.engine.next().await.unwrap();

        // 4 images, keep 1: h2 (previous entry) and new.jpg (current) survive anyway.
        assert_eq!(names(&f.downloads()), vec!["h2.jpg", "new.jpg"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn trim_matches_protected_paths_through_a_symlinked_dir() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh.png");
        fs::write(&fresh, b"fresh").unwrap();
        let f = fixture(local_registry(&fresh), 1, |root| {
            let d = root.join("downloads");
            write_aged(&d.join("prev.jpg"), b"p", 900);
            write_aged(&d.join("x.jpg"), b"x", 800);
            write_aged(&d.join("y.jpg"), b"y", 700);
            // History names the download dir through a symlink.
            std::os::unix::fs::symlink(&d, root.join("link")).unwrap();
            state_with(&[&root.join("link").join("prev.jpg")])
        });

        f.engine.next().await.unwrap();

        // History after `next`: [link/prev, fresh] (fresh lives outside the download dir).
        // 3 images, keep 1: prev.jpg is protected, so x and y go.
        assert_eq!(names(&f.downloads()), vec!["prev.jpg"]);
    }

    #[tokio::test]
    async fn trim_skips_non_images_subdirs_and_zero_cache_size() {
        let server = image_server("new.jpg").await;
        let url = format!("{}/img/new.jpg", server.uri());
        let make_registry = || {
            Registry::new().with(Arc::new(StaticProvider::new(
                "bing",
                vec![StaticProvider::result("bing", &url)],
            )))
        };
        let populate = |root: &Path| {
            let d = root.join("downloads");
            write_aged(&d.join("old.jpg"), b"o", 900);
            write_aged(&d.join("notes.txt"), b"t", 900);
            fs::create_dir(d.join("favorites")).unwrap();
            write_aged(&d.join("favorites").join("fav.jpg"), b"f", 900);
            State::new()
        };

        // cache_size 0 = unlimited.
        let f = fixture(make_registry(), 0, populate);
        f.engine.next().await.unwrap();
        assert_eq!(
            names(&f.downloads()),
            vec!["favorites", "new.jpg", "notes.txt", "old.jpg"]
        );

        let f = fixture(make_registry(), 1, populate);
        f.engine.next().await.unwrap();
        assert_eq!(
            names(&f.downloads()),
            vec!["favorites", "new.jpg", "notes.txt"]
        );
        assert!(f.downloads().join("favorites").join("fav.jpg").exists());
    }

    // ─── trim_cache vs favorites / trash (regression: Go deleted favorites in the cache) ─────

    /// One provider (`bing`) returning a local image outside the temp root, so `next` trims
    /// without downloading. The returned guard keeps the image alive.
    fn fresh_image() -> (TempDir, Registry) {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh.png");
        fs::write(&fresh, b"fresh").unwrap();
        let registry = local_registry(&fresh);
        (dir, registry)
    }

    fn three_aged_images(d: &Path) {
        write_aged(&d.join("a.jpg"), b"a", 300);
        write_aged(&d.join("b.jpg"), b"b", 200);
        write_aged(&d.join("c.jpg"), b"c", 100);
    }

    #[tokio::test]
    async fn trim_deletes_nothing_when_favorites_dir_is_the_download_dir() {
        let (_img, registry) = fresh_image();
        let f = fixture_with(
            registry,
            1,
            |cfg, root| cfg.favorites.dir = root.join("downloads"),
            |root| {
                three_aged_images(&root.join("downloads"));
                State::new()
            },
        );
        f.engine.next().await.unwrap();
        assert_eq!(names(&f.downloads()), vec!["a.jpg", "b.jpg", "c.jpg"]);
    }

    #[tokio::test]
    async fn trim_deletes_nothing_when_trash_dir_is_the_download_dir() {
        let (_img, registry) = fresh_image();
        let f = fixture_with(
            registry,
            1,
            |cfg, root| cfg.daemon.trash_dir = root.join("downloads"),
            |root| {
                three_aged_images(&root.join("downloads"));
                State::new()
            },
        );
        f.engine.next().await.unwrap();
        assert_eq!(names(&f.downloads()), vec!["a.jpg", "b.jpg", "c.jpg"]);
    }

    #[tokio::test]
    async fn trim_deletes_nothing_when_favorites_dir_contains_the_download_dir() {
        let (_img, registry) = fresh_image();
        let f = fixture_with(
            registry,
            1,
            |cfg, root| cfg.favorites.dir = root.to_path_buf(),
            |root| {
                three_aged_images(&root.join("downloads"));
                State::new()
            },
        );
        f.engine.next().await.unwrap();
        assert_eq!(names(&f.downloads()), vec!["a.jpg", "b.jpg", "c.jpg"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn trim_deletes_nothing_when_favorites_dir_is_a_symlink_to_the_download_dir() {
        let (_img, registry) = fresh_image();
        let f = fixture_with(
            registry,
            1,
            |cfg, root| {
                std::os::unix::fs::symlink(root.join("downloads"), root.join("favs-link")).unwrap();
                cfg.favorites.dir = root.join("favs-link");
            },
            |root| {
                three_aged_images(&root.join("downloads"));
                State::new()
            },
        );
        f.engine.next().await.unwrap();
        assert_eq!(names(&f.downloads()), vec!["a.jpg", "b.jpg", "c.jpg"]);
    }

    #[tokio::test]
    async fn trim_never_touches_a_favorites_dir_nested_in_the_download_dir() {
        let (_img, registry) = fresh_image();
        let f = fixture_with(
            registry,
            1,
            |cfg, root| cfg.favorites.dir = root.join("downloads").join("favs"),
            |root| {
                let d = root.join("downloads");
                three_aged_images(&d);
                let src = d.join("c.jpg");
                state_with(&[&src])
            },
        );
        // Favorite the current image (c.jpg) into the nested dir, then rotate.
        f.engine.favorite().await.unwrap();
        let favs = f.downloads().join("favs");
        write_aged(&favs.join("old-fav.jpg"), b"o", 9_000);

        f.engine.next().await.unwrap();

        // Top-level images: a, b, c; keep 1. c is the previous history entry; a and b go.
        assert_eq!(names(&f.downloads()), vec!["c.jpg", "favs"]);
        assert_eq!(names(&favs), vec!["c.jpg", "old-fav.jpg"]);
    }
}
