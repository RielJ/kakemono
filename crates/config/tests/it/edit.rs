//! Comment-preserving edits and atomic writes (Task 6).

use std::path::Path;

use kakemono_config::edit::{apply_patch, patch, set_desktop};
use kakemono_config::{
    Backend, CommandSpec, ConfigError, DesktopConfig, Fit, ManagedBy, load_with_home,
};
use serde_json::json;

const COMMENTED: &str = r#"# My wallpaper config — keep this comment
[daemon]
# How often to rotate
interval = "30m" # trailing comment
cache_size = 50

[wallhaven]
enabled = true
# Search terms
queries = ["nature"]
my_unknown_key = "kept"

# Desktop section below
[desktop]
backend = "auto"
"#;

fn write(dir: &Path, text: &str) -> std::path::PathBuf {
    let p = dir.join("config.toml");
    std::fs::write(&p, text).unwrap();
    p
}

#[test]
fn comments_survive_json_patch() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), COMMENTED);
    apply_patch(
        &p,
        &json!({
            "daemon": { "interval": "1h", "rotate_on_theme_change": false },
            "wallhaven": { "queries": ["space", "city"], "my_unknown_key": null },
            "reddit": { "subreddits": ["EarthPorn"] },
            "bing": { "market": "de-DE" },
        }),
    )
    .unwrap();
    let out = std::fs::read_to_string(&p).unwrap();
    insta::assert_snapshot!("json_patch", out);

    let cfg = load_with_home(&p, dir.path()).unwrap();
    assert_eq!(cfg.daemon.interval, "1h");
    assert!(!cfg.daemon.rotate_on_theme_change);
    assert_eq!(cfg.bing.market, "de-DE");
    // COMMENTED is a legacy config, so loading migrates `reddit.subreddits` away; check the file.
    let doc: toml_edit::DocumentMut = out.parse().unwrap();
    let subs: Vec<_> = doc["reddit"]["subreddits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(subs, ["EarthPorn"]);
}

#[test]
fn comments_survive_set_desktop() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), COMMENTED);
    let desktop = DesktopConfig {
        backend: Backend::Swww,
        fit: Fit::Fill,
        set_command: Some(
            CommandSpec::new(vec!["swww".into(), "img".into(), "{path}".into()]).unwrap(),
        ),
        managed_by: ManagedBy::User,
        detected_session: Some("hyprland".into()),
        detected_at: None,
        ..DesktopConfig::default()
    };
    set_desktop(&p, &desktop).unwrap();
    let out = std::fs::read_to_string(&p).unwrap();
    insta::assert_snapshot!("set_desktop", out);

    let cfg = load_with_home(&p, dir.path()).unwrap();
    assert_eq!(cfg.desktop.backend, Backend::Swww);
    assert_eq!(cfg.desktop.set_command, desktop.set_command);
    assert_eq!(cfg.desktop.detected_session.as_deref(), Some("hyprland"));
}

#[test]
fn patch_creates_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub").join("config.toml");
    patch(&p, |doc| {
        doc["daemon"]["interval"] = toml_edit::value("2h");
        Ok(())
    })
    .unwrap();
    let cfg = load_with_home(&p, dir.path()).unwrap();
    assert_eq!(cfg.daemon.interval, "2h");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn invalid_patch_leaves_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), COMMENTED);
    // download_limit_mb must be an integer; the result would not load. (Not cache_size: COMMENTED
    // is a legacy config and loading migrates cache_size away.)
    let err = apply_patch(&p, &json!({ "daemon": { "download_limit_mb": "lots" } })).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidPatch { .. }), "{err:?}");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), COMMENTED);

    let err = apply_patch(&p, &json!(["not", "an", "object"])).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidPatch { .. }), "{err:?}");

    let err = patch(&p, |_| Err("nope".into())).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidPatch { .. }), "{err:?}");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), COMMENTED);

    // No temp files left behind.
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["config.toml"]);
}

#[cfg(unix)]
#[test]
fn patch_keeps_file_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), COMMENTED);
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
    apply_patch(&p, &json!({ "daemon": { "interval": "1h" } })).unwrap();
    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

#[test]
fn malformed_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "[daemon\n");
    let err = apply_patch(&p, &json!({})).unwrap_err();
    assert!(matches!(err, ConfigError::EditParse { .. }), "{err:?}");
}
