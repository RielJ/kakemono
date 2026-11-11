//! The rotation loop (`Engine::run`) on tokio's paused clock, with `RecordingSetter`,
//! `ManualSignals` and a local folder source (internet off, so the downloader stays idle). The wall clock handed to the
//! engine follows tokio's clock plus a test-controlled skew, which stands in for a machine that
//! slept (its monotonic clock stopped while the wall clock moved on).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use chrono::{DateTime, TimeDelta};
use kakemono_config::Config;
use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine, EngineEvent};
use kakemono_platform::api::{Platform, Signal, Theme};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_protocol::types::WallpaperInfo;
use kakemono_sources::SourceSet;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

const MIN: Duration = Duration::from_secs(60);
const BASE: &str = "2026-03-04T05:06:07+08:00";

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    setter: RecordingSetter,
    signals: ManualSignals,
    /// The one image in the `pics` folder source.
    day: PathBuf,
    /// Seconds added to the engine's wall clock on top of tokio's clock.
    skew: Arc<AtomicI64>,
    engine: Engine,
    start: Instant,
}

impl Fixture {
    /// An engine whose only source is the folder `pics` holding `day.png`.
    fn new(edit: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let pics = root.join("pics");
        std::fs::create_dir(&pics).unwrap();
        let day = pics.join("day.png");
        std::fs::write(&day, b"day").unwrap();

        let mut cfg = Config::default();
        cfg.daemon.download_dir = root.join("downloads");
        cfg.daemon.trash_dir = root.join("trash");
        cfg.favorites.dir = root.join("favorites");
        cfg.daemon.interval = "30m".to_owned();
        cfg.daemon.internet = false;
        cfg.sources = vec![SourceEntry {
            id: "pics".to_owned(),
            enabled: true,
            kind: SourceType::Folder,
            location: pics.to_string_lossy().into_owned(),
            ..Default::default()
        }];
        edit(&mut cfg);

        let setter = RecordingSetter::new();
        let signals = ManualSignals::new(Theme::Light);
        let platform = Platform {
            setter: Arc::new(setter.clone()),
            signals: Arc::new(signals.clone()),
        };
        let start = Instant::now();
        let skew = Arc::new(AtomicI64::new(0));
        let base = DateTime::parse_from_rfc3339(BASE).unwrap();
        let clock_skew = Arc::clone(&skew);
        let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
            .with_sources(SourceSet::default())
            .with_clock(move || {
                let elapsed = TimeDelta::from_std(Instant::now() - start).unwrap();
                base + elapsed + TimeDelta::seconds(clock_skew.load(Ordering::SeqCst))
            });
        let engine = Engine::new(cfg, State::new(), root.join("state.json"), deps).unwrap();
        Fixture {
            _dir: dir,
            root,
            setter,
            signals,
            day,
            skew,
            engine,
            start,
        }
    }

    /// Spawns `run` and lets it subscribe before returning.
    async fn run(&self) -> Running {
        let shutdown = CancellationToken::new();
        let engine = self.engine.clone();
        let token = shutdown.clone();
        let handle = tokio::spawn(async move { engine.run(token).await });
        // Current-thread runtime: one yield runs the new task up to its first await,
        // which comes after it subscribed to signals and events.
        tokio::task::yield_now().await;
        Running { shutdown, handle }
    }

    fn image(&self, name: &str) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, b"image").unwrap();
        p
    }

    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Moves the engine's wall clock forward without moving tokio's clock (a sleep).
    fn sleep_machine(&self, by: Duration) {
        self.skew
            .fetch_add(i64::try_from(by.as_secs()).unwrap(), Ordering::SeqCst);
    }

    fn set_paths(&self) -> Vec<PathBuf> {
        self.setter.calls().into_iter().map(|(p, _)| p).collect()
    }
}

