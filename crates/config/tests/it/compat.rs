//! Go compatibility: defaults and template loading match what Go produced (fixtures captured in Task 1).

use std::path::{Path, PathBuf};

use kakemono_config::load::{load_or_default_with_home, load_with_home, parse_str};
use kakemono_config::{
    Backend, CommandSpec, Config, Fit, ManagedBy, PickerPlacement, PickerPosition, ThemeOverride,
    defaults_for_home,
};

const HOME: &str = "/home/golden";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn home() -> &'static Path {
    Path::new(HOME)
}

#[test]
fn defaults_match_go_defaults() {
    // go_defaults.toml is Go's `toml.Encode(config.Defaults())` with HOME=/home/golden.
    let go = load_with_home(&fixture("go_defaults.toml"), home()).expect("load go defaults");
    let mut ours = defaults_for_home(home());
    // Deviation: Go wrote the socket path explicitly; we leave it to the platform default.
    assert_eq!(ours.daemon.socket_path, None);
    assert_eq!(
        go.daemon.socket_path.as_deref(),
        Some(Path::new("/home/golden/.config/wallpaperd/daemon.sock"))
    );
    ours.daemon.socket_path.clone_from(&go.daemon.socket_path);
    // Go's defaults are a legacy config: loading migrates them to one enabled Bing row.
    assert_eq!(go.sources.len(), 1);
    assert_eq!(go.sources[0].kind, kakemono_config::SourceType::Bing);
    assert!(go.sources[0].enabled);
    ours.sources.clone_from(&go.sources);
    assert_eq!(ours, go);
}

#[test]
fn default_values_spelled_out() {
    let d = defaults_for_home(home());
    assert!(d.daemon.rotation_enabled);
    assert_eq!(d.daemon.interval, "30m");
    assert_eq!(
        d.daemon.interval_duration(),
        Ok(std::time::Duration::from_secs(1800))
    );
    assert_eq!(d.daemon.download_limit_mb, 500);
    assert!(d.daemon.download_limit_mb_enabled);
    assert_eq!(
        d.daemon.download_dir,
        Path::new("/home/golden/Pictures/Wallpapers/cache")
    );
    assert!(d.daemon.rotate_on_theme_change);
    assert!(!d.daemon.reapply_on_space_change);
    assert!(
        d.sources
            .iter()
            .any(|s| s.kind == kakemono_config::SourceType::Bing && s.enabled)
    );
    assert_eq!(d.bing.market, "en-US");
    assert_eq!(d.apod.api_key, "DEMO_KEY");
    assert_eq!(d.picker.position, PickerPosition::Bottom);
    assert_eq!(d.picker.placement, PickerPlacement::Edge);
    assert!(d.picker.close_on_blur);
    assert!(!d.picker.force_x11);
    assert_eq!(d.picker.custom, None);
    assert_eq!(d.desktop.backend, Backend::Auto);
    assert_eq!(d.desktop.fit, Fit::Fill);
    assert_eq!(d.desktop.managed_by, ManagedBy::Detect);
    assert_eq!(d.hooks.on_change, None);
}

#[test]
fn go_template_loads_like_go() {
    // go_template_loaded.toml is Go's `toml.Encode(config.Load(template))`. Go did not expand
    // socket_path; we do, so both sides go through our loader (which expands it) for comparison.
    let ours = load_with_home(&fixture("go_default_template.toml"), home()).expect("template");
    let go = load_with_home(&fixture("go_template_loaded.toml"), home()).expect("go loaded");
    assert_eq!(ours, go);

    // The template is a legacy config: loading migrates it, removing `[light]`, `[dark]`,
    // `reddit.subreddits` and the `enabled` flags (so those fall back to defaults) and leaving one
    // Bing row, the only enabled provider.
    let mut expected = defaults_for_home(home());
    expected.daemon.socket_path =
        Some(PathBuf::from("/home/golden/.config/wallpaperd/daemon.sock"));
    assert_eq!(ours.sources.len(), 1);
    assert_eq!(ours.sources[0].kind, kakemono_config::SourceType::Bing);
    expected.sources.clone_from(&ours.sources);
    assert_eq!(ours, expected);
}

#[test]
fn empty_file_is_defaults() {
    assert_eq!(parse_str("", home()).unwrap(), defaults_for_home(home()));
}

#[test]
fn missing_keys_in_present_section_keep_section_defaults() {
    let cfg = parse_str(
        "[wallhaven]\npurity = \"110\"\n[daemon]\ninterval = \"1h\"\n",
        home(),
    )
    .unwrap();
    let d = defaults_for_home(home());
    assert_eq!(cfg.wallhaven.purity, "110");
    assert_eq!(cfg.wallhaven.sorting, d.wallhaven.sorting);
    assert_eq!(cfg.wallhaven.categories, d.wallhaven.categories);
    assert_eq!(cfg.daemon.interval, "1h");
    assert_eq!(cfg.daemon.download_limit_mb, 500);
    assert_eq!(cfg.daemon.download_dir, d.daemon.download_dir);
}

#[test]
fn unknown_keys_and_sections_are_ignored() {
    let cfg = parse_str(
        "future = 1\n[daemon]\nnew_knob = true\n[mystery]\nx = 2\n",
        home(),
    )
    .unwrap();
    assert_eq!(cfg, defaults_for_home(home()));
}

