//! `[[sources]]`: one row per source instance (spec "Config shape").

use std::path::Path;

use kakemono_config::load::parse_str;
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
    let cfg = parse_str("[[sources]]\nid = \"b\"\n", home()).unwrap();
    assert_eq!(cfg.sources, vec![row("b", SourceType::Bing, "")]);
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
