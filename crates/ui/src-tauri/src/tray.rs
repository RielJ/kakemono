//! The tray icon and its menu. Every action except the two window items and Quit is one IPC
//! request through the shared [`DaemonLink`].
//!
//! Failures are logged and shown in the tray tooltip; the menu never blocks on the daemon.
//! Task 45 adds live Pause/Resume labelling from `subscribe`; until then the label is read
//! from `status` at startup and after each toggle.

use std::sync::Arc;

use kakemono_protocol::types::Request;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Runtime};

use crate::daemon_link::{DaemonLink, LinkError};
use crate::windows::{self, Page};

pub const TRAY_ID: &str = "kakemono";
const TOOLTIP: &str = "kakemono";

/// Menu item ids. Stable: Task 45 looks up `TOGGLE_PAUSE` to relabel it.
pub mod ids {
    pub const NEXT: &str = "next";
    pub const PREV: &str = "prev";
    pub const TOGGLE_PAUSE: &str = "toggle_pause";
    pub const FAVORITE: &str = "favorite";
    pub const TRASH: &str = "trash";
    pub const SHOW_PICKER: &str = "show_picker";
    pub const SETTINGS: &str = "settings";
    pub const QUIT: &str = "quit";
}

/// What a menu click does. Parsing the id once keeps the handler exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Next,
    Prev,
    TogglePause,
    Favorite,
    Trash,
    ShowPicker,
    Settings,
    Quit,
}

impl Action {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            ids::NEXT => Self::Next,
            ids::PREV => Self::Prev,
            ids::TOGGLE_PAUSE => Self::TogglePause,
            ids::FAVORITE => Self::Favorite,
            ids::TRASH => Self::Trash,
            ids::SHOW_PICKER => Self::ShowPicker,
            ids::SETTINGS => Self::Settings,
            ids::QUIT => Self::Quit,
            _ => return None,
        })
    }
}

/// Label of the pause item for a known paused state.
pub fn pause_label(paused: bool) -> &'static str {
    if paused { "Resume" } else { "Pause" }
}

/// Handles to the menu items whose text changes at runtime.
pub struct TrayItems<R: Runtime> {
    pub toggle_pause: MenuItem<R>,
}

/// Builds the tray icon and menu and registers the click handler.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayIcon<R>> {
    let toggle_pause = MenuItem::with_id(
        app,
        ids::TOGGLE_PAUSE,
        pause_label(false),
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, ids::NEXT, "Next", true, None::<&str>)?,
            &MenuItem::with_id(app, ids::PREV, "Previous", true, None::<&str>)?,
            &toggle_pause,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, ids::FAVORITE, "Favorite", true, None::<&str>)?,
            &MenuItem::with_id(app, ids::TRASH, "Move to Trash", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, ids::SHOW_PICKER, "Show Picker", true, None::<&str>)?,
            &MenuItem::with_id(app, ids::SETTINGS, "Settings…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, ids::QUIT, "Quit kakemono", true, None::<&str>)?,
        ],
    )?;
    app.manage(TrayItems { toggle_pause });

    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(tauri::include_image!("icons/tray.png"))
        .icon_as_template(true)
        .tooltip(TOOLTIP)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event)
        .build(app)?;

    refresh_pause_label(app.clone());
    Ok(tray)
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let Some(action) = Action::from_id(event.id().as_ref()) else {
        tracing::warn!(id = event.id().as_ref(), "unknown tray menu id");
        return;
    };
    match action {
        Action::Next => send(app, Request::Next),
        Action::Prev => send(app, Request::Prev),
        Action::Favorite => send(app, Request::Favorite),
        Action::Trash => send(app, Request::Trash),
        Action::TogglePause => toggle_pause(app.clone()),
        Action::ShowPicker => show_window(app, Page::Picker),
        Action::Settings => show_window(app, Page::Settings),
        Action::Quit => app.exit(0),
    }
}

fn show_window<R: Runtime>(app: &AppHandle<R>, page: Page) {
    if let Err(err) = windows::show(app, page) {
        tracing::error!(error = %err, page = page.entry(), "cannot open window");
    }
}

