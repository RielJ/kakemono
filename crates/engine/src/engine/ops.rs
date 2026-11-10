//! The wallpaper ops: history steps, picks, favorites and trash, pause, reload.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_config::Config;
use kakemono_protocol::types::WallpaperInfo;
use kakemono_sources::SourceSet;
use rand::SeedableRng;
use rand::rngs::StdRng;

use super::{
    Engine, EngineError, SourceOpError, compile_filters, create_dirs, desktop_changed, disk_info,
};
use crate::downloader::{self, DownloadEnv};
use crate::events::EngineEvent;
use crate::files;
use crate::gotext::{format_set_at, go_stat_error};
use crate::pool::{self, Picked, Pool};

impl Engine {
    /// Next wallpaper. If the user went back with `prev`, steps forward through history
    /// without fetching. Otherwise picks an image on disk ([`pool::pick`]) and makes no HTTP
    /// request. Only when the pool is empty, `[daemon] internet` is on and an online source is
    /// enabled, downloads one image on demand: sources in config order, falling back to the next
    /// one when a download fails ([`EngineError::Fetch`] lists every failure). Otherwise
    /// [`EngineError::NoImages`]. Then sets it and pushes it to history.
    ///
    /// # Errors
    ///
    /// [`EngineError::NoImages`], [`EngineError::Fetch`] when every on-demand download failed,
    /// or [`EngineError::Set`] when the setter failed (history is then unchanged).
    pub async fn next(&self) -> Result<WallpaperInfo, EngineError> {
        let _op = self.lock_op().await;
        self.next_locked().await
    }

    /// Steps back one history entry and sets it.
    ///
    /// # Errors
    ///
    /// [`EngineError::AtStart`] at the start of history, or [`EngineError::Set`] (the
    /// position is then unchanged).
    pub async fn prev(&self) -> Result<WallpaperInfo, EngineError> {
        let _op = self.lock_op().await;
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
        self.persist().await;
        self.changed(&info);
        Ok(info)
    }

    /// Sets the local file `path` and pushes it to history with source `local`, an empty
    /// `source_id` (no `[[sources]]` row) and the file name as title (Go `SetFile`). The path is
    /// stored as given.
    ///
    /// # Errors
    ///
    /// [`EngineError::FileNotFound`] when `path` cannot be stat'ed, or [`EngineError::Set`].
    pub async fn set_file(&self, path: &Path) -> Result<WallpaperInfo, EngineError> {
        let _op = self.lock_op().await;
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
        self.persist().await;
        self.changed(&info);
        Ok(info)
    }

