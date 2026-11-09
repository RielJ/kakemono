//! `detect` and `doctor` through the real binary (Task 37). Everything is isolated in a temp dir;
//! `--env-json` feeds a saved Linux session so the resolver runs on every OS.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn isolated_cmd(root: &Path) -> Command {
    let mut cmd = Command::cargo_bin("kakemono").unwrap();
    cmd.current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_RUNTIME_DIR", root.join("xdg-run"))
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .env("XDG_CACHE_HOME", root.join("xdg-cache"))
        .env("KAKEMONO_CONFIG", root.join("config.toml"))
        .env("KAKEMONO_SOCKET", root.join("no-daemon.sock"))
        .env_remove("VISUAL")
        .env_remove("EDITOR");
    cmd
}

const HYPRLAND: &str = r#"{
  "xdg_current_desktop": "Hyprland",
  "xdg_session_type": "wayland",
  "wayland_display": "wayland-1",
  "hyprland_instance_signature": "abc",
  "binaries": ["hyprctl", "hyprpaper"],
  "processes": ["hyprpaper"],
  "dbus_names": []
}"#;

const CONFIG: &str = r#"# my wallpapers
interval = "30m"

# desktop notes, keep me
[desktop]
backend = "auto" # trailing comment
fit = "fill"
"#;

fn setup(config: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("env.json"), HYPRLAND).unwrap();
    std::fs::write(dir.path().join("config.toml"), config).unwrap();
    dir
}

#[test]
fn detect_reports_without_writing() {
    let dir = setup(CONFIG);
    isolated_cmd(dir.path())
        .args(["detect", "--env-json", "env.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("session:   hyprland:wayland"))
        .stdout(predicate::str::contains("detected:  hyprpaper"));
    let after = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert_eq!(after, CONFIG);
}

#[test]
fn detect_write_saves_the_backend_and_keeps_comments() {
    let dir = setup(CONFIG);
    isolated_cmd(dir.path())
        .args(["detect", "--write", "--env-json", "env.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Saved backend = \"hyprpaper\""));
    let after = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    for kept in [
        "# my wallpapers",
        "# desktop notes, keep me",
        "fit = \"fill\"",
    ] {
        assert!(after.contains(kept), "lost {kept:?}:\n{after}");
    }
    assert!(after.contains("backend = \"hyprpaper\""), "{after}");
    assert!(
        after.contains("detected_session = \"hyprland:wayland\""),
        "{after}"
    );
    // The rewritten file still loads.
    isolated_cmd(dir.path())
        .args(["config", "path"])
        .assert()
        .success();
}

#[test]
fn detect_write_respects_managed_by_user_unless_forced() {
    let config = "[desktop]\nbackend = \"swaybg\"\nmanaged_by = \"user\"\n";
    let dir = setup(config);
    isolated_cmd(dir.path())
        .args(["detect", "--write", "--env-json", "env.json"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("managed_by = \"user\""));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
        config
    );
    isolated_cmd(dir.path())
        .args(["detect", "--write", "--force", "--env-json", "env.json"])
        .assert()
        .success();
    let after = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(after.contains("backend = \"hyprpaper\""), "{after}");
    assert!(after.contains("managed_by = \"user\""), "{after}");
}

#[test]
fn doctor_reports_locally_without_a_daemon() {
    let dir = setup(CONFIG);
    isolated_cmd(dir.path())
        .args(["doctor", "--env-json", "env.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("session:    hyprland:wayland"))
        .stdout(predicate::str::contains("hyprpaper"))
        .stdout(predicate::str::contains(
            "windowrulev2 = float, title:^(kakemono-picker)$",
        ))
        .stdout(predicate::str::contains("running:    no"));
}

#[test]
fn doctor_apply_without_a_daemon_fails() {
    let dir = setup(CONFIG);
    isolated_cmd(dir.path())
        .args(["doctor", "--apply", "--env-json", "env.json"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "daemon is not running; start it with `kakemono daemon`",
        ));
}
