//! `apply_json` writes a JSON array of objects as `[[tables]]`, so the UI's `sources` patch keeps
//! the file hand-editable.

use std::path::Path;

use kakemono_config::edit::apply_json;
use kakemono_config::{BrightnessFilter, SourceType, parse_str};
use serde_json::json;
use toml_edit::DocumentMut;

fn home() -> &'static Path {
    Path::new("/home/u")
}

fn doc(text: &str) -> DocumentMut {
    text.parse().expect("valid TOML")
}

fn two_rows() -> serde_json::Value {
    json!({"sources": [
        {"id": "wh-1a2b", "enabled": true, "type": "wallhaven", "location": "nature"},
        {"id": "dir-0001", "enabled": false, "type": "folder", "location": "~/Pictures"},
    ]})
}

#[test]
fn array_of_objects_onto_a_missing_key_writes_array_of_tables() {
    let mut d = doc("[daemon]\ninterval = \"30m\"\n");
    apply_json(&mut d, &two_rows()).expect("patch");
    let text = d.to_string();
    assert_eq!(
        text,
        "[daemon]\ninterval = \"30m\"\n\n\
         [[sources]]\nid = \"wh-1a2b\"\nenabled = true\ntype = \"wallhaven\"\nlocation = \"nature\"\n\n\
         [[sources]]\nid = \"dir-0001\"\nenabled = false\ntype = \"folder\"\nlocation = \"~/Pictures\"\n"
    );
    let cfg = parse_str(&text, home()).expect("loads");
    assert_eq!(cfg.sources.len(), 2);
    assert_eq!(cfg.sources[1].kind, SourceType::Folder);
    assert_eq!(cfg.sources[1].location, "/home/u/Pictures");
}

#[test]
fn array_of_objects_replaces_existing_blocks_in_place_keeping_the_comment() {
    let mut d = doc("[daemon]\ninterval = \"30m\"\n\n\
         # My sources\n\
         [[sources]]\nid = \"bing\"\nenabled = true\ntype = \"bing\"\nlocation = \"\"\n\n\
         [[sources]]\nid = \"old\"\nenabled = true\ntype = \"reddit\"\nlocation = \"pics\"\n\n\
         # Favorites\n[favorites]\ndir = \"~/fav\" # keep\n");
    apply_json(&mut d, &two_rows()).expect("patch");
    assert_eq!(
        d.to_string(),
        "[daemon]\ninterval = \"30m\"\n\n\
         # My sources\n\
         [[sources]]\nid = \"wh-1a2b\"\nenabled = true\ntype = \"wallhaven\"\nlocation = \"nature\"\n\n\
         [[sources]]\nid = \"dir-0001\"\nenabled = false\ntype = \"folder\"\nlocation = \"~/Pictures\"\n\n\
         # Favorites\n[favorites]\ndir = \"~/fav\" # keep\n"
    );
}

#[test]
fn empty_array_removes_every_block_and_writes_an_empty_list() {
    let mut d = doc("[daemon]\ninterval = \"30m\"\n\n\
         [[sources]]\nid = \"bing\"\nenabled = true\ntype = \"bing\"\nlocation = \"\"\n");
    apply_json(&mut d, &json!({"sources": []})).expect("patch");
    let text = d.to_string();
    assert!(!text.contains("[[sources]]"), "{text}");
    assert!(text.contains("sources = []"), "{text}");
    let cfg = parse_str(&text, home()).expect("loads");
    assert!(cfg.sources.is_empty());
}

#[test]
fn rows_patch_onto_an_empty_list() {
    let mut d = doc("sources = []\n");
    apply_json(&mut d, &two_rows()).expect("patch");
    let text = d.to_string();
    assert!(!text.contains("sources = []"), "{text}");
    assert_eq!(text.matches("[[sources]]").count(), 2, "{text}");
    let cfg = parse_str(&text, home()).expect("loads");
    assert_eq!(cfg.sources.len(), 2);
}

#[test]
fn second_patch_is_stable() {
    let mut d = doc("[daemon]\ninterval = \"30m\"\n");
    apply_json(&mut d, &two_rows()).expect("patch");
    let once = d.to_string();
    apply_json(&mut d, &two_rows()).expect("patch again");
    assert_eq!(d.to_string(), once);
}

#[test]
fn arrays_of_scalars_stay_inline() {
    let mut d = doc("[local]\ndirs = [\"/a\"]\n");
    apply_json(&mut d, &json!({"local": {"dirs": ["/b", "/c"]}})).expect("patch");
    assert_eq!(d.to_string(), "[local]\ndirs = [\"/b\", \"/c\"]\n");
}

#[test]
fn filters_object_merges_as_a_table() {
    let mut d = doc("[filters]\n# Keep landscape only\nlandscape = true\n");
    apply_json(&mut d, &json!({"filters": {"brightness": "dark"}})).expect("patch");
    assert_eq!(
        d.to_string(),
        "[filters]\n# Keep landscape only\nlandscape = true\nbrightness = \"dark\"\n"
    );
    let cfg = parse_str(&d.to_string(), home()).expect("loads");
    assert_eq!(cfg.filters.brightness, BrightnessFilter::Dark);
    assert!(cfg.filters.landscape);
}

/// What the UI's Save does: load, send every row back as JSON, patch.
fn ui_save(text: &str) -> String {
    let cfg = parse_str(text, home()).expect("loads");
    let mut d = doc(text);
    apply_json(&mut d, &json!({ "sources": cfg.sources })).expect("patch");
    d.to_string()
}

/// A type this version doesn't know (written by a newer kakemono) survives a save of the list.
#[test]
fn unknown_type_survives_a_ui_save_byte_for_byte() {
    let text = "[daemon]\ninterval = \"30m\"\n\n\
         [[sources]]\nid = \"bing\"\nenabled = true\ntype = \"bing\"\nlocation = \"\"\n\n\
         [[sources]]\nid = \"fl-0001\"\nenabled = true\ntype = \"flickr\" # newer build\nlocation = \"cats\"\n";
    assert_eq!(ui_save(text), text);
}

/// A row without `type` is saved without one: the save never invents a value.
#[test]
fn missing_type_survives_a_ui_save_byte_for_byte() {
    let text = "[[sources]]\nid = \"bing\"\nenabled = true\ntype = \"bing\"\nlocation = \"\"\n\n\
         [[sources]]\nid = \"x-1\"\nenabled = false\nlocation = \"cats\"\n";
    assert_eq!(ui_save(text), text);
}

/// The raw type is matched by id, so it follows its row when the UI reorders or drops others.
#[test]
fn unknown_type_follows_its_row_by_id() {
    let mut d = doc(
        "[[sources]]\nid = \"bing\"\nenabled = true\ntype = \"bing\"\nlocation = \"\"\n\n\
         [[sources]]\nid = \"fl-0001\"\nenabled = true\ntype = \"flickr\"\nlocation = \"cats\"\n",
    );
    apply_json(
        &mut d,
        &json!({"sources": [
            {"id": "fl-0001", "enabled": false, "type": "unsupported", "location": "dogs"},
        ]}),
    )
    .expect("patch");
    assert_eq!(
        d.to_string(),
        "[[sources]]\nid = \"fl-0001\"\nenabled = false\ntype = \"flickr\"\nlocation = \"dogs\"\n"
    );
}
