//! TODO(task 34): `open_platform(cfg)`, `ResilientSetter` (re-detect after repeated `SetError`s).

pub mod detect;

#[cfg(target_os = "linux")]
pub mod backends;
#[cfg(target_os = "linux")]
pub mod theme;
