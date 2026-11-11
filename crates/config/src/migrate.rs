//! Legacy config → `[[sources]]` migration, applied to a `toml_edit` document so comments
//! outside the removed keys survive.
//!
//! "Legacy" means: no `sources` key, and at least one of `[light]`, `[dark]`,
//! `daemon.cache_size`, `<provider>.enabled`, `reddit.subreddits` or `local.dirs`. An explicit
//! `sources = []` is respected.
//!
//! The legacy keys are read through private copies of the legacy structs and their defaults in
//! [`legacy`], so legacy semantics ("a missing `[bing]` means enabled", "keys missing inside
//! `[light]` keep the light defaults") hold after those fields leave [`crate::schema`].
//!
//! Rows are emitted in the order bing, wallhaven, unsplash, reddit, apod, local:
//! - A provider whose `enabled` flag is on gives enabled rows. A disabled provider listed in
//!   `[light].sources` or `[dark].sources` gives disabled rows. A disabled, unlisted provider
//!   gives none.
//! - Wallhaven and Unsplash: one row per distinct non-empty query (light queries first, then
//!   dark), or one row with an empty location if there are none.
//! - Reddit: one row per subreddit. Local: one `folder` row per dir, with the home dir written
//!   back as `~`.
//! - Ids come from [`deterministic_id`], so two processes migrating the same file agree.

use std::collections::HashSet;
use std::path::Path;

use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

use crate::schema::{SourceEntry, SourceType};
use crate::sources::deterministic_id;

/// What [`migrate_document`] produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReport {
    /// The `[[sources]]` rows written, with locations as written to the file (`~` not expanded).
    pub rows: Vec<SourceEntry>,
    /// Behaviour changes worth logging, e.g. a non-recursive `[local]` becoming recursive folders.
    pub notes: Vec<String>,
}

/// The providers that had an `enabled` flag, in migration order.
const PROVIDERS: [&str; 6] = ["bing", "wallhaven", "unsplash", "reddit", "apod", "local"];

/// Whether `doc` is a legacy config: no `sources` key and at least one legacy key.
pub fn needs_migration(doc: &DocumentMut) -> bool {
    if doc.contains_key("sources") {
        return false;
    }
    let has = |table: &str, key: &str| {
        doc.get(table)
            .and_then(Item::as_table_like)
            .is_some_and(|t| t.contains_key(key))
    };
    doc.contains_key("light")
        || doc.contains_key("dark")
        || has("daemon", "cache_size")
        || PROVIDERS.iter().any(|p| has(p, "enabled"))
        || has("reddit", "subreddits")
        || has("local", "dirs")
}

/// Converts a legacy `doc` in place: removes the legacy keys (and tables left empty) and appends
/// `[[sources]]`. Returns `None`, leaving `doc` untouched, when it [doesn't need
/// migration](needs_migration) or its legacy keys don't parse.
pub fn migrate_document(doc: &mut DocumentMut, home: &Path) -> Option<MigrationReport> {
    if !needs_migration(doc) {
        return None;
    }
    let old: legacy::LegacyConfig = toml::from_str(&doc.to_string()).ok()?;
    let (rows, notes) = build_rows(&old, home);
    remove_legacy_keys(doc);
    write_sources(doc, &rows);
    Some(MigrationReport { rows, notes })
}

