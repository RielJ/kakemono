//! Guards the legacy → `[[sources]]` migration across the removal of the legacy schema fields
//! (`[light]`/`[dark]`, `daemon.cache_size`, provider `enabled`, `reddit.subreddits`,
//! `local.dirs`/`recursive`). `migrate::legacy` keeps private copies of those structs, so loading
//! a legacy file must give the same rows and `download_limit_mb` before and after the cleanup.
//!
//! The expected rows were recorded before the legacy fields left `kakemono_config::schema`.

use std::path::{Path, PathBuf};

use crate::tilde_expanded;
use kakemono_config::load::{load_with_home, parse_str};
use kakemono_config::{Config, SourceType, migrate_file};

const HOME: &str = "/home/u";

fn home() -> &'static Path {
    Path::new(HOME)
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The shape of a real Go `wallpaperd` config: Wallhaven on, everything else off, seven distinct
/// queries across `[light]` and `[dark]`, the legacy socket path, and a `[picker]` table.
const USER_SHAPE: &str = r#"[daemon]
  rotation_enabled = false
  interval = "30m"
  download_dir = "/home/u/Downloads/wallpapers/"
  cache_size = 50
  trash_dir = "/home/u/Pictures/Wallpapers/trash"
  socket_path = "~/.config/wallpaperd/daemon.sock"

[wallhaven]
  enabled = true
  api_key = ""
  purity = "100"
  sorting = "random"
  categories = "111"

[unsplash]
  enabled = false
  access_key = ""

[reddit]
  enabled = false
  subreddits = ["EarthPorn", "wallpapers", "spaceporn"]

[bing]
  enabled = false
  market = "en-US"

[apod]
  enabled = false
  api_key = "DEMO_KEY"

[local]
  enabled = false
  dirs = []
  recursive = true

[light]
  sources = []
  queries = ["landscape", "nature", "cityscape"]

[dark]
  sources = ["wallhaven"]
  queries = ["space", "night", "dark minimal", "galaxy"]

[favorites]
  dir = "/home/u/Pictures/Wallpapers/favorites"

[picker]
  position = "right"
custom = { monitor = "Monitor #41057", x = 2872, y = 98 }
placement = "custom"
"#;

/// `(id, enabled, type, location)` per row.
type Row<'a> = (&'a str, bool, SourceType, &'a str);

fn rows(cfg: &Config) -> Vec<(String, bool, SourceType, String)> {
    cfg.sources
        .iter()
        .map(|s| (s.id.clone(), s.enabled, s.kind, s.location.clone()))
        .collect()
}

fn owned(want: &[Row<'_>]) -> Vec<(String, bool, SourceType, String)> {
    want.iter()
        .map(|&(id, on, kind, loc)| (id.to_owned(), on, kind, loc.to_owned()))
        .collect()
}

/// Loads `text` from a file (the daemon's path) and in memory (the UI's `parse_str`), checks both
/// agree, and that migrating the file on disk then loading it gives the same result.
fn load_all_ways(text: &str) -> Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, text).unwrap();
    let from_file = load_with_home(&path, home()).expect("legacy file loads");
    let in_memory = parse_str(text, home()).expect("legacy text parses");
    assert_eq!(from_file, in_memory);

    let migrated = migrate_file(&path, home()).expect("migrate_file");
    assert!(migrated.is_some(), "a legacy file is migrated on disk");
    let after = load_with_home(&path, home()).expect("migrated file loads");
    assert_eq!(
        rows(&after),
        rows(&from_file),
        "on-disk migration changed the rows"
    );
    assert_eq!(
        after.daemon.download_limit_mb,
        from_file.daemon.download_limit_mb
    );
    from_file
}

