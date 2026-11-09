//! TODO(task 34): `open_platform(cfg)`, `ResilientSetter` (re-detect after repeated `SetError`s).

pub mod detect;

#[cfg(target_os = "linux")]
pub mod backends;
#[cfg(target_os = "linux")]
pub mod theme;

/// Build the Linux platform from `[desktop]`. Polled inside the tokio runtime; must stay `Send`.
///
/// TODO(task 34): run detection and construct the matching backend. Stub: unconfigured.
#[cfg(target_os = "linux")]
pub async fn open_platform(cfg: &wallpaperd_config::schema::DesktopConfig) -> crate::api::Platform {
    crate::api::Platform::unconfigured_for(cfg)
}
