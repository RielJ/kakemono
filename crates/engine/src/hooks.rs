//! `[hooks] on_change` (plan §I).
//!
//! TODO(task 22): `hooks::fire`: `on_change` argv, cleared env plus pass-through list,
//! `Semaphore(4)`, 10 s timeout. The call site is already in `Engine::changed` (engine.rs).

use kakemono_platform::api::Theme;
use kakemono_protocol::types::WallpaperInfo;

use crate::engine::Engine;

/// Called by the engine after every wallpaper change, with the op lock still held. Must not
/// block or await: spawn the hook and return. `engine.inner.deps.runner` runs the command and
/// `engine.config().hooks` holds the argv. A no-op until Task 22.
pub(crate) fn on_change(engine: &Engine, info: &WallpaperInfo, theme: Theme) {
    let _ = (engine, info, theme);
}
