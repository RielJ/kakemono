//! `Engine`: the wallpaper ops (`next`, `prev`, `set_file`, `favorite`, `trash`, `pause`,
//! `resume`, `status`, `reload`) over history state, the local image pool, the online sources
//! and the platform seam. Port of Go `internal/wallpaper/manager.go`.
//!
//! # Changes pick from disk
//!
//! A change that isn't a step through history picks an image already on disk
//! ([`crate::pool`]): the background downloader ([`crate::downloader`], spawned by
//! [`Engine::run`]) keeps `<download_dir>/<id>/` filled for every online source. Only when the
//! pool is empty does `next` download one image on demand, and only with `[daemon] internet` on
//! and an online source enabled; otherwise it fails with [`EngineError::NoImages`].
//!
//! `WallpaperInfo.source` is the row's type string (`"bing"`, `"wallhaven"`, `"folder"`,
//! `"album-name"`, ...), `"manual"` for the manual folder and `"local"` for `set_file`. The row
//! id is logged at the change.
//!
//! # Locking
//!
//! - `Inner::op` (`tokio::sync::Mutex<()>`) serialises every op that sets the wallpaper or
//!   changes history. It is held across the pick, on-demand downloads and
//!   `WallpaperSetter::set`.
//! - `Inner::snap` (`std::sync::Mutex<Snapshot>`) holds the state, config, sources and filters.
//!   It is only held for short, non-async sections, so `status`, `pause`, `resume` and
//!   `subscribe` never wait for the network (Go's single mutex made `status` wait for
//!   downloads).
//! - `Inner::save` (`tokio::sync::Mutex<()>`) orders writes of `state.json`: the state is
//!   cloned *inside* it, so the last write always holds the latest state. The write itself runs
//!   on the blocking pool.
//! - `Inner::setter` (`std::sync::Mutex<Arc<dyn WallpaperSetter>>`) holds the setter in use.
//!   Ops clone the `Arc` out; it is only replaced under `op` ([`Engine::reload`] with a changed
//!   `[desktop]`), so a `set` in flight always finishes on the setter it started with.
//! - Lock order: `op`, then `save`, then `snap`. Never `.await` while holding `snap` or
//!   `setter`.
//!
//! History only changes after `WallpaperSetter::set` succeeded (Go moved the history position
//! first and left it moved when the set failed).

mod deps;
mod error;
mod ops;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, FixedOffset};
use kakemono_config::Config;
use kakemono_platform::api::{Platform, Theme, WallpaperSetter};
use kakemono_protocol::types::{StatusData, WallpaperInfo};
use kakemono_sources::SourceSet;
use reqwest::Client;
use tokio::sync::{Notify, broadcast};

pub use deps::{BrowseDeps, Clock, Deps, SourcesFactory};
pub use error::{BrowseError, EngineError, SourceOpError};
pub use ops::Provenance;

use crate::browse::session::Browser;
use crate::downloader::{DownloadEnv, SourceLocks};
use crate::events::EngineEvent;
use crate::facts::{self, FactsCache};
use crate::files::Protect;
use crate::filters::{FilterCtx, Filters};
use crate::gotext::format_set_at;
use crate::hooks;
use crate::state::State;

/// Capacity of the event channel. A receiver that falls further behind loses the oldest events.
pub const EVENT_CAPACITY: usize = 64;

/// The screen size the filters assume when `signals.screen_size()` doesn't know it.
pub const FALLBACK_SCREEN: (u32, u32) = (1920, 1080);

/// What `status` and the ops read. Guarded by `Inner::snap`.
pub(crate) struct Snapshot {
    pub(crate) state: State,
    pub(crate) cfg: Arc<Config>,
    /// The enabled online sources of `cfg`, built by `Deps::sources`.
    pub(crate) sources: Arc<SourceSet>,
    /// `cfg.filters`, compiled once per config.
    pub(crate) filters: Arc<Filters>,
}

