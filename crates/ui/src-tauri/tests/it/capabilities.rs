//! The Tauri ACL: each window gets only the commands and plugin permissions its page uses.
//!
//! `build.rs` hands [`COMMANDS`] to `tauri_build::AppManifest`, so every app command is denied
//! unless a capability allows it. These tests keep `capabilities/*.json`, the
//! `generate_handler!` list in `lib.rs`, and the frontend's `invoke` calls in step.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use kakemono_ui::ipc_commands::COMMANDS;
use serde_json::Value;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The capability files, by identifier.
fn capabilities() -> Vec<Value> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(crate_dir().join("capabilities"))
        .expect("capabilities dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| serde_json::from_str(&read(p)).expect("capability json"))
        .collect()
}

fn capability(identifier: &str) -> Value {
    capabilities()
        .into_iter()
        .find(|c| c["identifier"] == identifier)
        .unwrap_or_else(|| panic!("no capability {identifier:?}"))
}

/// Permission identifiers of a capability (scoped entries by their `identifier`).
fn permissions(cap: &Value) -> BTreeSet<String> {
    cap["permissions"]
        .as_array()
        .expect("permissions array")
        .iter()
        .map(|p| {
            p.as_str()
                .or_else(|| p["identifier"].as_str())
                .expect("permission id")
                .to_owned()
        })
        .collect()
}

/// App commands a capability allows (`allow-get-config` → `get_config`).
fn allowed_commands(cap: &Value) -> BTreeSet<String> {
    permissions(cap)
        .iter()
        .filter(|p| !p.contains(':'))
        .map(|p| {
            p.strip_prefix("allow-")
                .unwrap_or_else(|| panic!("unexpected app permission {p:?}"))
                .replace('-', "_")
        })
        .collect()
}

fn windows(cap: &Value) -> Vec<&str> {
    cap["windows"]
        .as_array()
        .expect("windows array")
        .iter()
        .map(|w| w.as_str().expect("window label"))
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn capabilities_one_per_window() {
    let caps = capabilities();
    let labels: Vec<Vec<&str>> = caps.iter().map(windows).collect();
    assert_eq!(caps.len(), 2, "one capability per window");
    assert!(labels.contains(&vec![kakemono_ui::windows::SETTINGS_LABEL]));
    assert!(labels.contains(&vec![kakemono_ui::windows::PICKER_LABEL]));
}

#[test]
fn capabilities_handler_list_matches_the_acl_manifest() {
    let lib = read(&crate_dir().join("src/lib.rs"));
    let start = lib.find("generate_handler![").expect("generate_handler!") + 18;
    let end = start + lib[start..].find(']').expect("closing ]");
    let registered: BTreeSet<String> = lib[start..end]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|path| path.rsplit("::").next().unwrap_or(path).to_owned())
        .collect();
    assert_eq!(registered, set(COMMANDS));
}

#[test]
fn capabilities_allow_only_known_commands_and_every_command_somewhere() {
    let known = set(COMMANDS);
    let mut allowed = BTreeSet::new();
    for cap in capabilities() {
        let cmds = allowed_commands(&cap);
        let unknown: Vec<_> = cmds.difference(&known).collect();
        assert!(
            unknown.is_empty(),
            "{}: unknown {unknown:?}",
            cap["identifier"]
        );
        allowed.extend(cmds);
    }
    let unused: Vec<_> = known.difference(&allowed).collect();
    assert!(
        unused.is_empty(),
        "registered but no window may call: {unused:?}"
    );
}

#[test]
fn capabilities_picker_gets_only_what_the_picker_calls() {
    let picker = capability("picker");
    assert_eq!(
        allowed_commands(&picker),
        set(&[
            "app_info",
            "browse_page",
            "browse_use",
            "daemon_connection",
            "favorite",
            "fetch_dropped_url",
            "get_config",
            "hide_picker",
            "import_dropped_file",
            "reset_picker_position",
            "set_file",
            "status",
            "tab_thumbnails",
            "thumbnails",
            "trash",
        ])
    );
    assert_eq!(
        permissions(&picker)
            .into_iter()
            .filter(|p| p.contains(':'))
            .collect::<BTreeSet<_>>(),
        set(&["core:default", "core:window:allow-start-dragging"])
    );
}

#[test]
fn capabilities_settings_plugin_permissions_are_the_used_ones() {
    let settings = capability("settings");
    assert_eq!(
        permissions(&settings)
            .into_iter()
            .filter(|p| p.contains(':'))
            .collect::<BTreeSet<_>>(),
        set(&[
            "core:default",
            "core:window:allow-hide",
            "dialog:allow-open"
        ])
    );
    for settings_only in ["save_config", "set_start_at_login", "delete_source_files"] {
        assert!(allowed_commands(&settings).contains(settings_only));
        assert!(!allowed_commands(&capability("picker")).contains(settings_only));
    }
}

#[test]
fn capabilities_cover_every_command_the_frontend_invokes() {
    let src = crate_dir().join("../frontend/src/shared");
    let mut invoked = BTreeSet::new();
    for file in ["api.ts", "useDaemonEvents.ts"] {
        let text = read(&src.join(file));
        for chunk in text.split("invoke<").skip(1) {
            let name = chunk
                .split_once("('")
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(name, _)| name.to_owned())
                .expect("invoke<T>('name'");
            invoked.insert(name);
        }
    }
    assert!(!invoked.is_empty());
    let known = set(COMMANDS);
    let missing: Vec<_> = invoked.difference(&known).collect();
    assert!(
        missing.is_empty(),
        "the frontend calls unregistered {missing:?}"
    );
}

#[test]
fn capabilities_csp_has_no_unsafe_inline() {
    // The `<style>` blocks in the HTML pages get a Tauri nonce at build time, which makes
    // `'unsafe-inline'` dead in CSP3 webviews, and React `style={{}}` goes through CSSOM,
    // which CSP does not block. So the token would only mislead.
    let conf: Value = serde_json::from_str(&read(&crate_dir().join("tauri.conf.json"))).unwrap();
    let csp = &conf["app"]["security"]["csp"];
    assert!(csp.is_object(), "{csp}");
    for (directive, sources) in csp.as_object().unwrap() {
        let sources = sources.as_str().unwrap();
        assert!(!sources.contains("unsafe-"), "{directive}: {sources}");
    }
}
