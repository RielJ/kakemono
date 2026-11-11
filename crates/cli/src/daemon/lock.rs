//! Single-instance lock: an exclusive `File::try_lock` on `<runtime_dir>/daemon.lock`.
//!
//! The lock, not the socket, decides which daemon runs. Connect-then-bind races when launchd and
//! a manual start both see a stale socket; a file lock does not, and the OS drops it on crash.
//! The lock file holds the owner's PID (informational only; the lock is the truth). It is never
//! deleted, so a new daemon never races an unlink.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Lock file name inside the runtime dir.
pub use kakemono_config::paths::DAEMON_LOCK_FILE_NAME as LOCK_FILE_NAME;

/// Why [`acquire`] failed.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another daemon holds the lock. `pid` is what it wrote to the lock file, if readable.
    #[error("{}", already_running_message(path, *pid))]
    AlreadyRunning { path: PathBuf, pid: Option<u32> },
    /// The runtime dir or lock file could not be created, opened or locked.
    /// The message ends with the cause, which is therefore not also the error's `source()`.
    #[error("daemon lock {}: {cause}", path.display())]
    Io { path: PathBuf, cause: io::Error },
}

fn already_running_message(path: &Path, pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!(
            "daemon is already running (pid {pid}, lock: {})",
            path.display()
        ),
        None => format!("daemon is already running (lock: {})", path.display()),
    }
}

/// Proof that this process is the only daemon. The lock is released when this is dropped (or
/// the process exits). Required by `server::bind`, so the stale-socket unlink can only happen
/// under the lock.
#[derive(Debug)]
pub struct LockGuard {
    file: File,
    path: PathBuf,
}

impl LockGuard {
    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // The OS releases it anyway when the fd closes; unlocking first is just explicit.
        let _ = self.file.unlock();
    }
}

/// Takes the daemon lock in `runtime_dir` (created with mode 0700 on Unix if missing) without
/// blocking, then writes this process's PID into the lock file.
pub fn acquire(runtime_dir: &Path) -> Result<LockGuard, LockError> {
    let path = runtime_dir.join(LOCK_FILE_NAME);
    let io_err = |cause| LockError::Io {
        path: path.clone(),
        cause,
    };
    create_private_dir(runtime_dir).map_err(io_err)?;
    // No truncate on open: the holder's PID must survive a failed attempt.
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(io_err)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let pid = read_pid(&mut file);
            return Err(LockError::AlreadyRunning { path, pid });
        }
        Err(TryLockError::Error(e)) => return Err(io_err(e)),
    }
    write_pid(&mut file).map_err(io_err)?;
    Ok(LockGuard { file, path })
}

fn read_pid(file: &mut File) -> Option<u32> {
    let mut text = String::new();
    // On Windows the holder's byte-range lock blocks reads; that is just "pid unknown".
    file.read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
}

fn write_pid(file: &mut File) -> io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;
    file.flush()
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let rt = dir.path().join("run");
        let guard = acquire(&rt).unwrap();
        assert_eq!(guard.path(), rt.join(LOCK_FILE_NAME));

        let err = acquire(&rt).unwrap_err();
        match &err {
            LockError::AlreadyRunning { path, pid } => {
                assert_eq!(path, &rt.join(LOCK_FILE_NAME));
                if cfg!(unix) {
                    assert_eq!(*pid, Some(std::process::id()));
                }
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            err.to_string().starts_with("daemon is already running ("),
            "{err}"
        );
        drop(guard);
        // The holder wrote its PID, and the failed attempt did not wipe it. This reads after the
        // drop because Windows blocks reads of a locked file.
        let text = std::fs::read_to_string(rt.join(LOCK_FILE_NAME)).unwrap();
        assert_eq!(text.trim(), std::process::id().to_string());
        let again = acquire(&rt).unwrap();
        drop(again);
    }

    #[cfg(unix)]
    #[test]
    fn creates_runtime_dir_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let rt = dir.path().join("a/b");
        let _g = acquire(&rt).unwrap();
        let mode = std::fs::metadata(&rt).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn unopenable_runtime_dir_is_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        let err = acquire(&blocker.join("run")).unwrap_err();
        assert!(matches!(err, LockError::Io { .. }), "{err:?}");
        assert!(err.to_string().starts_with("daemon lock "), "{err}");
    }

    #[test]
    fn message_without_pid() {
        let msg = already_running_message(Path::new("/r/daemon.lock"), None);
        assert_eq!(msg, "daemon is already running (lock: /r/daemon.lock)");
    }
}
