//! TODO(task 24): macOS platform: `open_platform` and wiring of the NSApplication host and NSWorkspace setter.

pub mod mainloop;
pub mod setter;
pub mod signals;

/// Build the macOS platform. Polled inside the tokio runtime; must stay `Send`.
///
/// TODO(task 25): NSWorkspace setter plus signals. Stub: unconfigured.
pub async fn open_platform(cfg: &kakemono_config::schema::DesktopConfig) -> crate::api::Platform {
    crate::api::Platform::unconfigured_for(cfg)
}
