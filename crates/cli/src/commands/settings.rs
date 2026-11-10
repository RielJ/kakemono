//! `kakemono settings`: open the settings window in the UI app. The search and
//! launch rules are [`super::pick`]'s.

use std::io::Write;
use std::path::Path;

use super::pick::{self, Detached, Page, Search};
use crate::cli::Context;

/// `kakemono settings`.
pub fn run(ctx: &Context, _out: &mut dyn Write) -> anyhow::Result<()> {
    pick::open(
        ctx,
        Page::Settings,
        &Search::from_environment(),
        &Path::is_file,
        &Detached,
    )
}
