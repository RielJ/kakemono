//! The OS seam: `WallpaperSetter` and `SystemSignals`, the main-thread `host()`, argv execution,
//! URL open / file reveal and service installation (plan §A, §B, §E, §F).

pub mod api;
pub mod deadline;
pub mod exec;
#[cfg(feature = "fake")]
pub mod fake;
pub mod host;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows", test))]
mod hub;
pub mod open;
#[cfg(any(target_os = "linux", target_os = "windows", test))]
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
mod screen;
pub mod service;

/// Linux backends. `linux::detect` (the env snapshot types and the pure resolver) compiles on
/// every OS so its table tests run on all CI runners; the rest is Linux-only.
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(all(test, not(target_os = "windows")))]
#[path = "windows/logic.rs"]
mod windows_logic;

pub use api::{
    BackendInfo, CommandStatus, Fit, ParseEnumError, Platform, SetError, Signal, StaticSignals,
    SystemSignals, Theme, UNCONFIGURED_HINT, UnconfiguredSetter, WallpaperSetter,
};
pub use host::{default_host, host};
