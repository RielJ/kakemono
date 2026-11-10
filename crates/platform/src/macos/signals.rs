//! macOS `SystemSignals`:
//!
//! - `NSWorkspaceActiveSpaceDidChangeNotification` → [`Signal::SpaceChanged`] (always emitted; the
//!   engine re-applies only when `[daemon] reapply_on_space_change = true`)
//! - `NSWorkspaceDidWakeNotification` → [`Signal::Resumed`]
//! - `NSApplicationDidChangeScreenParametersNotification` → [`Signal::DisplaysChanged`]
//! - theme: `[desktop] theme` when set, otherwise dark-light (polls `AppleInterfaceStyle` every
//!   second) → [`Signal::ThemeChanged`], deduplicated.
//! - screen size: the largest `NSScreen` frame × `backingScaleFactor`, read on the main thread
//!   when the observers are registered and again before each `DisplaysChanged`, and cached so
//!   [`SystemSignals::screen_size`] never blocks.
//!
//! The observers are block observers registered on the main thread. They live until the macOS
//! host's run loop ends ([`remove_observers`]).

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use block2::RcBlock;
use kakemono_config::schema::DesktopConfig;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSScreen, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidWakeNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSNotificationName};
use tokio::sync::broadcast;

use super::mainloop::on_main;
use crate::api::{Signal, SystemSignals, Theme};
use crate::hub::{SignalHub, mode_theme};

/// How long registering the observers may wait for the main thread.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);

/// macOS signals. Cheap to share; the theme watcher thread stops after the last clone is dropped
/// (at the next theme change).
pub struct MacSignals {
    shared: Arc<Shared>,
}

struct Shared {
    hub: SignalHub,
    /// The largest screen in pixels, packed as `width << 32 | height`; `0` when unknown.
    screen: AtomicU64,
}

impl Shared {
    fn screen(&self) -> Option<(u32, u32)> {
        match self.screen.load(Ordering::SeqCst) {
            0 => None,
            packed => Some(((packed >> 32) as u32, packed as u32)),
        }
    }

    fn store_screen(&self, size: Option<(u32, u32)>) {
        let packed = size.map_or(0, |(w, h)| (u64::from(w) << 32) | u64::from(h));
        self.screen.store(packed, Ordering::SeqCst);
    }

    /// Re-read the largest screen. Keeps the last known size when AppKit lists no usable
    /// screen (e.g. briefly while displays reconfigure). Main thread only (`mtm`).
    fn refresh_screen(&self, mtm: MainThreadMarker) {
        let screens = NSScreen::screens(mtm);
        let size = largest_pixels(screens.iter().map(|screen| {
            let frame = screen.frame();
            (
                frame.size.width,
                frame.size.height,
                screen.backingScaleFactor(),
            )
        }));
        match size {
            Some(size) => self.store_screen(Some(size)),
            None => tracing::debug!("NSScreen lists no usable screen; keeping the last size"),
        }
    }
}

/// The largest of `(width_pt, height_pt, scale)` screens by pixel area, in whole pixels. Ties keep
/// the first. Screens with a non-finite or non-positive size are skipped.
fn largest_pixels(screens: impl IntoIterator<Item = (f64, f64, f64)>) -> Option<(u32, u32)> {
    let mut best: Option<(u32, u32)> = None;
    for (width, height, scale) in screens {
        let px = |points: f64| {
            let pixels = (points * scale).round();
            // `as` saturates; the range check rejects NaN, zero and absurd sizes.
            (pixels >= 1.0 && pixels <= f64::from(u32::MAX)).then_some(pixels as u32)
        };
        let (Some(w), Some(h)) = (px(width), px(height)) else {
            continue;
        };
        let area = |(w, h): (u32, u32)| u64::from(w) * u64::from(h);
        if best.is_none_or(|b| area((w, h)) > area(b)) {
            best = Some((w, h));
        }
    }
    best
}

