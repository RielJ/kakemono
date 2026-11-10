//! Atomic file replacement.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::resolve::resolve;

/// Temp names tried before giving up. Only stale files from a crashed run with the same PID can
/// collide, since the counter is per process.
const MAX_TEMP_ATTEMPTS: u32 = 100;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Pauses before giving up on a rename that Windows refused for a moment; see
/// [`rename_backoff`].
const RENAME_RETRIES: u32 = 36;

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
    let permissions = match fs::metadata(target) {
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
    retry(
        || fs::rename(temp, target),
        is_transient_rename_error,
        rename_backoff(),
        std::thread::sleep,
    )
}

/// Pauses between rename attempts: 1 ms doubling to 64 ms, then 64 ms each, about 2 s in all.
fn rename_backoff() -> impl Iterator<Item = Duration> {
    (0..RENAME_RETRIES).map(|i| Duration::from_millis(1 << i.min(6)))
}

/// Runs `op`, and again after each pause while it fails with an error `transient` accepts.
/// Returns the first success, the first other error, or the last error once `pauses` runs out.
fn retry<T>(
    mut op: impl FnMut() -> io::Result<T>,
    transient: impl Fn(&io::Error) -> bool,
    pauses: impl IntoIterator<Item = Duration>,
    mut sleep: impl FnMut(Duration),
) -> io::Result<T> {
    let mut pauses = pauses.into_iter();
    loop {
        match op() {
            Err(e) if transient(&e) => match pauses.next() {
                Some(pause) => sleep(pause),
                None => return Err(e),
            },
            other => return other,
        }
    }
}

/// `ERROR_ACCESS_DENIED` and `ERROR_SHARING_VIOLATION`: `MoveFileExW` returns them while another
/// handle has the source or the target open without `FILE_SHARE_DELETE`, which only lasts as
/// long as that handle. Go's `cmd/go/internal/robustio` retries the same codes.
#[cfg(windows)]
fn is_transient_rename_error(e: &io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    matches!(
        e.raw_os_error(),
        Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
    )
}

/// A POSIX rename replaces the target even while it is open, so no error is transient.
#[cfg(not(windows))]
fn is_transient_rename_error(_e: &io::Error) -> bool {
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    fn busy() -> io::Error {
        io::Error::from(io::ErrorKind::ResourceBusy)
    }

    fn is_busy(e: &io::Error) -> bool {
        e.kind() == io::ErrorKind::ResourceBusy
    }

    /// An op that fails with `errors` in order, then succeeds; counts its calls.
    fn failing(errors: Vec<io::Error>, calls: &mut u32) -> impl FnMut() -> io::Result<()> + '_ {
        let mut errors = errors.into_iter();
        move || {
            *calls += 1;
            errors.next().map_or(Ok(()), Err)
        }
    }

    #[test]
    fn retry_succeeds_after_transient_errors() {
        let mut calls = 0;
        let mut slept = Vec::new();
        let result = retry(
            failing(vec![busy(), busy()], &mut calls),
            is_busy,
            rename_backoff(),
            |d| slept.push(d),
        );
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(calls, 3);
        assert_eq!(slept, [Duration::from_millis(1), Duration::from_millis(2)]);
    }

    #[test]
    fn retry_returns_the_last_error_when_the_pauses_run_out() {
        let mut calls = 0;
        let errors = (0..10).map(|_| busy()).collect();
        let pauses = [Duration::ZERO; 3];
        let err = retry(failing(errors, &mut calls), is_busy, pauses, |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ResourceBusy);
        assert_eq!(calls, 4);
    }

    #[test]
    fn retry_returns_other_errors_at_once() {
        let mut calls = 0;
        let errors = vec![io::Error::from(io::ErrorKind::NotFound), busy()];
        let err = retry(
            failing(errors, &mut calls),
            is_busy,
            rename_backoff(),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(calls, 1);
    }

    #[test]
    fn rename_backoff_waits_about_two_seconds_in_all() {
        let total: Duration = rename_backoff().sum();
        assert!(
            (Duration::from_millis(1900)..=Duration::from_millis(2100)).contains(&total),
            "{total:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn access_denied_and_sharing_violation_are_transient_on_windows() {
        for code in [5, 32] {
            assert!(
                is_transient_rename_error(&io::Error::from_raw_os_error(code)),
                "{code}"
            );
        }
        let not_found = io::Error::from_raw_os_error(2);
        assert!(!is_transient_rename_error(&not_found));
    }
}
