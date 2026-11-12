//! [`Engine::browse_use`]: set, favorite or
//! download a remote item the user picked in the Sources tab.
//!
//! The item comes from the browse item index by `(source_id, remote_id)`; the UI never sends a
//! URL. The first use downloads the full image into `<download_dir>/<id>/` under the source's
//! [`SourceLocks`](crate::downloader::SourceLocks) lock, so it never races the background
//! downloader: the provider resolves the image URL, the guard checks it against the provider's
//! media hosts, the guarded client downloads it, the URL goes into the seen ledger (the
//! background loop won't fetch it again), and the download folder is trimmed with the new file
//! protected. Later uses of the same item reuse the file while it exists. `[filters]` are not
//! applied: the user chose this image.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_config::valid_id;
use kakemono_protocol::types::{BrowseAction, BrowseUseData};
use kakemono_sources::{GuardError, OnlineSource};
use reqwest::Url;

use super::session::{IndexedItem, Ready};
use crate::downloader::{DownloadEnv, record_seen};
use crate::engine::{BrowseError, Engine, Provenance};
use crate::files::{self, SEEN_LEDGER};

impl Engine {
    /// Applies `action` to the browsed item `remote_id` of source `source_id` (see the
    /// [module docs](self)): `Set` also sets it as the wallpaper with the source's provenance,
    /// `Favorite` also copies it into `[favorites] dir`, `Download` only downloads it. Returns
    /// the downloaded file, and the new wallpaper for `Set`.
    ///
    /// # Errors
    ///
    /// [`BrowseError::InternetOff`], [`BrowseError::NotAvailable`],
    /// [`BrowseError::ItemExpired`] for an item not in the index,
    /// [`BrowseError::UnknownSource`] when its source is no longer an enabled online row of the
    /// same type, then the resolve, guard, download, set or favorite failure.
    pub async fn browse_use(
        &self,
        source_id: &str,
        remote_id: &str,
        action: BrowseAction,
    ) -> Result<BrowseUseData, BrowseError> {
        let (cfg, sources) = {
            let snap = self.snap();
            (Arc::clone(&snap.cfg), Arc::clone(&snap.sources))
        };
        if !cfg.daemon.internet {
            return Err(BrowseError::InternetOff);
        }
        let browser = &self.inner.browser;
        let ready = browser.ready()?;
        let (indexed, _) = browser
            .index()
            .get(source_id, remote_id)
            .ok_or(BrowseError::ItemExpired)?;
        let source = sources
            .get(source_id)
            .filter(|s| s.kind == indexed.kind)
            .ok_or_else(|| BrowseError::UnknownSource(source_id.to_owned()))?;
        if !valid_id(source_id) {
            return Err(BrowseError::InvalidId(source_id.to_owned()));
        }

        let limits = ready.deps.limits;
        let path = tokio::time::timeout(
            limits.use_timeout,
            self.downloaded(ready, source, &indexed, &cfg.daemon.download_dir),
        )
        .await
        .map_err(|_| BrowseError::DownloadTimeout(limits.use_timeout))??;

        let wallpaper = match action {
            BrowseAction::Set => {
                let title = if indexed.item.title.is_empty() {
                    file_name(&path)
                } else {
                    indexed.item.title.clone()
                };
                let provenance = Provenance {
                    source: indexed.kind.as_str().to_owned(),
                    source_id: source_id.to_owned(),
                    title,
                    source_url: indexed.item.source_url.clone(),
                };
                Some(self.set_file_with(&path, provenance).await?)
            }
            BrowseAction::Favorite => {
                let favorites = self.config().favorites.dir.clone();
                files::favorite(&path, &favorites).await?;
                None
            }
            BrowseAction::Download => None,
        };
        Ok(BrowseUseData {
            path: path.to_string_lossy().into_owned(),
            wallpaper,
        })
    }

    /// The item's downloaded file: the earlier download while it exists, otherwise a new one.
    async fn downloaded(
        &self,
        ready: &Ready,
        source: &OnlineSource,
        indexed: &IndexedItem,
        download_dir: &Path,
    ) -> Result<PathBuf, BrowseError> {
        let _lock = self.inner.source_locks.lock(&source.id).await;
        // Read under the lock: a use of the same item that held it may have just downloaded.
        let earlier = self
            .inner
            .browser
            .index()
            .get(&indexed.source_id, &indexed.item.remote_id)
            .and_then(|(_, path)| path);
        if let Some(path) = earlier
            && tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_file())
        {
            return Ok(path);
        }

        let provider = &source.provider;
        let url = provider
            .resolve_image(&indexed.item)
            .await
            .map_err(BrowseError::Resolve)?;
        let parsed = Url::parse(&url).map_err(|_| BrowseError::BadUrl(provider.name()))?;
        if let Err(err) = ready
            .deps
            .guard
            .check_url(&parsed, &provider.allowed_hosts())
        {
            tracing::warn!(source_id = %source.id, error = %err, "browse_use: image refused");
            return Err(err.into());
        }
        let http = provider
            .media_client()
            .unwrap_or_else(|| ready.deps.http.clone());
        let dir = download_dir.join(&source.id);
        let path = kakemono_sources::download(&http, &url, &dir)
            .await
            .map_err(|err| match GuardError::find_in(&err) {
                Some(guard) => {
                    tracing::warn!(source_id = %source.id, error = %guard, "browse_use: redirect refused");
                    BrowseError::Guard(guard.clone())
                }
                None => BrowseError::Download(err),
            })?;
        let ledger = dir.join(SEEN_LEDGER);
        if let Err(err) = record_seen(&ledger, &url).await {
            tracing::warn!(ledger = %ledger.display(), "can't record a downloaded url: {err}");
        }
        let cfg = self.config();
        if cfg.daemon.download_limit_mb_enabled {
            let limit = u64::from(cfg.daemon.download_limit_mb) * 1024 * 1024;
            files::trim_to_mb(download_dir, limit, &self.protect().file(path.clone())).await;
        }
        self.inner.browser.index().set_downloaded(
            &indexed.source_id,
            &indexed.item.remote_id,
            path.clone(),
        );
        Ok(path)
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
