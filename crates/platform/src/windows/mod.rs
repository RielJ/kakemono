//! Windows platform (plan §F): `host` (hidden signal window + message loop on the calling thread,
//! tokio on a worker) and `open_platform` (`IDesktopWallpaper` setter + registry/window signals).
//!
//! Threads:
//! - **host thread** (the caller, normally `main`): owns the hidden signal window and pumps its
//!   messages until the worker posts `WM_QUIT`. No COM.
//! - **`kakemono-host`**: runs the tokio runtime and the daemon (`default_host`).
//! - **`kakemono-desktop`**: an STA that owns `IDesktopWallpaper` ([`setter`]).
//! - **`kakemono-theme`**: blocks on `RegNotifyChangeKeyValue` ([`signals`]).
//!
//! Note: inside this crate, refer to the `windows` crate as `::windows` from the crate root
//! (`lib.rs`) to avoid ambiguity with this module.

pub mod logic;
pub mod setter;
pub mod signals;

use std::sync::Arc;

use kakemono_config::schema::DesktopConfig;

use crate::api::{Platform, UnconfiguredSetter};

/// Windows `host()` (see `crate::host::host`).
///
/// Call it on the main thread. (Windows has no main-thread requirement; the thread that calls
/// this owns the window and its message loop for the whole run.)
pub fn host<F, Fut>(cfg: &DesktopConfig, f: F) -> anyhow::Result<()>
where
    F: FnOnce(Platform) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    // Create the queue before the worker exists, so its WM_QUIT can't be posted to a thread that
    // has no queue yet.
    let host_thread = signals::ensure_message_queue();
    // Without the window the daemon still runs; it just gets no display/resume signals.
    let window = signals::SignalWindow::create()
        .inspect_err(|e| {
            tracing::warn!(error = %e, "cannot create the signal window; display/resume signals are off");
        })
        .ok();

    let result = std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("kakemono-host".to_owned())
            .spawn_scoped(scope, move || {
                // Ends the message loop below when the daemon returns or panics.
                let _quit = signals::QuitOnDrop(host_thread);
                crate::host::default_host(open_platform(cfg), f)
            })?;
        signals::run_message_loop();
        match worker.join() {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    });
    drop(window);
    result
}

/// Build the Windows platform. Polled inside the tokio runtime; must stay `Send`.
///
/// The desktop thread's COM start-up and the first registry read block briefly, so they run on
/// `spawn_blocking`.
pub async fn open_platform(cfg: &DesktopConfig) -> Platform {
    let cfg = cfg.clone();
    let opened = tokio::task::spawn_blocking(move || {
        let signals = signals::WinSignals::open(&cfg);
        let setter = setter::WinSetter::spawn();
        (signals, setter)
    })
    .await;
    match opened {
        Ok((signals, Ok(setter))) => Platform {
            setter: Arc::new(setter),
            signals: Arc::new(signals),
        },
        Ok((signals, Err(e))) => {
            tracing::error!(error = %e, "cannot start the Windows desktop thread");
            Platform {
                setter: Arc::new(UnconfiguredSetter::new(format!(
                    "the Windows desktop thread failed to start: {e}"
                ))),
                signals: Arc::new(signals),
            }
        }
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use ::windows::Win32::UI::Shell::{
        DWPOS_CENTER, DWPOS_FILL, DWPOS_FIT, DWPOS_STRETCH, DWPOS_TILE,
    };
    use ::windows::Win32::UI::WindowsAndMessaging::{
        PBT_APMRESUMEAUTOMATIC, WM_DISPLAYCHANGE, WM_POWERBROADCAST,
    };

    use super::*;

    /// The plain consts in `logic.rs` (which can't import `windows`) must match the `windows` crate.
    #[test]
    fn logic_consts_match_windows_crate() {
        assert_eq!(logic::DWPOS_CENTER, DWPOS_CENTER.0);
        assert_eq!(logic::DWPOS_TILE, DWPOS_TILE.0);
        assert_eq!(logic::DWPOS_STRETCH, DWPOS_STRETCH.0);
        assert_eq!(logic::DWPOS_FIT, DWPOS_FIT.0);
        assert_eq!(logic::DWPOS_FILL, DWPOS_FILL.0);
        assert_eq!(logic::WM_DISPLAYCHANGE, WM_DISPLAYCHANGE);
        assert_eq!(logic::WM_POWERBROADCAST, WM_POWERBROADCAST);
        assert_eq!(logic::PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMEAUTOMATIC);
    }

    /// Creates a hidden window and runs the loop on the test thread; does not touch the desktop.
    #[test]
    fn host_runs_f_and_returns() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = Arc::clone(&ran);
        host(&DesktopConfig::default(), move |p| async move {
            tokio::spawn(async {}).await?;
            assert_eq!(p.setter.info().name, "windows");
            ran2.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert!(ran.load(Ordering::SeqCst));
    }

    #[test]
    fn host_returns_fs_error() {
        let err = host(&DesktopConfig::default(), |_| async {
            anyhow::bail!("boom")
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    #[should_panic(expected = "daemon panicked")]
    fn host_propagates_a_panic_and_still_quits() {
        let _ = host(&DesktopConfig::default(), |_| async {
            panic!("daemon panicked")
        });
    }
}
