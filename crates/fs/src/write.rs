//! Atomic file replacement.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::busy;
use crate::resolve::resolve;

/// Temp names tried before giving up. Only stale files from a crashed run with the same PID can
/// collide, since the counter is per process.
const MAX_TEMP_ATTEMPTS: u32 = 100;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replaces the file at `path` with `bytes`, so that readers see either the old or the new
/// contents, never a mix.
///
/// The bytes go to a new temp file next to the target (`.<name>.tmp.<pid>.<n>`, created with
/// `create_new`, so concurrent writers never share one), which is synced and renamed over the
/// target; then the folder is synced. If the target exists, its permissions are kept; a new file
/// gets mode 0600 on Unix. If `path` is a symlink, the file it points at is replaced and the link
/// stays.
///
/// The folder must exist. This does not take a [`PathLock`](crate::PathLock): concurrent writes
/// are safe, but for a read-modify-write use [`update`](crate::update).
///
/// # Errors
///
/// Any I/O error from resolving the path, writing the temp file, the rename, or syncing the
/// folder. On error before the rename the target is unchanged and the temp file is removed.
///
/// On Windows a rename fails with "access denied" or a sharing violation while some other handle
/// has the temp file or the target open without delete sharing: another writer's rename onto the
/// same target, or a virus scanner or indexer looking at the file that was just written. Those
/// errors are retried for about 2 s before they are returned.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_resolved(&resolve(path)?, bytes)
}

/// [`atomic_write`] to a path that is already resolved (no symlinks to follow).
pub(crate) fn write_resolved(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = target.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a file path: {}", target.display()),
        )
    })?;
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let permissions = match busy::retry(|| fs::metadata(target)) {
        Ok(meta) => Some(meta.permissions()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let (file, temp) = create_temp(dir, name)?;
    if let Err(e) = fill_and_rename(file, &temp, target, bytes, permissions) {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    sync_dir(dir)
}

pub(crate) fn create_temp(dir: &Path, name: &OsStr) -> io::Result<(File, PathBuf)> {
    let pid = std::process::id();
    let mut last_err = None;
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temp_name = OsStr::new(".").to_owned();
        temp_name.push(name);
        temp_name.push(format!(".tmp.{pid}.{n}"));
        let temp = dir.join(temp_name);
        match private_options().open(&temp) {
            Ok(file) => return Ok((file, temp)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists)))
}

/// `create_new` write options; mode 0600 on Unix, since the config may hold API keys.
pub(crate) fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn fill_and_rename(
    mut file: File,
    temp: &Path,
    target: &Path,
    bytes: &[u8],
    permissions: Option<Permissions>,
) -> io::Result<()> {
    file.write_all(bytes)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.sync_all()?;
    drop(file);
    busy::retry(|| fs::rename(temp, target))
}

/// Makes the rename durable. Some file systems can't sync a folder (`EINVAL`); the rename has
/// already happened there, so that is not an error.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    match File::open(dir).and_then(|d| d.sync_all()) {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        other => other,
    }
}

/// Windows can't open a folder as a `File` without extra flags, and NTFS journals the rename.
#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}
