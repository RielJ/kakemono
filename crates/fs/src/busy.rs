//! Retrying file operations that Windows refuses for a moment.

use std::io;
use std::time::Duration;

/// Pauses before giving up; see [`backoff`].
const RETRIES: u32 = 36;

/// Runs `op`, and again for about 2 s while Windows reports the file as busy (see [`is_busy`]).
/// Elsewhere it runs `op` once.
pub(crate) fn retry<T>(op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_with(op, is_busy, backoff(), std::thread::sleep)
}

/// Pauses between attempts: 1 ms doubling to 64 ms, then 64 ms each, about 2 s in all.
fn backoff() -> impl Iterator<Item = Duration> {
    (0..RETRIES).map(|i| Duration::from_millis(1 << i.min(6)))
}

/// Runs `op`, and again after each pause while it fails with an error `transient` accepts.
/// Returns the first success, the first other error, or the last error once `pauses` runs out.
fn retry_with<T>(
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

/// `ERROR_ACCESS_DENIED` and `ERROR_SHARING_VIOLATION`. A rename returns them while another
/// handle has the source or the target open without `FILE_SHARE_DELETE`, and opening a name
/// returns them while another writer's rename is replacing it. Both last only as long as the
/// other operation. Go's `cmd/go/internal/robustio` retries the same codes.
#[cfg(windows)]
fn is_busy(e: &io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    matches!(
        e.raw_os_error(),
        Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
    )
}

/// A POSIX rename replaces the target even while it is open, so no error is transient.
#[cfg(not(windows))]
fn is_busy(_e: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy_err() -> io::Error {
        io::Error::from(io::ErrorKind::ResourceBusy)
    }

    fn is_resource_busy(e: &io::Error) -> bool {
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
        let result = retry_with(
            failing(vec![busy_err(), busy_err()], &mut calls),
            is_resource_busy,
            backoff(),
            |d| slept.push(d),
        );
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(calls, 3);
        assert_eq!(slept, [Duration::from_millis(1), Duration::from_millis(2)]);
    }

    #[test]
    fn retry_returns_the_last_error_when_the_pauses_run_out() {
        let mut calls = 0;
        let errors = (0..10).map(|_| busy_err()).collect();
        let pauses = [Duration::ZERO; 3];
        let err = retry_with(
            failing(errors, &mut calls),
            is_resource_busy,
            pauses,
            |_| {},
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ResourceBusy);
        assert_eq!(calls, 4);
    }

    #[test]
    fn retry_returns_other_errors_at_once() {
        let mut calls = 0;
        let errors = vec![io::Error::from(io::ErrorKind::NotFound), busy_err()];
        let err = retry_with(
            failing(errors, &mut calls),
            is_resource_busy,
            backoff(),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(calls, 1);
    }

    #[test]
    fn backoff_waits_about_two_seconds_in_all() {
        let total: Duration = backoff().sum();
        assert!(
            (Duration::from_millis(1900)..=Duration::from_millis(2100)).contains(&total),
            "{total:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn access_denied_and_sharing_violation_are_transient_on_windows() {
        for code in [5, 32] {
            assert!(is_busy(&io::Error::from_raw_os_error(code)), "{code}");
        }
        let not_found = io::Error::from_raw_os_error(2);
        assert!(!is_busy(&not_found));
    }
}
