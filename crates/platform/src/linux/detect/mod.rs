//! Linux backend discovery. Compiles on every OS: `resolve` is pure and its table tests
//! run on all CI runners. Only `env::capture*` / `env::probe_dbus_names` touch the system, and
//! those are `cfg(target_os = "linux")`.

pub mod env;
pub mod resolve;

pub use env::Env;
pub use resolve::{BackendKind, Origin, Resolution, Session, SessionType, resolve, session_key};
