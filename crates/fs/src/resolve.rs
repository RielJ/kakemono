//! The real path behind a file name, also for files that don't exist yet.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Symlink hops followed before giving up, as Linux's `MAXSYMLINKS`.
const MAX_SYMLINK_HOPS: usize = 40;

/// The file `path` names after following symlinks.
///
/// An existing file gives its canonical path. A missing one (including the target of a dangling
/// symlink) gives the canonical path of its nearest existing ancestor joined with the rest, so a
/// file has the same resolved path before and after it is created.
pub(crate) fn resolve(path: &Path) -> io::Result<PathBuf> {
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