fn build_rows(old: &legacy::LegacyConfig, home: &Path) -> (Vec<SourceEntry>, Vec<String>) {
    let listed = |name: &str| {
        old.light.sources.iter().any(|s| s == name) || old.dark.sources.iter().any(|s| s == name)
    };
    // `Some(enabled)` when the provider gets rows.
    let state = |name: &str, enabled: bool| (enabled || listed(name)).then_some(enabled);
    let queries = distinct_queries(&old.light.queries, &old.dark.queries);

    let mut planned: Vec<(SourceType, String, bool)> = Vec::new();
    if let Some(on) = state("bing", old.bing.enabled) {
        planned.push((SourceType::Bing, String::new(), on));
    }
    for (name, kind, enabled) in [
        ("wallhaven", SourceType::Wallhaven, old.wallhaven.enabled),
        ("unsplash", SourceType::Unsplash, old.unsplash.enabled),
    ] {
        if let Some(on) = state(name, enabled) {
            if queries.is_empty() {
                planned.push((kind, String::new(), on));
            }
            planned.extend(queries.iter().map(|q| (kind, q.clone(), on)));
        }
    }
    if let Some(on) = state("reddit", old.reddit.enabled) {
        planned.extend(
            old.reddit
                .subreddits
                .iter()
                .map(|s| (SourceType::Reddit, s.clone(), on)),
        );
    }
    if let Some(on) = state("apod", old.apod.enabled) {
        planned.push((SourceType::Apod, String::new(), on));
    }
    let mut notes = Vec::new();
    if let Some(on) = state("local", old.local.enabled) {
        let before = planned.len();
        planned.extend(
            old.local
                .dirs
                .iter()
                .map(|d| (SourceType::Folder, collapse_home(d, home), on)),
        );
        if !old.local.recursive && planned.len() > before {
            notes.push(
                "[local] recursive = false: folder sources always include subfolders".to_owned(),
            );
        }
    }
    if planned.is_empty() {
        notes.push("no provider was enabled or listed: wrote an empty sources list".to_owned());
    }

    let mut taken = HashSet::new();
    let rows = planned
        .into_iter()
        .map(|(kind, location, enabled)| {
            let id = deterministic_id(kind, &location, &taken);
            taken.insert(id.clone());
            SourceEntry {
                id,
                enabled,
                kind,
                location,
                ..Default::default()
            }
        })
        .collect();
    (rows, notes)
}

/// Light queries then dark, trimmed, without empties or repeats, first occurrence kept.
fn distinct_queries(light: &[String], dark: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    light
        .iter()
        .chain(dark)
        .map(|q| q.trim())
        .filter(|q| !q.is_empty() && seen.insert(*q))
        .map(str::to_owned)
        .collect()
}

/// `/home/u/Pictures` → `~/Pictures` when under `home`; anything else unchanged.
fn collapse_home(dir: &str, home: &Path) -> String {
    if dir.starts_with('~') || home.as_os_str().is_empty() {
        return dir.to_owned();
    }
    match Path::new(dir).strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => dir.to_owned(),
    }
}

fn remove_legacy_keys(doc: &mut DocumentMut) {
    doc.remove("light");
    doc.remove("dark");
    let removals: [(&str, &[&str]); 7] = [
        ("daemon", &["cache_size"]),
        ("bing", &["enabled"]),
        ("wallhaven", &["enabled"]),
        ("unsplash", &["enabled"]),
        ("reddit", &["enabled", "subreddits"]),
        ("apod", &["enabled"]),
        ("local", &["enabled", "dirs", "recursive"]),
    ];
    for (table, keys) in removals {
        let Some(t) = doc.get_mut(table).and_then(Item::as_table_like_mut) else {
            continue;
        };
        for key in keys {
            t.remove(key);
        }
        if t.is_empty() {
            doc.remove(table);
        }
    }
}

fn write_sources(doc: &mut DocumentMut, rows: &[SourceEntry]) {
    if rows.is_empty() {
        doc.insert("sources", value(Array::new()));
        return;
    }
    // No blank line above the header when nothing else is left in the file.
    let gap = if doc.as_table().is_empty() { "" } else { "\n" };
    let mut aot = ArrayOfTables::new();
    for (i, row) in rows.iter().enumerate() {
        let mut t = Table::new();
        t.insert("id", value(row.id.as_str()));
        t.insert("enabled", value(row.enabled));
        t.insert("type", value(row.kind.as_str()));
        t.insert("location", value(row.location.as_str()));
        if i == 0 {
            t.decor_mut().set_prefix(format!(
                "{gap}# --- Sources ---\n# Migrated from [light], [dark] and the per-provider settings.\n"
            ));
        }
        aot.push(t);
    }
    doc.insert("sources", Item::ArrayOfTables(aot));
}

/// Private copies of the legacy schema and its defaults (Go `config.Defaults()`), kept here so
/// migration keeps working after the legacy fields are removed from [`crate::schema`].
mod legacy {
    use serde::Deserialize;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    pub struct LegacyConfig {
        pub wallhaven: Flag<Off>,
        pub unsplash: Flag<Off>,
        pub reddit: Reddit,
        pub bing: Flag<On>,
        pub apod: Flag<Off>,
        pub local: Local,
        pub light: Theme<Light>,
        pub dark: Theme<Dark>,
    }

    /// The default of an `enabled` flag.
    pub trait FlagDefault {
        const ENABLED: bool;
    }
    #[derive(Debug)]
    pub struct On;
    #[derive(Debug)]
    pub struct Off;
    impl FlagDefault for On {
        const ENABLED: bool = true;
    }
    impl FlagDefault for Off {
        const ENABLED: bool = false;
    }

