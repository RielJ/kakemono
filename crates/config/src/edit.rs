//! Comment-preserving edits to `config.toml` through `toml_edit`, written atomically.
//!
//! Every edit re-parses the result as a [`Config`] before writing, so a patch can't leave a file
//! that no longer loads.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use crate::defaults::home_dir;
use crate::error::ConfigError;
use crate::load::parse_str;
use crate::schema::{Config, DesktopConfig};

/// Reads `path` (an empty document if it doesn't exist), runs `f`, checks the result still loads,
/// and writes it atomically (temp file + rename, keeping the file mode; new files are 0600).
///
/// `f` returns `Err(reason)` to abort without writing; that becomes [`ConfigError::InvalidPatch`].
pub fn patch<F>(path: &Path, f: F) -> Result<(), ConfigError>
where
    F: FnOnce(&mut DocumentMut) -> Result<(), String>,
{
    let invalid = |reason: String| ConfigError::InvalidPatch {
        path: path.to_owned(),
        reason,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let mut doc: DocumentMut = text.parse().map_err(|source| ConfigError::EditParse {
        path: path.to_owned(),
        source: Box::new(source),
    })?;
    f(&mut doc).map_err(invalid)?;
    let out = doc.to_string();
    let _: Config = parse_str(&out, &home_dir()).map_err(|e| invalid(e.to_string()))?;
    write_atomic(path, out.as_bytes())
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
                let rows = to_array_of_tables(items).map_err(|e| format!("{key}: {e}"))?;
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

/// Writes `bytes` to `path` via a temp file in the same directory and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let write_err = |source| ConfigError::Write {
        path: path.to_owned(),
        source,
    };
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|source| ConfigError::CreateDir {
        path: dir.to_owned(),
        source,
    })?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let tmp: PathBuf = dir.join(format!(".{name}.tmp.{}", std::process::id()));

    let result = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        if let Ok(meta) = std::fs::metadata(path) {
            f.set_permissions(meta.permissions())?;
        }
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(write_err(e));
    }
    Ok(())
}
