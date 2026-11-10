//! Integration tests for this crate, built as one test binary (`it`).
//!
//! One binary per crate instead of one per file: each file under `tests/` is otherwise
//! linked separately against the crate and all its dependencies. Run one module with
//! `cargo test --test it <module>::`.

mod browse;
mod cli;
mod daemon;
mod detect;
mod epipe;
mod reload_desktop;
mod server;
mod support;
