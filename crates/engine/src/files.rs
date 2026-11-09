//! Favorites, trash and download-cache trimming (plan §D).
//!
//! TODO(task 20): port Go `copyFile` (collision suffix), `Favorite`, `Trash` (rename, else
//! copy + remove across devices) and `trimCache` (keep the newest `cache_size`, never the current
//! image or the last 2 history entries). Task 19 only fixed the signatures: `engine.rs` already
//! calls these from `Engine::{favorite, trash}` and after every new download.

use std::path::{Path, PathBuf};

use crate::engine::EngineError;
use crate::state::State;

/// Copies `src` into `favorites_dir` and returns the new file's path. Errors use
/// `EngineError::Files` (rendered as `manager: <message>`). Stub until Task 20.
pub(crate) async fn favorite(src: &Path, favorites_dir: &Path) -> Result<PathBuf, EngineError> {
    let _ = (src, favorites_dir);
    Err(EngineError::Files(
        "favorite is not implemented yet".to_owned(),
    ))
}

/// Moves `src` into `trash_dir` and returns its new path. The engine then runs `next`.
/// Stub until Task 20.
pub(crate) async fn trash(src: &Path, trash_dir: &Path) -> Result<PathBuf, EngineError> {
    let _ = (src, trash_dir);
    Err(EngineError::Files(
        "trash is not implemented yet".to_owned(),
    ))
}

/// Deletes the oldest images in `download_dir` beyond `cache_size` (0 = unlimited). `state` is
/// the state after the new image was pushed (its `current` and history are protected). Failures
/// are logged, never returned. No-op until Task 20.
pub(crate) async fn trim_cache(download_dir: &Path, cache_size: u32, state: &State) {
    let _ = (download_dir, cache_size, state);
}
