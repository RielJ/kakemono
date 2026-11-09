//! Local directory provider (port of Go `internal/providers/local`).
//!
//! Scans the configured directories (recursively unless `recursive = false`) for image files
//! ([`is_image_file`]) and returns them in random order. `image_url` is the absolute file path,
//! so the engine uses the file in place instead of downloading. The query is ignored.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use kakemono_config::schema::LocalConfig;
use rand::seq::SliceRandom;
use walkdir::WalkDir;

use crate::download::is_image_file;
use crate::provider::{Provider, ProviderError, WallpaperResult};

/// The local-directory provider.
#[derive(Debug, Clone)]
pub struct Local {
    cfg: LocalConfig,
}

impl Local {
    /// A provider over `cfg.dirs`. The config loader has already expanded `~`.
    pub fn new(cfg: LocalConfig) -> Self {
        Self { cfg }
    }
}

#[async_trait]
impl Provider for Local {
    fn name(&self) -> &'static str {
        "local"
    }

    fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    /// Never fails on a bad directory: missing, unreadable or non-directory entries in `dirs`
    /// are skipped, as in Go. No dirs or no images gives `Ok(vec![])`.
    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        if self.cfg.dirs.is_empty() {
            return Ok(Vec::new());
        }
        let dirs = self.cfg.dirs.clone();
        let recursive = self.cfg.recursive;
        // Walking a large tree blocks, so keep it off the async workers.
        tokio::task::spawn_blocking(move || {
            let mut paths: Vec<PathBuf> = dirs
                .iter()
                .flat_map(|dir| collect_images(dir, recursive))
                .collect();
            paths.shuffle(&mut rand::rng());
            paths.into_iter().filter_map(to_result).collect()
        })
        .await
        .map_err(|e| ProviderError::Other(format!("local: scan: {e}")))
    }
}

/// Absolute paths of the image files under `dir`; only its direct children when not
/// `recursive`. Returns nothing when `dir` is missing or not a directory. Unreadable entries are
/// skipped. Symlinks are not followed into directories (Go's `filepath.WalkDir` did the same);
/// a symlink is kept only when it points to a regular file.
pub fn collect_images(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let Ok(dir) = std::path::absolute(dir) else {
        return Vec::new();
    };
    match std::fs::metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            tracing::debug!(dir = %dir.display(), "local: not a directory, skipped");
            return Vec::new();
        }
        Err(e) => {
            tracing::debug!(dir = %dir.display(), error = %e, "local: directory skipped");
            return Vec::new();
        }
    }

    let mut walk = WalkDir::new(&dir).min_depth(1);
    if !recursive {
        walk = walk.max_depth(1);
    }
    walk.into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            let ft = entry.file_type();
            if ft.is_dir() || !is_image_file(entry.path()) {
                return false;
            }
            !ft.is_symlink() || entry.path().is_file()
        })
        .map(walkdir::DirEntry::into_path)
        .collect()
}

/// One image path as a result. `None` for paths that are not valid UTF-8, since `image_url` is a
/// `String` and a lossy conversion would name a file that does not exist.
fn to_result(path: PathBuf) -> Option<WallpaperResult> {
    let Some(image_url) = path.to_str().map(str::to_owned) else {
        tracing::debug!(path = %path.display(), "local: non-UTF-8 path skipped");
        return None;
    };
    let source_url = url::Url::from_file_path(&path)
        .map(String::from)
        .unwrap_or_else(|()| format!("file://{image_url}"));
    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(WallpaperResult {
        source_url,
        image_url,
        title,
        author: String::new(),
        source: "local".to_owned(),
    })
}
