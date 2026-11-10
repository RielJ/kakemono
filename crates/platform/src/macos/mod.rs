//! macOS platform: the NSApplication main-thread host ([`mainloop`]), the NSWorkspace
//! setter ([`setter`]) and the Space/wake/screen/theme signals ([`signals`]).
//!
//! `[desktop] backend` and `set_command` are ignored on macOS: the native setter is always used,
//! as on Windows.

use std::sync::Arc;

use kakemono_config::schema::DesktopConfig;

use crate::api::Platform;

pub mod mainloop;
pub mod setter;
pub mod signals;

/// Build the macOS platform. Polled inside the tokio runtime; must stay `Send`.
///
/// Registers the signal observers on the main thread, so call it inside [`crate::host::host`].
/// Outside the host it still returns (after a timeout) with theme signals only.
pub async fn open_platform(cfg: &DesktopConfig) -> Platform {
    Platform {
        setter: Arc::new(setter::MacSetter::new()),
        signals: Arc::new(signals::MacSignals::start(cfg).await),
    }
}
