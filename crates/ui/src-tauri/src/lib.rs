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
pub mod commands_sources;
pub mod daemon_link;
pub mod error;
pub mod events;
/// The app command names that `build.rs` turns into ACL permissions.
pub mod ipc_commands;
pub mod lifecycle;
pub mod picker;
pub mod picker_place;
pub mod thumbs;
pub mod tray;
pub mod updater;
pub mod windows;

use std::sync::Arc;

use kakemono_config::paths::{ConfigSource, Paths};
use tauri::{App, AppHandle, Manager, RunEvent, Runtime};

use crate::daemon_link::DaemonLink;
use crate::windows::Page;

/// Passed by the login item, so a second launch from it can be told apart from the user's.
pub const AUTOSTART_FLAG: &str = "--autostart";

/// What a second launch (argv forwarded by single-instance) opens: its page flag, else the
/// Welcome window while it is open (`welcome_open`), else Settings. A login item that runs while
/// the app is already up opens nothing.
pub fn second_launch_page<S: AsRef<str>>(args: &[S], welcome_open: bool) -> Option<Page> {
    let from_login = args
        .iter()
        .skip(1)
        .any(|arg| arg.as_ref() == AUTOSTART_FLAG);
    match page_from_args(args) {
        Some(page) => Some(page),
        None if from_login => None,
        None if welcome_open => Some(Page::Welcome),
        None => Some(Page::Settings),
    }
}

/// Command-line flags the UI understands. A second launch forwards its argv to the running
/// instance (single-instance), which handles them the same way.
pub fn page_from_args<S: AsRef<str>>(args: &[S]) -> Option<Page> {
    args.iter().skip(1).find_map(|arg| match arg.as_ref() {
        "--settings" => Some(Page::Settings),
        "--picker" => Some(Page::Picker),
        _ => None,
    })
}

/// What the first launch opens: its page flag, else the Welcome window on a `first_start`
/// ([`is_first_start`]) unless the login item started it, else nothing.
pub fn startup_page<S: AsRef<str>>(args: &[S], first_start: bool) -> Option<Page> {
    let from_login = args
        .iter()
        .skip(1)
        .any(|arg| arg.as_ref() == AUTOSTART_FLAG);
    match page_from_args(args) {
        Some(page) => Some(page),
        None if first_start && !from_login => Some(Page::Welcome),
        None => None,
    }
}

/// True when the config the daemon will read does not exist yet. The daemon writes the template
/// for a missing default or explicit (`$KAKEMONO_CONFIG`) path, so this must be asked before
/// the daemon starts.
pub fn is_fresh(source: &ConfigSource) -> bool {
    match source {
        ConfigSource::Missing(_) => true,
        ConfigSource::Explicit(path) => !path.exists(),
        ConfigSource::Primary(_) => false,
    }
}

/// The very first start: the config is missing ([`is_fresh`]), or it still has
/// `[ui] show_welcome = true`, as the template writes it (the CLI or daemon ran first and
/// created it). A config that exists but can't be read or parsed is not a first start.
pub fn is_first_start(source: &ConfigSource) -> bool {
    if is_fresh(source) {
        return true;
    }
    match kakemono_config::load_or_default(source.path()) {
        Ok(config) => config.ui.show_welcome,
        Err(err) => {
            tracing::debug!(error = %err, "cannot read the config; not a first start");
            false
        }
    }
}

/// [`is_first_start`] for the config the UI and daemon locate. A lookup that fails counts as
/// not a first start.
fn config_is_first_start() -> bool {
    match Paths::detect() {
        Ok(paths) => is_first_start(&paths.locate_config(None)),
        Err(err) => {
            tracing::debug!(error = %err, "cannot locate the config; not a first start");
            false
        }
    }
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
            let welcome_open = app.get_webview_window(windows::WELCOME_LABEL).is_some();
            if let Some(page) = second_launch_page(&argv, welcome_open) {
                open_requested(app, page);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_FLAG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(link)
        .manage(picker::PickerState::detect())
        .manage(Arc::new(thumbs::Thumbs::detect()))
        .manage(Arc::new(commands_sources::BrowsePreviews::default()))
        .manage(events::LiveState::default())
        .manage(updater::PendingUpdate::default())
        .manage(windows::Quitting::default())
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
            commands::current_thumbnail,
            commands::tab_thumbnails,
            commands_sources::check_source,
            commands_sources::download_now,
            commands_sources::delete_source_files,
            commands_sources::open_source_folder,
            commands_sources::fetch_dropped_url,
            commands_sources::import_dropped_file,
            commands_sources::browse_page,
            commands_sources::browse_use,
            commands_desktop::desktop_backend,
            commands_desktop::test_desktop,
            commands_desktop::detect_desktop,
            lifecycle::ensure_daemon,
            lifecycle::start_at_login,
            lifecycle::set_start_at_login,
            commands::reveal,
            commands::open_folder,
            commands::app_info,
            events::daemon_connection,
            picker::show_picker,
            windows::open_welcome,
            tray::tray_available,
            picker::hide_picker,
            picker::reset_picker_position,
            picker::picker_resize_begin,
            picker::picker_resize,
            updater::check_update,
            updater::install_update,
            updater::open_update_page,
        ])
        .setup(setup)
        .build(tauri::generate_context!())?;

    app.run(|app, event| {
        // `code: None` means the last window closed; keep running in the tray.
        // `app.exit(n)` (Quit) passes `Some(n)` and is allowed through; it is marked so a
        // welcome window closing on the way out opens nothing.
        if let RunEvent::ExitRequested { api, code, .. } = event {
            match code {
                None => api.prevent_exit(),
                Some(_) => windows::mark_quitting(app),
            }
        }
    });
    Ok(())
}

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let handle = app.handle().clone();

    // Before the daemon starts: it writes the config when there is none.
    let first_start = config_is_first_start();

    // Live updates; its own connection, never the shared link.
    let link = app.state::<Arc<DaemonLink>>().inner().clone();
    events::spawn(&handle, link.endpoint().clone());
    // Start the daemon if nothing answers; the live-update task connects once it is up.
    lifecycle::spawn_at_startup(link);

    // Old thumbnails are deleted once per launch, on the blocking pool.
    let thumbs = app.state::<Arc<thumbs::Thumbs>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || match thumbs.prune_cache(thumbs::CACHE_MAX_AGE) {
        Ok(0) => {}
        Ok(removed) => tracing::debug!(removed, "pruned old thumbnails"),
        Err(err) => tracing::warn!(error = %err, "cannot prune the thumbnail cache"),
    });

    // Pre-create the picker hidden so showing it is instant.
    if let Err(err) = picker::create(&handle) {
        tracing::error!(error = %err, "cannot create the picker window");
    }

    let argv: Vec<String> = std::env::args().collect();
    let requested = startup_page(&argv, first_start);
    if let Some(page) = requested {
        open_requested(&handle, page);
    }
    // The tray check asks the session bus on Linux, so it runs off the main thread.
    tauri::async_runtime::spawn(start_tray(handle, requested.is_none()));
    Ok(())
}