impl MacSignals {
    /// Start the theme watcher and register the notification observers on the main thread.
    ///
    /// Never fails: without a running main run loop (outside [`host`]) the observers are not
    /// registered in time and only theme changes are reported.
    ///
    /// [`host`]: crate::host::host
    pub async fn start(cfg: &DesktopConfig) -> Self {
        let signals = Self::with_theme_source(cfg.theme);
        // Weak: if the hop times out, the queued closure must not keep the signals alive.
        let shared = Arc::downgrade(&signals.shared);
        match on_main(REGISTER_TIMEOUT, move |mtm| add_observers(mtm, &shared)).await {
            Ok(()) => tracing::debug!("macOS space, wake and screen observers registered"),
            Err(err) => tracing::warn!(%err, "macOS space, wake and screen signals are disabled"),
        }
        signals
    }

    /// Signals with the theme from the override, or from dark-light plus a watcher thread.
    fn with_theme_source(theme: kakemono_config::schema::ThemeOverride) -> Self {
        if let Some(theme) = Theme::from_override(theme) {
            return Self::new(theme);
        }
        let initial = match dark_light::detect() {
            Ok(mode) => mode_theme(mode).unwrap_or(Theme::Light),
            Err(err) => {
                tracing::warn!(%err, "could not read the macOS appearance; assuming light");
                Theme::Light
            }
        };
        let signals = Self::new(initial);
        spawn_theme_watcher(Arc::downgrade(&signals.shared));
        signals
    }

    /// Signals with a fixed starting theme and nothing attached.
    fn new(theme: Theme) -> Self {
        Self {
            shared: Arc::new(Shared {
                hub: SignalHub::new(theme),
                screen: AtomicU64::new(0),
            }),
        }
    }
}

impl SystemSignals for MacSignals {
    fn theme(&self) -> Theme {
        self.shared.hub.theme()
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.shared.hub.subscribe()
    }

    fn screen_size(&self) -> Option<(u32, u32)> {
        self.shared.screen()
    }
}

