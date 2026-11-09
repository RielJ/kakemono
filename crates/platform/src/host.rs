//! `host()`: owns the process main thread for the platform and runs the daemon on tokio (plan §B).
//!
//! - macOS: `macos::mainloop::host` (NSApplication on the main thread, tokio on workers).
//! - Windows: `windows::host` (message window on the main thread, tokio on a worker).
//! - Everything else: [`default_host`], plain tokio, with `linux::open_platform` on Linux and an
//!   unconfigured platform elsewhere.

use std::future::Future;
use std::time::Duration;

use kakemono_config::schema::DesktopConfig;

use crate::api::Platform;

/// How long runtime shutdown waits for leftover blocking tasks after `f` returns.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Owns the process main thread for the platform's needs (an NSApplication run loop on macOS,
/// a hidden message window on Windows, plain tokio on Linux) and runs `f` on a tokio runtime.
///
/// Call it from `main`, on the main thread, once. Returns `f`'s result.
pub fn host<F, Fut>(cfg: &DesktopConfig, f: F) -> anyhow::Result<()>
where
    F: FnOnce(Platform) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send,
{
    #[cfg(target_os = "macos")]
    {
        crate::macos::mainloop::host(cfg, f)
    }
    #[cfg(target_os = "windows")]
    {
        crate::windows::host(cfg, f)
    }
    #[cfg(target_os = "linux")]
    {
        default_host(crate::linux::open_platform(cfg), f)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let platform = Platform::unconfigured_for(cfg);
        default_host(async move { platform }, f)
    }
}

/// Plain tokio host: build a multi-thread runtime, await `open` inside it (so the opener can use
/// tokio, D-Bus, `spawn`), then run `f(platform)` to completion on the calling thread.
///
/// `open` is only polled inside the runtime. Blocking tasks still running after `f` returns get
/// a short grace period, then the runtime is dropped.
pub fn default_host<P, F, Fut>(open: P, f: F) -> anyhow::Result<()>
where
    P: Future<Output = Platform>,
    F: FnOnce(Platform) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("kakemono-rt")
        .build()?;
    let result = runtime.block_on(async move {
        let platform = open.await;
        tracing::info!(backend = ?platform.setter.info(), "platform opened");
        f(platform).await
    });
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    result
}

/// The OS openers must return `Send` futures: the macOS and Windows hosts spawn them onto a
/// runtime worker. This fails to compile if a later change makes one `!Send`.
#[allow(dead_code)]
fn assert_openers_are_send(cfg: &DesktopConfig) {
    fn is_send<T: Send>(_: &T) {}
    #[cfg(target_os = "macos")]
    is_send(&crate::macos::open_platform(cfg));
    #[cfg(target_os = "windows")]
    is_send(&crate::windows::open_platform(cfg));
    #[cfg(target_os = "linux")]
    is_send(&crate::linux::open_platform(cfg));
    let _ = cfg;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Fit, SetError, Theme};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn default_host_runs_f_with_the_opened_platform() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = Arc::clone(&ran);
        default_host(
            async { Platform::unconfigured("hint", Theme::Dark) },
            move |p| async move {
                // Inside a multi-thread runtime: spawning works.
                tokio::spawn(async {}).await?;
                assert_eq!(p.signals.theme(), Theme::Dark);
                let err = p.setter.set(Path::new("/x.jpg"), Fit::Fill).await;
                assert!(matches!(err, Err(SetError::Unconfigured { .. })));
                ran2.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
        assert!(ran.load(Ordering::SeqCst));
    }

    #[test]
    fn default_host_returns_fs_error() {
        let err = default_host(
            async { Platform::unconfigured("hint", Theme::Light) },
            |_| async { anyhow::bail!("boom") },
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn opener_runs_inside_the_runtime() {
        default_host(
            async {
                // Would panic outside a runtime.
                let _ = tokio::runtime::Handle::current();
                Platform::unconfigured("hint", Theme::Light)
            },
            |_| async { Ok(()) },
        )
        .unwrap();
    }

    // `host()` itself takes over the main thread on macOS/Windows, so only exercise it on Linux.
    #[cfg(target_os = "linux")]
    #[test]
    fn host_runs_f_on_linux() {
        host(&DesktopConfig::default(), |p| async move {
            let _ = p.setter.info();
            Ok(())
        })
        .unwrap();
    }
}