struct Running {
    shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

impl Running {
    async fn stop(self) {
        self.shutdown.cancel();
        timeout(Duration::from_secs(1), self.handle)
            .await
            .expect("run did not stop on shutdown")
            .unwrap();
    }
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

/// Polls `cond` every virtual millisecond for up to one virtual second.
async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("timed out waiting for {what}");
}

fn assert_near(actual: Duration, expected: Duration) {
    assert!(
        actual >= expected && actual < expected + Duration::from_secs(1),
        "elapsed {actual:?}, expected {expected:?}"
    );
}

// ─── timer ────────────────────────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn ticks_every_interval() {
    let f = Fixture::new(|_| {});
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    assert_eq!(
        changed_within(&mut rx, 30 * MIN - Duration::from_secs(1)).await,
        None
    );
    let info = changed_within(&mut rx, MIN).await.expect("tick at 30m");
    assert_near(f.elapsed(), 30 * MIN);
    assert_eq!(info.source, "folder");
    assert_eq!(PathBuf::from(&info.path), f.day);

    changed_within(&mut rx, 31 * MIN)
        .await
        .expect("tick at 60m");
    assert_near(f.elapsed(), 60 * MIN);
    assert_eq!(f.engine.status().history_count, 2);
    run.stop().await;
}

/// A second `run` while one is running returns at once: two loops would race on the
/// single-waiter notifies and run two downloaders.
#[tokio::test(start_paused = true)]
async fn a_second_run_returns_at_once() {
    let f = Fixture::new(|_| {});
    let mut rx = f.engine.subscribe();
    let run = f.run().await;
    timeout(
        Duration::from_secs(1),
        f.engine.run(CancellationToken::new()),
    )
    .await
    .expect("a second run must return at once");
    changed_within(&mut rx, 31 * MIN)
        .await
        .expect("the first loop still ticks");
    assert_eq!(f.engine.status().history_count, 1, "one loop, one tick");
    run.stop().await;
    // After the first loop stopped, `run` works again.
    let again = f.run().await;
    again.stop().await;
}

