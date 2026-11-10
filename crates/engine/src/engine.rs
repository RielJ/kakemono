//! `Engine`: the wallpaper ops (`next`, `prev`, `set_file`, `favorite`, `trash`, `pause`,
//! `resume`, `status`, `reload`) over history state, the local image pool, the online sources
//! and the platform seam (plan §D). Port of Go `internal/wallpaper/manager.go`.
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
//! - `Inner::save` (`std::sync::Mutex<()>`) orders writes of `state.json`: the state is cloned
//!   *inside* it, so the last write always holds the latest state.
//! - `Inner::setter` (`std::sync::Mutex<Arc<dyn WallpaperSetter>>`) holds the setter in use.
//!   Ops clone the `Arc` out; it is only replaced under `op` ([`Engine::reload`] with a changed
//!   `[desktop]`), so a `set` in flight always finishes on the setter it started with.
//! - Lock order: `op`, then `save`, then `snap`. Never `.await` while holding `save`, `snap` or
//!   `setter`.
//!
//! History only changes after `WallpaperSetter::set` succeeded (Go moved the history position
//! first and left it moved when the set failed).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, FixedOffset, Local, SecondsFormat};
use kakemono_config::Config;
use kakemono_platform::api::{Platform, SetError, Theme, WallpaperSetter};
use kakemono_platform::exec::{CommandRunner, TokioRunner};
use kakemono_protocol::types::{StatusData, WallpaperInfo};
use kakemono_sources::{SourceSet, WallpaperResult, http as source_http};
use rand::SeedableRng;
use rand::rngs::StdRng;
use reqwest::{Client, Url};
use tokio::sync::{Notify, broadcast};

use crate::downloader::{self, DownloadEnv};
use crate::events::EngineEvent;
use crate::facts::{self, FactsCache};
use crate::files::Protect;
use crate::filters::{FilterCtx, Filters};
use crate::pool::{self, Picked, Pool};
use crate::state::State;
use crate::{files, hooks};

/// Capacity of the event channel. A receiver that falls further behind loses the oldest events.
pub const EVENT_CAPACITY: usize = 64;

/// Wall-clock source. Used for `WallpaperInfo.set_at` and `state.last_changed`.
pub type Clock = Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>;

/// Builds the online sources for a config. Called by [`Engine::new`] and on every
/// [`Engine::reload`], so `[[sources]]` edits and provider keys take effect on reload.
pub type SourcesFactory = Arc<dyn Fn(&Config, Client) -> SourceSet + Send + Sync>;

/// The screen size the filters assume when `signals.screen_size()` doesn't know it.
pub const FALLBACK_SCREEN: (u32, u32) = (1920, 1080);