/// Forward dark-light's change watcher (a 1 s poll thread) into `shared`. dark-light's `stream()`
/// is the same watcher behind a futures channel; reading it directly avoids a `futures` dep.
fn spawn_theme_watcher(shared: Weak<Shared>) {
    let watcher = match dark_light::subscribe() {
        Ok(watcher) => watcher,
        Err(err) => {
            tracing::warn!(%err, "macOS appearance changes will not be followed");
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("kakemono-theme".to_owned())
        .spawn(move || {
            for mode in watcher.iter() {
                let Some(shared) = shared.upgrade() else {
                    break;
                };
                // `Unspecified` keeps the last theme.
                if let Some(theme) = mode_theme(mode) {
                    shared.hub.update_theme(theme);
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(%err, "could not start the macOS appearance watcher");
    }
}

type Observer = (
    Retained<NSNotificationCenter>,
    Retained<ProtocolObject<dyn NSObjectProtocol>>,
);

thread_local! {
    /// Observer tokens, kept on the main thread (they are not `Send`) until [`remove_observers`].
    static OBSERVERS: RefCell<Vec<Observer>> = const { RefCell::new(Vec::new()) };
}

/// Read the screen size, then register the Space, wake and screen-parameter observers. Main
/// thread only (`mtm`). Does nothing if the signals were already dropped.
fn add_observers(mtm: MainThreadMarker, shared: &Weak<Shared>) {
    let Some(strong) = shared.upgrade() else {
        return;
    };
    strong.refresh_screen(mtm);
    let tx = strong.hub.sender();
    drop(strong);
    let workspace_center = NSWorkspace::sharedWorkspace().notificationCenter();
    let default_center = NSNotificationCenter::defaultCenter();
    // SAFETY: AppKit's exported notification-name constants; valid for the life of the process.
    let (space, wake, screens) = unsafe {
        (
            NSWorkspaceActiveSpaceDidChangeNotification,
            NSWorkspaceDidWakeNotification,
            NSApplicationDidChangeScreenParametersNotification,
        )
    };
    let table: [(&Retained<NSNotificationCenter>, &NSNotificationName, Signal); 3] = [
        (&workspace_center, space, Signal::SpaceChanged),
        (&workspace_center, wake, Signal::Resumed),
        (&default_center, screens, Signal::DisplaysChanged),
    ];
    let mut tokens = Vec::with_capacity(table.len());
    for (center, name, signal) in table {
        let tx = tx.clone();
        let shared = Weak::clone(shared);
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            tracing::debug!(?signal, "macOS notification");
            // AppKit posts the screen-parameter change on the main thread. Refresh before
            // sending so receivers see the new size. Off the main thread, keep the old size.
            if signal == Signal::DisplaysChanged
                && let (Some(mtm), Some(shared)) = (MainThreadMarker::new(), shared.upgrade())
            {
                shared.refresh_screen(mtm);
            }
            let _ = tx.send(signal);
        });
        // SAFETY: `name` is a valid notification name; no sender filter (`nil` object); `nil`
        // queue delivers on the posting thread. The block sends `Signal` (Copy) on a
        // `broadcast::Sender` (`Send + Sync`) and touches AppKit only behind a
        // `MainThreadMarker` check, so it is safe on any thread.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        tokens.push((Retained::clone(center), token));
    }
    OBSERVERS.with_borrow_mut(|observers| observers.extend(tokens));
}

/// Unregister every observer added by [`MacSignals::start`]. Called by the macOS host after the
/// run loop ends. Main thread only (`mtm`).
pub(crate) fn remove_observers(mtm: MainThreadMarker) {
    let _ = mtm;
    for (center, token) in OBSERVERS.with_borrow_mut(std::mem::take) {
        // SAFETY: `token` is the observer object `addObserverForName:...` returned for `center`.
        unsafe { center.removeObserver(token.as_ref()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kakemono_config::schema::ThemeOverride;

    #[test]
    fn theme_changes_are_deduplicated() {
        let signals = MacSignals::new(Theme::Light);
        let mut rx = signals.subscribe();
        assert!(!signals.shared.hub.update_theme(Theme::Light));
        assert!(signals.shared.hub.update_theme(Theme::Dark));
        assert!(!signals.shared.hub.update_theme(Theme::Dark));
        assert!(signals.shared.hub.update_theme(Theme::Light));
        assert_eq!(rx.try_recv().unwrap(), Signal::ThemeChanged(Theme::Dark));
        assert_eq!(rx.try_recv().unwrap(), Signal::ThemeChanged(Theme::Light));
        assert!(rx.try_recv().is_err());
        assert_eq!(signals.theme(), Theme::Light);
    }

    #[test]
    fn override_fixes_the_theme() {
        let signals = MacSignals::with_theme_source(ThemeOverride::Dark);
        assert_eq!(signals.theme(), Theme::Dark);
        // No watcher thread holds a reference.
        assert_eq!(Arc::weak_count(&signals.shared), 0);
    }

    #[test]
    fn largest_screen_in_pixels() {
        // A 1512×982 pt Retina built-in (2x) beats a 1920×1080 pt external (1x).
        assert_eq!(
            largest_pixels([(1920.0, 1080.0, 1.0), (1512.0, 982.0, 2.0)]),
            Some((3024, 1964))
        );
        // Fractional sizes round; empty and degenerate screens are ignored.
        assert_eq!(largest_pixels([(1440.5, 900.0, 2.0)]), Some((2881, 1800)));
        assert_eq!(largest_pixels([]), None);
        assert_eq!(
            largest_pixels([(0.0, 900.0, 2.0), (f64::NAN, 1.0, 1.0)]),
            None
        );
        // Ties keep the first (the main screen comes first in `NSScreen.screens`).
        assert_eq!(
            largest_pixels([(1080.0, 1920.0, 1.0), (1920.0, 1080.0, 1.0)]),
            Some((1080, 1920))
        );
    }

    #[test]
    fn screen_size_is_cached_and_packed() {
        let signals = MacSignals::new(Theme::Light);
        assert_eq!(signals.screen_size(), None);
        signals.shared.store_screen(Some((6016, 3384)));
        assert_eq!(signals.screen_size(), Some((6016, 3384)));
        signals.shared.store_screen(None);
        assert_eq!(signals.screen_size(), None);
    }

    // Outside the host nothing drains the main queue: `start` must still return, with the
    // override theme and no OS signals.
    #[tokio::test]
    async fn start_without_a_run_loop_degrades() {
        tokio::time::pause();
        let cfg = DesktopConfig {
            theme: ThemeOverride::Light,
            ..DesktopConfig::default()
        };
        let signals = MacSignals::start(&cfg).await;
        assert_eq!(signals.theme(), Theme::Light);
    }
}