fn assert_migrates(text: &str, want: &[Row<'_>]) {
    let cfg = load_all_ways(text);
    assert_eq!(rows(&cfg), owned(want));
    // `cache_size` is dropped, not converted: the size limit keeps its default.
    assert_eq!(cfg.daemon.download_limit_mb, 500);
    assert!(cfg.daemon.download_limit_mb_enabled);
}

#[test]
fn user_shape_gives_seven_wallhaven_rows() {
    assert_migrates(
        USER_SHAPE,
        &[
            ("wh-2051", true, SourceType::Wallhaven, "landscape"),
            ("wh-4cd4", true, SourceType::Wallhaven, "nature"),
            ("wh-59f2", true, SourceType::Wallhaven, "cityscape"),
            ("wh-1a1a", true, SourceType::Wallhaven, "space"),
            ("wh-8195", true, SourceType::Wallhaven, "night"),
            ("wh-cb2c", true, SourceType::Wallhaven, "dark minimal"),
            ("wh-5cc0", true, SourceType::Wallhaven, "galaxy"),
        ],
    );
}

#[test]
fn go_fixtures_give_one_bing_row() {
    for name in [
        "go_defaults.toml",
        "go_default_template.toml",
        "go_template_loaded.toml",
    ] {
        let text = std::fs::read_to_string(fixture(name)).unwrap();
        let cfg = load_all_ways(&text);
        assert_eq!(
            rows(&cfg),
            owned(&[("bing-3caa", true, SourceType::Bing, "")]),
            "{name}"
        );
        assert_eq!(cfg.daemon.download_limit_mb, 500, "{name}");
    }
}

#[test]
fn every_legacy_key_feeds_the_rows() {
    // Each provider on, local dirs, subreddits, and an explicit download_limit_mb that must stay.
    let text = r#"
[daemon]
cache_size = 7
download_limit_mb = 900

[wallhaven]
enabled = true
[unsplash]
enabled = true
[reddit]
enabled = true
subreddits = ["EarthPorn", "spaceporn"]
[bing]
enabled = true
[apod]
enabled = true
[local]
enabled = true
dirs = ["~/Pictures/walls", "/srv/art"]
recursive = false

[light]
sources = ["bing"]
queries = ["sea"]
[dark]
queries = ["stars"]
"#;
    let cfg = load_all_ways(text);
    assert_eq!(
        rows(&cfg),
        owned(&[
            ("bing-3caa", true, SourceType::Bing, ""),
            ("wh-8c5a", true, SourceType::Wallhaven, "sea"),
            ("wh-c1a5", true, SourceType::Wallhaven, "stars"),
            ("us-d90b", true, SourceType::Unsplash, "sea"),
            ("us-4fd7", true, SourceType::Unsplash, "stars"),
            ("rd-d345", true, SourceType::Reddit, "EarthPorn"),
            ("rd-1337", true, SourceType::Reddit, "spaceporn"),
            ("apod-bde1", true, SourceType::Apod, ""),
            (
                "dir-727f",
                true,
                SourceType::Folder,
                tilde_expanded(home(), "Pictures/walls").as_str()
            ),
            ("dir-590b", true, SourceType::Folder, "/srv/art"),
        ])
    );
    assert_eq!(cfg.daemon.download_limit_mb, 900);
}

#[test]
fn legacy_keys_next_to_sources_still_load() {
    // A file that already has `[[sources]]` is not migrated; leftover legacy keys are ignored,
    // never an error.
    let text = r#"
[daemon]
cache_size = 5
[wallhaven]
enabled = true
[reddit]
enabled = true
subreddits = ["pics"]
[local]
enabled = true
dirs = ["/x"]
recursive = false
[light]
sources = ["reddit"]
queries = ["a"]
[dark]
sources = ["local"]

[[sources]]
id = "apod"
type = "apod"
"#;
    let cfg = parse_str(text, home()).expect("legacy keys beside [[sources]] load");
    assert_eq!(rows(&cfg), owned(&[("apod", true, SourceType::Apod, "")]));
    assert_eq!(cfg.daemon.download_limit_mb, 500);
}
