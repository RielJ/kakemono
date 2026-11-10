//! Writing a dropped file into `manual_dir`: name cleaning, content sniffing, size limit.

use super::{MAX_DROP_BYTES, SourceError, SrcResult};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// File-name extensions a dropped file may have.
const DROP_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// The bytes of a raw request body. The postMessage IPC fallback sends a `Uint8Array` as a JSON
/// array of numbers, so that is accepted too.
/// [`tauri::ipc::Request`] only lends the body, so it is copied, but only after the size check.
pub(super) fn body_bytes(body: &tauri::ipc::InvokeBody) -> SrcResult<Vec<u8>> {
    let len = match body {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.len(),
        tauri::ipc::InvokeBody::Json(serde_json::Value::Array(items)) => items.len(),
        tauri::ipc::InvokeBody::Json(_) => 0,
    };
    if len > MAX_DROP_BYTES {
        return Err(too_big());
    }
    match body {
        tauri::ipc::InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        tauri::ipc::InvokeBody::Json(json @ serde_json::Value::Array(_)) => {
            serde_json::from_value(json.clone()).map_err(|_| {
                SourceError::invalid("import_dropped_file: the body is not a byte array")
            })
        }
        tauri::ipc::InvokeBody::Json(_) => Err(SourceError::invalid(
            "import_dropped_file: expected the file's bytes as the request body",
        )),
    }
}

fn too_big() -> SourceError {
    SourceError::invalid(format!(
        "the image is larger than {} MB",
        MAX_DROP_BYTES / 1024 / 1024
    ))
}

// ── Dropped files ───────────────────────────────────────────────────────────

/// Writes `bytes` to a new file in `manual_dir` (created if missing) and returns its path.
///
/// `name` is reduced to its last component and to safe characters. Its extension must be
/// jpg, jpeg, png or webp, the bytes must start like a JPEG, PNG or WebP file, and there must
/// be at most [`MAX_DROP_BYTES`]. When the extension doesn't fit the content it is replaced
/// (`a.jpg` holding a PNG → `a.png`). A taken name gets `-2`, `-3`, ….
pub fn write_dropped_file(manual_dir: &Path, name: &str, bytes: &[u8]) -> SrcResult<PathBuf> {
    if bytes.len() > MAX_DROP_BYTES {
        return Err(too_big());
    }
    let (stem, ext) = safe_file_name(name);
    let ext_lower = ext.to_ascii_lowercase();
    if !DROP_EXTS.contains(&ext_lower.as_str()) {
        return Err(SourceError::invalid(format!(
            "{name:?} is not a .jpg, .jpeg, .png or .webp file"
        )));
    }
    let sniffed =
        sniff(bytes).ok_or_else(|| SourceError::invalid("not a JPEG, PNG or WebP image"))?;
    let fits = match sniffed {
        "jpg" => matches!(ext_lower.as_str(), "jpg" | "jpeg"),
        other => ext_lower == other,
    };
    let ext = if fits { ext } else { sniffed.to_owned() };

    std::fs::create_dir_all(manual_dir)
        .map_err(|e| SourceError::io(format!("create {}", manual_dir.display()), e))?;
    for n in 1_u32.. {
        let file = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        let path = manual_dir.join(file);
        let mut out = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(SourceError::io(format!("save {}", path.display()), e)),
        };
        if let Err(e) = out.write_all(bytes).and_then(|()| out.sync_all()) {
            let _ = std::fs::remove_file(&path);
            return Err(SourceError::io(format!("save {}", path.display()), e));
        }
        return Ok(path);
    }
    Err(SourceError::invalid(
        "no free file name in the manual folder",
    ))
}

/// `(stem, extension)` of the last component of `name` (after `/` or `\`). Characters other
/// than letters, digits, space, `-`, `_` and `.` become `_`, leading dots and `..` are removed,
/// and the stem is cut to 100 characters. An empty stem becomes `image`.
fn safe_file_name(name: &str) -> (String, String) {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.');
    let (stem, ext) = clean.rsplit_once('.').unwrap_or((clean, ""));
    let stem: String = stem.replace("..", "_").chars().take(100).collect();
    let stem = stem.trim_end_matches('.').trim();
    let stem = if stem.is_empty() { "image" } else { stem };
    (stem.to_owned(), ext.to_owned())
}

