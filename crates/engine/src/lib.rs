//! The wallpaper engine: history state, ops (next/prev/set/favorite/trash), the rotation loop,
//! file management, events and hooks (plan §A, §D).
//!
//! TODO(task 19): re-export `Engine`, `Deps`, `EngineError`.

pub mod engine;
pub mod events;
pub mod files;
pub mod hooks;
pub mod rotation;
pub mod state;
