//! Opening the webview windows on demand.
//!
//! The settings and welcome windows are built the first time they are asked for and shown
//! again afterwards. The picker is pre-created hidden at startup and placed on every show
//! (`picker.rs`). When the welcome window closes, Settings opens ([`after_welcome_closed`]).

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::commands::CommandError;
use crate::picker;

/// Window label of the picker. Also its title, which compositor window rules match
/// (`picker::TITLE`).
pub const PICKER_LABEL: &str = "kakemono-picker";
/// Window label of the settings window.
pub const SETTINGS_LABEL: &str = "settings";
/// Window label of the first-start welcome window.
pub const WELCOME_LABEL: &str = "welcome";

/// The pages of the Vite multi-page app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Settings,
    Picker,
    Welcome,
}

impl Page {
    pub fn label(self) -> &'static str {
        match self {
            Self::Settings => SETTINGS_LABEL,
            Self::Picker => PICKER_LABEL,
            Self::Welcome => WELCOME_LABEL,
        }
    }

    /// Entry HTML, relative to the dev server or the embedded `frontend/dist`.
    pub fn entry(self) -> &'static str {
        match self {
            Self::Settings => "settings.html",
            Self::Picker => "picker.html",
            Self::Welcome => "welcome.html",
        }
    }
}

/// Set once the app is on its way out (Quit, an update restart), so windows that close on the
/// way out open nothing. Managed app state.
#[derive(Debug, Default)]
pub struct Quitting(AtomicBool);

impl Quitting {
    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn get(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Marks the app as quitting ([`Quitting`]). Call before `exit` or `restart`.
pub fn mark_quitting<R: Runtime>(app: &AppHandle<R>) {
    if let Some(quitting) = app.try_state::<Quitting>() {
        quitting.set();
    }
}

/// What opens after the welcome window closes, however it closed (Finish, Skip, the close
/// button): Settings, unless the app is quitting. Settings is a single window, so after a
/// replay from Settings this only focuses it.
pub fn after_welcome_closed(quitting: bool) -> Option<Page> {
    (!quitting).then_some(Page::Settings)
}

/// Shows the page's window and focuses it. Settings and Welcome are created on first use; the
/// picker is placed by [`picker::show`].
pub fn show<R: Runtime>(app: &AppHandle<R>, page: Page) -> tauri::Result<WebviewWindow<R>> {
    let window = match page {
        Page::Picker => return picker::show(app),
        Page::Settings => match app.get_webview_window(SETTINGS_LABEL) {
            Some(window) => window,
            None => build_settings(app)?,
        },
        Page::Welcome => match app.get_webview_window(WELCOME_LABEL) {
            Some(window) => window,
            None => build_welcome(app)?,
        },
    };
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    Ok(window)
}

/// Opens `page` from a menu, a second launch or startup, logging a failure. The picker is shown
/// from an async task, so its config read stays off the main thread.
pub fn open<R: Runtime>(app: &AppHandle<R>, page: Page) {
    let log = move |err: tauri::Error| {
        tracing::error!(error = %err, page = page.entry(), "cannot open window");
    };
    match page {
        Page::Picker => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = picker::show_async(&app).await {
                    log(err);
                }
            });
        }
        Page::Settings | Page::Welcome => {
            if let Err(err) = show(app, page) {
                log(err);
            }
        }
    }
}

fn build_settings<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    WebviewWindowBuilder::new(
        app,
        SETTINGS_LABEL,
        WebviewUrl::App(Page::Settings.entry().into()),
    )
    .title("kakemono Settings")
    .inner_size(760.0, 560.0)
    .min_inner_size(640.0, 440.0)
    .center()
    .visible(false)
    // Image drops are HTML5 events in the page (`useImageDrop`); the native handler would
    // swallow them.
    .disable_drag_drop_handler()
    .build()
}

/// `open_welcome`: Settings → General → "Show welcome tips". Opens or focuses the welcome window
/// and writes nothing; closing it then focuses the Settings window that is already open.
#[tauri::command]
pub async fn open_welcome<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    // Async, like `show_picker`: a sync command runs on the main thread, and building a
    // window there deadlocks on Windows.
    show(&app, Page::Welcome)?;
    Ok(())
}

/// The first-start welcome window: fixed size, centred. When it is destroyed (any way of
/// closing it), Settings opens unless the app is quitting ([`after_welcome_closed`]). Settings is
/// opened from a task, not inside the window-event callback, because building a window there can
/// deadlock on Windows.
fn build_welcome<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let window = WebviewWindowBuilder::new(
        app,
        WELCOME_LABEL,
        WebviewUrl::App(Page::Welcome.entry().into()),
    )
    .title("Welcome to kakemono")
    // Screen 3, the longest, needs about 380 px; the page scrolls rather than clips past 440.
    .inner_size(560.0, 440.0)
    .resizable(false)
    .maximizable(false)
    .center()
    .visible(false)
    .build()?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if !matches!(event, WindowEvent::Destroyed) {
            return;
        }
        let quitting = handle.try_state::<Quitting>().is_some_and(|q| q.get());
        if let Some(page) = after_welcome_closed(quitting) {
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move { open(&handle, page) });
        }
    });
    Ok(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_welcome_window_opens_settings() {
        assert_eq!(after_welcome_closed(false), Some(Page::Settings));
    }

    #[test]
    fn quitting_while_the_welcome_window_is_open_opens_nothing() {
        assert_eq!(after_welcome_closed(true), None);
    }

    #[test]
    fn quitting_starts_unset_and_sticks_once_set() {
        let quitting = Quitting::default();
        assert!(!quitting.get());
        quitting.set();
        assert!(quitting.get());
    }

    #[test]
    fn welcome_has_its_own_label_and_page() {
        assert_eq!(Page::Welcome.label(), "welcome");
        assert_eq!(Page::Welcome.entry(), "welcome.html");
    }
}
