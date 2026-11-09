//! TODO(task 28): Linux backend discovery (plan §E). Compiles on every OS: `resolve` is pure, and
//! only `env::capture` touches the system (gate it with `cfg(target_os = "linux")`).

pub mod env;
pub mod resolve;
