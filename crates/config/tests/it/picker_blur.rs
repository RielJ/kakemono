//! `[picker] blur_nsfw` (picker-browse Task 25): default, load/edit round-trip and template.

use std::path::Path;

use kakemono_config::defaults_for_home;
use kakemono_config::edit::apply_patch;
use kakemono_config::load::{load_with_home, parse_str};
use kakemono_config::paths::TEMPLATE;
use serde_json::json;

const HOME: &str = "/home/golden";

fn home() -> &'static Path {
    Path::new(HOME)
}

/// The lines of `[picker]` in `text`, up to the next blank line or section header.
fn picker_section(text: &str) -> Vec<&str> {
    text.lines()
        .skip_while(|l| l.trim() != "[picker]")
        .skip(1)
        .take_while(|l| !l.trim().is_empty() && !l.trim_start().starts_with('['))
        .collect()
}

#[test]
fn defaults_blur_nsfw() {
    assert!(defaults_for_home(home()).picker.blur_nsfw);
}

#[test]
fn config_without_the_key_blurs_nsfw() {
    let cfg = parse_str("[picker]\nposition = \"top\"\n", home()).unwrap();
    assert!(cfg.picker.blur_nsfw);
}

#[test]
fn blur_nsfw_false_round_trips_through_load_and_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[picker]\n# keep me\nblur_nsfw = false\n").unwrap();
    assert!(!load_with_home(&path, dir.path()).unwrap().picker.blur_nsfw);

    apply_patch(&path, &json!({ "picker": { "blur_nsfw": true } })).unwrap();
    assert!(load_with_home(&path, dir.path()).unwrap().picker.blur_nsfw);

    apply_patch(&path, &json!({ "picker": { "blur_nsfw": false } })).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "[picker]\n# keep me\nblur_nsfw = false\n");
    assert!(!load_with_home(&path, dir.path()).unwrap().picker.blur_nsfw);
}

#[test]
fn template_parses_with_blur_nsfw_on() {
    let cfg = parse_str(TEMPLATE, home()).unwrap();
    assert!(cfg.picker.blur_nsfw);
}

#[test]
fn template_documents_blur_nsfw_under_picker() {
    let section = picker_section(TEMPLATE);
    let at = section
        .iter()
        .position(|l| *l == "# blur_nsfw = true")
        .unwrap_or_else(|| panic!("no `# blur_nsfw = true` under [picker]: {section:#?}"));
    let comment = at.checked_sub(1).map(|i| section[i]).unwrap_or_default();
    assert!(
        comment.starts_with("# ") && !comment.contains('='),
        "`# blur_nsfw = true` needs a one-line comment above it, got {comment:?}"
    );
}