/// Why an op failed. Crosses IPC as its `Display` string; messages keep Go's `manager: ` prefix
/// and wording where Go had the same failure.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// `favorite`/`trash` with no current wallpaper.
    #[error("manager: no current wallpaper")]
    NoCurrent,
    /// `trash` with an `expected` path that isn't the current wallpaper (Task 45b).
    #[error("manager: current wallpaper changed: expected {expected}, current {current}")]
    NotCurrent { expected: String, current: String },
    /// `prev` at the oldest history entry (or with empty history).
    #[error("manager: already at the beginning of history")]
    AtStart,
    /// `set_file` on a path that can't be stat'ed. The payload is Go's `os.Stat` error text,
    /// `stat <path>: <reason>`.
    #[error("manager: file not found: {0}")]
    FileNotFound(String),
    /// The platform setter failed.
    #[error("manager: set wallpaper: {0}")]
    Set(#[source] SetError),
    /// `reload` with a `[desktop]` that cannot be used (Task 37b). The old config and setter
    /// stay.
    #[error("[desktop]: {0}")]
    Desktop(#[source] SetError),
    /// A change found no image on disk and couldn't download one: `[daemon] internet` is off
    /// or no online source is enabled. Go had no such failure; the text is meant for the UI.
    #[error("No images yet. Add a source or turn on Internet access.")]
    NoImages,
    /// The pool was empty and every on-demand download failed. One message per online source,
    /// in config order, without the `manager: ` prefix (e.g. `provider bing fetch: …`,
    /// `download image: …`). URLs in download errors are redacted. With one source the text
    /// equals Go's.
    #[error("manager: {}", .0.join("; "))]
    Fetch(Vec<String>),
    /// Creating one of the engine's directories failed.
    #[error("manager: create dir {}: {source}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Favorite/trash file operations (Task 20). The payload has no `manager: ` prefix.
    #[error("manager: {0}")]
    Files(String),
    /// `fetch_url` failed (Task 16): a bad input, `[daemon] internet` off for a URL, a download
    /// or copy failure, not a JPEG/PNG/WebP, or over the size cap. Shown as is, no prefix.
    #[error("{0}")]
    Manual(String),
}

/// Everything the engine needs from outside. Cheap to clone.
#[derive(Clone)]
pub struct Deps {
    /// The wallpaper setter and system signals (`signals.theme()` picks the sources). The
    /// setter is the first one; `reload` may replace it ([`Engine::setter`]).
    pub platform: Platform,
    /// Shared HTTP client for providers and downloads (build with
    /// `kakemono_sources::http::client`).
    pub http: Client,
    /// Builds the online sources from config. Default: `SourceSet::from_config`.
    pub sources: SourcesFactory,
    /// Runs external commands (hooks, Task 22). Default: `TokioRunner`.
    pub runner: Arc<dyn CommandRunner>,
    /// Wall clock. Default: local time.
    pub now: Clock,
}

impl Deps {
    /// Production defaults: the real online sources, `TokioRunner`, local time.
    pub fn new(platform: Platform, http: Client) -> Self {
        Self {
            platform,
            http,
            sources: Arc::new(SourceSet::from_config),
            runner: Arc::new(TokioRunner),
            now: Arc::new(|| Local::now().fixed_offset()),
        }
    }

    /// Use `factory` to build the online sources (on construction and every reload).
    #[must_use]
    pub fn with_sources_factory(
        mut self,
        factory: impl Fn(&Config, Client) -> SourceSet + Send + Sync + 'static,
    ) -> Self {
        self.sources = Arc::new(factory);
        self
    }

    /// Use `sources` regardless of config (tests). Reloads keep it.
    #[must_use]
    pub fn with_sources(self, sources: SourceSet) -> Self {
        self.with_sources_factory(move |_, _| sources.clone())
    }

    /// Use `runner` for external commands.
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn CommandRunner>) -> Self {
        self.runner = runner;
        self
    }

    /// Use `now` as the wall clock.
    #[must_use]
    pub fn with_clock(
        mut self,
        now: impl Fn() -> DateTime<FixedOffset> + Send + Sync + 'static,
    ) -> Self {
        self.now = Arc::new(now);
        self
    }
}

