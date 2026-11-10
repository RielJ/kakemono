//! The `{ kind, message }` error every command sends the frontend.
//!
//! Each command error type ([`CommandError`](crate::commands::CommandError),
//! [`SourceError`](crate::commands_sources::SourceError),
//! [`LifecycleError`](crate::lifecycle::LifecycleError)) implements [`Kinded`] and serialises
//! through [`WireError`], so the wire shape is defined once. `ErrorKind` and `WireError` are
//! exported to TypeScript with ts-rs.

use serde::Serialize;
use ts_rs::TS;

/// Stable machine-readable error kind. The frontend branches on a few of these
/// (`daemon_not_running`, `daemon_unreachable`); the rest pick the wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Nothing listens on the daemon socket.
    DaemonNotRunning,
    /// A source command could not reach the daemon: it is not running and could not be
    /// started, or the connection failed. The UI offers "Add anyway" only for this.
    DaemonUnreachable,
    /// The daemon did not answer in time; it may still finish the request.
    DaemonTimeout,
    /// The daemon answered with an error, or the connection broke mid-request.
    Daemon,
    Config,
    /// Bad input from the frontend.
    Invalid,
    Io,
    Opener,
    /// A bug or a version mismatch: a reply that did not decode, a failed background task.
    Internal,
    CliNotFound,
    /// The CLI exited non-zero; the message carries its stderr.
    CliFailed,
    CliOutput,
    Autostart,
}

/// What a failed command sends the frontend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct WireError {
    pub kind: ErrorKind,
    pub message: String,
}

/// An error with a stable [`ErrorKind`]; its `Display` text is the message.
pub trait Kinded: std::fmt::Display {
    fn kind(&self) -> ErrorKind;
}

impl<E: Kinded> From<&E> for WireError {
    fn from(err: &E) -> Self {
        Self {
            kind: err.kind(),
            message: err.to_string(),
        }
    }
}

/// Implements `Serialize` for [`Kinded`] errors by way of [`WireError`].
macro_rules! serialize_as_wire_error {
    ($($ty:ty),+ $(,)?) => {$(
        impl ::serde::Serialize for $ty {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                ::serde::Serialize::serialize(&$crate::error::WireError::from(self), serializer)
            }
        }
    )+};
}
pub(crate) use serialize_as_wire_error;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("bad input")]
    struct Bad;

    impl Kinded for Bad {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Invalid
        }
    }

    serialize_as_wire_error!(Bad);

    #[test]
    fn errors_serialise_as_kind_and_message() {
        let json = serde_json::to_value(Bad).expect("json");
        assert_eq!(
            json,
            serde_json::json!({ "kind": "invalid", "message": "bad input" })
        );
        let json = serde_json::to_value(ErrorKind::DaemonNotRunning).expect("json");
        assert_eq!(json, "daemon_not_running");
    }
}