pub(crate) struct Inner {
    /// Serialises the ops that set the wallpaper or change history. See the module docs.
    pub(crate) op: tokio::sync::Mutex<()>,
    pub(crate) snap: Mutex<Snapshot>,
    /// Orders `state.json` writes.
    pub(crate) save: tokio::sync::Mutex<()>,
    pub(crate) state_path: PathBuf,
    pub(crate) deps: Deps,
    /// The setter in use. See the module docs.
    pub(crate) setter: Mutex<Arc<dyn WallpaperSetter>>,
    pub(crate) events: broadcast::Sender<EngineEvent>,
    /// Notified (`notify_one`, so a permit is stored when nobody waits) after every wallpaper
    /// change, pause/resume and reload. The rotation loop (Task 21) recomputes its due time.
    pub(crate) reset: Notify,
    pub(crate) hooks: crate::hooks::HookSlots,
    /// Image facts for the filters, at `<state dir>/facts.json`. Shared with the downloader.
    pub(crate) facts: Arc<FactsCache>,
    /// Wakes the downloader (`notify_one`) after a reload.
    pub(crate) wake: Arc<Notify>,
    /// One lock per online source, held by every download ([`download_one`](crate::downloader::download_one)).
    pub(crate) source_locks: SourceLocks,
    /// The image an op chose and is setting, until it is in history. The downloader's deletes
    /// spare it ([`DownloadEnv::protect`]): they don't take `op`.
    pub(crate) pending: Mutex<Option<PathBuf>>,
    /// Set while [`Engine::run`] runs: a second loop would race the first on the
    /// single-waiter notifies.
    pub(crate) running: AtomicBool,
    /// Browse sessions and the browsed-item index ([`crate::browse::session`]).
    pub(crate) browser: Browser,
}

