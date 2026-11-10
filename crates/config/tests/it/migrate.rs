//! Legacy (`[light]`/`[dark]`, provider `enabled` flags, `reddit.subreddits`, `local.dirs`) to
//! `[[sources]]` migration. Each fixture's full output is an `insta` snapshot under
//! `tests/snapshots/`.

use std::path::{Path, PathBuf};

use crate::tilde_expanded;
use kakemono_config::{
    MigrationReport, SourceEntry, SourceType, deterministic_id, migrate_document, needs_migration,
    parse_str, valid_id,
};
use toml_edit::DocumentMut;

const HOME: &str = "/home/u";

fn home() -> &'static Path {
    Path::new(HOME)
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/migrate")
        .join(name)
}

fn doc(text: &str) -> DocumentMut {
    text.parse().expect("fixture is valid TOML")
}

fn fixture(name: &str) -> DocumentMut {
    let path = fixture_path(name);
    doc(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

/// Migrates `name` and returns the report and the resulting TOML.
fn migrate_fixture(name: &str) -> (MigrationReport, String) {
    let mut d = fixture(name);
    let report = migrate_document(&mut d, home()).expect("fixture needs migration");
    (report, d.to_string())
}

fn kinds(rows: &[SourceEntry]) -> Vec<(SourceType, &str, bool)> {
    rows.iter()
        .map(|r| (r.kind, r.location.as_str(), r.enabled))
        .collect()
}

/// The migrated text loads, and its `sources` are the report's rows (with `~` expanded).
fn assert_loads_as_report(text: &str, report: &MigrationReport) {
    let cfg = parse_str(text, home()).expect("migrated config loads");
    assert_eq!(cfg.sources.len(), report.rows.len(), "{text}");
    for (loaded, row) in cfg.sources.iter().zip(&report.rows) {
        assert_eq!(loaded.id, row.id);
        assert_eq!(loaded.enabled, row.enabled);
        assert_eq!(loaded.kind, row.kind);
        let want = match (row.kind.is_local(), row.location.as_str()) {
            (true, "~") => HOME.to_owned(),
            (true, loc) => loc
                .strip_prefix("~/")
                .map_or_else(|| loc.to_owned(), |rest| tilde_expanded(home(), rest)),
            (false, loc) => loc.to_owned(),
        };
        assert_eq!(loaded.location, want);
    }
    assert!(report.rows.iter().all(|r| valid_id(&r.id)));
}

#[test]
fn go_template_becomes_one_bing_row() {
    let (report, text) = migrate_fixture("go_template.toml");
    assert_eq!(kinds(&report.rows), [(SourceType::Bing, "", true)]);
    assert_eq!(
        report.rows[0].id,
        deterministic_id(SourceType::Bing, "", &Default::default())
    );
    assert_loads_as_report(&text, &report);
    insta::assert_snapshot!("go_template", text);
}

#[test]
fn light_and_dark_queries_become_distinct_rows_in_order() {
    let (report, text) = migrate_fixture("light_dark_split.toml");
    assert_eq!(
        kinds(&report.rows),
        [
            (SourceType::Bing, "", true),
            (SourceType::Wallhaven, "landscape", true),
            (SourceType::Wallhaven, "nature", true),
            (SourceType::Wallhaven, "space", true),
            (SourceType::Wallhaven, "night", true),
            (SourceType::Reddit, "EarthPorn", true),
        ]
    );
    assert_loads_as_report(&text, &report);
    // Credentials and options stay; legacy keys go.
    let cfg = parse_str(&text, home()).expect("load");
    assert_eq!(cfg.wallhaven.api_key, "secret");
    assert_eq!(cfg.wallhaven.purity, "110");
    assert_eq!(cfg.daemon.interval, "15m");
    insta::assert_snapshot!("light_dark_split", text);
}

#[test]
fn disabled_but_listed_providers_become_disabled_rows() {
    let (report, text) = migrate_fixture("disabled_but_listed.toml");
    // Wallhaven is disabled and unlisted: no row.
    assert_eq!(
        kinds(&report.rows),
        [
            (SourceType::Bing, "", false),
            (SourceType::Unsplash, "mountains", false),
            // The dark queries are the legacy defaults ([dark] has no `queries`).
            (SourceType::Unsplash, "space", false),
            (SourceType::Unsplash, "night", false),
            (SourceType::Unsplash, "dark", false),
            (SourceType::Apod, "", false),
        ]
    );
    assert_loads_as_report(&text, &report);
    insta::assert_snapshot!("disabled_but_listed", text);
}

#[test]
fn reddit_gets_one_row_per_subreddit() {
    let (report, text) = migrate_fixture("reddit_multi.toml");
    assert_eq!(
        kinds(&report.rows),
        [
            // Bing is disabled but listed in the legacy default `[light].sources`.
            (SourceType::Bing, "", false),
            (SourceType::Reddit, "EarthPorn", true),
            (SourceType::Reddit, "wallpapers", true),
            (SourceType::Reddit, "spaceporn", true),
        ]
    );
    assert_loads_as_report(&text, &report);
    insta::assert_snapshot!("reddit_multi", text);
}

#[test]
fn local_dirs_become_folder_rows_with_tilde_paths() {
    let (report, text) = migrate_fixture("local_dirs.toml");
    assert_eq!(
        kinds(&report.rows),
        [
            (SourceType::Bing, "", true),
            (SourceType::Folder, "~/Pictures/Walls", true),
            (SourceType::Folder, "~/Downloads/art", true),
            (SourceType::Folder, "/mnt/share/pics", true),
        ]
    );
    assert!(
        report.notes.iter().any(|n| n.contains("recursive")),
        "{:?}",
        report.notes
    );
    assert_loads_as_report(&text, &report);
    insta::assert_snapshot!("local_dirs", text);
}

#[test]
fn a_config_without_legacy_keys_is_left_alone() {
    let mut d = fixture("no_legacy.toml");
    let before = d.to_string();
    assert!(!needs_migration(&d));
    assert_eq!(migrate_document(&mut d, home()), None);
    assert_eq!(d.to_string(), before);
}

#[test]
fn explicit_empty_sources_is_respected() {
    let mut d = fixture("explicit_empty_sources.toml");
    let before = d.to_string();
    assert!(!needs_migration(&d));
    assert_eq!(migrate_document(&mut d, home()), None);
    assert_eq!(d.to_string(), before);
}

#[test]
fn comments_outside_removed_keys_survive() {
    let (report, text) = migrate_fixture("comments.toml");
    assert_eq!(kinds(&report.rows), [(SourceType::Bing, "", true)]);
    for kept in [
        "# My kakemono config",
        "# Rotate every quarter hour",
        "# Where downloads go",
        "# Favorites live here",
    ] {
        assert!(text.contains(kept), "{kept:?} lost:\n{text}");
    }
    assert!(!text.contains("cache_size"), "{text}");
    assert!(!text.contains("\n[light]"), "{text}");
    assert_loads_as_report(&text, &report);
    insta::assert_snapshot!("comments", text);
}

#[test]
fn migration_is_idempotent() {
    for name in [
        "go_template.toml",
        "light_dark_split.toml",
        "disabled_but_listed.toml",
        "reddit_multi.toml",
        "local_dirs.toml",
        "comments.toml",
    ] {
        let mut d = fixture(name);
        migrate_document(&mut d, home()).expect(name);
        let once = d.to_string();
        assert!(!needs_migration(&d), "{name}");
        assert_eq!(migrate_document(&mut d, home()), None, "{name}");
        assert_eq!(d.to_string(), once, "{name}");
    }
}

#[test]
fn migration_is_deterministic() {
    for name in [
        "light_dark_split.toml",
        "reddit_multi.toml",
        "local_dirs.toml",
    ] {
        let (a, text_a) = migrate_fixture(name);
        let (b, text_b) = migrate_fixture(name);
        assert_eq!(a, b, "{name}");
        assert_eq!(text_a, text_b, "{name}");
    }
}

#[test]
fn ids_are_unique_even_for_repeated_rows() {
    let mut d = doc(
        "[bing]\nenabled = false\n\n[light]\nsources = []\n\n[dark]\nsources = []\n\n[reddit]\nenabled = true\nsubreddits = [\"pics\", \"pics\"]\n",
    );
    let report = migrate_document(&mut d, home()).expect("legacy");
    assert_eq!(report.rows.len(), 2);
    assert_ne!(report.rows[0].id, report.rows[1].id);
    assert_eq!(report.rows[1].id, format!("{}-2", report.rows[0].id));
}

#[test]
fn an_empty_file_needs_no_migration() {
    let mut d = doc("");
    assert!(!needs_migration(&d));
    assert_eq!(migrate_document(&mut d, home()), None);
    assert_eq!(d.to_string(), "");
}

#[test]
fn each_legacy_key_triggers_migration() {
    for text in [
        "[light]\n",
        "[dark]\n",
        "[daemon]\ncache_size = 5\n",
        "[wallhaven]\nenabled = true\n",
        "[unsplash]\nenabled = true\n",
        "[reddit]\nenabled = true\n",
        "[bing]\nenabled = true\n",
        "[apod]\nenabled = true\n",
        "[local]\nenabled = true\n",
        "[reddit]\nsubreddits = [\"x\"]\n",
        "[local]\ndirs = []\n",
    ] {
        assert!(needs_migration(&doc(text)), "{text}");
    }
    for text in [
        "[daemon]\ninterval = \"5m\"\n",
        "[wallhaven]\napi_key = \"k\"\n",
        "[bing]\nmarket = \"en-GB\"\n",
    ] {
        assert!(!needs_migration(&doc(text)), "{text}");
    }
}

#[test]
fn light_only_uses_light_queries_and_dark_defaults() {
    let mut d = doc(
        "[wallhaven]\nenabled = true\n\n[light]\nsources = [\"wallhaven\"]\nqueries = [\"sea\"]\n",
    );
    let report = migrate_document(&mut d, home()).expect("legacy");
    assert_eq!(
        kinds(&report.rows),
        [
            (SourceType::Bing, "", true),
            (SourceType::Wallhaven, "sea", true),
            (SourceType::Wallhaven, "space", true),
            (SourceType::Wallhaven, "night", true),
            (SourceType::Wallhaven, "dark", true),
        ]
    );
    assert_loads_as_report(&d.to_string(), &report);
}

#[test]
fn dark_only_uses_light_defaults_and_dark_queries() {
    let mut d = doc("[unsplash]\nenabled = true\n\n[dark]\nqueries = [\"stars\", \"\"]\n");
    let report = migrate_document(&mut d, home()).expect("legacy");
    assert_eq!(
        kinds(&report.rows),
        [
            (SourceType::Bing, "", true),
            (SourceType::Unsplash, "landscape", true),
            (SourceType::Unsplash, "nature", true),
            (SourceType::Unsplash, "stars", true),
        ]
    );
}

#[test]
fn empty_query_lists_give_one_row_with_an_empty_location() {
    let mut d =
        doc("[wallhaven]\nenabled = true\n\n[light]\nqueries = []\n\n[dark]\nqueries = []\n");
    let report = migrate_document(&mut d, home()).expect("legacy");
    assert_eq!(
        kinds(&report.rows),
        [
            (SourceType::Bing, "", true),
            (SourceType::Wallhaven, "", true)
        ]
    );
}

#[test]
fn nothing_enabled_writes_an_explicit_empty_list() {
    let mut d = doc("[bing]\nenabled = false\n\n[light]\nsources = []\n\n[dark]\nsources = []\n");
    let report = migrate_document(&mut d, home()).expect("legacy");
    assert!(report.rows.is_empty());
    let text = d.to_string();
    let cfg = parse_str(&text, home()).expect("load");
    assert!(cfg.sources.is_empty(), "{text}");
    assert!(!needs_migration(&d));
}

#[test]
fn emptied_tables_are_removed_and_others_kept() {
    let mut d = doc(
        "[daemon]\ncache_size = 5\n\n[bing]\nenabled = true\nmarket = \"en-GB\"\n\n[local]\nenabled = false\ndirs = []\nrecursive = true\n",
    );
    migrate_document(&mut d, home()).expect("legacy");
    assert!(d.get("daemon").is_none(), "{d}");
    assert!(d.get("local").is_none(), "{d}");
    assert_eq!(d["bing"]["market"].as_str(), Some("en-GB"));
    assert!(d["bing"].get("enabled").is_none(), "{d}");
}

/// Manual check against a real config:
/// `KAKEMONO_MIGRATE_FIXTURE=path cargo test -p kakemono-config --test migrate real_config -- --ignored --nocapture`
#[test]
#[ignore = "needs KAKEMONO_MIGRATE_FIXTURE"]
fn real_config() {
    let path = std::env::var_os("KAKEMONO_MIGRATE_FIXTURE").expect("set KAKEMONO_MIGRATE_FIXTURE");
    let text = std::fs::read_to_string(&path).expect("read fixture");
    let mut d = doc(&text);
    let home = kakemono_config::defaults::home_dir();
    match migrate_document(&mut d, &home) {
        None => println!("no migration needed"),
        Some(report) => {
            for row in &report.rows {
                println!("{row:?}");
            }
            for note in &report.notes {
                println!("note: {note}");
            }
            println!("---\n{d}");
            parse_str(&d.to_string(), &home).expect("migrated config loads");
        }
    }
}