#[test]
fn tilde_paths_are_expanded() {
    let cfg = parse_str(
        r#"
[daemon]
download_dir = "~/dl"
trash_dir = "~/trash"
socket_path = "~/s.sock"
[favorites]
dir = "~/fav"
[[sources]]
id = "dir-a"
type = "folder"
location = "~/a"
[[sources]]
id = "dir-abs"
type = "folder"
location = "/abs"
[[sources]]
id = "dir-home"
type = "folder"
location = "~"
"#,
        home(),
    )
    .unwrap();
    assert_eq!(cfg.daemon.download_dir, Path::new("/home/golden/dl"));
    assert_eq!(cfg.daemon.trash_dir, Path::new("/home/golden/trash"));
    assert_eq!(
        cfg.daemon.socket_path.as_deref(),
        Some(Path::new("/home/golden/s.sock"))
    );
    assert_eq!(cfg.favorites.dir, Path::new("/home/golden/fav"));
    let locations: Vec<_> = cfg.sources.iter().map(|s| s.location.as_str()).collect();
    assert_eq!(locations, ["/home/golden/a", "/abs", "/home/golden"]);
}

#[test]
fn desktop_section_from_plan_parses() {
    let cfg = parse_str(
        r#"
[desktop]
backend = "pcmanfm-qt"
set_command = ["awww", "img", "{path}", "--transition-type", "fade"]
persistent = true
fit = "tile"
theme = "dark"
managed_by = "user"
detected_session = "hyprland:wayland"
detected_at = "2026-11-09T17:46:15Z"

[hooks]
on_change = "sketchybar --trigger 'wallpaper changed'"

[picker]
placement = "custom"
custom = { x = 10, y = -20, monitor = "DP-1" }
force_x11 = true
"#,
        home(),
    )
    .unwrap();
    let d = &cfg.desktop;
    assert_eq!(d.backend, Backend::PcmanfmQt);
    assert_eq!(
        d.set_command.as_ref().map(CommandSpec::argv),
        Some(&["awww", "img", "{path}", "--transition-type", "fade"].map(String::from)[..])
    );
    assert!(d.persistent);
    assert_eq!(d.fit, Fit::Tile);
    assert_eq!(d.theme, ThemeOverride::Dark);
    assert_eq!(d.managed_by, ManagedBy::User);
    assert_eq!(d.detected_session.as_deref(), Some("hyprland:wayland"));
    assert_eq!(
        d.detected_at.map(|t| t.to_rfc3339()),
        Some("2026-11-09T17:46:15+00:00".to_owned())
    );
    assert_eq!(
        cfg.hooks.on_change.as_ref().map(CommandSpec::argv),
        Some(&["sketchybar", "--trigger", "wallpaper changed"].map(String::from)[..])
    );
    assert_eq!(cfg.picker.placement, PickerPlacement::Custom);
    let custom = cfg.picker.custom.as_ref().unwrap();
    assert_eq!(
        (custom.x, custom.y, custom.monitor.as_str()),
        (10, -20, "DP-1")
    );
    assert!(cfg.picker.force_x11);
    assert!(cfg.picker.close_on_blur);
}

#[test]
fn every_backend_string_round_trips() {
    for b in Backend::ALL {
        let cfg = parse_str(&format!("[desktop]\nbackend = \"{b}\"\n"), home()).unwrap();
        assert_eq!(cfg.desktop.backend, b);
    }
}

#[test]
fn empty_commands_load_as_none() {
    for v in ["[]", "\"\"", "\"   \""] {
        let cfg = parse_str(
            &format!("[desktop]\nset_command = {v}\n[hooks]\non_change = {v}\n"),
            home(),
        )
        .unwrap();
        assert_eq!(cfg.desktop.set_command, None, "{v}");
        assert_eq!(cfg.hooks.on_change, None, "{v}");
    }
}

#[test]
fn bad_values_are_errors() {
    for text in [
        "[desktop]\nset_command = \"echo 'unterminated\"\n",
        "[desktop]\nbackend = \"nope\"\n",
        "[picker]\nposition = \"middle\"\n",
        "[daemon]\ndownload_limit_mb = -1\n",
        "not toml",
    ] {
        assert!(parse_str(text, home()).is_err(), "{text}");
    }
}

#[test]
fn command_spec_api() {
    let c = CommandSpec::parse("feh --bg-fill '{path}'").unwrap();
    assert_eq!(c.program(), "feh");
    assert_eq!(c.args(), &["--bg-fill".to_owned(), "{path}".to_owned()]);
    assert!(CommandSpec::new(vec![]).is_err());
    assert!(CommandSpec::parse("").is_err());
}

#[test]
fn serialized_config_reloads_equal() {
    let mut cfg = defaults_for_home(home());
    cfg.desktop.set_command = Some(CommandSpec::parse("swaybg -i {path}").unwrap());
    cfg.desktop.detected_at = Some("2026-11-09T17:46:15Z".parse().unwrap());
    cfg.picker.custom = Some(kakemono_config::PickerCustom {
        x: 1,
        y: 2,
        monitor: "eDP-1".into(),
    });
    let text = toml::to_string(&cfg).unwrap();
    let back: Config = parse_str(&text, home()).unwrap();
    assert_eq!(back, cfg);
}

#[test]
fn load_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope.toml");
    let err = load_with_home(&path, home()).unwrap_err();
    assert!(err.is_not_found());
    assert!(err.to_string().starts_with("config: cannot read "), "{err}");
    assert_eq!(
        load_or_default_with_home(&path, home()).unwrap(),
        defaults_for_home(home())
    );
}

#[test]
fn load_or_default_propagates_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "[daemon\n").unwrap();
    let err = load_or_default_with_home(&path, home()).unwrap_err();
    assert!(
        err.to_string().starts_with("config: cannot parse "),
        "{err}"
    );
}
