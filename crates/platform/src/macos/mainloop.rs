//! TODO(task 24): `host`: tokio runtime plus NSApplication (ActivationPolicy::Prohibited) on the real main thread.

/// macOS `host()` (see `crate::host::host`).
///
/// TODO(task 24): run NSApplication on the main thread and `f` on tokio. Stub: plain tokio.
pub fn host<F, Fut>(cfg: &wallpaperd_config::schema::DesktopConfig, f: F) -> anyhow::Result<()>
where
    F: FnOnce(crate::api::Platform) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    crate::host::default_host(super::open_platform(cfg), f)
}
