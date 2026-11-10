//! `host()`: owns the process main thread for the platform and runs the daemon on tokio.
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

/// The macOS and Windows hosts: run the daemon ([`default_host`] with `open` and `f`) on a
/// `kakemono-host` worker thread while the calling thread runs `native_loop`.
///
/// `stop` is dropped on the worker once the daemon returns or panics; its `Drop` must make
/// `native_loop` return (`StopAppOnDrop`, `QuitOnDrop`). A panic in the daemon is re-raised here
/// after the loop ends.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) fn run_beside_native_loop<P, G, F, Fut>(
    open: P,
    stop: G,
    f: F,
    native_loop: impl FnOnce(),
) -> anyhow::Result<()>
where
    P: Future<Output = Platform> + Send,
    G: Send,
    F: FnOnce(Platform) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<()>>,
{
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("kakemono-host".to_owned())
            .spawn_scoped(scope, move || {
                let _stop = stop;
                default_host(open, f)
            })?;
        native_loop();
        match worker.join() {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
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

    /// Ends the fake native loop when dropped, like `QuitOnDrop`.
    struct Quit(std::sync::mpsc::Sender<()>);

    impl Drop for Quit {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    fn run_with_fake_loop(
        f: impl FnOnce(Platform) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>
        + Send,
    ) -> (anyhow::Result<()>, bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        let looped = AtomicBool::new(false);
        let result = run_beside_native_loop(
            async { Platform::unconfigured("hint", Theme::Light) },
            Quit(tx),
            f,
            || {
                rx.recv().unwrap();
                looped.store(true, Ordering::SeqCst);
            },
        );
        (result, looped.load(Ordering::SeqCst))
    }

    #[test]
    fn native_loop_ends_when_the_daemon_returns() {
        let (result, looped) = run_with_fake_loop(|_| Box::pin(async { anyhow::bail!("boom") }));
        assert_eq!(result.unwrap_err().to_string(), "boom");
        assert!(looped);
    }

    #[test]
    #[should_panic(expected = "daemon panicked")]
    fn native_loop_ends_and_the_panic_propagates() {
        let _ = run_with_fake_loop(|_| Box::pin(async { panic!("daemon panicked") }));
    }

    // `host()` itself takes over the main thread on macOS/Windows, so only exercise it on Linux.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "runs the real Linux open_platform against this session; also needs KAKEMONO_REAL_DESKTOP_TEST=1"]
    fn host_runs_f_on_linux() {
        if std::env::var_os("KAKEMONO_REAL_DESKTOP_TEST").is_none_or(|v| v != "1") {
            eprintln!("skipped: set KAKEMONO_REAL_DESKTOP_TEST=1");
            return;
        }
        host(&DesktopConfig::default(), |p| async move {
            let _ = p.setter.info();
            Ok(())
        })
        .unwrap();
    }
}
