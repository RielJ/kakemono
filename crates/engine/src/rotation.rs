//! The rotation loop: `Engine::run(shutdown)` (plan §D). It rotates every `[daemon] interval`,
//! skips ticks while paused, and reacts to platform signals.
//!
//! # Timer
//!
//! The loop keeps an *anchor*: the wall time (`Deps.now`) at which the current interval
//! started. The next tick is due at `anchor + interval`. The anchor moves to "now":
//!
//! - after every wallpaper change (`EngineEvent::Changed`, so manual `next`/`prev`/`set`/`trash`
//!   reset the timer; Go's ticker did not reset);
//! - when a tick fires, whether it rotated, failed or was skipped because rotation is paused.
//!   A failed fetch retries one interval later, as Go's ticker did. A paused tick keeps the
//!   cadence, so after `resume` the next rotation comes at the next tick, as in Go.
//!
//! On start the anchor is `state.last_changed` (Go started a fresh 30 m ticker instead), so a
//! daemon that starts after an overdue tick rotates at once. With no `last_changed` the first
//! tick comes one interval after start, as in Go.
//!
//! With `[daemon] change_on_start`, `run` rotates once before the loop (reason `"start"`,
//! skipped while paused) and the anchor moves to now.
//!
//! # Downloader
//!
//! `run` also spawns the background downloader ([`crate::downloader::Downloader`]) under a
//! child of the same `CancellationToken`, woken by `reload`. On shutdown it stops the
//! downloader, waits for it, then saves the facts cache.
//!
//! Due times are wall-clock based and recomputed on every loop turn, so a sleeping machine
//! (whose monotonic clock may stop) catches up on the next wake-up, and `reload` applies a
//! new interval at once. The anchor never lies in the future: a wall clock that went back
//! moves it to now.
//!
//! `[daemon] rotation_enabled = false` turns the timer off (Go didn't start the ticker). An
//! unparsable or zero interval falls back to 30 m with a warning (Go: unparsable → 30 m; zero
//! made `time.NewTicker` panic).
//!
//! # Signals
//!
//! | signal | reaction |
//! |---|---|
//! | `ThemeChanged(t)` | emits `EngineEvent::ThemeChanged(t)`; if `rotate_on_theme_change` and not paused, `next()` (sources of the new theme) |
//! | `DisplaysChanged` | `reapply()` |
//! | `SpaceChanged` | `reapply()` only if `reapply_on_space_change` (default off: re-setting clears "Show on all Spaces") |
//! | `Resumed` | if a tick is overdue, rotation is enabled and not paused: rotate like a tick; otherwise `reapply()` |
//!
//! A `ThemeChanged` for the theme the loop already knows is ignored. If the signal receiver
//! lags, the loop compares `signals.theme()` with the known theme so a missed theme flip still
//! rotates. Op errors are logged at `warn`; the loop keeps running.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use kakemono_config::Config;
use kakemono_platform::api::{Signal, Theme};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::downloader::Downloader;
use crate::engine::Engine;
use crate::events::EngineEvent;
use crate::interval::IntervalSetting;

/// Holds the engine's `running` flag; clears it on drop (also when `run` is cancelled).
struct RunGuard<'a>(&'a AtomicBool);

impl<'a> RunGuard<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Interval used when `[daemon] interval` is unparsable or zero (Go's fallback).
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(30 * 60);

impl Engine {
    /// Runs the rotation timer, the signal reactions and the background downloader until
    /// `shutdown` is cancelled, then saves the facts cache and the state. An op in flight when
    /// `shutdown` fires is dropped. One loop per engine: the reset and wake notifies wake a single
    /// waiter, so a call while another loop runs logs a warning and returns at once.
    pub async fn run(&self, shutdown: CancellationToken) {
        let Some(_running) = RunGuard::acquire(&self.inner.running) else {
            tracing::warn!("the engine loop is already running; not starting a second one");
            return;
        };
        let stop_downloader = shutdown.child_token();
        let downloader = {
            let engine = self.clone();
            let (token, wake) = (stop_downloader.clone(), self.downloader_wake());
            tokio::spawn(async move { Downloader::new().run(&engine, token, wake).await })
        };
        self.rotation_loop(&shutdown).await;
        stop_downloader.cancel();
        if let Err(e) = downloader.await {
            tracing::warn!(error = %e, "the downloader task failed");
        }
        self.save_facts().await;
        self.save_state().await;
    }

