//! kakemono-ui: the tray, settings window and picker. A pure IPC client of the daemon; it
//! depends only on `kakemono-protocol` and `kakemono-config`.
//!
//! Startup is tray-only: no window opens unless asked for (`--settings`, `--picker`, the
//! tray menu, or a Linux session without a tray host). Closing the last window keeps the
//! app alive; only Quit exits.
//!
//! At startup the UI starts the daemon when it does not answer ([`lifecycle`]).

pub mod commands;
pub mod commands_desktop;
pub mod daemon_link;
pub mod events;
pub mod lifecycle;
pub mod picker;
pub mod picker_place;
pub mod thumbs;
pub mod tray;
pub mod windows;

use std::sync::Arc;

use tauri::{App, AppHandle, Manager, RunEvent, Runtime};

use crate::daemon_link::DaemonLink;
use crate::windows::Page;

/// Command-line flags the UI understands. A second launch forwards its argv to the running
/// instance (single-instance), which handles them the same way.
pub fn page_from_args<S: AsRef<str>>(args: &[S]) -> Option<Page> {
    args.iter().skip(1).find_map(|arg| match arg.as_ref() {
        "--settings" => Some(Page::Settings),
        "--picker" => Some(Page::Picker),
        _ => None,
    })
}

/// Runs the app until Quit.
pub fn run() -> anyhow::Result<()> {
    init_tracing();
    // Before Tauri starts: may re-exec under XWayland (`[picker] force_x11`).
    picker::relaunch_under_x11_if_asked();

    let link = Arc::new(DaemonLink::from_environment()?);
    tracing::info!(endpoint = %link.endpoint(), "daemon endpoint");

    let builder = thumbs::register(tauri::Builder::default());
    let app = builder
        // Must be the first plugin: it exits a second instance before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            open_requested(app, page_from_args(&argv).unwrap_or(Page::Settings));
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(link)
        .manage(picker::PickerState::detect())
        .manage(Arc::new(thumbs::Thumbs::detect()))
        .manage(events::LiveState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::status,
            commands::next,
            commands::prev,
            commands::pause,
            commands::resume,
            commands::favorite,
            commands::trash,
            commands::set_file,
            commands::thumbnails,
            commands::doctor,
            commands::daemon_running,
            commands_desktop::desktop_backend,
            commands_desktop::test_desktop,
            commands_desktop::detect_desktop,
            lifecycle::ensure_daemon,
            lifecycle::start_at_login,
            lifecycle::set_start_at_login,
            commands::reveal,
            commands::open_url,
            commands::app_info,
            events::daemon_connection,
            picker::show_picker,
            picker::hide_picker,
            picker::reset_picker_position,
        ])
        .setup(setup)
        .build(tauri::generate_context!())?;

    app.run(|_app, event| {
        // `code: None` means the last window closed; keep running in the tray.
        // `app.exit(n)` (Quit) passes `Some(n)` and is allowed through.
        if let RunEvent::ExitRequested {
            api, code: None, ..
        } = event
        {
            api.prevent_exit();
        }
    });
    Ok(())
}

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let handle = app.handle().clone();
    let has_tray = tauri::async_runtime::block_on(tray::tray_supported());
    if has_tray {
        tray::build(&handle)?;
    } else {
        tracing::warn!(
            "no StatusNotifierWatcher on the session bus; running without a tray icon \
             (on GNOME install the AppIndicator extension)"
        );
    }

    // Live updates; its own connection, never the shared link.
    let link = app.state::<Arc<DaemonLink>>().inner().clone();
    events::spawn(&handle, link.endpoint().clone());
    // Start the daemon if nothing answers; the live-update task connects once it is up.
    lifecycle::spawn_at_startup(link);

    // Pre-create the picker hidden so showing it is instant.
    if let Err(err) = picker::create(&handle) {
        tracing::error!(error = %err, "cannot create the picker window");
    }

    let argv: Vec<String> = std::env::args().collect();
    match page_from_args(&argv) {
        Some(page) => open_requested(&handle, page),
        // Without a tray there is no other way in, so show settings.
        None if !has_tray => open_requested(&handle, Page::Settings),
        None => {}
    }
    Ok(())
}

fn open_requested<R: Runtime>(app: &AppHandle<R>, page: Page) {
    if let Err(err) = windows::show(app, page) {
        tracing::error!(error = %err, page = page.entry(), "cannot open window");
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("KAKEMONO_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // A second init (tests) is harmless; ignore it.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_select_a_page_and_argv0_is_ignored() {
        assert_eq!(page_from_args(&["kakemono-ui"]), None);
        assert_eq!(
            page_from_args(&["kakemono-ui", "--picker"]),
            Some(Page::Picker)
        );
        assert_eq!(
            page_from_args(&["kakemono-ui", "--settings"]),
            Some(Page::Settings)
        );
        assert_eq!(
            page_from_args(&["--picker"]),
            None,
            "argv[0] is the program"
        );
        assert_eq!(
            page_from_args(&["kakemono-ui", "--verbose", "--picker"]),
            Some(Page::Picker)
        );
    }
}
