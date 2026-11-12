//! Integration tests for this crate, built as one test binary (`it`).
//!
//! One binary per crate instead of one per file: each file under `tests/` is otherwise
//! linked separately against the crate and all its dependencies. Run one module with
//! `cargo test --test it <module>::`.

mod compat;
mod daemon_filters;
mod duration_cases;
mod edit;
mod edit_sources;
mod json_wire;
mod legacy_guard;
mod migrate;
mod migrate_file;
mod paths;
mod picker_blur;
mod sources_schema;
mod ui_welcome;

use std::path::Path;

/// A local source's `location` after `~/<rest>` is expanded: `home` joined with `rest` by
/// `Path::join`, so the joining separator is `\` on Windows (`rest` keeps its own `/`).
fn tilde_expanded(home: &Path, rest: &str) -> String {
    home.join(rest).to_string_lossy().into_owned()
}
