//! The tray icon and its menu. Every action except the two window items and Quit is one IPC
//! request through the shared [`DaemonLink`].
//!
//! Failures are logged and shown in the tray tooltip; the menu never blocks on the daemon.
//! The Pause/Resume label and the "daemon not running" tooltip are kept live by
//! [`crate::events`] from `subscribe`; a toggle also relabels at once.

use std::sync::Arc;

use kakemono_protocol::types::Request;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Runtime};

use crate::daemon_link::{DaemonLink, LinkError};
use crate::windows::{self, Page};

pub const TRAY_ID: &str = "kakemono";
const TOOLTIP: &str = "kakemono";

/// Menu item ids. Stable.
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
        Action::Trash => send(app, Request::Trash { expected: None }),
        Action::TogglePause => toggle_pause(app.clone()),
        Action::ShowPicker => windows::open(app, Page::Picker),
        Action::Settings => windows::open(app, Page::Settings),
        Action::Quit => {
            windows::mark_quitting(app);
            app.exit(0);
        }
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
    match &result {
        Ok(()) => set_problem(app, None),
        Err(err) if err.is_not_running() => {
            tracing::warn!(action = what, "daemon is not running");
            set_problem(app, Some("daemon not running"));
        }
        Err(err) => {
            tracing::warn!(action = what, error = %err, "daemon request failed");
            set_problem(app, Some(&err.to_string()));
        }
    }
}

/// Tooltip text: `kakemono`, or `kakemono: <first line of problem>`.
pub fn tooltip(problem: Option<&str>) -> String {
    match problem {
        None => TOOLTIP.to_owned(),
        Some(problem) => format!("{TOOLTIP}: {}", first_line(problem)),
    }
}

/// Shows `problem` in the tooltip, or clears it.
pub fn set_problem<R: Runtime>(app: &AppHandle<R>, problem: Option<&str>) {
    if let Some(tray) = app.tray_by_id(TRAY_ID)
        && let Err(err) = tray.set_tooltip(Some(&tooltip(problem)))
    {
        tracing::debug!(error = %err, "cannot set tray tooltip");
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// How long startup waits for the session bus to say whether a tray host exists. The check
/// runs on the main thread before any window opens.
/// `tray_available`: whether this session can show the tray icon ([`tray_supported`]). The
/// welcome window asks so it can say where kakemono lives.
#[tauri::command]
pub async fn tray_available() -> bool {
    tray_supported().await
}

#[cfg(target_os = "linux")]
pub const TRAY_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether a tray can be shown. On Linux the icon goes through the StatusNotifierItem
/// protocol and needs a `org.kde.StatusNotifierWatcher` on the session bus (stock GNOME has
/// none without an extension). Elsewhere the tray always exists.
/// On Linux the bus query gives up after `TRAY_CHECK_TIMEOUT` (no tray).
pub async fn tray_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        match tokio::time::timeout(TRAY_CHECK_TIMEOUT, status_notifier_watcher_present()).await {
            Ok(Ok(present)) => present,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "cannot query the session bus for a tray host");
                false
            }
            Err(_) => {
                tracing::warn!("the session bus did not answer the tray host query in time");
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
    fn tooltip_names_the_problem_or_just_the_app() {
        assert_eq!(tooltip(None), "kakemono");
        assert_eq!(
            tooltip(Some("daemon not running")),
            "kakemono: daemon not running"
        );
        assert_eq!(tooltip(Some("a\nb")), "kakemono: a");
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