/// Builds the tray if the desktop can show one. Without a tray there is no other way in, so
/// settings opens when `open_settings_without_tray` (no page was asked for on the command line).
async fn start_tray<R: Runtime>(app: AppHandle<R>, open_settings_without_tray: bool) {
    if tray::tray_supported().await {
        if let Err(err) = tray::build(&app) {
            tracing::error!(error = %err, "cannot build the tray icon");
        }
        return;
    }
    tracing::warn!(
        "no StatusNotifierWatcher on the session bus; running without a tray icon \
         (on GNOME install the AppIndicator extension)"
    );
    if open_settings_without_tray {
        open_requested(&app, Page::Settings);
    }
}

fn open_requested<R: Runtime>(app: &AppHandle<R>, page: Page) {
    windows::open(app, page);
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
        assert_eq!(page_from_args(&["kakemono-ui", AUTOSTART_FLAG]), None);
    }

    #[test]
    fn a_second_launch_opens_settings_unless_it_is_the_login_item() {
        assert_eq!(
            second_launch_page(&["kakemono-ui"], false),
            Some(Page::Settings)
        );
        assert_eq!(
            second_launch_page(&["kakemono-ui", "--picker"], false),
            Some(Page::Picker)
        );
        assert_eq!(
            second_launch_page(&["kakemono-ui", AUTOSTART_FLAG], false),
            None
        );
        assert_eq!(
            second_launch_page(&[AUTOSTART_FLAG], false),
            Some(Page::Settings)
        );
    }

    #[test]
    fn a_second_launch_while_welcome_is_open_focuses_welcome() {
        assert_eq!(
            second_launch_page(&["kakemono-ui"], true),
            Some(Page::Welcome)
        );
    }

    #[test]
    fn a_second_launch_page_flag_wins_over_an_open_welcome() {
        assert_eq!(
            second_launch_page(&["kakemono-ui", "--picker"], true),
            Some(Page::Picker)
        );
    }

    #[test]
    fn a_second_launch_from_the_login_item_leaves_welcome_alone() {
        assert_eq!(
            second_launch_page(&["kakemono-ui", AUTOSTART_FLAG], true),
            None
        );
    }

    #[test]
    fn a_first_start_opens_welcome() {
        assert_eq!(startup_page(&["kakemono-ui"], true), Some(Page::Welcome));
    }

    #[test]
    fn a_first_start_by_the_login_item_opens_nothing() {
        assert_eq!(startup_page(&["kakemono-ui", AUTOSTART_FLAG], true), None);
    }

    #[test]
    fn a_page_flag_wins_over_a_first_start() {
        assert_eq!(
            startup_page(&["kakemono-ui", "--picker"], true),
            Some(Page::Picker)
        );
    }

    #[test]
    fn an_existing_install_opens_nothing_without_a_flag() {
        assert_eq!(startup_page(&["kakemono-ui"], false), None);
    }

    #[test]
    fn an_existing_install_still_honours_a_page_flag() {
        assert_eq!(
            startup_page(&["kakemono-ui", "--settings"], false),
            Some(Page::Settings)
        );
    }

    #[test]
    fn a_missing_config_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(is_fresh(&ConfigSource::Missing(path)));
    }

    #[test]
    fn a_missing_explicit_config_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(is_fresh(&ConfigSource::Explicit(path)));
    }

    #[test]
    fn an_existing_config_is_not_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        assert!(!is_fresh(&ConfigSource::Explicit(path.clone())));
        assert!(!is_fresh(&ConfigSource::Primary(path)));
    }

    #[test]
    fn a_missing_config_is_a_first_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(is_first_start(&ConfigSource::Missing(path)));
    }

    #[test]
    fn a_config_with_show_welcome_is_a_first_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nshow_welcome = true\n").unwrap();
        assert!(is_first_start(&ConfigSource::Primary(path)));
    }

    #[test]
    fn the_template_is_a_first_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        kakemono_config::paths::init(&path).unwrap();
        assert!(is_first_start(&ConfigSource::Primary(path)));
    }

    #[test]
    fn a_config_without_show_welcome_is_not_a_first_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nappearance = \"dark\"\n").unwrap();
        assert!(!is_first_start(&ConfigSource::Primary(path)));
    }

    #[test]
    fn an_unreadable_config_is_not_a_first_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui\nshow_welcome = true\n").unwrap();
        assert!(!is_first_start(&ConfigSource::Explicit(path)));
    }
}
