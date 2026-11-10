//! The theme and signal hub shared by the macOS, Linux and Windows `SystemSignals` (audit M5).

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::broadcast;

use crate::api::{Signal, Theme};

/// Broadcast capacity. Signals are rare; a lagging subscriber re-reads `theme()` (contract).
pub(crate) const CAPACITY: usize = 64;

/// The current theme plus the [`Signal`] channel. `ThemeChanged` is sent only on a real change.
#[derive(Debug)]
pub(crate) struct SignalHub {
    tx: broadcast::Sender<Signal>,
    dark: AtomicBool,
}

impl SignalHub {
    pub(crate) fn new(theme: Theme) -> Self {
        Self {
            tx: broadcast::channel(CAPACITY).0,
            dark: AtomicBool::new(theme == Theme::Dark),
        }
    }

    pub(crate) fn theme(&self) -> Theme {
        if self.dark.load(Ordering::SeqCst) {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    /// Record `theme`; broadcast `ThemeChanged` only if it differs from the current one.
    /// Returns whether it changed.
    pub(crate) fn update_theme(&self, theme: Theme) -> bool {
        let was_dark = self.dark.swap(theme == Theme::Dark, Ordering::SeqCst);
        let changed = was_dark != (theme == Theme::Dark);
        if changed {
            tracing::info!(%theme, "system theme changed");
            self.send(Signal::ThemeChanged(theme));
        }
        changed
    }

    /// Broadcast `signal`. No subscriber yet is fine.
    pub(crate) fn send(&self, signal: Signal) {
        let _ = self.tx.send(signal);
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.tx.subscribe()
    }

    /// A sender for callbacks that must not hold the hub (macOS notification blocks).
    #[cfg(target_os = "macos")]
    pub(crate) fn sender(&self) -> broadcast::Sender<Signal> {
        self.tx.clone()
    }
}

/// A dark-light reading as a theme. `Unspecified` ("no preference") is `None`, and each OS
/// decides: Linux reads it as light (the portal's `color-scheme` 0), macOS keeps the last theme.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) fn mode_theme(mode: dark_light::Mode) -> Option<Theme> {
    match mode {
        dark_light::Mode::Dark => Some(Theme::Dark),
        dark_light::Mode::Light => Some(Theme::Light),
        dark_light::Mode::Unspecified => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_changes_are_deduplicated() {
        let hub = SignalHub::new(Theme::Light);
        let mut rx = hub.subscribe();
        assert!(!hub.update_theme(Theme::Light));
        assert!(hub.update_theme(Theme::Dark));
        assert!(!hub.update_theme(Theme::Dark));
        assert_eq!(hub.theme(), Theme::Dark);
        hub.send(Signal::Resumed);
        assert_eq!(rx.try_recv().unwrap(), Signal::ThemeChanged(Theme::Dark));
        assert_eq!(rx.try_recv().unwrap(), Signal::Resumed);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unspecified_mode_has_no_theme() {
        assert_eq!(mode_theme(dark_light::Mode::Unspecified), None);
        assert_eq!(mode_theme(dark_light::Mode::Dark), Some(Theme::Dark));
        assert_eq!(mode_theme(dark_light::Mode::Light), Some(Theme::Light));
    }
}
