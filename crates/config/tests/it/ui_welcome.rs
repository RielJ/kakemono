//! First-run onboarding: `[ui] show_welcome`, the one template value for new installs
//! (`show_welcome = true`) that `Config::default()` does not share. Rotation, rotation on a
//! theme switch, background downloads and the light appearance are defaults for everyone, so
//! the template only documents them.

use std::path::Path;

use kakemono_config::edit::apply_patch;
use kakemono_config::load::{load_with_home, parse_str};
use kakemono_config::paths::TEMPLATE;
use kakemono_config::{Appearance, Config, defaults_for_home};
use serde_json::json;

const HOME: &str = "/home/golden";

fn home() -> &'static Path {
    Path::new(HOME)
}

fn template() -> Config {
    parse_str(TEMPLATE, home()).expect("the template parses")
}

#[test]
fn template_turns_rotation_off() {
    assert!(!template().daemon.rotation_enabled);
}

#[test]
fn template_documents_rotation_off_commented() {
    assert!(TEMPLATE.lines().any(|l| l == "# rotation_enabled = false"));
}

#[test]
fn template_has_no_active_rotation_enabled() {
    let doc: toml::Table = TEMPLATE.parse().expect("the template is valid TOML");
    let daemon = doc["daemon"].as_table().expect("an active [daemon] table");
    assert!(!daemon.contains_key("rotation_enabled"));
}

#[test]
fn defaults_keep_rotation_off() {
    assert!(!defaults_for_home(home()).daemon.rotation_enabled);
}

#[test]
fn a_config_without_the_key_does_not_rotate() {
    let cfg = parse_str("[daemon]\ninterval = \"1h\"\n", home()).unwrap();
    assert!(!cfg.daemon.rotation_enabled);
}

#[test]
fn defaults_keep_background_downloads_off() {
    assert!(!defaults_for_home(home()).daemon.download_enabled);
}

#[test]
fn template_keeps_background_downloads_off() {
    assert!(!template().daemon.download_enabled);
}

#[test]
fn template_documents_download_enabled_commented_with_on_demand_note() {
    let lines: Vec<&str> = TEMPLATE.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == "# download_enabled = false")
        .expect("a commented `# download_enabled = false`");
    assert_eq!(
        lines[at - 1],
        "# Download new images in the background. When off, Next downloads one image on demand."
    );
}

#[test]
fn template_keeps_change_on_start_at_its_default() {
    assert!(!template().daemon.change_on_start);
}

#[test]
fn template_leaves_change_on_start_commented() {
    assert!(TEMPLATE.lines().any(|l| l == "# change_on_start = false"));
}

#[test]
fn template_keeps_the_light_appearance() {
    assert_eq!(template().ui.appearance, Appearance::Light);
}

#[test]
fn template_documents_appearance_commented_with_the_choices() {
    let lines: Vec<&str> = TEMPLATE.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == "# appearance = \"light\"")
        .expect("a commented `# appearance = \"light\"`");
    assert!(
        lines[at - 1].contains(r#""light", "dark" or "system""#),
        "{}",
        lines[at - 1]
    );
}

#[test]
fn defaults_do_not_rotate_on_a_theme_switch() {
    assert!(!defaults_for_home(home()).daemon.rotate_on_theme_change);
}

#[test]
fn template_does_not_rotate_on_a_theme_switch() {
    assert!(!template().daemon.rotate_on_theme_change);
}

#[test]
fn template_documents_rotate_on_theme_change_commented() {
    let lines: Vec<&str> = TEMPLATE.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == "# rotate_on_theme_change = false")
        .expect("a commented `# rotate_on_theme_change = false`");
    assert!(lines[at - 1].starts_with("# Pick a new wallpaper when the system switches"));
}

#[test]
fn a_config_can_still_turn_rotate_on_theme_change_on() {
    let cfg = parse_str("[daemon]\nrotate_on_theme_change = true\n", home()).unwrap();
    assert!(cfg.daemon.rotate_on_theme_change);
}

#[test]
fn template_shows_the_welcome() {
    assert!(template().ui.show_welcome);
}

#[test]
fn template_ui_table_holds_only_show_welcome() {
    let doc: toml::Table = TEMPLATE.parse().expect("the template is valid TOML");
    let ui = doc["ui"].as_table().expect("an active [ui] table");
    assert_eq!(ui.keys().collect::<Vec<_>>(), ["show_welcome"]);
}

#[test]
fn defaults_keep_change_on_start_off() {
    assert!(!defaults_for_home(home()).daemon.change_on_start);
}

#[test]
fn defaults_keep_the_light_appearance() {
    assert_eq!(defaults_for_home(home()).ui.appearance, Appearance::Light);
}

#[test]
fn defaults_do_not_show_the_welcome() {
    assert!(!defaults_for_home(home()).ui.show_welcome);
}

#[test]
fn a_config_without_the_key_does_not_show_the_welcome() {
    let cfg = parse_str("[ui]\nappearance = \"dark\"\n", home()).unwrap();
    assert!(!cfg.ui.show_welcome);
}

#[test]
fn show_welcome_round_trips_through_load_and_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[ui]\n# keep me\nshow_welcome = true\n").unwrap();
    assert!(load_with_home(&path, dir.path()).unwrap().ui.show_welcome);

    apply_patch(&path, &json!({ "ui": { "show_welcome": false } })).unwrap();
    assert!(!load_with_home(&path, dir.path()).unwrap().ui.show_welcome);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "[ui]\n# keep me\nshow_welcome = false\n");
}
