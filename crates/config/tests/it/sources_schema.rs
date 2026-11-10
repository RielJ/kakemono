//! `[[sources]]`: one row per source instance.

use std::path::Path;

use kakemono_config::load::{parse_report, parse_str};
use kakemono_config::{SourceEntry, SourceType, defaults_for_home};

const HOME: &str = "/home/golden";

fn home() -> &'static Path {
    Path::new(HOME)
}

fn row(id: &str, kind: SourceType, location: &str) -> SourceEntry {
    SourceEntry {
        id: id.to_owned(),
        enabled: true,
        kind,
        location: location.to_owned(),
    }
}

#[test]
fn album_name_row_round_trips() {
    let text = r#"
[[sources]]
id = "aln-1a2b"
enabled = false
type = "album-name"
location = "/srv/albums/trip"
"#;
    let cfg = parse_str(text, home()).unwrap();
    assert_eq!(
        cfg.sources,
        vec![SourceEntry {
            enabled: false,
            ..row("aln-1a2b", SourceType::AlbumName, "/srv/albums/trip")
        }]
    );
    let back = parse_str(&toml::to_string(&cfg).unwrap(), home()).unwrap();
    assert_eq!(back.sources, cfg.sources);
    let json = serde_json::to_value(&cfg.sources[0]).unwrap();
    assert_eq!(json["type"], "album-name");
}

#[test]
fn every_type_string_round_trips() {
    for (s, kind) in [
        ("wallhaven", SourceType::Wallhaven),
        ("unsplash", SourceType::Unsplash),
        ("reddit", SourceType::Reddit),
        ("bing", SourceType::Bing),
        ("apod", SourceType::Apod),
        ("folder", SourceType::Folder),
        ("images", SourceType::Images),
        ("album-name", SourceType::AlbumName),
        ("album-date", SourceType::AlbumDate),
    ] {
        let cfg = parse_str(
            &format!("[[sources]]\nid = \"x\"\ntype = \"{s}\"\nlocation = \"\"\n"),
            home(),
        )
        .unwrap();
        assert_eq!(cfg.sources[0].kind, kind, "{s}");
        assert_eq!(serde_json::to_value(kind).unwrap(), s);
    }
}

#[test]
fn unknown_type_loads_as_unsupported() {
    let cfg = parse_str(
        "[[sources]]\nid = \"fl-0001\"\ntype = \"flickr\"\nlocation = \"cats\"\n",
        home(),
    )
    .unwrap();
    assert_eq!(cfg.sources[0].kind, SourceType::Unsupported);
    assert_eq!(cfg.sources[0].location, "cats");
}

#[test]
fn missing_keys_in_a_row_take_row_defaults() {
    let cfg = parse_str("[[sources]]\nid = \"b\"\ntype = \"bing\"\n", home()).unwrap();
    assert_eq!(cfg.sources, vec![row("b", SourceType::Bing, "")]);
}

/// A row without `type` is a typo, not a request for Bing: it loads as `unsupported` (skipped at
/// runtime) and the load report says so.
#[test]
fn missing_type_loads_as_unsupported_with_a_warning() {
    let loaded = parse_report("[[sources]]\nid = \"b\"\nlocation = \"cats\"\n", home()).unwrap();
    assert_eq!(
        loaded.config.sources,
        vec![row("b", SourceType::Unsupported, "cats")]
    );
    assert!(!loaded.config.sources[0].kind.is_online());
    assert_eq!(loaded.warnings, [r#"source "b": missing type; skipped"#]);
}

#[test]
fn unknown_type_is_reported() {
    let loaded = parse_report(
        "[[sources]]\nid = \"fl-0001\"\ntype = \"flickr\"\nlocation = \"cats\"\n",
        home(),
    )
    .unwrap();
    assert_eq!(
        loaded.warnings,
        [r#"source "fl-0001": unknown type "flickr"; skipped"#]
    );
}

/// `normalize_sources` replacements reach the load report instead of being dropped (M8).
#[test]
fn replaced_ids_are_reported() {
    let loaded = parse_report(
        "[[sources]]\nid = \"dup\"\ntype = \"bing\"\n\n[[sources]]\nid = \"dup\"\ntype = \"apod\"\n",
        home(),
    )
    .unwrap();
    assert_eq!(loaded.config.sources[0].id, "dup");
    let new_id = &loaded.config.sources[1].id;
    assert_ne!(new_id, "dup");
    assert_eq!(
        loaded.warnings,
        [format!(
            r#"sources[1] (apod ""): duplicate id "dup"; using {new_id:?}"#
        )]
    );
}

#[test]
fn a_clean_config_has_no_warnings() {
    let loaded = parse_report(kakemono_config::paths::TEMPLATE, home()).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert!(parse_report("", home()).unwrap().warnings.is_empty());
}

#[test]
fn missing_sources_gives_the_three_default_rows() {
    let cfg = parse_str("", home()).unwrap();
    assert_eq!(
        cfg.sources,
        vec![
            row("bing", SourceType::Bing, ""),
            row("wh-nature", SourceType::Wallhaven, "nature"),
            row(
                "favorites",
                SourceType::Folder,
                "/home/golden/Pictures/Wallpapers/favorites"
            ),
        ]
    );
    assert_eq!(cfg.sources, defaults_for_home(home()).sources);
}

#[test]
fn explicit_empty_sources_stays_empty() {
    let cfg = parse_str("sources = []\n", home()).unwrap();
    assert!(cfg.sources.is_empty());
}

#[test]
fn tilde_is_expanded_in_local_locations_only() {
    let cfg = parse_str(
        r#"
[[sources]]
id = "a"
type = "folder"
location = "~/Pictures"
[[sources]]
id = "b"
type = "images"
location = "~/one.jpg"
[[sources]]
id = "c"
type = "album-name"
location = "~"
[[sources]]
id = "d"
type = "album-date"
location = "~/x"
[[sources]]
id = "e"
type = "wallhaven"
location = "~/not-a-path"
"#,
        home(),
    )
    .unwrap();
    let locations: Vec<&str> = cfg.sources.iter().map(|s| s.location.as_str()).collect();
    assert_eq!(
        locations,
        [
            "/home/golden/Pictures",
            "/home/golden/one.jpg",
            "/home/golden",
            "/home/golden/x",
            "~/not-a-path",
        ]
    );
}
