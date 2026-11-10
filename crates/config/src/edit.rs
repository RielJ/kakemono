//! Comment-preserving edits to `config.toml` through `toml_edit`, written atomically.
//!
//! Every edit re-parses the result as a [`Config`] before writing, so a patch can't leave a file
//! that no longer loads.

use std::io;
use std::path::Path;

use kakemono_fs::UpdateError;
use serde_json::{Map, Value as Json};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use crate::defaults::home_dir;
use crate::error::ConfigError;
use crate::load::parse_str;
use crate::schema::{Config, DesktopConfig, SourceEntry, SourceType};
use crate::sources::normalize_sources;

/// Reads `path` (an empty document if it doesn't exist), runs `f`, checks the result still loads,
/// and writes it with [`kakemono_fs::update`]: atomically, keeping the file mode (new files are
/// 0600) and any symlink, under a per-file lock so concurrent patches in this process don't lose
/// each other's changes.
///
/// `f` returns `Err(reason)` to abort without writing; that becomes [`ConfigError::InvalidPatch`].
pub fn patch<F>(path: &Path, f: F) -> Result<(), ConfigError>
where
    F: FnOnce(&mut DocumentMut) -> Result<(), String>,
{
    create_parent(path)?;
    kakemono_fs::update(path, |old| patched(path, old, f)).map_err(|e| match e {
        UpdateError::Read(source) => ConfigError::Read {
            path: path.to_owned(),
            cause: source,
        },
        UpdateError::Rejected(e) => e,
        UpdateError::Write(source) => ConfigError::Write {
            path: path.to_owned(),
            cause: source,
        },
    })
}

/// The new file text for [`patch`]: `old` (or an empty document) edited by `f` and validated.
fn patched<F>(path: &Path, old: Option<&[u8]>, f: F) -> Result<Vec<u8>, ConfigError>
where
    F: FnOnce(&mut DocumentMut) -> Result<(), String>,
{
    let invalid = |reason: String| ConfigError::InvalidPatch {
        path: path.to_owned(),
        reason,
    };
    let text = std::str::from_utf8(old.unwrap_or_default()).map_err(|e| ConfigError::Read {
        path: path.to_owned(),
        cause: io::Error::new(io::ErrorKind::InvalidData, e),
    })?;
    let mut doc: DocumentMut = text.parse().map_err(|cause| ConfigError::EditParse {
        path: path.to_owned(),
        cause: Box::new(cause),
    })?;
    f(&mut doc).map_err(invalid)?;
    let out = doc.to_string();
    let _: Config = parse_str(&out, &home_dir()).map_err(|e| invalid(e.to_string()))?;
    Ok(out.into_bytes())
}

/// Replaces the `[desktop]` keys with `desktop`, keeping comments and unknown keys. `None` fields
/// remove their key.
pub fn set_desktop(path: &Path, desktop: &DesktopConfig) -> Result<(), ConfigError> {
    patch(path, |doc| set_desktop_doc(doc, desktop))
}

/// Merges a JSON patch (as sent by the UI) into the file. Objects merge recursively into tables,
/// `null` removes a key, anything else replaces the value and keeps its comments.
pub fn apply_patch(path: &Path, json_patch: &Json) -> Result<(), ConfigError> {
    patch(path, |doc| apply_json(doc, json_patch))
}

/// [`set_desktop`] on an in-memory document.
pub fn set_desktop_doc(doc: &mut DocumentMut, desktop: &DesktopConfig) -> Result<(), String> {
    let mut obj = match serde_json::to_value(desktop).map_err(|e| e.to_string())? {
        Json::Object(m) => m,
        other => return Err(format!("desktop serialised to {other}, expected an object")),
    };
    // Optional fields are skipped when `None`; turn them into removals.
    for key in ["set_command", "detected_session", "detected_at"] {
        obj.entry(key).or_insert(Json::Null);
    }
    let mut root = Map::new();
    root.insert("desktop".to_owned(), Json::Object(obj));
    apply_json(doc, &Json::Object(root))
}

