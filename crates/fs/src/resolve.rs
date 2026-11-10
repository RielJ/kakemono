//! The real path behind a file name, also for files that don't exist yet.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::busy;

/// Symlink hops followed before giving up, as Linux's `MAXSYMLINKS`.
const MAX_SYMLINK_HOPS: usize = 40;

/// Times to resolve again when another writer replaced the file mid-resolve; see [`resolve`].
const MAX_REPLACED_RETRIES: usize = 100;

/// The file `path` names after following symlinks.
///
/// An existing file gives its canonical path. A missing one (including the target of a dangling
/// symlink) gives the canonical path of its nearest existing ancestor joined with the rest, so a
/// file has the same resolved path before and after it is created.
///
/// On Windows, `canonicalize` opens the file and asks the handle for its path, which races with
/// another writer renaming a new file over it. The open can be refused while the rename runs,
/// which is retried (see [`busy::retry`]). If the rename lands between the open and the query,
/// the handle is left on the replaced file, which NTFS has moved to `\$Extend\$Deleted`. That
/// path is never the answer, so the path is resolved again.
pub(crate) fn resolve(path: &Path) -> io::Result<PathBuf> {
    busy::retry(|| resolve_settled(path))
}

/// [`resolve`], once the open succeeded.
fn resolve_settled(path: &Path) -> io::Result<PathBuf> {
    for _ in 0..MAX_REPLACED_RETRIES {
        let resolved = resolve_once(path)?;
        if !(cfg!(windows) && is_ntfs_deleted(&resolved)) {
            return Ok(resolved);
        }
    }
    Err(io::Error::other(format!(
        "{} kept being replaced while it was resolved",
        path.display()
    )))
}

/// Whether `path` is inside NTFS's `\$Extend\$Deleted`, where a replaced file waits for its last
/// handle to close.
fn is_ntfs_deleted(path: &Path) -> bool {
    let names: Vec<&OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    names
        .windows(2)
        .any(|w| w[0].eq_ignore_ascii_case("$Extend") && w[1].eq_ignore_ascii_case("$Deleted"))
}

fn resolve_once(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            resolve_missing(&follow_dangling_link(path)?)
        }
        other => other,
    }
}

/// `path`, or the final target when `path` is a chain of symlinks ending at a missing file.
fn follow_dangling_link(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_owned();
    for _ in 0..MAX_SYMLINK_HOPS {
        let is_link = match fs::symlink_metadata(&current) {
            Ok(meta) => meta.is_symlink(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        if !is_link {
            return Ok(current);
        }
        let target = fs::read_link(&current)?;
        current = match current.parent() {
            Some(dir) => dir.join(target),
            None => target,
        };
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("too many levels of symbolic links: {}", path.display()),
    ))
}

fn resolve_missing(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut missing = Vec::new();
    let mut base = absolute.as_path();
    loop {
        match fs::canonicalize(base) {
            Ok(real) => {
                return Ok(missing
                    .iter()
                    .rev()
                    .fold(real, |acc: PathBuf, name| acc.join(name)));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // A `..` or root component has no file name; the absolute path is the best key.
                let (Some(parent), Some(name)) = (base.parent(), base.file_name()) else {
                    return Ok(absolute);
                };
                missing.push(name);
                base = parent;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_resolves_like_the_created_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("a.txt");
        let before = resolve(&path).unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(&path, "x").unwrap();
        assert_eq!(before, resolve(&path).unwrap());
    }

    #[test]
    fn ntfs_deleted_paths_are_recognised() {
        let deleted = if cfg!(windows) {
            r"\\?\C:\$Extend\$Deleted\000400000004BEE832637B1C"
        } else {
            "/$Extend/$deleted/000400000004BEE832637B1C"
        };
        assert!(is_ntfs_deleted(Path::new(deleted)));
        assert!(!is_ntfs_deleted(Path::new("/tmp/$Extend/state.json")));
        assert!(!is_ntfs_deleted(Path::new("/tmp/$Deleted/state.json")));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_loop_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        assert!(resolve(&a).is_err());
    }
}
