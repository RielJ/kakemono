//! Atomic create-new.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

use crate::write::{create_temp, private_options, sync_dir};

/// Creates the file at `path` with `bytes`, unless something already exists there. Readers never
/// see a partial file, and an existing file (or symlink) is never replaced, even by another
/// process racing on the same path.
///
/// The bytes go to a new temp file next to the target (as for [`atomic_write`](crate::atomic_write)),
/// which is synced and hard-linked to `path` (an atomic create-new), then removed; then the folder
/// is synced. The new file gets mode 0600 on Unix. Where the file system can't hard-link, the
/// bytes are written straight to `path` opened with `create_new`, which still never replaces a
/// file but can show a partial one while it is written.
///
/// Returns `Ok(true)` when this call created the file, `Ok(false)` when `path` already existed.
/// The folder must exist.
///
/// # Errors
///
/// Any I/O error from writing the temp file, the link, the fallback write, or syncing the folder.
/// No temp file is left behind.
pub fn create_new(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a file path: {}", path.display()),
        )
    })?;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (mut file, temp) = create_temp(dir, name)?;
    let filled = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = filled {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    let linked = fs::hard_link(&temp, path);
    let _ = fs::remove_file(&temp);
    match linked {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        // No hard links here (FAT, some network shares): fall back to a plain create-new. A real
        // failure, such as a read-only folder, fails there too and is reported from there.
        Err(_) => {
            if !write_new(path, bytes)? {
                return Ok(false);
            }
        }
    }
    sync_dir(dir)?;
    Ok(true)
}

/// Writes `bytes` to `path` opened with `create_new` (mode 0600 on Unix). `Ok(false)` when `path`
/// exists. A failed write removes the file it created.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let mut file = match private_options().open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(e);
    }
    Ok(true)
}
