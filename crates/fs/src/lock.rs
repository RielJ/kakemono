//! In-process locks keyed by resolved path, and read-modify-write under one.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use crate::resolve::resolve;
use crate::write::write_resolved;

/// Resolved paths whose [`PathLock`] is held.
static HELD: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
/// Signalled whenever a [`PathLock`] is released.
static RELEASED: Condvar = Condvar::new();

// The set is only changed by single `insert`/`remove` calls, so a panic can't leave it half
// updated and a poisoned lock is safe to use.
fn held() -> MutexGuard<'static, BTreeSet<PathBuf>> {
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An exclusive in-process lock on one file, released on drop.
///
/// The key is the file's resolved path (symlinks followed; see [`atomic_write`]), so a link and
/// its target share one lock. Hold it across a read-modify-write so that two threads can't both
/// read the old file and each write back a version without the other's change. It doesn't
/// coordinate with other processes.
///
/// The lock is not reentrant: acquiring a path the current thread already holds deadlocks.
/// Inside the lock, write with [`atomic_write`], not [`update`].
///
/// [`atomic_write`]: crate::atomic_write
#[derive(Debug)]
#[must_use = "the lock is released as soon as the guard is dropped"]
pub struct PathLock {
    path: PathBuf,
}

impl PathLock {
    /// Blocks until no other `PathLock` holds the file `path` resolves to, then takes it.
    ///
    /// # Errors
    ///
    /// An I/O error from resolving `path` (other than "not found", which is allowed).
    pub fn acquire(path: &Path) -> io::Result<Self> {
        let path = resolve(path)?;
        let mut held = held();
        while held.contains(&path) {
            held = RELEASED.wait(held).unwrap_or_else(PoisonError::into_inner);
        }
        held.insert(path.clone());
        Ok(Self { path })
    }

    /// The resolved path this lock holds: the file a write should go to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PathLock {
    fn drop(&mut self) {
        held().remove(&self.path);
        RELEASED.notify_all();
    }
}

/// Why [`update`] failed. The file is unchanged in every case.
#[derive(Debug)]
pub enum UpdateError<E> {
    /// Resolving or reading the file failed.
    Read(io::Error),
    /// The closure returned an error; nothing was written.
    Rejected(E),
    /// Writing the new contents failed.
    Write(io::Error),
}

impl<E: fmt::Display> fmt::Display for UpdateError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(_) => f.write_str("cannot read the file"),
            Self::Rejected(e) => e.fmt(f),
            Self::Write(_) => f.write_str("cannot write the file"),
        }
    }
}

impl<E: Error + 'static> Error for UpdateError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(e) | Self::Write(e) => Some(e),
            Self::Rejected(e) => e.source(),
        }
    }
}

/// Reads the file at `path`, passes its bytes (`None` if it doesn't exist) to `f`, and writes
/// what `f` returns with [`atomic_write`](crate::atomic_write), all under a [`PathLock`].
///
/// The folder must exist when the file is written.
///
/// # Errors
///
/// [`UpdateError::Rejected`] when `f` fails, otherwise the read or write I/O error.
pub fn update<F, E>(path: &Path, f: F) -> Result<(), UpdateError<E>>
where
    F: FnOnce(Option<&[u8]>) -> Result<Vec<u8>, E>,
{
    let lock = PathLock::acquire(path).map_err(UpdateError::Read)?;
    let old = match fs::read(lock.path()) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(UpdateError::Read(e)),
    };
    let new = f(old.as_deref()).map_err(UpdateError::Rejected)?;
    write_resolved(lock.path(), &new).map_err(UpdateError::Write)
}