/// The wallpaper engine. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("state_path", &self.inner.state_path)
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// An engine over `state` (load it with `State::load_from_paths`), saving to `state_path`.
    ///
    /// Creates the download, trash and favorites directories (Go `NewManager`); empty paths are
    /// skipped. Loads the facts cache next to `state_path` (`facts.json`).
    ///
    /// # Errors
    ///
    /// [`EngineError::CreateDir`] when a directory cannot be created.
    pub fn new(
        cfg: Config,
        state: State,
        state_path: PathBuf,
        deps: Deps,
    ) -> Result<Self, EngineError> {
        create_dirs(&cfg)?;
        let sources = Arc::new((deps.sources)(&cfg, deps.http.clone()));
        let filters = Arc::new(compile_filters(&cfg));
        let facts = Arc::new(FactsCache::load(
            state_path.with_file_name(facts::CACHE_FILE),
        ));
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let browser = Browser::new(deps.browse.clone());
        Ok(Self {
            inner: Arc::new(Inner {
                op: tokio::sync::Mutex::new(()),
                snap: Mutex::new(Snapshot {
                    state,
                    cfg: Arc::new(cfg),
                    sources,
                    filters,
                }),
                save: tokio::sync::Mutex::new(()),
                state_path,
                setter: Mutex::new(Arc::clone(&deps.platform.setter)),
                deps,
                events,
                reset: Notify::new(),
                hooks: crate::hooks::HookSlots::default(),
                facts,
                wake: Arc::new(Notify::new()),
                source_locks: SourceLocks::default(),
                pending: Mutex::new(None),
                running: AtomicBool::new(false),
                browser,
            }),
        })
    }

    /// Whether auto-rotation is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.snap().state.paused()
    }

    /// The daemon status. Never waits for a running op; `theme` is `signals.theme()`.
    #[must_use]
    pub fn status(&self) -> StatusData {
        let theme = self.theme();
        let snap = self.snap();
        StatusData {
            current: snap.state.current().cloned(),
            paused: snap.state.paused(),
            interval: snap.cfg.daemon.interval.clone(),
            theme: theme.as_str().to_owned(),
            history_count: snap.state.history_count(),
            history_index: snap.state.history_index(),
        }
    }

    /// The setter in use: the platform's, or the one the last `[desktop]` reload built.
    pub fn setter(&self) -> Arc<dyn WallpaperSetter> {
        Arc::clone(
            &self
                .inner
                .setter
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Receives every event sent after this call.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    /// The current config.
    #[must_use]
    pub fn config(&self) -> Arc<Config> {
        Arc::clone(&self.snap().cfg)
    }

    /// A copy of the history state.
    #[must_use]
    pub fn state(&self) -> State {
        self.snap().state.clone()
    }

    /// The system theme (`signals.theme()`).
    #[must_use]
    pub fn theme(&self) -> Theme {
        self.inner.deps.platform.signals.theme()
    }

    /// The platform in use: the signals handed to the engine and the current
    /// [`setter`](Self::setter). A snapshot; a later reload may replace the setter.
    #[must_use]
    pub fn platform(&self) -> Platform {
        Platform {
            setter: self.setter(),
            signals: Arc::clone(&self.inner.deps.platform.signals),
        }
    }

    // ─── crate-internal ─────────────────────────────────────────────────────────────────────

    /// Sends `event` to subscribers (no-op without subscribers).
    pub(crate) fn emit(&self, event: EngineEvent) {
        let _ = self.inner.events.send(event);
    }

    /// Wakes the rotation loop so it recomputes its due time.
    pub(crate) fn reset_timer(&self) {
        self.inner.reset.notify_one();
    }

    /// Waits for the op lock ([`Inner::op`]). Ops hold the guard for their whole run, across
    /// network I/O and the setter, on purpose: it is what serialises them (a tokio mutex, so
    /// holding it across `.await` is sound).
    pub(crate) async fn lock_op(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.op.lock().await
    }

    /// Locks the snapshot. A poisoned lock is recovered: the snapshot is plain data.
    pub(crate) fn snap(&self) -> MutexGuard<'_, Snapshot> {
        self.inner
            .snap
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes `state.json` on the blocking pool and waits for it. Failures are logged, not
    /// returned (Go ignored them).
    pub(crate) async fn persist(&self) {
        let _save = self.inner.save.lock().await;
        let state = self.snap().state.clone();
        let path = self.inner.state_path.clone();
        match tokio::task::spawn_blocking(move || state.save(&path)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "could not save state"),
            Err(e) => tracing::warn!(error = %e, "state save task failed"),
        }
    }

    /// [`Engine::persist`] without waiting, for the sync ops (`pause`, `resume`). Without a
    /// tokio runtime the write runs on this thread.
    fn persist_soon(&self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let engine = self.clone();
            runtime.spawn(async move { engine.persist().await });
            return;
        }
        let _save = self.inner.save.blocking_lock();
        let state = self.snap().state.clone();
        if let Err(e) = state.save(&self.inner.state_path) {
            tracing::warn!(error = %e, "could not save state");
        }
    }

    /// Saves the facts cache now (it is otherwise saved debounced). Failures are logged.
    pub(crate) async fn save_facts(&self) {
        let facts = Arc::clone(&self.inner.facts);
        match tokio::task::spawn_blocking(move || facts.save()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "can't save the facts cache"),
            Err(e) => tracing::warn!(error = %e, "facts cache save task failed"),
        }
    }

    /// The wake handle of the downloader [`Engine::run`] spawns.
    pub(crate) fn downloader_wake(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.wake)
    }

    /// Sets `image` with the configured fit.
    pub(crate) async fn set_image(&self, image: &Path) -> Result<(), EngineError> {
        let fit = self.snap().cfg.desktop.fit;
        self.setter()
            .set(image, fit)
            .await
            .map_err(EngineError::Set)
    }

    /// Installs `setter`. Callers hold `Inner::op`.
    fn swap_setter(&self, setter: Arc<dyn WallpaperSetter>) {
        *self
            .inner
            .setter
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = setter;
    }

    /// After every wallpaper change: event, rotation reset, hook.
    pub(crate) fn changed(&self, info: &WallpaperInfo) {
        tracing::info!(path = %info.path, source = %info.source, "wallpaper changed");
        self.emit(EngineEvent::Changed(info.clone()));
        self.reset_timer();
        hooks::on_change(self, info, self.theme());
    }

    /// Marks `path` as the image being set until the guard drops (see `Inner::pending`).
    /// Callers hold `Inner::op`, so there is one at a time.
    fn pending(&self, path: &Path) -> PendingGuard<'_> {
        *self
            .inner
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(path.to_owned());
        PendingGuard(self)
    }
}

