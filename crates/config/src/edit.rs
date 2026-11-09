//! Comment-preserving edits to `config.toml` through `toml_edit`, written atomically.
//!
//! Every edit re-parses the result as a [`Config`] before writing, so a patch can't leave a file
//! that no longer loads.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};
use toml_edit::{Array, DocumentMut, InlineTable, Item, TableLike, Value};

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
