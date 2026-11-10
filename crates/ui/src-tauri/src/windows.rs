//! Opening the two webview windows on demand.
//!
//! The settings window is built the first time it is asked for and shown again afterwards.
//! The picker is pre-created hidden at startup and placed on every show (`picker.rs`).

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::picker;

/// Window label of the picker. Also its title, which compositor window rules match
/// (`picker::TITLE`).
pub const PICKER_LABEL: &str = "kakemono-picker";
/// Window label of the settings window.
pub const SETTINGS_LABEL: &str = "settings";

/// The pages of the Vite multi-page app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Settings,
    Picker,
}

impl Page {
    pub fn label(self) -> &'static str {
        match self {
            Self::Settings => SETTINGS_LABEL,
            Self::Picker => PICKER_LABEL,
        }
    }

    /// Entry HTML, relative to the dev server or the embedded `frontend/dist`.
    pub fn entry(self) -> &'static str {
        match self {
            Self::Settings => "settings.html",
            Self::Picker => "picker.html",
        }
    }
}

/// Shows the page's window and focuses it. Settings is created on first use; the picker is
/// placed by [`picker::show`].
pub fn show<R: Runtime>(app: &AppHandle<R>, page: Page) -> tauri::Result<WebviewWindow<R>> {
    match page {
        Page::Picker => picker::show(app),
        Page::Settings => {
            let window = match app.get_webview_window(SETTINGS_LABEL) {
                Some(window) => window,
                None => build_settings(app)?,
            };
            window.show()?;
            window.unminimize()?;
            window.set_focus()?;
            Ok(window)
        }
    }
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
        Page::Settings => {
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