fn link<R: Runtime>(app: &AppHandle<R>) -> Arc<DaemonLink> {
    app.state::<Arc<DaemonLink>>().inner().clone()
}

/// Fires one request without blocking the menu thread.
fn send<R: Runtime>(app: &AppHandle<R>, request: Request) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let label = format!("{request:?}");
        let result = link(&app).call(request).await.map(drop);
        report(&app, &label, result);
    });
}

/// Reads the current state and sends the opposite, so a stale label never inverts the
/// user's intent.
fn toggle_pause<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let link = link(&app);
        let result = async {
            let paused = link.status().await?.paused;
            let request = if paused {
                Request::Resume
            } else {
                Request::Pause
            };
            link.call(request).await?;
            Ok::<bool, LinkError>(!paused)
        }
        .await;
        if let Ok(paused) = result {
            set_pause_label(&app, paused);
        }
        report(&app, "toggle pause", result.map(drop));
    });
}

/// Sets the pause item from IPC `status`. Leaves the default label when the daemon is down.
pub fn refresh_pause_label<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let result = link(&app).status().await;
        if let Ok(status) = &result {
            set_pause_label(&app, status.paused);
        }
        report(&app, "status", result.map(drop));
    });
}

pub fn set_pause_label<R: Runtime>(app: &AppHandle<R>, paused: bool) {
    let Some(items) = app.try_state::<TrayItems<R>>() else {
        return;
    };
    if let Err(err) = items.toggle_pause.set_text(pause_label(paused)) {
        tracing::warn!(error = %err, "cannot relabel the pause item");
    }
}

/// Logs the outcome and mirrors daemon reachability in the tooltip, the one static cue a
/// tray has.
fn report<R: Runtime>(app: &AppHandle<R>, what: &str, result: Result<(), LinkError>) {
    let tooltip = match &result {
        Ok(()) => TOOLTIP.to_owned(),
        Err(err) if err.is_not_running() => {
            tracing::warn!(action = what, "daemon is not running");
            format!("{TOOLTIP}: daemon not running")
        }
        Err(err) => {
            tracing::warn!(action = what, error = %err, "daemon request failed");
            format!("{TOOLTIP}: {}", first_line(&err.to_string()))
        }
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID)
        && let Err(err) = tray.set_tooltip(Some(&tooltip))
    {
        tracing::debug!(error = %err, "cannot set tray tooltip");
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// Whether a tray can be shown. On Linux the icon goes through the StatusNotifierItem
/// protocol and needs a `org.kde.StatusNotifierWatcher` on the session bus (stock GNOME has
/// none without an extension). Elsewhere the tray always exists.
pub async fn tray_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        match status_notifier_watcher_present().await {
            Ok(present) => present,
            Err(err) => {
                tracing::warn!(error = %err, "cannot query the session bus for a tray host");
                false
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

#[cfg(target_os = "linux")]
async fn status_notifier_watcher_present() -> zbus::Result<bool> {
    let connection = zbus::Connection::session().await?;
    let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
    let name = zbus::names::BusName::try_from("org.kde.StatusNotifierWatcher")?;
    Ok(dbus.name_has_owner(name).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_id_parses_to_its_action() {
        let table = [
            (ids::NEXT, Action::Next),
            (ids::PREV, Action::Prev),
            (ids::TOGGLE_PAUSE, Action::TogglePause),
            (ids::FAVORITE, Action::Favorite),
            (ids::TRASH, Action::Trash),
            (ids::SHOW_PICKER, Action::ShowPicker),
            (ids::SETTINGS, Action::Settings),
            (ids::QUIT, Action::Quit),
        ];
        for (id, action) in table {
            assert_eq!(Action::from_id(id), Some(action), "{id}");
        }
        assert_eq!(Action::from_id("bogus"), None);
    }

    #[test]
    fn pause_label_names_the_next_action() {
        assert_eq!(pause_label(false), "Pause");
        assert_eq!(pause_label(true), "Resume");
    }

    #[test]
    fn tooltip_uses_the_first_line_of_multiline_errors() {
        assert_eq!(
            first_line("kakemono daemon is not running.\nStart it"),
            "kakemono daemon is not running."
        );
        assert_eq!(first_line(""), "");
    }
}
