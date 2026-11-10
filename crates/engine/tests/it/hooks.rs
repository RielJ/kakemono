//! Engine events and `[hooks] on_change` against fakes: `RecordingRunner`, `RecordingSetter`,
//! `ManualSignals` and a local folder source. Temp dirs only; no process is ever spawned.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_config::{CommandSpec, Config};
use kakemono_engine::hooks::{self, HOOK_TIMEOUT, MAX_CONCURRENT_HOOKS, PASS_THROUGH_ENV};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine, EngineEvent};
use kakemono_platform::api::{Fit, Platform, Theme};
use kakemono_platform::exec::{Invocation, Output, RecordingRunner};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_protocol::types::{Event, WallpaperInfo};
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    runner: RecordingRunner,
    setter: RecordingSetter,
    engine: Engine,
}

impl Fixture {
    /// An image file in the temp dir.
    fn image(&self, name: &str) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, b"fake image data").unwrap();
        p
    }
}

fn argv(v: &[&str]) -> Vec<String> {
    v.iter().map(|t| (*t).to_owned()).collect()
}

/// An engine whose only source is the folder `pics` holding `local.png`, with
/// `on_change = hook` (if any).
fn fixture(hook: Option<&[&str]>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let pics = root.join("pics");
    std::fs::create_dir(&pics).unwrap();
    std::fs::write(pics.join("local.png"), b"png").unwrap();

    let mut cfg = Config::default();
    cfg.daemon.download_dir = root.join("downloads");
    cfg.daemon.trash_dir = root.join("trash");
    cfg.favorites.dir = root.join("favorites");
    cfg.desktop.fit = Fit::Center;
    cfg.daemon.internet = false;
    cfg.sources = vec![SourceEntry {
        id: "pics".to_owned(),
        enabled: true,
        kind: SourceType::Folder,
        location: pics.to_string_lossy().into_owned(),
    }];
    cfg.hooks.on_change = hook.map(|h| CommandSpec::new(argv(h)).unwrap());

    let runner = RecordingRunner::new();
    let setter = RecordingSetter::new();
    let platform = Platform {
        setter: Arc::new(setter.clone()),
        signals: Arc::new(ManualSignals::new(Theme::Light)),
    };
    let deps = Deps::new(platform, reqwest::Client::new())
        .with_sources(kakemono_sources::SourceSet::default())
        .with_runner(Arc::new(runner.clone()));
    let engine = Engine::new(cfg, State::new(), root.join("state.json"), deps).unwrap();
    Fixture {
        _dir: dir,
        root,
        runner,
        setter,
        engine,
    }
}