    /// The rotation timer and the signal reactions (see the module docs).
    async fn rotation_loop(&self, shutdown: &CancellationToken) {
        let shutdown = shutdown.clone();
        let mut signals = self.platform().signals.subscribe();
        let mut events = self.subscribe();
        let mut signals_open = true;
        let mut events_open = true;
        let mut rot = Rotation::start(self);
        tracing::info!(
            interval = ?rot.interval(&self.config()),
            "rotation loop started"
        );
        if self.config().daemon.change_on_start {
            if self.is_paused() {
                tracing::info!("auto-rotation paused; not changing the wallpaper on start");
            } else if !self.rotate("start", &shutdown).await {
                tracing::info!("rotation loop stopped");
                return;
            }
            rot.anchor = self.now();
        }

        loop {
            let wait = rot.wait(self);
            let timer = async move {
                match wait {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending().await,
                }
            };
            let keep_going = tokio::select! {
                biased;
                () = shutdown.cancelled() => false,
                event = events.recv(), if events_open => {
                    match event {
                        Ok(EngineEvent::Changed(_)) | Err(RecvError::Lagged(_)) => {
                            rot.anchor = self.now();
                        }
                        Ok(EngineEvent::Paused(_) | EngineEvent::ThemeChanged(_)) => {}
                        Err(RecvError::Closed) => events_open = false,
                    }
                    true
                }
                // Pause flips and reloads: the next turn recomputes the due time.
                () = self.inner.reset.notified() => true,
                signal = signals.recv(), if signals_open => match signal {
                    Ok(signal) => self.on_signal(signal, &mut rot, &shutdown).await,
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "missed platform signals");
                        let theme = self.theme();
                        if theme == rot.theme {
                            true
                        } else {
                            self.on_signal(Signal::ThemeChanged(theme), &mut rot, &shutdown).await
                        }
                    }
                    Err(RecvError::Closed) => {
                        tracing::info!("platform signals ended; the timer keeps running");
                        signals_open = false;
                        true
                    }
                },
                () = timer => self.tick(&mut rot, &shutdown).await,
            };
            if !keep_going {
                break;
            }
        }
        tracing::info!("rotation loop stopped");
    }

    /// Reacts to one platform signal. Returns `false` if `shutdown` fired meanwhile.
    async fn on_signal(
        &self,
        signal: Signal,
        rot: &mut Rotation,
        shutdown: &CancellationToken,
    ) -> bool {
        let cfg = self.config();
        match signal {
            Signal::ThemeChanged(theme) => {
                if theme == rot.theme {
                    return true;
                }
                rot.theme = theme;
                tracing::info!(%theme, "system theme changed");
                self.emit(EngineEvent::ThemeChanged(theme));
                if !cfg.daemon.rotate_on_theme_change {
                    true
                } else if self.is_paused() {
                    tracing::info!("auto-rotation paused; not rotating for the theme change");
                    true
                } else {
                    self.rotate("theme change", shutdown).await
                }
            }
            Signal::DisplaysChanged => self.reapply_logged("display change", shutdown).await,
            Signal::SpaceChanged => {
                if cfg.daemon.reapply_on_space_change {
                    self.reapply_logged("space change", shutdown).await
                } else {
                    true
                }
            }
            Signal::Resumed => {
                let overdue = rot
                    .interval(&cfg)
                    .is_some_and(|interval| rot.remaining(self.now(), interval).is_zero());
                if overdue && !self.is_paused() {
                    tracing::info!("woke after an overdue tick");
                    self.tick(rot, shutdown).await
                } else {
                    self.reapply_logged("wake", shutdown).await
                }
            }
        }
    }

    /// A tick: rotate unless paused. The next interval starts now either way, so a failed
    /// fetch retries one interval later instead of spinning.
    async fn tick(&self, rot: &mut Rotation, shutdown: &CancellationToken) -> bool {
        rot.anchor = self.now();
        if self.is_paused() {
            tracing::debug!("auto-rotation paused; tick skipped");
            return true;
        }
        self.rotate("auto-rotate", shutdown).await
    }

    /// `next()`, logging the outcome. Returns `false` if `shutdown` fired first.
    async fn rotate(&self, why: &str, shutdown: &CancellationToken) -> bool {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => false,
            result = self.next() => {
                match result {
                    Ok(info) => tracing::info!(why, path = %info.path, source = %info.source, "rotated"),
                    Err(e) => tracing::warn!(why, error = %e, "rotation failed"),
                }
                true
            }
        }
    }

    /// `reapply()`, logging failures. Returns `false` if `shutdown` fired first.
    async fn reapply_logged(&self, why: &str, shutdown: &CancellationToken) -> bool {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => false,
            result = self.reapply() => {
                match result {
                    Ok(Some(info)) => tracing::debug!(why, path = %info.path, "re-applied wallpaper"),
                    Ok(None) => {}
                    Err(e) => tracing::warn!(why, error = %e, "re-applying the wallpaper failed"),
                }
                true
            }
        }
    }

    fn now(&self) -> DateTime<FixedOffset> {
        (self.inner.deps.now)()
    }
}

