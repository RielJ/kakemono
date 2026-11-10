//! File helpers shared by the kakemono crates (audit refactors 1 and 2).
//!
//! - [`atomic_write`]: replace a file through a unique temp file and a rename, keeping its
//!   permissions and any symlink that points at it.
//! - [`PathLock`] and [`update`]: serialise read-modify-write cycles on one file within the
//!   process.
//! - [`copy_unique`] and [`copy_unique_into`]: copy without ever overwriting, using Go's
//!   `<stem>_<unix nanos>.<ext>` collision name.

mod copy;
mod lock;
mod resolve;
mod write;

pub use copy::{CopyError, CopyOp, collision_name, copy_unique, copy_unique_into};
pub use lock::{PathLock, UpdateError, update};
pub use write::atomic_write;