/// [`apply_patch`] on an in-memory document. The patch must be a JSON object.
///
/// A top-level array whose items are all objects (like `sources`) is written as `[[key]]` blocks,
/// so the file stays hand-editable. It replaces existing blocks where they stood, keeping the
/// comment above the first one. A top-level empty array removes the blocks and writes `key = []`.
pub fn apply_json(doc: &mut DocumentMut, json_patch: &Json) -> Result<(), String> {
    let Json::Object(obj) = json_patch else {
        return Err("patch must be a JSON object".to_owned());
    };
    merge(doc.as_table_mut(), obj, true)
}

fn merge(table: &mut dyn TableLike, obj: &Map<String, Json>, root: bool) -> Result<(), String> {
    for (key, val) in obj {
        match val {
            Json::Null => {
                table.remove(key);
            }
            Json::Array(items) if root && is_rows(items) => {
                let mut rows = to_array_of_tables(items).map_err(|e| format!("{key}: {e}"))?;
                if key == SOURCES_KEY {
                    keep_raw_types(table.get(key), &mut rows);
                }
                replace_rows(table, key, rows);
            }
            Json::Array(items)
                if root
                    && items.is_empty()
                    && table.get(key).is_some_and(Item::is_array_of_tables) =>
            {
                // Re-insert rather than overwrite: the old `[[key]]` header leaves key decor
                // that would print as `key= []`.
                table.remove(key);
                table.insert(key, Item::Value(Value::Array(Array::new())));
            }
            Json::Object(child) => {
                let exists = table.get(key).is_some_and(Item::is_table_like);
                if !exists {
                    let fresh = if root {
                        toml_edit::table()
                    } else {
                        Item::Value(Value::InlineTable(InlineTable::new()))
                    };
                    table.insert(key, fresh);
                }
                let inner = table
                    .get_mut(key)
                    .and_then(Item::as_table_like_mut)
                    .ok_or_else(|| format!("{key}: not a table"))?;
                merge(inner, child, false)?;
            }
            scalar => {
                let mut new = to_value(scalar).map_err(|e| format!("{key}: {e}"))?;
                match table.get_mut(key) {
                    Some(item) => {
                        if let Some(old) = item.as_value() {
                            *new.decor_mut() = old.decor().clone();
                        }
                        *item = Item::Value(new);
                    }
                    None => {
                        table.insert(key, Item::Value(new));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The top-level key of the source rows.
const SOURCES_KEY: &str = "sources";

/// The raw `type` of each `[[sources]]` row in `doc`, in file order: `None` when the row has no
/// `type` key. Handles both `[[sources]]` blocks and an inline `sources = [{…}]` array.
pub(crate) fn raw_source_types(doc: &DocumentMut) -> Vec<Option<Item>> {
    source_rows(doc.get(SOURCES_KEY))
        .into_iter()
        .map(|row| row.get("type").cloned())
        .collect()
}

/// The rows at `item` (`[[sources]]` blocks or an inline array of tables) as table-likes.
fn source_rows(item: Option<&Item>) -> Vec<&dyn TableLike> {
    match item {
        Some(Item::ArrayOfTables(aot)) => aot.iter().map(|t| t as &dyn TableLike).collect(),
        Some(Item::Value(Value::Array(arr))) => arr
            .iter()
            .filter_map(Value::as_inline_table)
            .map(|t| t as &dyn TableLike)
            .collect(),
        _ => Vec::new(),
    }
}

/// A row this version loads as `unsupported` has its real `type` in the file: one written by a
/// newer kakemono, a typo, or none at all. The UI sends such a row back as `"unsupported"`; this
/// puts the file's raw `type` back (or leaves the key out, as the file had it) so a save of the
/// list never destroys or invents a type. Rows are matched by id, normalised the way loading
/// does, so the type follows its row through reorders and removals.
fn keep_raw_types(old: Option<&Item>, rows: &mut ArrayOfTables) {
    let old_rows = source_rows(old);
    if old_rows.is_empty() {
        return;
    }
    let str_of = |row: &dyn TableLike, key: &str| {
        row.get(key)
            .and_then(Item::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let mut entries: Vec<SourceEntry> = old_rows
        .iter()
        .map(|row| SourceEntry {
            id: str_of(*row, "id"),
            enabled: true,
            kind: row
                .get("type")
                .and_then(Item::as_str)
                .map_or(SourceType::Unsupported, SourceType::from_config_str),
            location: str_of(*row, "location"),
        })
        .collect();
    normalize_sources(&mut entries);
    let unsupported = SourceType::Unsupported.as_str();
    for row in rows.iter_mut() {
        if row.get("type").and_then(Item::as_str) != Some(unsupported) {
            continue;
        }
        let Some(id) = row.get("id").and_then(Item::as_str) else {
            continue;
        };
        let Some(i) = entries
            .iter()
            .position(|e| e.id == id && e.kind == SourceType::Unsupported)
        else {
            continue;
        };
        match old_rows[i].get("type") {
            Some(raw) => {
                row.insert("type", raw.clone());
            }
            None => {
                row.remove("type");
            }
        }
    }
}

/// Keys a `[[sources]]` row writes first, in this order; any others follow alphabetically. JSON
/// objects arrive key-sorted, which would put `id` after `enabled`.
const ROW_KEY_ORDER: [&str; 4] = ["id", "enabled", "type", "location"];

fn is_rows(items: &[Json]) -> bool {
    !items.is_empty() && items.iter().all(Json::is_object)
}

fn to_array_of_tables(items: &[Json]) -> Result<ArrayOfTables, String> {
    let mut aot = ArrayOfTables::new();
    for item in items {
        let Json::Object(map) = item else {
            return Err("expected an array of objects".to_owned());
        };
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by_key(|k| {
            let rank = ROW_KEY_ORDER.iter().position(|o| o == k);
            (rank.unwrap_or(ROW_KEY_ORDER.len()), k.as_str())
        });
        let mut t = Table::new();
        for k in keys {
            let v = &map[k.as_str()];
            if !v.is_null() {
                t.insert(
                    k,
                    Item::Value(to_value(v).map_err(|e| format!("{k}: {e}"))?),
                );
            }
        }
        aot.push(t);
    }
    Ok(aot)
}

/// Puts `rows` at `key`. Existing `[[key]]` blocks are replaced where the first one stood, and
/// the first new block takes over its leading comment; anything else at `key` is dropped and the
/// blocks go at the end of the file.
fn replace_rows(table: &mut dyn TableLike, key: &str, mut rows: ArrayOfTables) {
    let anchor = table
        .get(key)
        .and_then(Item::as_array_of_tables)
        .and_then(|old| old.get(0))
        .map(|first| (first.position(), first.decor().prefix().cloned()));
    if let Some((position, prefix)) = anchor {
        for t in rows.iter_mut() {
            t.set_position(position);
        }
        if let (Some(first), Some(prefix)) = (rows.get_mut(0), prefix) {
            first.decor_mut().set_prefix(prefix);
        }
    }
    table.remove(key);
    table.insert(key, Item::ArrayOfTables(rows));
}

fn to_value(j: &Json) -> Result<Value, String> {
    Ok(match j {
        Json::Null => return Err("null is only allowed as a direct table value".to_owned()),
        Json::Bool(b) => Value::from(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(f) = n.as_f64().filter(|_| !n.is_u64()) {
                Value::from(f)
            } else {
                return Err(format!("number {n} is out of range"));
            }
        }
        Json::String(s) => Value::from(s.as_str()),
        Json::Array(items) => {
            let mut arr = Array::new();
            for item in items {
                arr.push(to_value(item)?);
            }
            Value::Array(arr)
        }
        Json::Object(map) => {
            let mut t = InlineTable::new();
            for (k, v) in map {
                if !v.is_null() {
                    t.insert(k, to_value(v)?);
                }
            }
            Value::InlineTable(t)
        }
    })
}

/// Writes `bytes` to `path` with [`kakemono_fs::atomic_write`], creating the folder first.
///
/// This doesn't lock the file; a read-modify-write goes through [`patch`] or holds a
/// [`kakemono_fs::PathLock`].
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    create_parent(path)?;
    kakemono_fs::atomic_write(path, bytes).map_err(|cause| ConfigError::Write {
        path: path.to_owned(),
        cause,
    })
}

fn create_parent(path: &Path) -> Result<(), ConfigError> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|cause| ConfigError::CreateDir {
        path: dir.to_owned(),
        cause,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    const ROUNDS: usize = 50;

    /// Runs `body` on its own thread; a deadlock fails the test with "timed out" instead of
    /// hanging the run, and a panic in `body` is passed on.
    fn within_deadline(body: impl FnOnce() + Send + 'static) {
        const DEADLINE: Duration = Duration::from_secs(10);
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            body();
            let _ = tx.send(());
        });
        match rx.recv_timeout(DEADLINE) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => panic!("timed out after {DEADLINE:?}"),
            Err(RecvTimeoutError::Disconnected) => {
                if let Err(panic) = handle.join() {
                    std::panic::resume_unwind(panic);
                }
            }
        }
    }

    fn interval(path: &Path) -> String {
        let text = std::fs::read_to_string(path).unwrap();
        parse_str(&text, &home_dir()).unwrap().daemon.interval
    }

    #[test]
    fn concurrent_patches_of_different_keys_both_survive() {
        within_deadline(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, "# keep me\n[daemon]\ninterval = \"1h\"\n").unwrap();
            let barrier = Barrier::new(2);

            std::thread::scope(|s| {
                let writers = [("interval", "daemon"), ("theme", "ui")].map(|(key, table)| {
                    let (path, barrier) = (&path, &barrier);
                    s.spawn(move || {
                        barrier.wait();
                        for round in 0..ROUNDS {
                            let value = match key {
                                "interval" => json!(format!("{}m", round + 1)),
                                _ => json!(if round % 2 == 0 { "dark" } else { "light" }),
                            };
                            apply_patch(path, &json!({ table: { key: value } })).unwrap();
                        }
                    })
                });
                for writer in writers {
                    writer.join().unwrap();
                }
            });

            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.starts_with("# keep me\n"), "{text}");
            assert_eq!(interval(&path), format!("{ROUNDS}m"), "{text}");
            let doc: DocumentMut = text.parse().unwrap();
            assert_eq!(doc["ui"]["theme"].as_str(), Some("light"), "{text}");
        });
    }

    #[test]
    fn concurrent_writes_use_unique_temp_files() {
        within_deadline(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let barrier = Barrier::new(4);

            std::thread::scope(|s| {
                for writer in 0..4 {
                    let (path, barrier) = (&path, &barrier);
                    s.spawn(move || {
                        barrier.wait();
                        for round in 0..ROUNDS {
                            let body = format!("# writer {writer} round {round}\n").repeat(64);
                            write_atomic(path, body.as_bytes()).unwrap();
                        }
                    });
                }
            });

            let text = std::fs::read_to_string(&path).unwrap();
            let first = text.lines().next().unwrap();
            assert_eq!(text, format!("{first}\n").repeat(64), "torn write");
            let names: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(names, ["config.toml"], "temp files left behind");
        });
    }

    #[cfg(unix)]
    #[test]
    fn patch_through_a_symlink_keeps_the_link() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let dotfiles = dir.path().join("dotfiles");
        std::fs::create_dir(&dotfiles).unwrap();
        let target = dotfiles.join("config.toml");
        std::fs::write(&target, "# managed by stow\n[daemon]\ninterval = \"1h\"\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        apply_patch(&link, &json!({ "daemon": { "interval": "2h" } })).unwrap();

        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(interval(&target), "2h");
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.starts_with("# managed by stow\n"), "{text}");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }
}
