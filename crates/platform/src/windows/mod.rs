//! TODO(task 26): Windows platform: `host` (tokio on a worker, message window on main) and `open_platform`.
//!
//! Note: inside this crate, refer to the `windows` crate as `::windows` from the crate root
//! (`lib.rs`) to avoid ambiguity with this module.

pub mod setter;
pub mod signals;

/// Windows `host()` (see `crate::host::host`).
///
/// TODO(task 26): tokio on a worker thread, message window on the main thread. Stub: plain tokio.
pub fn host<F, Fut>(cfg: &wallpaperd_config::schema::DesktopConfig, f: F) -> anyhow::Result<()>
where
    F: FnOnce(crate::api::Platform) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    crate::host::default_host(open_platform(cfg), f)
}

/// Build the Windows platform. Polled inside the tokio runtime; must stay `Send`.
///
/// TODO(task 27): IDesktopWallpaper setter plus signals. Stub: unconfigured.
pub async fn open_platform(cfg: &wallpaperd_config::schema::DesktopConfig) -> crate::api::Platform {
    crate::api::Platform::unconfigured_for(cfg)
}