#[tokio::test(start_paused = true)]
async fn manual_next_resets_the_timer() {
    let f = Fixture::new(|_| {});
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    tokio::time::sleep(20 * MIN).await;
    f.engine.next().await.unwrap();
    assert!(changed_within(&mut rx, MIN).await.is_some(), "manual next");

    // Not at 30m: the interval restarted at 20m.
    assert_eq!(changed_within(&mut rx, 29 * MIN).await, None);
    changed_within(&mut rx, 2 * MIN).await.expect("tick at 50m");
    assert_near(f.elapsed(), 50 * MIN);
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn pause_suppresses_ticks_persists_and_resume_keeps_the_cadence() {
    let f = Fixture::new(|_| {});
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    f.engine.pause();
    f.engine.save_state().await;
    assert!(State::load(&f.root.join("state.json")).unwrap().paused());
    assert_eq!(changed_within(&mut rx, 135 * MIN).await, None);
    assert!(f.setter.calls().is_empty());

    // Resumed at 2h15m: the next tick is on the 30m cadence (2h30m), as with Go's ticker.
    f.engine.resume();
    f.engine.save_state().await;
    assert!(!State::load(&f.root.join("state.json")).unwrap().paused());
    assert_eq!(
        changed_within(&mut rx, 15 * MIN - Duration::from_secs(1)).await,
        None
    );
    changed_within(&mut rx, MIN).await.expect("tick at 2h30m");
    assert_near(f.elapsed(), 150 * MIN);
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn first_tick_counts_from_last_changed() {
    let f = Fixture::new(|_| {});
    f.engine.set_file(&f.image("a.png")).await.unwrap();
    tokio::time::sleep(25 * MIN).await; // daemon down for 25m
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    assert_eq!(
        changed_within(&mut rx, 5 * MIN - Duration::from_secs(1)).await,
        None
    );
    changed_within(&mut rx, MIN)
        .await
        .expect("tick 30m after last change");
    assert_near(f.elapsed(), 30 * MIN);
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn overdue_at_startup_rotates_at_once() {
    let f = Fixture::new(|_| {});
    f.engine.set_file(&f.image("a.png")).await.unwrap();
    tokio::time::sleep(3 * 60 * MIN).await;
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    let info = changed_within(&mut rx, Duration::from_secs(1))
        .await
        .expect("immediate tick");
    assert_eq!(info.source, "folder");
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn reload_applies_a_new_interval() {
    let f = Fixture::new(|_| {});
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    tokio::time::sleep(10 * MIN).await;
    let mut cfg = (*f.engine.config()).clone();
    cfg.daemon.interval = "5m".to_owned();
    f.engine.reload(cfg).await.unwrap();

    // 10m have passed since start, more than the new 5m: rotate now, then every 5m.
    changed_within(&mut rx, Duration::from_secs(1))
        .await
        .expect("overdue after reload");
    assert_near(f.elapsed(), 10 * MIN);
    changed_within(&mut rx, 6 * MIN).await.expect("tick at 15m");
    assert_near(f.elapsed(), 15 * MIN);
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn failed_tick_retries_one_interval_later() {
    let f = Fixture::new(|_| {});
    f.setter.fail_next(usize::MAX);
    let run = f.run().await;

    tokio::time::sleep(30 * MIN + Duration::from_secs(1)).await;
    assert_eq!(f.setter.attempts(), 1);
    tokio::time::sleep(29 * MIN).await;
    assert_eq!(f.setter.attempts(), 1, "no retry before the next interval");
    tokio::time::sleep(MIN).await;
    assert_eq!(f.setter.attempts(), 2);
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn rotation_disabled_never_ticks_but_shutdown_stops() {
    let f = Fixture::new(|c| c.daemon.rotation_enabled = false);
    let mut rx = f.engine.subscribe();
    let run = f.run().await;
    assert_eq!(changed_within(&mut rx, 5 * 60 * MIN).await, None);
    assert_eq!(f.setter.attempts(), 0);
    run.stop().await;
}

// ─── signals ──────────────────────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn theme_changed_to_dark_rotates() {
    let f = Fixture::new(|_| {});
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    f.signals.set_theme(Theme::Dark);
    assert_eq!(
        timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        EngineEvent::ThemeChanged(Theme::Dark)
    );
    let info = changed_within(&mut rx, Duration::from_secs(1))
        .await
        .expect("rotation on theme change");
    assert_eq!(info.source, "folder");

    // The rotation restarted the interval.
    assert_eq!(
        changed_within(&mut rx, 30 * MIN - Duration::from_secs(1)).await,
        None
    );
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn theme_change_does_not_rotate_when_disabled_or_paused() {
    for (rotate_on_theme_change, paused) in [(false, false), (true, true)] {
        let f = Fixture::new(|c| c.daemon.rotate_on_theme_change = rotate_on_theme_change);
        if paused {
            f.engine.pause();
        }
        let mut rx = f.engine.subscribe();
        let run = f.run().await;

        f.signals.set_theme(Theme::Dark);
        assert_eq!(
            timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap()
                .unwrap(),
            EngineEvent::ThemeChanged(Theme::Dark)
        );
        assert_eq!(changed_within(&mut rx, MIN).await, None);
        assert_eq!(f.setter.attempts(), 0);
        run.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn theme_changed_to_the_known_theme_is_ignored() {
    let f = Fixture::new(|_| {});
    let a = f.image("a.png");
    f.engine.set_file(&a).await.unwrap();
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    f.signals.emit(Signal::ThemeChanged(Theme::Light)); // already light
    f.signals.emit(Signal::DisplaysChanged); // sentinel: handled after the theme signal
    until("display re-apply", || f.setter.calls().len() == 2).await;
    assert!(rx.try_recv().is_err(), "no ThemeChanged, no Changed");
    assert_eq!(f.engine.status().history_count, 1);
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn space_changed_is_ignored_by_default() {
    let f = Fixture::new(|_| {});
    assert!(!f.engine.config().daemon.reapply_on_space_change);
    let a = f.image("a.png");
    f.engine.set_file(&a).await.unwrap();
    let run = f.run().await;

    f.signals.emit(Signal::SpaceChanged);
    // Signals are handled in order: once the display change re-applied, the Space change
    // has been handled too.
    f.signals.emit(Signal::DisplaysChanged);
    until("display re-apply", || f.setter.calls().len() == 2).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(f.set_paths(), vec![a.clone(), a]);
    assert_eq!(f.engine.status().history_count, 1);
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn space_changed_reapplies_when_enabled_without_touching_history() {
    let f = Fixture::new(|c| c.daemon.reapply_on_space_change = true);
    let a = f.image("a.png");
    f.engine.set_file(&a).await.unwrap();
    let before = f.engine.state();
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    f.signals.emit(Signal::SpaceChanged);
    until("space re-apply", || f.setter.calls().len() == 2).await;
    assert_eq!(f.set_paths(), vec![a.clone(), a]);
    assert_eq!(f.engine.state(), before);
    assert!(rx.try_recv().is_err(), "re-apply sends no event");
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn resumed_after_an_overdue_tick_rotates() {
    let f = Fixture::new(|_| {});
    let a = f.image("a.png");
    f.engine.set_file(&a).await.unwrap();
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    tokio::time::sleep(5 * MIN).await;
    f.sleep_machine(2 * 60 * MIN);
    f.signals.emit(Signal::Resumed);
    let info = changed_within(&mut rx, Duration::from_secs(1))
        .await
        .expect("rotation on wake");
    assert_eq!(info.source, "folder");
    assert_near(f.elapsed(), 5 * MIN);
    assert_eq!(f.engine.status().history_count, 2);

    // The interval restarted at the wake-up rotation.
    assert_eq!(
        changed_within(&mut rx, 30 * MIN - Duration::from_secs(1)).await,
        None
    );
    changed_within(&mut rx, MIN)
        .await
        .expect("tick 30m after wake");
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn resumed_before_the_tick_only_reapplies() {
    let f = Fixture::new(|_| {});
    let a = f.image("a.png");
    f.engine.set_file(&a).await.unwrap();
    let mut rx = f.engine.subscribe();
    let run = f.run().await;

    f.sleep_machine(10 * MIN);
    f.signals.emit(Signal::Resumed);
    until("wake re-apply", || f.setter.calls().len() == 2).await;
    assert_eq!(f.set_paths(), vec![a.clone(), a]);
    assert!(rx.try_recv().is_err());

    // 10m of the interval passed while asleep: the tick comes 20m later.
    assert_eq!(
        changed_within(&mut rx, 20 * MIN - Duration::from_secs(2)).await,
        None
    );
    changed_within(&mut rx, MIN).await.expect("tick");
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn resumed_while_paused_only_reapplies() {
    let f = Fixture::new(|_| {});
    let a = f.image("a.png");
    f.engine.set_file(&a).await.unwrap();
    f.engine.pause();
    let run = f.run().await;

    f.sleep_machine(3 * 60 * MIN);
    f.signals.emit(Signal::Resumed);
    until("wake re-apply", || f.setter.calls().len() == 2).await;
    assert_eq!(f.engine.status().history_count, 1);
    run.stop().await;
}

#[tokio::test(start_paused = true)]
async fn displays_changed_reapplies_and_without_current_does_nothing() {
    let f = Fixture::new(|_| {});
    let run = f.run().await;

    f.signals.emit(Signal::DisplaysChanged);
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(f.setter.calls().is_empty(), "no current wallpaper yet");

    let a = f.image("a.png");
    f.engine.set_file(&a).await.unwrap();
    f.signals.emit(Signal::DisplaysChanged);
    until("display re-apply", || f.setter.calls().len() == 2).await;
    assert_eq!(f.set_paths(), vec![a.clone(), a]);
    run.stop().await;
}
