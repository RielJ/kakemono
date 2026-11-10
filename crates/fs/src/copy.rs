//! Copies that never overwrite (Go `copyFile`, moved from `kakemono-engine`).

use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Collision names tried before [`copy_unique`] gives up.
const MAX_NAME_ATTEMPTS: u32 = 1000;

/// The step of a copy that failed, named as in Go's `*PathError` (`open`, `write`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyOp {
    /// Opening the source or creating the destination.
    Open,
    /// Copying the bytes or syncing the destination.
    Write,
}

impl CopyOp {
    /// Go's operation word: `open` or `write`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Write => "write",
        }
    }
}

impl fmt::Display for CopyOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why [`copy_unique`] failed: the step, the path it failed on, and the I/O error.
///
/// The parts are separate so callers can render Go's `<op> <path>: <reason>` text.
#[derive(Debug)]
pub struct CopyError {
    op: CopyOp,
    path: PathBuf,
    source: io::Error,
}

impl CopyError {
    fn new(op: CopyOp, path: &Path, source: io::Error) -> Self {
        Self {
            op,
            path: path.to_owned(),
            source,
        }
    }

    /// The step that failed.
    pub fn op(&self) -> CopyOp {
        self.op
    }

    /// The source for a failed open of the source, else the destination file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The underlying I/O error.
    pub fn io_error(&self) -> &io::Error {
        &self.source
    }
}

impl fmt::Display for CopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot {} {}", self.op, self.path.display())
    }
}

impl Error for CopyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

impl From<CopyError> for io::Error {
    fn from(err: CopyError) -> Self {
        io::Error::new(err.source.kind(), err)
    }
}

/// Copies `src` to `dst`, or to [`collision_name`]`(dst, <unix nanos>)` when `dst` exists, and
/// returns the path written. Never overwrites a file.
///
/// Each name is claimed with `create_new`, so concurrent copies to one name all get distinct
/// files. On a collision the nanosecond suffix counts up from the current time, up to 1000 names.
/// A copy that fails after the claim removes the partial file.
///
/// # Errors
///
/// [`CopyOp::Open`] on `src` when it can't be opened, [`CopyOp::Open`] on the destination when it
/// can't be created (`AlreadyExists` when no name was free), [`CopyOp::Write`] when copying fails.
pub fn copy_unique(src: &Path, dst: &Path) -> Result<PathBuf, CopyError> {
    let mut input = File::open(src).map_err(|e| CopyError::new(CopyOp::Open, src, e))?;
    let (mut output, written) = create_unique(dst)?;
    if let Err(e) = io::copy(&mut input, &mut output).and_then(|_| output.sync_all()) {
        drop(output);
        let _ = fs::remove_file(&written);
        return Err(CopyError::new(CopyOp::Write, &written, e));
    }
    Ok(written)
}

/// [`copy_unique`] to `dir/<file name of src>`.
///
/// # Errors
///
/// As [`copy_unique`]; also [`CopyOp::Open`] on `src` with `InvalidInput` when `src` has no file
/// name.
pub fn copy_unique_into(src: &Path, dir: &Path) -> Result<PathBuf, CopyError> {
    let name = src.file_name().ok_or_else(|| {
        CopyError::new(
            CopyOp::Open,
            src,
            io::Error::new(io::ErrorKind::InvalidInput, "not a file path"),
        )
    })?;
    copy_unique(src, &dir.join(name))
}

/// Go's collision name `<base>_<n><ext>`, where `ext` is Go's `filepath.Ext` (from the last `.`
/// of the file name, so `.hidden` becomes `_<n>.hidden`).
pub fn collision_name(dst: &Path, n: u128) -> PathBuf {
    let name = dst
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (base, ext) = name
        .rfind('.')
        .map_or((name.as_str(), ""), |i| name.split_at(i));
    dst.with_file_name(format!("{base}_{n}{ext}"))
}

fn create_unique(dst: &Path) -> Result<(File, PathBuf), CopyError> {
    let mut candidate = dst.to_owned();
    let mut nanos = unix_nanos();
    for _ in 0..=MAX_NAME_ATTEMPTS {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((file, candidate)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                candidate = collision_name(dst, nanos);
                nanos = nanos.saturating_add(1);
            }
            Err(e) => return Err(CopyError::new(CopyOp::Open, &candidate, e)),
        }
    }
    Err(CopyError::new(
        CopyOp::Open,
        dst,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("no free file name after {MAX_NAME_ATTEMPTS} attempts"),
        ),
    ))
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}
