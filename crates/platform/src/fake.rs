//! Test doubles for the seam (behind the `fake` feature): `RecordingSetter`, `ArgvSetter` (a
//! setter that rebuilds from `[desktop]`) and `ManualSignals`.
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
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
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

/// What an [`ArgvSetter`] did, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgvEvent {
    /// A `set` that "ran" this argv (`{path}` and `{fit}` filled in).
    Ran(Vec<String>),
    /// A `rebuild` that replaced the setter running `from` with one running `to`.
    Rebuilt { from: Vec<String>, to: Vec<String> },
}

/// A command setter built from `[desktop] set_command` that records what it would run instead
/// of running it. `rebuild` builds a new one from the new `[desktop]` (sharing the log), so it
/// stands in for a platform whose setter depends on `[desktop]` (Linux). Clones share the log.
#[derive(Clone, Debug)]
pub struct ArgvSetter {
    argv: Vec<String>,
    log: Arc<Mutex<Vec<ArgvEvent>>>,
}

impl ArgvSetter {
    /// A setter for `desktop.set_command`. Fails with `Unconfigured` when there is none, like a
    /// `[desktop]` that does not resolve.
    pub fn from_desktop(
        desktop: &kakemono_config::schema::DesktopConfig,
    ) -> Result<Self, SetError> {
        Self::with_log(desktop, Arc::default())
    }

    fn with_log(
        desktop: &kakemono_config::schema::DesktopConfig,
        log: Arc<Mutex<Vec<ArgvEvent>>>,
    ) -> Result<Self, SetError> {
        let argv = desktop
            .set_command
            .as_ref()
            .map(|c| c.argv().to_vec())
            .ok_or_else(|| SetError::Unconfigured {
                hint: "`[desktop] set_command` is empty".to_owned(),
            })?;
        Ok(Self { argv, log })
    }

    /// The argv template this setter runs.
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// Everything recorded so far, by this setter and every setter rebuilt from it.
    pub fn events(&self) -> Vec<ArgvEvent> {
        lock(&self.log).clone()
    }

    /// The argvs every `set` ran, in order.
    pub fn runs(&self) -> Vec<Vec<String>> {
        lock(&self.log)
            .iter()
            .filter_map(|e| match e {
                ArgvEvent::Ran(argv) => Some(argv.clone()),
                ArgvEvent::Rebuilt { .. } => None,
            })
            .collect()
    }

    /// How many successful `rebuild`s there were.
    pub fn rebuilds(&self) -> usize {
        lock(&self.log)
            .iter()
            .filter(|e| matches!(e, ArgvEvent::Rebuilt { .. }))
            .count()
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for ArgvSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        let path = image.to_string_lossy();
        let argv = self
            .argv
            .iter()
            .map(|arg| arg.replace("{path}", &path).replace("{fit}", fit.as_str()))
            .collect();
        lock(&self.log).push(ArgvEvent::Ran(argv));
        Ok(())
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "command".to_owned(),
            detail: self.argv.join(" "),
        }
    }

    async fn rebuild(
        &self,
        desktop: &kakemono_config::schema::DesktopConfig,
    ) -> Result<Option<Arc<dyn WallpaperSetter>>, SetError> {
        let new = Self::with_log(desktop, Arc::clone(&self.log))?;
        lock(&self.log).push(ArgvEvent::Rebuilt {
            from: self.argv.clone(),
            to: new.argv.clone(),
        });
        Ok(Some(Arc::new(new)))
    }
}

/// Signals driven by the test. Clones share state.
#[derive(Clone, Debug)]
pub struct ManualSignals {
    theme: Arc<Mutex<Theme>>,
    screen: Arc<Mutex<Option<(u32, u32)>>>,
    tx: broadcast::Sender<Signal>,
}

impl ManualSignals {
    pub fn new(theme: Theme) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            theme: Arc::new(Mutex::new(theme)),
            screen: Arc::default(),
            tx,
        }
    }

    /// Set what `screen_size()` returns (`None` at first). Emits nothing; follow it with
    /// `emit(Signal::DisplaysChanged)` to model a display change.
    pub fn set_screen_size(&self, size: Option<(u32, u32)>) {
        *lock(&self.screen) = size;
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

    fn screen_size(&self) -> Option<(u32, u32)> {
        *lock(&self.screen)
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

    #[tokio::test]
    async fn argv_setter_records_runs_and_rebuilds() {
        use kakemono_config::schema::{CommandSpec, DesktopConfig};
        let desktop = |argv: &[&str]| DesktopConfig {
            set_command: Some(
                CommandSpec::new(argv.iter().map(|s| (*s).to_owned()).collect()).unwrap(),
            ),
            ..DesktopConfig::default()
        };
        let setter = ArgvSetter::from_desktop(&desktop(&["a", "{path}", "{fit}"])).unwrap();
        setter.set(Path::new("/x.jpg"), Fit::Fill).await.unwrap();
        let new = setter
            .rebuild(&desktop(&["b", "{path}"]))
            .await
            .unwrap()
            .unwrap();
        new.set(Path::new("/y.jpg"), Fit::Fill).await.unwrap();
        assert_eq!(new.info().detail, "b {path}");
        assert_eq!(
            setter.runs(),
            [vec!["a", "/x.jpg", "fill"], vec!["b", "/y.jpg"]]
        );
        assert_eq!(setter.rebuilds(), 1);

        let err = setter.rebuild(&DesktopConfig::default()).await;
        assert!(matches!(err, Err(SetError::Unconfigured { .. })));
        assert_eq!(setter.rebuilds(), 1, "a failed rebuild records nothing");
        assert!(ArgvSetter::from_desktop(&DesktopConfig::default()).is_err());
    }

    #[tokio::test]
    async fn setters_do_not_rebuild_by_default() {
        let setter = RecordingSetter::new();
        let rebuilt = setter
            .rebuild(&kakemono_config::schema::DesktopConfig::default())
            .await
            .unwrap();
        assert!(rebuilt.is_none());
    }

    #[test]
    fn manual_signals_screen_size_is_settable() {
        let signals = ManualSignals::default();
        let dyn_signals: Arc<dyn SystemSignals> = Arc::new(signals.clone());
        let mut rx = dyn_signals.subscribe();
        assert_eq!(dyn_signals.screen_size(), None);

        signals.set_screen_size(Some((5120, 2880)));
        assert_eq!(dyn_signals.screen_size(), Some((5120, 2880)));
        signals.set_screen_size(None);
        assert_eq!(dyn_signals.screen_size(), None);
        assert_eq!(
            rx.try_recv(),
            Err(TryRecvError::Empty),
            "setting the size emits nothing; emit DisplaysChanged explicitly"
        );
    }

    #[test]
    fn emit_without_subscribers_is_fine() {
        ManualSignals::default().emit(Signal::DisplaysChanged);
    }
}
