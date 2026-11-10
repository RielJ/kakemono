//! Integration tests for this crate, built as one test binary (`it`).
//!
//! One binary per crate instead of one per file: each file under `tests/` is otherwise
//! linked separately against the crate and all its dependencies. Run one module with
//! `cargo test --test it <module>::`.

mod browse_preview;
mod browse_session;
mod browse_use;
mod common;
mod engine_core;
mod files;
mod hooks;
mod manual;
mod pick_from_disk;
mod provenance;
mod reload_setter;
mod rotation;
mod state;