/// Polls until `runner` has recorded `n` runs (the hook is spawned, not awaited).
async fn wait_for_runs(runner: &RecordingRunner, n: usize) -> Vec<Invocation> {
    for _ in 0..400 {
        let runs = runner.invocations();
        if runs.len() >= n {
            return runs;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("expected {n} hook runs, got {:?}", runner.invocations());
}

/// Lets spawned hook tasks run (they record their run on their first poll), then returns
/// what was recorded.
async fn settle(runner: &RecordingRunner) -> Vec<Invocation> {
    crate::common::settle().await;
    runner.invocations()
}

fn env_map(inv: &Invocation) -> HashMap<String, String> {
    inv.env.iter().cloned().collect()
}

/// An absolute path on the host.
#[cfg(not(windows))]
const ABSOLUTE_PATH: &str = "/tmp/a.png";
#[cfg(windows)]
const ABSOLUTE_PATH: &str = r"C:\tmp\a.png";

/// An absolute path on the host with spaces and shell syntax, and its `file://` URI.
#[cfg(not(windows))]
const SPECIAL_PATH: (&str, &str) = (
    "/tmp/my pics/a b;$(x).png",
    "file:///tmp/my%20pics/a%20b;$(x).png",
);
#[cfg(windows)]
const SPECIAL_PATH: (&str, &str) = (
    r"C:\tmp\my pics\a b;$(x).png",
    "file:///C:/tmp/my%20pics/a%20b;$(x).png",
);

fn info(path: &str) -> WallpaperInfo {
    WallpaperInfo {
        path: path.to_owned(),
        source: "bing".to_owned(),
        title: "Title with \"quotes\" and $(rm -rf ~)".to_owned(),
        author: "Ann Author".to_owned(),
        source_url: "https://example.test/page?a=1&b=2".to_owned(),
        ..WallpaperInfo::default()
    }
}

// ─── events ───────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn hooks_events_broadcast_from_every_changing_op() {
    let f = fixture(None);
    let mut events = f.engine.subscribe();
    let a = f.image("a.png");

    let first = f.engine.next().await.unwrap();
    assert_eq!(
        events.try_recv().unwrap(),
        EngineEvent::Changed(first.clone())
    );

    let set = f.engine.set_file(&a).await.unwrap();
    assert_eq!(
        events.try_recv().unwrap(),
        EngineEvent::Changed(set.clone())
    );

    let back = f.engine.prev().await.unwrap();
    assert_eq!(back, first);
    assert_eq!(events.try_recv().unwrap(), EngineEvent::Changed(first));

    // `next` after `prev` steps forward through history.
    let fwd = f.engine.next().await.unwrap();
    assert_eq!(fwd, set);
    assert_eq!(events.try_recv().unwrap(), EngineEvent::Changed(set));

    f.engine.pause();
    assert_eq!(events.try_recv().unwrap(), EngineEvent::Paused(true));
    f.engine.pause(); // no flip, no event
    f.engine.resume();
    assert_eq!(events.try_recv().unwrap(), EngineEvent::Paused(false));

    // `reapply` changes nothing the user sees: no event.
    f.engine.reapply().await.unwrap();
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn hooks_events_trash_sends_changed_for_the_next_wallpaper() {
    let f = fixture(None);
    let a = f.image("a.png");
    f.engine.set_file(&a).await.unwrap();
    let mut events = f.engine.subscribe();

    let next = f.engine.trash().await.unwrap();

    assert_eq!(events.try_recv().unwrap(), EngineEvent::Changed(next));
    assert!(events.try_recv().is_err());
}

#[test]
fn hooks_events_map_onto_the_subscribe_protocol() {
    let wp = info("/tmp/a.png");
    let cases = [
        (
            EngineEvent::Changed(wp.clone()),
            serde_json::json!({"event": "changed", "data": serde_json::to_value(&wp).unwrap()}),
        ),
        (
            EngineEvent::Paused(true),
            serde_json::json!({"event": "paused", "data": true}),
        ),
        (
            EngineEvent::ThemeChanged(Theme::Dark),
            serde_json::json!({"event": "theme_changed", "data": "dark"}),
        ),
        (
            EngineEvent::ThemeChanged(Theme::Light),
            serde_json::json!({"event": "theme_changed", "data": "light"}),
        ),
    ];
    for (event, json) in cases {
        let wire = Event::from(event.clone());
        assert_eq!(serde_json::to_value(&wire).unwrap(), json, "{event:?}");
        // And the client side decodes it back to the same protocol event.
        let decoded: Event = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, wire);
    }
}

// ─── invocation (pure) ────────────────────────────────────────────────────────────────────

#[test]
fn hooks_invocation_renders_placeholders_without_splitting() {
    let spec = argv(&["hook", "{path}", "{uri}", "--fit={fit}", "{theme}", "{}"]);
    let (path, uri) = SPECIAL_PATH;
    let inv = hooks::invocation(&spec, &info(path), Theme::Dark, Fit::Center, |_| None).unwrap();

    assert_eq!(
        inv.argv,
        argv(&["hook", path, uri, "--fit=center", "dark", "{}"])
    );
    assert_eq!(inv.timeout, HOOK_TIMEOUT);
    assert_eq!(HOOK_TIMEOUT, Duration::from_secs(10));
    assert!(inv.clear_env);
}

#[test]
fn hooks_invocation_env_is_cleared_with_pass_through_and_kakemono_vars() {
    let parent: HashMap<&str, &str> = [
        ("HOME", "/home/test"),
        ("PATH", "/usr/bin:/bin"),
        ("WPD_SECRET_TEST", "hunter2"),
        ("AWS_SECRET_ACCESS_KEY", "nope"),
    ]
    .into();
    let wp = info(ABSOLUTE_PATH);
    let inv = hooks::invocation(&argv(&["hook"]), &wp, Theme::Light, Fit::Fill, |name| {
        parent.get(name).map(|v| (*v).to_owned())
    })
    .unwrap();

    assert!(inv.clear_env);
    let env = env_map(&inv);
    assert_eq!(
        env,
        HashMap::from(
            [
                ("HOME", "/home/test"),
                ("PATH", "/usr/bin:/bin"),
                ("KAKEMONO_PATH", ABSOLUTE_PATH),
                ("KAKEMONO_SOURCE", "bing"),
                ("KAKEMONO_TITLE", "Title with \"quotes\" and $(rm -rf ~)"),
                ("KAKEMONO_AUTHOR", "Ann Author"),
                ("KAKEMONO_SOURCE_URL", "https://example.test/page?a=1&b=2"),
                ("KAKEMONO_THEME", "light"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
        )
    );
    assert!(!env.contains_key("WPD_SECRET_TEST"));
}

#[test]
fn hooks_invocation_makes_a_relative_path_absolute() {
    let inv = hooks::invocation(
        &argv(&["hook", "{path}"]),
        &info("rel/a.png"),
        Theme::Light,
        Fit::Fill,
        |_| None,
    )
    .unwrap();
    let expected = std::env::current_dir().unwrap().join("rel").join("a.png");
    assert_eq!(inv.argv[1], expected.to_str().unwrap());
    assert_eq!(env_map(&inv)["KAKEMONO_PATH"], expected.to_str().unwrap());
}

#[test]
fn hooks_invocation_rejects_an_unknown_placeholder() {
    let err = hooks::invocation(
        &argv(&["hook", "{secret}"]),
        &info("/tmp/a.png"),
        Theme::Light,
        Fit::Fill,
        |_| None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("secret"), "{err}");
}

// ─── on_change through the engine ─────────────────────────────────────────────────────────

#[tokio::test]
async fn hooks_next_runs_on_change_with_argv_and_env() {
    let f = fixture(Some(&[
        "sketchybar",
        "--trigger",
        "wallpaper_changed",
        "WALLPAPER={path}",
    ]));

    let wp = f.engine.next().await.unwrap();

    let runs = wait_for_runs(&f.runner, 1).await;
    assert_eq!(runs.len(), 1);
    let inv = &runs[0];
    assert_eq!(
        inv.argv,
        vec![
            "sketchybar".to_owned(),
            "--trigger".to_owned(),
            "wallpaper_changed".to_owned(),
            format!("WALLPAPER={}", wp.path),
        ]
    );
    assert!(inv.clear_env);
    assert_eq!(inv.timeout, HOOK_TIMEOUT);

    let env = env_map(inv);
    assert_eq!(env["KAKEMONO_PATH"], wp.path);
    assert_eq!(env["KAKEMONO_SOURCE"], "folder");
    assert_eq!(env["KAKEMONO_TITLE"], wp.title);
    assert_eq!(env["KAKEMONO_THEME"], "light");
    // HOME comes from the test process (passed through); nothing else does.
    match std::env::var("HOME") {
        Ok(home) => assert_eq!(env.get("HOME"), Some(&home)),
        Err(_) => assert!(!env.contains_key("HOME")),
    }
    for key in env.keys() {
        assert!(
            key.starts_with("KAKEMONO_") || PASS_THROUGH_ENV.contains(&key.as_str()),
            "unexpected env var {key}"
        );
    }
    assert!(!env.contains_key("WPD_SECRET_TEST"));
    assert!(!env.contains_key("CARGO_PKG_NAME"), "parent env leaked");
}

#[tokio::test]
async fn hooks_not_configured_runs_nothing() {
    let f = fixture(None);
    f.engine.next().await.unwrap();
    assert!(settle(&f.runner).await.is_empty());
}

#[tokio::test]
async fn hooks_reapply_and_pause_do_not_run_on_change() {
    let f = fixture(Some(&["hook"]));
    f.engine.next().await.unwrap();
    wait_for_runs(&f.runner, 1).await;

    f.engine.reapply().await.unwrap();
    f.engine.pause();
    f.engine.resume();

    assert_eq!(settle(&f.runner).await.len(), 1);
}

#[tokio::test]
async fn hooks_failure_is_logged_and_never_blocks_the_change() {
    let f = fixture(Some(&["hook", "{path}"]));
    f.runner.push_output(Output::failure(3, "boom"));
    f.runner.push_spawn_error("no such file");
    let a = f.image("a.png");

    let first = f.engine.next().await.unwrap();
    wait_for_runs(&f.runner, 1).await;
    let second = f.engine.set_file(&a).await.unwrap();
    wait_for_runs(&f.runner, 2).await;

    // Both changes happened and were recorded despite the failing hooks.
    assert_eq!(
        f.setter.calls(),
        vec![
            (PathBuf::from(&first.path), Fit::Center),
            (a.clone(), Fit::Center)
        ]
    );
    assert_eq!(f.engine.status().current, Some(second));

    // Failed runs released their slots: later changes still run the hook.
    f.engine.prev().await.unwrap();
    wait_for_runs(&f.runner, 3).await;
}

#[tokio::test]
async fn hooks_unknown_placeholder_skips_the_hook_but_not_the_change() {
    let f = fixture(Some(&["hook", "{nope}"]));
    let wp = f.engine.next().await.unwrap();
    assert_eq!(f.engine.status().current, Some(wp));
    assert!(settle(&f.runner).await.is_empty());
}

#[tokio::test]
async fn hooks_change_does_not_wait_for_a_slow_hook() {
    let f = fixture(Some(&["hook"]));
    f.runner.set_delay(Duration::from_secs(60));

    let started = std::time::Instant::now();
    f.engine.next().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    wait_for_runs(&f.runner, 1).await;
    assert_eq!(f.runner.in_flight(), 1);
}

#[tokio::test]
async fn hooks_at_most_four_in_flight_and_the_rest_dropped() {
    let f = fixture(Some(&["hook", "{path}"]));
    f.runner.set_delay(Duration::from_secs(60));
    let images: Vec<PathBuf> = (0..4).map(|i| f.image(&format!("{i}.png"))).collect();

    // Five quick changes: `next` plus four `set`s.
    f.engine.next().await.unwrap();
    for img in &images {
        f.engine.set_file(img).await.unwrap();
    }

    let runs = wait_for_runs(&f.runner, MAX_CONCURRENT_HOOKS).await;
    let runs_after = settle(&f.runner).await;
    assert_eq!(runs_after.len(), MAX_CONCURRENT_HOOKS, "{runs:?}");
    assert_eq!(MAX_CONCURRENT_HOOKS, 4);
    assert!(f.runner.max_in_flight() <= 4);
    assert_eq!(f.runner.in_flight(), 4);
    // The fifth (last) change's run was the one dropped.
    let last = images.last().unwrap();
    assert!(
        runs_after
            .iter()
            .all(|r| r.argv[1] != last.to_str().unwrap()),
        "{runs_after:?}"
    );
    // Every change still happened.
    assert_eq!(f.setter.calls().len(), 5);
    assert_eq!(
        f.engine.status().current.map(|c| c.path),
        Some(Path::new(last).to_string_lossy().into_owned())
    );
}
