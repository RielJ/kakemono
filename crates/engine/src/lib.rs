//! The wallpaper engine: history state, ops (next/prev/set/favorite/trash), the rotation loop,
//! file management, events and hooks (plan §A, §D).

pub mod downloader;
pub mod engine;
pub mod events;
pub mod facts;
pub mod files;
pub mod filters;
pub mod hooks;
pub mod manual;
pub mod pool;
pub mod rotation;
pub mod state;

pub use engine::{Clock, Deps, Engine, EngineError, FALLBACK_SCREEN, SourcesFactory};
pub use events::EngineEvent;
pub use facts::{FactsCache, ImageFacts};
pub use filters::{FilterCtx, Filters};