impl fmt::Debug for Deps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Deps")
            .field("setter", &self.platform.setter.info())
            .finish_non_exhaustive()
    }
}

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
    pub(crate) save: Mutex<()>,
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
        Ok(Self {
            inner: Arc::new(Inner {
                op: tokio::sync::Mutex::new(()),
                snap: Mutex::new(Snapshot {
                    state,
                    cfg: Arc::new(cfg),
                    sources,
                    filters,
                }),
                save: Mutex::new(()),
                state_path,
                setter: Mutex::new(Arc::clone(&deps.platform.setter)),
                deps,
                events,
                reset: Notify::new(),
                hooks: crate::hooks::HookSlots::default(),
                facts,
                wake: Arc::new(Notify::new()),
            }),
        })
    }

    /// Next wallpaper. If the user went back with `prev`, steps forward through history
    /// without fetching. Otherwise picks an image on disk ([`pool::pick`]) and makes no HTTP
    /// request. Only when the pool is empty, `[daemon] internet` is on and an online source is
    /// enabled, downloads one image on demand: sources in config order, falling back to the next
    /// one when a download fails ([`EngineError::Fetch`] lists every failure). Otherwise
    /// [`EngineError::NoImages`]. Then sets it and pushes it to history.
    pub async fn next(&self) -> Result<WallpaperInfo, EngineError> {
        let _op = self.inner.op.lock().await;
        self.next_locked().await
    }

    /// Steps back one history entry and sets it.
    pub async fn prev(&self) -> Result<WallpaperInfo, EngineError> {
        let _op = self.inner.op.lock().await;
        let info = {
            let snap = self.snap();
            let pos = snap.state.history_position().ok_or(EngineError::AtStart)?;
            if !snap.state.can_go_prev() {
                return Err(EngineError::AtStart);
            }
            snap.state.history()[pos - 1].clone()
        };
        self.set_image(Path::new(&info.path)).await?;
        self.snap().state.prev();
        self.persist();
        self.changed(&info);
        Ok(info)
    }

    /// Sets the local file `path` and pushes it to history with source `local`, an empty
    /// `source_id` (no `[[sources]]` row) and the file name as title (Go `SetFile`). The path is
    /// stored as given.
    pub async fn set_file(&self, path: &Path) -> Result<WallpaperInfo, EngineError> {
        let _op = self.inner.op.lock().await;
        if let Err(e) = tokio::fs::metadata(path).await {
            return Err(EngineError::FileNotFound(go_stat_error(path, &e)));
        }
        self.set_image(path).await?;
        let now = (self.inner.deps.now)();
        let info = WallpaperInfo {
            path: path.to_string_lossy().into_owned(),
            source: "local".to_owned(),
            title: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            set_at: format_set_at(&now),
            ..WallpaperInfo::default()
        };
        self.snap().state.push(info.clone(), now);
        self.persist();
        self.changed(&info);
        Ok(info)
    }

    /// Copies the current wallpaper into `[favorites] dir` (Task 20 implements the copy).
    pub async fn favorite(&self) -> Result<(), EngineError> {
        let _op = self.inner.op.lock().await;
        let (current, dir) = {
            let snap = self.snap();
            let current = snap.state.current().ok_or(EngineError::NoCurrent)?;
            (PathBuf::from(&current.path), snap.cfg.favorites.dir.clone())
        };
        files::favorite(&current, &dir).await?;
        Ok(())
    }

    /// Moves the current wallpaper to `[daemon] trash_dir` (Task 20), then runs `next` under
    /// the same lock (Go released its lock in between).
    pub async fn trash(&self) -> Result<WallpaperInfo, EngineError> {
        self.trash_expected(None).await
    }

    /// [`Engine::trash`], but with `Some(path)` only if the current wallpaper is still `path`.
    /// The check runs under the op lock, so no rotation can slip in between; a mismatch
    /// returns [`EngineError::NotCurrent`] and trashes nothing.
    pub async fn trash_expected(
        &self,
        expected: Option<&str>,
    ) -> Result<WallpaperInfo, EngineError> {
        let _op = self.inner.op.lock().await;
        let (current, dir) = {
            let snap = self.snap();
            let current = snap.state.current().ok_or(EngineError::NoCurrent)?;
            if let Some(expected) = expected.filter(|e| *e != current.path) {
                return Err(EngineError::NotCurrent {
                    expected: expected.to_owned(),
                    current: current.path.clone(),
                });
            }
            (
                PathBuf::from(&current.path),
                snap.cfg.daemon.trash_dir.clone(),
            )
        };
        files::trash(&current, &dir).await?;
        self.next_locked().await
    }

    /// Pauses auto-rotation and saves the state. Doesn't wait for a running op.
    pub fn pause(&self) {
        self.set_paused(true);
    }

    /// Resumes auto-rotation and saves the state. Doesn't wait for a running op.
    pub fn resume(&self) {
        self.set_paused(false);
    }

    /// Whether auto-rotation is paused.
    pub fn is_paused(&self) -> bool {
        self.snap().state.paused()
    }

    /// The daemon status. Never waits for a running op; `theme` is `signals.theme()`.
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

    /// Re-applies the current wallpaper without touching history (display change, wake, Space
    /// change; Task 21). `Ok(None)` when there is no current wallpaper. No event, no hook.
    pub async fn reapply(&self) -> Result<Option<WallpaperInfo>, EngineError> {
        let _op = self.inner.op.lock().await;
        let Some(info) = self.snap().state.current().cloned() else {
            return Ok(None);
        };
        self.set_image(Path::new(&info.path)).await?;
        Ok(Some(info))
    }

    /// Replaces the config: creates its directories, rebuilds the online sources and the
    /// filters (logging filter warnings once), and wakes the rotation loop (interval changes
    /// take effect here) and the downloader. Waits for a running op.
    ///
    /// When `[desktop]` changed (ignoring `detected_at`), the setter is rebuilt with
    /// [`WallpaperSetter::rebuild`] under the op lock, after every other step that can fail, so
    /// the next apply uses the new backend. An unchanged `[desktop]` rebuilds nothing.
    ///
    /// All or nothing: on error ([`EngineError::Desktop`] when the new `[desktop]` cannot be
    /// used) the old config and the old setter stay.
    pub async fn reload(&self, cfg: Config) -> Result<(), EngineError> {
        let _op = self.inner.op.lock().await;
        create_dirs(&cfg)?;
        let sources = Arc::new((self.inner.deps.sources)(
            &cfg,
            self.inner.deps.http.clone(),
        ));
        let old = self.config();
        if desktop_changed(&old.desktop, &cfg.desktop) {
            let current = self.setter();
            match current.rebuild(&cfg.desktop).await {
                Ok(Some(new)) => {
                    tracing::info!(
                        old = %current.info().name,
                        new = %new.info().name,
                        detail = %new.info().detail,
                        "[desktop] changed; wallpaper setter rebuilt"
                    );
                    self.swap_setter(new);
                }
                Ok(None) => tracing::debug!("[desktop] changed; the setter does not depend on it"),
                Err(e) => {
                    tracing::warn!(error = %e, "[desktop] changed but cannot be used; keeping the old config and setter");
                    return Err(EngineError::Desktop(e));
                }
            }
        }
        let filters = Arc::new(compile_filters(&cfg));
        {
            let mut snap = self.snap();
            snap.cfg = Arc::new(cfg);
            snap.sources = sources;
            snap.filters = filters;
        }
        self.inner.reset.notify_one();
        self.inner.wake.notify_one();
        Ok(())
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
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    /// The current config.
    pub fn config(&self) -> Arc<Config> {
        Arc::clone(&self.snap().cfg)
    }

    /// A copy of the history state.
    pub fn state(&self) -> State {
        self.snap().state.clone()
    }

    /// The system theme (`signals.theme()`).
    pub fn theme(&self) -> Theme {
        self.inner.deps.platform.signals.theme()
    }

    /// The platform in use: the signals handed to the engine and the current
    /// [`setter`](Self::setter). A snapshot; a later reload may replace the setter.
    pub fn platform(&self) -> Platform {
        Platform {
            setter: self.setter(),
            signals: Arc::clone(&self.inner.deps.platform.signals),
        }
    }

    // ─── crate-internal extension points (Tasks 20–22) ─────────────────────────────────────

    /// Sends `event` to subscribers (no-op without subscribers).
    pub(crate) fn emit(&self, event: EngineEvent) {
        let _ = self.inner.events.send(event);
    }

    /// Wakes the rotation loop so it recomputes its due time.
    pub(crate) fn reset_timer(&self) {
        self.inner.reset.notify_one();
    }

    /// Locks the snapshot. A poisoned lock is recovered: the snapshot is plain data.
    pub(crate) fn snap(&self) -> MutexGuard<'_, Snapshot> {
        self.inner
            .snap
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes `state.json`. Failures are logged, not returned (Go ignored them).
    pub(crate) fn persist(&self) {
        let _save = self
            .inner
            .save
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let state = self.snap().state.clone();
        if let Err(e) = state.save(&self.inner.state_path) {
            tracing::warn!(error = %e, "could not save state");
        }
    }

    /// `next` with `Inner::op` already held by the caller.
    pub(crate) async fn next_locked(&self) -> Result<WallpaperInfo, EngineError> {
        let future = {
            let snap = self.snap();
            snap.state
                .history_position()
                .filter(|_| snap.state.has_future_history())
                .map(|pos| snap.state.history()[pos + 1].clone())
        };
        if let Some(info) = future {
            self.set_image(Path::new(&info.path)).await?;
            self.snap().state.next();
            self.persist();
            self.changed(&info);
            return Ok(info);
        }
        self.pick_and_set().await
    }

    /// Downloads one image for the online source `id` into `<download_dir>/<id>/` (the
    /// downloader's [`downloader::download_one`]) without setting it. Ignores the downloader's
    /// backoff. Errors: the source isn't an enabled online row, `[daemon] internet` is off, or
    /// the download failed (`provider {name} fetch: …`, `download image: …`).
    pub async fn download_one(&self, id: &str) -> Result<PathBuf, String> {
        let (cfg, sources) = {
            let snap = self.snap();
            (Arc::clone(&snap.cfg), Arc::clone(&snap.sources))
        };
        let source = sources
            .get(id)
            .ok_or_else(|| format!("no enabled online source {id:?}"))?;
        if !cfg.daemon.internet {
            return Err("Internet access is off".to_owned());
        }
        downloader::download_one(self, source).await
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

    // ─── internals ──────────────────────────────────────────────────────────────────────────

    fn set_paused(&self, paused: bool) {
        let was = {
            let mut snap = self.snap();
            let was = snap.state.paused();
            snap.state.set_paused(paused);
            was
        };
        self.persist();
        if was != paused {
            tracing::info!(
                paused,
                "auto-rotation {}",
                if paused { "paused" } else { "resumed" }
            );
            self.emit(EngineEvent::Paused(paused));
            self.reset_timer();
        }
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

    /// Picks an image from the pool and sets it; with an empty pool, downloads one on demand
    /// (see [`Engine::next`]).
    async fn pick_and_set(&self) -> Result<WallpaperInfo, EngineError> {
        let (cfg, sources, filters, state) = {
            let snap = self.snap();
            (
                Arc::clone(&snap.cfg),
                Arc::clone(&snap.sources),
                Arc::clone(&snap.filters),
                snap.state.clone(),
            )
        };
        let pool = {
            let cfg = Arc::clone(&cfg);
            tokio::task::spawn_blocking(move || Pool::collect(&cfg))
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "listing the image pool failed");
                    Pool::default()
                })
        };
        let ctx = self.filter_ctx();
        let mut rng = StdRng::from_rng(&mut rand::rng());
        let picked = pool::pick(&pool, &filters, &ctx, &self.inner.facts, &state, &mut rng).await;
        if !filters.is_empty() {
            self.inner.facts.schedule_save();
        }
        match picked {
            Some(picked) => self.set_picked(picked).await,
            None => self.download_and_set(&cfg, &sources).await,
        }
    }

    /// Sets an image [`pool::pick`] chose, then records it (album cursor, history).
    async fn set_picked(&self, picked: Picked) -> Result<WallpaperInfo, EngineError> {
        self.set_image(&picked.path).await?;
        let now = (self.inner.deps.now)();
        let info = disk_info(&picked.path, picked.kind.as_str(), &picked.source_id, &now);
        tracing::info!(
            source_id = %picked.source_id,
            outcome = ?picked.outcome,
            path = %picked.path.display(),
            "picked from disk"
        );
        {
            let mut snap = self.snap();
            picked.commit(&mut snap.state);
            snap.state.push(info.clone(), now);
        }
        self.persist();
        self.changed(&info);
        Ok(info)
    }

    /// The empty-pool fallback: one on-demand download, online sources in config order (with
    /// an empty pool every source folder is empty, so all tie on "fewest files").
    async fn download_and_set(
        &self,
        cfg: &Config,
        sources: &SourceSet,
    ) -> Result<WallpaperInfo, EngineError> {
        if !cfg.daemon.internet || sources.is_empty() {
            return Err(EngineError::NoImages);
        }
        let mut failures = Vec::new();
        for source in &sources.online {
            let path = match downloader::download_one(self, source).await {
                Ok(path) => path,
                Err(message) => {
                    tracing::warn!(source_id = %source.id, error = %message, "on-demand download failed");
                    failures.push(message);
                    continue;
                }
            };
            self.set_image(&path).await?;
            let now = (self.inner.deps.now)();
            let info = disk_info(&path, source.kind.as_str(), &source.id, &now);
            tracing::info!(
                source_id = %source.id,
                path = %path.display(),
                "empty pool; downloaded on demand"
            );
            self.snap().state.push(info.clone(), now);
            self.persist();
            self.changed(&info);
            return Ok(info);
        }
        Err(EngineError::Fetch(failures))
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

    /// The manual, favorites and trash dirs, every local `[[sources]]` location (enabled or
    /// not), the current wallpaper and the history, all from the live config and state.
    fn protect(&self) -> Protect {
        let snap = self.snap();
        let cfg = &snap.cfg;
        Protect::new().config(cfg).state(&snap.state)
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

/// The local file for `result` (Go `resolveLocalPath`): a `file://` URL or an absolute path
/// must exist; anything else is downloaded into `download_dir`. The error has no `manager: `
/// prefix, and signed-URL secrets in download errors are redacted.
pub async fn resolve_local_path(
    http: &Client,
    result: &WallpaperResult,
    download_dir: &Path,
) -> Result<PathBuf, String> {
    let image_url = result.image_url.as_str();

    if let Some(rest) = image_url.strip_prefix("file://") {
        // Go trimmed the prefix literally. Also accept a percent-encoded URL (`%20`).
        let literal = PathBuf::from(rest);
        let decoded = Url::parse(image_url)
            .ok()
            .and_then(|u| u.to_file_path().ok());
        for candidate in decoded.iter().chain(std::iter::once(&literal)) {
            if tokio::fs::metadata(candidate).await.is_ok() {
                return Ok(candidate.clone());
            }
        }
        return Err(local_not_found(&literal).await);
    }

    let path = Path::new(image_url);
    if path.is_absolute() {
        if tokio::fs::metadata(path).await.is_ok() {
            return Ok(path.to_path_buf());
        }
        return Err(local_not_found(path).await);
    }

    kakemono_sources::download(http, image_url, download_dir)
        .await
        .map_err(|e| format!("download image: {}", redact_in(&e.to_string(), image_url)))
}

async fn local_not_found(path: &Path) -> String {
    let reason = match tokio::fs::metadata(path).await {
        Err(e) => go_stat_error(path, &e),
        Ok(_) => format!("stat {}: vanished", path.display()),
    };
    format!(
        "local file not found {:?}: {reason}",
        path.to_string_lossy()
    )
}

/// `message` with every spelling of `url` replaced by its redacted form (secret query values →
/// `REDACTED`, see `kakemono_sources::http::redact_url`). An unparsable URL loses its whole
/// query instead.
pub fn redact_in(message: &str, url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => {
            let redacted = source_http::redact_url(&parsed);
            if redacted == parsed {
                return message.to_owned();
            }
            message
                .replace(url, redacted.as_str())
                .replace(parsed.as_str(), redacted.as_str())
        }
        Err(_) => match url.split_once('?') {
            Some((base, _)) => message.replace(url, &format!("{base}?{}", source_http::REDACTED)),
            None => message.to_owned(),
        },
    }
}

/// Go's `os.Stat` error text: `stat <path>: <reason>`, with Go's lower-case reasons for the
/// common cases.
pub(crate) fn go_stat_error(path: &Path, err: &io::Error) -> String {
    let reason = match err.kind() {
        io::ErrorKind::NotFound => "no such file or directory".to_owned(),
        io::ErrorKind::PermissionDenied => "permission denied".to_owned(),
        io::ErrorKind::NotADirectory => "not a directory".to_owned(),
        _ => err.to_string(),
    };
    format!("stat {}: {reason}", path.display())
}

/// Go `time.Now().Format(time.RFC3339)`: whole seconds, `Z` for UTC.
pub(crate) fn format_set_at(now: &DateTime<FixedOffset>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, true)
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

    #[test]
    fn set_at_is_rfc3339_seconds() {
        let t = DateTime::parse_from_rfc3339("2026-01-02T03:04:05.678+08:00").unwrap();
        assert_eq!(format_set_at(&t), "2026-01-02T03:04:05+08:00");
        let utc = DateTime::parse_from_rfc3339("2026-01-02T03:04:05.9Z").unwrap();
        assert_eq!(format_set_at(&utc), "2026-01-02T03:04:05Z");
    }

    #[test]
    fn redact_in_replaces_raw_and_normalised_spellings() {
        let url = "https://cdn.test/a b.jpg?sig=S3CRET&w=1";
        let msg = format!(
            "download: GET {url}: error for url (https://cdn.test/a%20b.jpg?sig=S3CRET&w=1)"
        );
        let out = redact_in(&msg, url);
        assert!(!out.contains("S3CRET"), "{out}");
        assert!(out.contains("sig=REDACTED"), "{out}");
    }

    #[test]
    fn redact_in_unparsable_url_drops_query() {
        let out = redact_in(
            "download: invalid url ::bad?sig=S3CRET: x",
            "::bad?sig=S3CRET",
        );
        assert_eq!(out, "download: invalid url ::bad?REDACTED: x");
    }

    #[test]
    fn stat_error_text_matches_go() {
        let e = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(
            go_stat_error(Path::new("/x.png"), &e),
            "stat /x.png: no such file or directory"
        );
    }
}