    /// Copies the current wallpaper into `[favorites] dir` (Task 20 implements the copy).
    ///
    /// # Errors
    ///
    /// [`EngineError::NoCurrent`] without a current wallpaper, or [`EngineError::Files`] when
    /// the copy failed.
    pub async fn favorite(&self) -> Result<(), EngineError> {
        let _op = self.lock_op().await;
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
    ///
    /// # Errors
    ///
    /// [`EngineError::NoCurrent`], [`EngineError::Files`] when the move failed, or any error of
    /// [`Engine::next`] (the file is then already in the trash).
    pub async fn trash(&self) -> Result<WallpaperInfo, EngineError> {
        self.trash_expected(None).await
    }

    /// [`Engine::trash`], but with `Some(path)` only if the current wallpaper is still `path`.
    /// The check runs under the op lock, so no rotation can slip in between; a mismatch
    /// returns [`EngineError::NotCurrent`] and trashes nothing.
    ///
    /// # Errors
    ///
    /// Those of [`Engine::trash`], and [`EngineError::NotCurrent`] on a mismatch.
    pub async fn trash_expected(
        &self,
        expected: Option<&str>,
    ) -> Result<WallpaperInfo, EngineError> {
        let _op = self.lock_op().await;
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

    /// Writes `state.json` now, after any write [`Engine::pause`] or [`Engine::resume`] started,
    /// and waits for it. Ops save on their own; call this before reading the file or exiting.
    pub async fn save_state(&self) {
        self.persist().await;
    }

    /// Pauses auto-rotation and saves the state in the background ([`Engine::save_state`]
    /// waits for it). Doesn't wait for a running op.
    pub fn pause(&self) {
        self.set_paused(true);
    }

    /// Resumes auto-rotation and saves the state in the background ([`Engine::save_state`]
    /// waits for it). Doesn't wait for a running op.
    pub fn resume(&self) {
        self.set_paused(false);
    }

    /// Re-applies the current wallpaper without touching history (display change, wake, Space
    /// change; Task 21). `Ok(None)` when there is no current wallpaper. No event, no hook.
    ///
    /// # Errors
    ///
    /// [`EngineError::Set`] when the setter failed.
    pub async fn reapply(&self) -> Result<Option<WallpaperInfo>, EngineError> {
        let _op = self.lock_op().await;
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
    /// [`WallpaperSetter::rebuild`](kakemono_platform::api::WallpaperSetter::rebuild) under
    /// the op lock, after every other step that can fail, so the next apply uses the new
    /// backend. An unchanged `[desktop]` rebuilds nothing.
    ///
    /// All or nothing for the config and the setter: on error ([`EngineError::Desktop`] when
    /// the new `[desktop]` cannot be used) the old config and the old setter stay. The new
    /// config's folders are created first, so they can exist after a failed reload.
    ///
    /// # Errors
    ///
    /// [`EngineError::CreateDir`] when a new folder cannot be created, or
    /// [`EngineError::Desktop`] when the new `[desktop]` cannot be used.
    pub async fn reload(&self, cfg: Config) -> Result<(), EngineError> {
        let _op = self.lock_op().await;
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
            self.persist().await;
            self.changed(&info);
            return Ok(info);
        }
        self.pick_and_set().await
    }

    /// Downloads one image for the online source `id` into `<download_dir>/<id>/` (the
    /// downloader's [`downloader::download_one`]) without setting it. Ignores the downloader's
    /// backoff. Errors: the source isn't an enabled online row, `[daemon] internet` is off, or
    /// the download failed (`provider {name} fetch: …`, `download image: …`).
    ///
    /// # Errors
    ///
    /// [`SourceOpError::UnknownSource`], [`SourceOpError::InternetOff`] or
    /// [`SourceOpError::Download`].
    pub async fn download_one(&self, id: &str) -> Result<PathBuf, SourceOpError> {
        let (cfg, sources) = {
            let snap = self.snap();
            (Arc::clone(&snap.cfg), Arc::clone(&snap.sources))
        };
        let source = sources
            .get(id)
            .ok_or_else(|| SourceOpError::UnknownSource(id.to_owned()))?;
        if !cfg.daemon.internet {
            return Err(SourceOpError::InternetOff);
        }
        Ok(downloader::download_one(self, source).await?)
    }

    fn set_paused(&self, paused: bool) {
        let was = {
            let mut snap = self.snap();
            let was = snap.state.paused();
            snap.state.set_paused(paused);
            was
        };
        self.persist_soon();
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
        let _pending = self.pending(&picked.path);
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
        self.persist().await;
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
                Err(failure) => {
                    tracing::warn!(source_id = %source.id, error = %failure, "on-demand download failed");
                    failures.push(failure);
                    continue;
                }
            };
            let _pending = self.pending(&path);
            self.set_image(&path).await?;
            let now = (self.inner.deps.now)();
            let info = disk_info(&path, source.kind.as_str(), &source.id, &now);
            tracing::info!(
                source_id = %source.id,
                path = %path.display(),
                "empty pool; downloaded on demand"
            );
            self.snap().state.push(info.clone(), now);
            self.persist().await;
            self.changed(&info);
            return Ok(info);
        }
        Err(EngineError::Fetch(failures))
    }
}
