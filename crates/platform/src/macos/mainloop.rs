//! macOS `host()`: `NSApplication` on the real main thread, the daemon on tokio (plan §B, §F).
//!
//! The main thread runs `NSApplication::run` with the `Prohibited` activation policy (no Dock icon,
//! no menu bar). A scoped worker thread runs [`crate::host::default_host`] with
//! [`super::open_platform`] and `f`. When `f` returns (or panics), the worker posts `stop:` plus a
//! dummy event to the main queue, `run` returns, and `host` returns `f`'s result.
//!
//! Code that needs AppKit hops to the main thread with [`on_main`]. The closure gets a
//! [`MainThreadMarker`], so main-thread-only APIs stay checked by objc2's types.

use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType,
};
use objc2_foundation::NSPoint;

/// A hop to the main thread did not complete.
#[derive(Debug, thiserror::Error)]
pub enum MainHopError {
    /// The main queue did not run the closure in time. Usually the run loop is not running
    /// (the caller is not inside [`host`]) or the main thread is blocked.
    #[error("the main thread did not respond within {0:?} (is the kakemono host running?)")]
    Timeout(Duration),
    /// The closure was dropped without a result.
    #[error("the main-thread task was dropped before it finished")]
    Dropped,
}

/// Run `f` on the main thread (via the main dispatch queue) and await its result.
///
/// Never blocks the calling thread. The main queue is only drained while the main run loop runs,
/// that is inside [`host`]; elsewhere this fails with [`MainHopError::Timeout`]. On timeout, `f`
/// stays queued and may still run later, so `f` must be safe to run late (idempotent).
pub async fn on_main<R, F>(timeout: Duration, f: F) -> Result<R, MainHopError>
where
    F: FnOnce(MainThreadMarker) -> R + Send + 'static,
    R: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    DispatchQueue::main().exec_async(move || {
        // The main queue only runs on the main thread, so this is always `Some`. If it ever is
        // not, `tx` is dropped and the caller gets `Dropped` instead of undefined behaviour.
        if let Some(mtm) = MainThreadMarker::new() {
            let _ = tx.send(f(mtm));
        }
    });
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(MainHopError::Dropped),
        Err(_) => Err(MainHopError::Timeout(timeout)),
    }
}

/// macOS `host()` (see `crate::host::host`).
///
/// Must be called on the main thread; otherwise it returns an error without running `f`.
pub fn host<F, Fut>(cfg: &kakemono_config::schema::DesktopConfig, f: F) -> anyhow::Result<()>
where
    F: FnOnce(crate::api::Platform) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    let Some(mtm) = MainThreadMarker::new() else {
        anyhow::bail!("kakemono_platform::host must be called on the main thread on macOS");
    };
    let app = NSApplication::sharedApplication(mtm);
    // A background agent: no Dock icon, no menu bar, never becomes active.
    // An unbundled binary already starts background-only, and AppKit then
    // returns NO; only warn if the policy really is something else.
    if !app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited)
        && app.activationPolicy() != NSApplicationActivationPolicy::Prohibited
    {
        tracing::warn!(
            policy = ?app.activationPolicy(),
            "NSApplication refused the Prohibited activation policy"
        );
    }

    // `app.run()` returns after `stop_app` runs on the main queue, posted when the daemon ends.
    let result =
        crate::host::run_beside_native_loop(super::open_platform(cfg), StopAppOnDrop, f, || {
            app.run()
        });
    super::signals::remove_observers(mtm);
    result
}

/// Stops `NSApplication::run` when dropped. If `run` has not started yet, the queued block runs
/// as soon as it does, so the stop is never lost.
struct StopAppOnDrop;

impl Drop for StopAppOnDrop {
    fn drop(&mut self) {
        DispatchQueue::main().exec_async(|| {
            if let Some(mtm) = MainThreadMarker::new() {
                stop_app(mtm);
            }
        });
    }
}

/// `stop:` only takes effect after the run loop handles an event, so post a dummy one.
fn stop_app(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    );
    match event {
        Some(event) => app.postEvent_atStart(&event, true),
        None => tracing::warn!("could not create the event that stops NSApplication"),
    }
}

#[cfg(test)]
mod tests {
    use kakemono_config::schema::DesktopConfig;

    use super::*;

    // Test threads are never the main thread, so `host` must refuse without touching AppKit.
    #[test]
    fn host_refuses_off_the_main_thread() {
        let err = host(&DesktopConfig::default(), |_| async { Ok(()) }).unwrap_err();
        assert!(err.to_string().contains("main thread"), "{err}");
    }

    // Nothing drains the main queue in the test harness, so a hop times out instead of hanging.
    #[tokio::test]
    async fn on_main_times_out_without_a_run_loop() {
        let err = on_main(Duration::from_millis(50), |_| ())
            .await
            .unwrap_err();
        assert!(matches!(err, MainHopError::Timeout(_)), "{err}");
    }
}
