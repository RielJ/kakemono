//! macOS `SystemSignals` (plan §F):
//!
//! - `NSWorkspaceActiveSpaceDidChangeNotification` → [`Signal::SpaceChanged`] (always emitted; the
//!   engine re-applies only when `[daemon] reapply_on_space_change = true`)
//! - `NSWorkspaceDidWakeNotification` → [`Signal::Resumed`]
//! - `NSApplicationDidChangeScreenParametersNotification` → [`Signal::DisplaysChanged`]
//! - theme: `[desktop] theme` when set, otherwise dark-light (polls `AppleInterfaceStyle` every
//!   second) → [`Signal::ThemeChanged`], deduplicated.
//!
//! The observers are block observers registered on the main thread. They live until the macOS
//! host's run loop ends ([`remove_observers`]).

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use block2::RcBlock;
use kakemono_config::schema::DesktopConfig;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidWakeNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSNotificationName};
use tokio::sync::broadcast;

use super::mainloop::on_main;
use crate::api::{Signal, SystemSignals, Theme};

/// Broadcast capacity, as in the fake.
const CAPACITY: usize = 64;

/// How long registering the observers may wait for the main thread.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);

/// macOS signals. Cheap to share; the theme watcher thread stops after the last clone is dropped
/// (at the next theme change).
pub struct MacSignals {
    shared: Arc<Shared>,
}

struct Shared {
    tx: broadcast::Sender<Signal>,
    dark: AtomicBool,
}

impl Shared {
    fn theme(&self) -> Theme {
        if self.dark.load(Ordering::SeqCst) {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    /// Record `theme`; broadcast `ThemeChanged` only if it differs from the current one.
    fn update_theme(&self, theme: Theme) -> bool {
        let was_dark = self.dark.swap(theme == Theme::Dark, Ordering::SeqCst);
        let changed = was_dark != (theme == Theme::Dark);
        if changed {
            let _ = self.tx.send(Signal::ThemeChanged(theme));
        }
        changed
    }
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
        let tx = signals.shared.tx.clone();
        match on_main(REGISTER_TIMEOUT, move |mtm| add_observers(mtm, &tx)).await {
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
            Ok(mode) => theme_from_mode(mode).unwrap_or(Theme::Light),
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
        let (tx, _) = broadcast::channel(CAPACITY);
        Self {
            shared: Arc::new(Shared {
                tx,
                dark: AtomicBool::new(theme == Theme::Dark),
            }),
        }
    }
}

impl SystemSignals for MacSignals {
    fn theme(&self) -> Theme {
        self.shared.theme()
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.shared.tx.subscribe()
    }
}

fn theme_from_mode(mode: dark_light::Mode) -> Option<Theme> {
    match mode {
        dark_light::Mode::Dark => Some(Theme::Dark),
        dark_light::Mode::Light => Some(Theme::Light),
        dark_light::Mode::Unspecified => None,
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
                if let Some(theme) = theme_from_mode(mode) {
                    shared.update_theme(theme);
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

/// Register the Space, wake and screen-parameter observers. Main thread only (`mtm`).
fn add_observers(mtm: MainThreadMarker, tx: &broadcast::Sender<Signal>) {
    let _ = mtm;
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
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            tracing::debug!(?signal, "macOS notification");
            let _ = tx.send(signal);
        });
        // SAFETY: `name` is a valid notification name; no sender filter (`nil` object); `nil`
        // queue delivers on the posting thread. The block only sends `Signal` (Copy) on a
        // `broadcast::Sender`, which is `Send + Sync`, so it is safe on any thread.
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
        assert!(!signals.shared.update_theme(Theme::Light));
        assert!(signals.shared.update_theme(Theme::Dark));
        assert!(!signals.shared.update_theme(Theme::Dark));
        assert!(signals.shared.update_theme(Theme::Light));
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
    fn unspecified_mode_is_ignored() {
        assert_eq!(theme_from_mode(dark_light::Mode::Unspecified), None);
        assert_eq!(theme_from_mode(dark_light::Mode::Dark), Some(Theme::Dark));
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
