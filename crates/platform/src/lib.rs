//! The OS seam: `WallpaperSetter` and `SystemSignals`, the main-thread `host()`, argv execution,
//! URL open / file reveal and service installation (plan §A, §B, §E, §F).
//!
//! TODO(task 7): re-export the seam types from `api` and `host`.

pub mod api;
pub mod exec;
#[cfg(feature = "fake")]
pub mod fake;
pub mod host;
pub mod open;
pub mod service;

/// Linux backends. `linux::detect` (the env snapshot types and the pure resolver) compiles on
/// every OS so its table tests run on all CI runners; the rest is Linux-only.
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
