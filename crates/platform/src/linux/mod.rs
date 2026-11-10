//! Linux platform (plan §E): detection ([`detect`]), backends ([`backends`]), signals
//! ([`theme`]), the self-healing [`ResilientSetter`] and [`open_platform`].
//!
//! `detect` compiles everywhere (pure resolver). `backends` and [`ResilientSetter`] compile on
//! every Unix so their tests also run on macOS. `theme`, the D-Bus backends and
//! [`open_platform`] are Linux-only.

pub mod detect;

#[cfg(unix)]
pub mod backends;
#[cfg(target_os = "linux")]
mod live;
#[cfg(unix)]
mod resilient;
#[cfg(target_os = "linux")]
pub mod theme;

#[cfg(unix)]
pub use resilient::{Active, Backends, REDETECT_AFTER, ResilientSetter, same_backend};

#[cfg(target_os = "linux")]
pub use live::{LiveBackends, open_platform, open_platform_in, open_setter};
