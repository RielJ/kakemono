//! Integration tests for `kakemono-fs`, in one test program.

mod copy;
mod create;
mod write;

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// How long a test that takes locks or spawns threads may run.
const DEADLINE: Duration = Duration::from_secs(10);

/// Runs `body` on its own thread and returns its result. A deadlock fails the test with
/// "timed out" instead of hanging the run; a panic in `body` is passed on.
fn within_deadline<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(body());
    });
    match rx.recv_timeout(DEADLINE) {
        Ok(value) => value,
        Err(RecvTimeoutError::Timeout) => panic!("timed out after {DEADLINE:?}"),
        Err(RecvTimeoutError::Disconnected) => match handle.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => unreachable!("the body returned without sending"),
        },
    }
}