/// Loop-local timer state.
struct Rotation {
    /// Wall time at which the current interval started.
    anchor: DateTime<FixedOffset>,
    /// The theme the loop last acted on.
    theme: Theme,
    /// `[daemon] interval`, warned once per bad value.
    interval: IntervalSetting,
}

impl Rotation {
    fn start(engine: &Engine) -> Self {
        let now = engine.now();
        Self {
            anchor: engine.state().last_changed().unwrap_or(now),
            theme: engine.theme(),
            interval: IntervalSetting::default(),
        }
    }

    /// How long until the next tick; `None` when rotation is disabled. Moves an anchor in the
    /// future (the wall clock went back) to now.
    fn wait(&mut self, engine: &Engine) -> Option<Duration> {
        let interval = self.interval(&engine.config())?;
        let now = engine.now();
        if self.anchor > now {
            self.anchor = now;
        }
        Some(self.remaining(now, interval))
    }

    /// `anchor + interval - now`, clamped to `0..=interval`.
    fn remaining(&self, now: DateTime<FixedOffset>, interval: Duration) -> Duration {
        match (now - self.anchor).to_std() {
            Ok(elapsed) => interval.saturating_sub(elapsed),
            Err(_) => interval, // anchor in the future
        }
    }

    /// The configured interval, or `None` when rotation is disabled.
    fn interval(&mut self, cfg: &Config) -> Option<Duration> {
        cfg.daemon.rotation_enabled.then(|| {
            self.interval.resolve(
                "interval",
                &cfg.daemon.interval,
                cfg.daemon.interval_duration(),
                DEFAULT_INTERVAL,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn rot(anchor: &str) -> Rotation {
        Rotation {
            anchor: at(anchor),
            theme: Theme::Light,
            interval: IntervalSetting::default(),
        }
    }

    #[test]
    fn remaining_is_clamped_to_the_interval() {
        let r = rot("2026-01-01T10:00:00Z");
        let i = Duration::from_secs(1800);
        assert_eq!(
            r.remaining(at("2026-01-01T10:10:00Z"), i),
            Duration::from_secs(1200)
        );
        assert_eq!(r.remaining(at("2026-01-01T12:00:00Z"), i), Duration::ZERO);
        assert_eq!(r.remaining(at("2026-01-01T09:00:00Z"), i), i);
    }

    #[test]
    fn interval_falls_back_and_respects_rotation_enabled() {
        let mut r = rot("2026-01-01T10:00:00Z");
        let mut cfg = Config::default();
        cfg.daemon.interval = "5m".to_owned();
        assert_eq!(r.interval(&cfg), Some(Duration::from_secs(300)));
        for bad in ["nope", "0s"] {
            cfg.daemon.interval = bad.to_owned();
            assert_eq!(r.interval(&cfg), Some(DEFAULT_INTERVAL));
            assert_eq!(r.interval.warned(), Some(bad));
        }
        cfg.daemon.rotation_enabled = false;
        assert_eq!(r.interval(&cfg), None);
    }
}