/// Clears `Inner::pending` on drop.
struct PendingGuard<'a>(&'a Engine);

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        *self
            .0
            .inner
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// The engine is the downloader's host ([`crate::downloader`] module docs).
impl DownloadEnv for Engine {
    fn config(&self) -> Arc<Config> {
        Engine::config(self)
    }

    fn sources(&self) -> Arc<SourceSet> {
        Arc::clone(&self.snap().sources)
    }

    fn facts(&self) -> Arc<FactsCache> {
        Arc::clone(&self.inner.facts)
    }

    fn http(&self) -> Client {
        self.inner.deps.http.clone()
    }

    fn source_locks(&self) -> &SourceLocks {
        &self.inner.source_locks
    }

    fn filters(&self) -> Arc<Filters> {
        Arc::clone(&self.snap().filters)
    }

    /// The manual, favorites and trash dirs, every local `[[sources]]` location (enabled or
    /// not), the current wallpaper and the history, all from the live config and state, plus
    /// the image an op is setting.
    fn protect(&self) -> Protect {
        let pending = self
            .inner
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let snap = self.snap();
        let protect = Protect::new().config(&snap.cfg).state(&snap.state);
        match pending {
            Some(path) => protect.file(path),
            None => protect,
        }
    }

    /// `signals.screen_size()` (or [`FALLBACK_SCREEN`]) and the system theme.
    fn filter_ctx(&self) -> FilterCtx {
        FilterCtx {
            screen: self
                .inner
                .deps
                .platform
                .signals
                .screen_size()
                .unwrap_or(FALLBACK_SCREEN),
            theme: self.theme(),
        }
    }
}

/// The history entry for an image from disk: its path, `source` (the row type, `"manual"`),
/// `source_id` (the row id, `"manual"`) and file name as title.
pub(crate) fn disk_info(
    path: &Path,
    source: &str,
    source_id: &str,
    now: &DateTime<FixedOffset>,
) -> WallpaperInfo {
    WallpaperInfo {
        path: path.to_string_lossy().into_owned(),
        source: source.to_owned(),
        source_id: source_id.to_owned(),
        title: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        set_at: format_set_at(now),
        ..WallpaperInfo::default()
    }
}

/// Compiles `cfg.filters`, logging each warning (an invalid regex turns its filter off).
fn compile_filters(cfg: &Config) -> Filters {
    let (filters, warnings) = Filters::compile(&cfg.filters);
    for warning in warnings {
        tracing::warn!("[filters]: {warning}");
    }
    filters
}

/// True when `[desktop]` changed in a way the setter could depend on. `detected_at` is only a
/// timestamp of the last detection, so a re-detection that found the same backend rebuilds
/// nothing.
fn desktop_changed(
    old: &kakemono_config::DesktopConfig,
    new: &kakemono_config::DesktopConfig,
) -> bool {
    let comparable = |d: &kakemono_config::DesktopConfig| kakemono_config::DesktopConfig {
        detected_at: None,
        ..d.clone()
    };
    comparable(old) != comparable(new)
}

fn create_dirs(cfg: &Config) -> Result<(), EngineError> {
    for dir in [
        &cfg.daemon.download_dir,
        &cfg.daemon.trash_dir,
        &cfg.favorites.dir,
    ] {
        if dir.as_os_str().is_empty() {
            continue;
        }
        std::fs::create_dir_all(dir).map_err(|source| EngineError::CreateDir {
            path: dir.clone(),
            source,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_at_alone_is_not_a_desktop_change() {
        let old = kakemono_config::DesktopConfig::default();
        let stamped = kakemono_config::DesktopConfig {
            detected_at: Some(chrono::Utc::now()),
            ..old.clone()
        };
        assert!(!desktop_changed(&old, &stamped));
        let pinned = kakemono_config::DesktopConfig {
            persistent: true,
            ..old.clone()
        };
        assert!(desktop_changed(&old, &pinned));
    }
}
