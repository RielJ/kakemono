//! Helpers shared by the test modules.

use std::path::{Path, PathBuf};

/// `dir` with symlinks resolved (macOS temp dirs sit behind `/var` → `/private/var`, and the
/// daemon reports resolved paths), in the form a user types. On Windows that means without the
/// `\\?\` prefix `canonicalize` adds (and with long names instead of 8.3 ones such as
/// `RUNNER~1`): real configs, arguments and working directories don't carry the prefix, so
/// tests with it would exercise paths no user has.
pub fn real_dir(dir: &Path) -> PathBuf {
    let real = dir.canonicalize().unwrap();
    #[cfg(windows)]
    if let Some(plain) = real
        .to_str()
        .and_then(|s| s.strip_prefix(r"\\?\"))
        .filter(|s| s.as_bytes().get(1) == Some(&b':'))
    {
        return PathBuf::from(plain);
    }
    real
}
