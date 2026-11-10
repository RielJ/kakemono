// `build.rs` `include!`s this file, so it holds plain comments and one const only.
//
// Every app command registered in `lib.rs`'s `generate_handler!`, by name.
//
// `build.rs` `include!`s this file and passes the list to `tauri_build::AppManifest`, so each
// command gets an `allow-<name>` permission and is denied to any window whose capability
// (`capabilities/*.json`) does not grant it. `tests/it/capabilities.rs` checks that this list,
// the handler list and the capabilities agree.

/// App command names, as `#[tauri::command]` registers them.
pub const COMMANDS: &[&str] = &[
    "get_config",
    "save_config",
    "status",
    "next",
    "prev",
    "pause",
    "resume",
    "favorite",
    "trash",
    "set_file",
    "thumbnails",
    "tab_thumbnails",
    "check_source",
    "download_now",
    "delete_source_files",
    "open_source_folder",
    "fetch_dropped_url",
    "import_dropped_file",
    "desktop_backend",
    "test_desktop",
    "detect_desktop",
    "ensure_daemon",
    "start_at_login",
    "set_start_at_login",
    "reveal",
    "open_folder",
    "app_info",
    "daemon_connection",
    "show_picker",
    "hide_picker",
    "reset_picker_position",
];
