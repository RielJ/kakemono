//! The wallpaper engine: the history state, the ops (next/prev/set/favorite/
//! trash, manual fetch and import), the image pool and its filters, the rotation loop, the
//! background downloader, file management, events and hooks.

pub mod browse;
pub mod downloader;
pub mod engine;
pub mod events;
pub mod facts;
pub mod files;
pub mod filters;
mod gotext;
pub mod hooks;
mod interval;
pub mod manual;
pub mod pool;
pub mod rotation;
pub mod state;

pub use engine::{
    Clock, Deps, Engine, EngineError, FALLBACK_SCREEN, Provenance, SourceOpError, SourcesFactory,
};
pub use events::EngineEvent;
pub use facts::{FactsCache, ImageFacts};
pub use filters::{FilterCtx, Filters};
pub use manual::ManualError;
