//! Integration tests for this crate, built as one test binary (`it`).
//!
//! One binary per crate instead of one per file: each file under `tests/` is otherwise
//! linked separately against the crate and all its dependencies. Run one module with
//! `cargo test --test it <module>::`.

mod exec;
mod linux_gsettings;
mod linux_kde;
mod linux_persistent;
mod linux_presets;
mod linux_resolve;
mod linux_xfce;
mod service;