    /// A provider table of which only `enabled` matters here.
    #[derive(Debug, Deserialize)]
    #[serde(default, bound = "")]
    pub struct Flag<D: FlagDefault> {
        pub enabled: bool,
        #[serde(skip)]
        _default: std::marker::PhantomData<D>,
    }

    impl<D: FlagDefault> Default for Flag<D> {
        fn default() -> Self {
            Self {
                enabled: D::ENABLED,
                _default: std::marker::PhantomData,
            }
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(default)]
    pub struct Reddit {
        pub enabled: bool,
        pub subreddits: Vec<String>,
    }

    impl Default for Reddit {
        fn default() -> Self {
            Self {
                enabled: false,
                subreddits: strings(&["EarthPorn", "wallpapers"]),
            }
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(default)]
    pub struct Local {
        pub enabled: bool,
        pub dirs: Vec<String>,
        pub recursive: bool,
    }

    impl Default for Local {
        fn default() -> Self {
            Self {
                enabled: false,
                dirs: Vec::new(),
                recursive: true,
            }
        }
    }

    /// The defaults of a theme table.
    pub trait ThemeDefault {
        const SOURCES: &'static [&'static str];
        const QUERIES: &'static [&'static str];
    }
    #[derive(Debug)]
    pub struct Light;
    #[derive(Debug)]
    pub struct Dark;
    impl ThemeDefault for Light {
        const SOURCES: &'static [&'static str] = &["bing"];
        const QUERIES: &'static [&'static str] = &["landscape", "nature"];
    }
    impl ThemeDefault for Dark {
        const SOURCES: &'static [&'static str] = &["bing"];
        const QUERIES: &'static [&'static str] = &["space", "night", "dark"];
    }

    /// `[light]` / `[dark]`. Keys missing inside a present table keep that theme's defaults.
    #[derive(Debug, Deserialize)]
    #[serde(default, bound = "")]
    pub struct Theme<D: ThemeDefault> {
        pub sources: Vec<String>,
        pub queries: Vec<String>,
        #[serde(skip)]
        _default: std::marker::PhantomData<D>,
    }

    impl<D: ThemeDefault> Default for Theme<D> {
        fn default() -> Self {
            Self {
                sources: strings(D::SOURCES),
                queries: strings(D::QUERIES),
                _default: std::marker::PhantomData,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_home_rewrites_paths_under_home_only() {
        let home = Path::new("/home/u");
        assert_eq!(collapse_home("/home/u/Pictures", home), "~/Pictures");
        assert_eq!(collapse_home("/home/u", home), "~");
        assert_eq!(collapse_home("~/x", home), "~/x");
        assert_eq!(collapse_home("/home/user2/x", home), "/home/user2/x");
        assert_eq!(collapse_home("/mnt/x", home), "/mnt/x");
        assert_eq!(collapse_home("/mnt/x", Path::new("")), "/mnt/x");
    }

    #[test]
    fn legacy_defaults_match_go() {
        let old: legacy::LegacyConfig = toml::from_str("").expect("empty parses");
        assert!(old.bing.enabled);
        assert!(!old.wallhaven.enabled && !old.unsplash.enabled && !old.apod.enabled);
        assert!(!old.reddit.enabled && !old.local.enabled);
        assert_eq!(old.reddit.subreddits, ["EarthPorn", "wallpapers"]);
        assert!(old.local.recursive && old.local.dirs.is_empty());
        assert_eq!(old.light.sources, ["bing"]);
        assert_eq!(old.light.queries, ["landscape", "nature"]);
        assert_eq!(old.dark.queries, ["space", "night", "dark"]);
    }

    #[test]
    fn present_theme_table_keeps_missing_keys_at_their_defaults() {
        let old: legacy::LegacyConfig =
            toml::from_str("[dark]\nsources = [\"apod\"]\n").expect("parses");
        assert_eq!(old.dark.sources, ["apod"]);
        assert_eq!(old.dark.queries, ["space", "night", "dark"]);
    }

    #[test]
    fn unparseable_legacy_keys_leave_the_document_alone() {
        let text = "[bing]\nenabled = \"yes\"\n";
        let mut doc: DocumentMut = text.parse().expect("toml");
        assert_eq!(migrate_document(&mut doc, Path::new("/home/u")), None);
        assert_eq!(doc.to_string(), text);
    }
}
