//! Test doubles for the seam (behind the `fake` feature): `RecordingSetter` and `ManualSignals`.
//!
//! ```
//! # async fn demo() {
//! use std::{path::Path, sync::Arc};
//! use kakemono_platform::fake::{ManualSignals, RecordingSetter};
//! use kakemono_platform::{Fit, Platform, Signal, Theme, WallpaperSetter};
//!
//! let setter = RecordingSetter::new();
//! let signals = ManualSignals::new(Theme::Light);
//! let platform = Platform { setter: Arc::new(setter.clone()), signals: Arc::new(signals.clone()) };
//!
//! let mut rx = platform.signals.subscribe();
//! signals.set_theme(Theme::Dark); // emits ThemeChanged(Dark)
//! assert_eq!(rx.recv().await.unwrap(), Signal::ThemeChanged(Theme::Dark));
//!
//! setter.fail_next(1);
//! assert!(platform.setter.set(Path::new("/a.jpg"), Fit::Fill).await.is_err());
//! platform.setter.set(Path::new("/a.jpg"), Fit::Fill).await.unwrap();
//! assert_eq!(setter.calls(), vec![("/a.jpg".into(), Fit::Fill)]);
//! # }
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::broadcast;

use crate::api::{BackendInfo, Fit, SetError, Signal, SystemSignals, Theme, WallpaperSetter};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Records every successful `set` call. Clones share state, so keep one clone for assertions.
#[derive(Clone, Debug, Default)]
pub struct RecordingSetter {
    /// Successful calls, in order.
    pub calls: Arc<Mutex<Vec<(PathBuf, Fit)>>>,
    fail_remaining: Arc<AtomicUsize>,
    attempts: Arc<AtomicUsize>,
}

impl RecordingSetter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the next `n` calls fail with `SetError::Os("injected failure")` (not recorded in `calls`).
    pub fn fail_next(&self, n: usize) {
        self.fail_remaining.store(n, Ordering::SeqCst);
    }

    /// A snapshot of the successful calls.
    pub fn calls(&self) -> Vec<(PathBuf, Fit)> {
        lock(&self.calls).clone()
    }

    /// The most recent successful call.
    pub fn last(&self) -> Option<(PathBuf, Fit)> {
        lock(&self.calls).last().cloned()
    }

    /// Every `set` call, including the injected failures.
    pub fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for RecordingSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let failed = self
            .fail_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if failed {
            return Err(SetError::Os("injected failure".to_owned()));
        }
        lock(&self.calls).push((image.to_path_buf(), fit));
        Ok(())
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "fake".to_owned(),
            detail: "RecordingSetter".to_owned(),
        }
    }
}

/// Signals driven by the test. Clones share state.
#[derive(Clone, Debug)]
pub struct ManualSignals {
    theme: Arc<Mutex<Theme>>,
    tx: broadcast::Sender<Signal>,
}

impl ManualSignals {
    pub fn new(theme: Theme) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            theme: Arc::new(Mutex::new(theme)),
            tx,
        }
    }

    /// Broadcast `signal` as-is. A `ThemeChanged` here does not update `theme()`; use `set_theme`.
    pub fn emit(&self, signal: Signal) {
        // No receivers is fine: the engine may not have subscribed yet.
        let _ = self.tx.send(signal);
    }

    /// Change the theme and emit `ThemeChanged` if it differs, as the real platforms do.
    pub fn set_theme(&self, theme: Theme) {
        let changed = std::mem::replace(&mut *lock(&self.theme), theme) != theme;
        if changed {
            self.emit(Signal::ThemeChanged(theme));
        }
    }
}

impl Default for ManualSignals {
    fn default() -> Self {
        Self::new(Theme::Light)
    }
}

impl SystemSignals for ManualSignals {
    fn theme(&self) -> Theme {
        *lock(&self.theme)
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;

    #[tokio::test]
    async fn recording_setter_records_and_fails_on_demand() {
        let setter = RecordingSetter::new();
        let dyn_setter: Arc<dyn WallpaperSetter> = Arc::new(setter.clone());

        dyn_setter
            .set(Path::new("/a.jpg"), Fit::Fill)
            .await
            .unwrap();
        setter.fail_next(2);
        for _ in 0..2 {
            let err = dyn_setter.set(Path::new("/b.jpg"), Fit::Fit).await;
            assert!(matches!(err, Err(SetError::Os(_))));
        }
        dyn_setter
            .set(Path::new("/c.jpg"), Fit::Tile)
            .await
            .unwrap();

        assert_eq!(
            setter.calls(),
            vec![
                (PathBuf::from("/a.jpg"), Fit::Fill),
                (PathBuf::from("/c.jpg"), Fit::Tile)
            ]
        );
        assert_eq!(setter.last(), Some((PathBuf::from("/c.jpg"), Fit::Tile)));
        assert_eq!(setter.attempts(), 4);
        assert_eq!(dyn_setter.info().name, "fake");
    }

    #[tokio::test]
    async fn manual_signals_emit_and_theme() {
        let signals = ManualSignals::new(Theme::Light);
        let dyn_signals: Arc<dyn SystemSignals> = Arc::new(signals.clone());
        let mut rx = dyn_signals.subscribe();

        signals.emit(Signal::SpaceChanged);
        signals.set_theme(Theme::Light); // unchanged: no event
        signals.set_theme(Theme::Dark);
        signals.emit(Signal::Resumed);

        assert_eq!(rx.recv().await.unwrap(), Signal::SpaceChanged);
        assert_eq!(rx.recv().await.unwrap(), Signal::ThemeChanged(Theme::Dark));
        assert_eq!(rx.recv().await.unwrap(), Signal::Resumed);
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(dyn_signals.theme(), Theme::Dark);
    }

    #[test]
    fn emit_without_subscribers_is_fine() {
        ManualSignals::default().emit(Signal::DisplaysChanged);
    }
}
