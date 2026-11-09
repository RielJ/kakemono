//! Opening the two webview windows on demand.
//!
//! The app starts with no windows (`tauri.conf.json` `app.windows` is empty). Each page is
//! built the first time it is asked for and shown again afterwards. Task 43 replaces the
//! picker part with a pre-created, positioned window (`picker.rs`).

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Window label and app_id/class of the picker. Compositor window rules match on it.
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

/// Shows the page's window, creating it on first use, and focuses it.
pub fn show<R: Runtime>(app: &AppHandle<R>, page: Page) -> tauri::Result<WebviewWindow<R>> {
    let window = match app.get_webview_window(page.label()) {
        Some(window) => window,
        None => build(app, page)?,
    };
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    Ok(window)
}

fn build<R: Runtime>(app: &AppHandle<R>, page: Page) -> tauri::Result<WebviewWindow<R>> {
    let builder =
        WebviewWindowBuilder::new(app, page.label(), WebviewUrl::App(page.entry().into()));
    let builder = match page {
        Page::Settings => builder
            .title("kakemono Settings")
            .inner_size(760.0, 560.0)
            .min_inner_size(640.0, 440.0)
            .center(),
        Page::Picker => builder
            .title("kakemono Picker")
            .inner_size(900.0, 168.0)
            .decorations(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible_on_all_workspaces(true)
            .center(),
    };
    builder.visible(false).build()
}
