//! File helpers shared by the kakemono crates (audit refactors 1 and 2).
//!
//! - [`atomic_write`]: replace a file through a unique temp file and a rename, keeping its
//!   permissions and any symlink that points at it.
//! - [`create_new`]: create a file atomically, never replacing one that exists.
//! - [`PathLock`] and [`update`]: serialise read-modify-write cycles on one file within the
//!   process.
//! - [`copy_unique`] and [`copy_unique_into`]: copy without ever overwriting, using Go's
//!   `<stem>_<unix nanos>.<ext>` collision name.

mod busy;
mod copy;
mod create;
mod lock;
mod resolve;
mod write;

pub use copy::{CopyError, CopyOp, collision_name, copy_unique, copy_unique_into};
pub use create::create_new;
pub use lock::{PathLock, UpdateError, update};
pub use write::atomic_write;