/// The extension for the image type whose magic bytes `bytes` start with.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Decodes `%XX` escapes (as `encodeURIComponent` writes them). `None` for a bad escape or
/// bytes that aren't UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::*;
    use crate::error::{ErrorKind, Kinded};

    // ── write_dropped_file ──

    #[test]
    fn dropped_files_get_unique_names_in_manual_dir() {
        let root = tempfile::tempdir().unwrap();
        let manual = root.path().join("manual");
        let first = write_dropped_file(&manual, "photo.jpg", JPEG).unwrap();
        let second = write_dropped_file(&manual, "photo.jpg", JPEG).unwrap();
        assert_eq!(first, manual.join("photo.jpg"));
        assert_eq!(second, manual.join("photo-2.jpg"));
        assert_eq!(std::fs::read(&second).unwrap(), JPEG);
        assert_eq!(
            write_dropped_file(&manual, "x.webp", WEBP).unwrap(),
            manual.join("x.webp")
        );
        assert_eq!(
            write_dropped_file(&manual, "y.JPEG", JPEG).unwrap(),
            manual.join("y.JPEG")
        );
    }

    #[test]
    fn dropped_file_names_cannot_leave_manual_dir() {
        let root = tempfile::tempdir().unwrap();
        let manual = root.path().join("manual");
        for name in [
            "../../evil.png",
            "/etc/evil.png",
            "..\\evil.png",
            ".evil.png",
        ] {
            let path = write_dropped_file(&manual, name, PNG).unwrap();
            assert_eq!(path.parent(), Some(manual.as_path()), "{name}");
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(!file.starts_with('.') && !file.contains(".."), "{file}");
        }
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            1,
            "only manual/ was created"
        );
    }

    #[test]
    fn dropped_files_must_be_jpeg_png_or_webp() {
        let root = tempfile::tempdir().unwrap();
        let manual = root.path().join("manual");
        for (name, bytes) in [
            ("a.gif", PNG),
            ("noext", PNG),
            ("a.png", b"GIF89a".as_slice()),
            ("a.png", b"".as_slice()),
        ] {
            let err = write_dropped_file(&manual, name, bytes).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid, "{name}");
        }
        assert!(!manual.exists() || std::fs::read_dir(&manual).unwrap().count() == 0);
        assert_eq!(
            write_dropped_file(&manual, "a.jpg", PNG).unwrap(),
            manual.join("a.png"),
            "extension fixed to the content"
        );
    }

    #[test]
    fn dropped_files_over_64_mb_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut big = vec![0_u8; MAX_DROP_BYTES + 1];
        big[..PNG.len()].copy_from_slice(PNG);
        let err = write_dropped_file(root.path(), "big.png", &big).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Invalid);
        assert!(err.to_string().contains("64 MB"), "{err}");
    }

    #[test]
    fn percent_decoding_reads_encode_uri_component() {
        assert_eq!(
            percent_decode("na%C3%AFve%20photo.jpg").as_deref(),
            Some("naïve photo.jpg")
        );
        assert_eq!(percent_decode("plain.png").as_deref(), Some("plain.png"));
        assert_eq!(percent_decode("bad%2"), None);
        assert_eq!(percent_decode("bad%zz"), None);
        assert_eq!(percent_decode("%FF"), None);
    }

    #[test]
    fn dropped_bytes_come_raw_or_as_a_json_array() {
        use tauri::ipc::InvokeBody;
        assert_eq!(body_bytes(&InvokeBody::Raw(vec![1, 2])).unwrap(), [1, 2]);
        let json = InvokeBody::Json(serde_json::json!([255, 0]));
        assert_eq!(body_bytes(&json).unwrap(), [255, 0]);
        for bad in [serde_json::json!([256]), serde_json::json!({"a": 1})] {
            let err = body_bytes(&InvokeBody::Json(bad)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Invalid);
        }
    }

    #[test]
    fn oversized_bodies_are_refused_before_copying() {
        use tauri::ipc::InvokeBody;
        let raw = InvokeBody::Raw(vec![0; MAX_DROP_BYTES + 1]);
        let err = body_bytes(&raw).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Invalid);
        assert!(err.to_string().contains("64 MB"), "{err}");
        let json = InvokeBody::Json(serde_json::Value::Array(vec![
            serde_json::Value::from(0);
            MAX_DROP_BYTES + 1
        ]));
        assert_eq!(body_bytes(&json).unwrap_err().kind(), ErrorKind::Invalid);
    }
}
