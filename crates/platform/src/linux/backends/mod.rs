//! Linux `WallpaperSetter` backends.
//!
//! `command`, `presets`, `persistent` and `gsettings` are plain argv/process code, so they compile
//! on every Unix and their tests run on macOS CI too. They only ship on Linux (`open_platform`).
//! The D-Bus backends (`kde`, `xfce`) are Linux-only.

pub mod command;
pub mod gsettings;
#[cfg(target_os = "linux")]
pub mod kde;
pub mod persistent;
pub mod presets;
#[cfg(target_os = "linux")]
pub mod xfce;
