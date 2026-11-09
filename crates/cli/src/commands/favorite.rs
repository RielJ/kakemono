//! `favorite`: copy the current wallpaper to the favorites dir (Go `cmd/favorite.go`).

use std::io::Write;

use kakemono_protocol::types::Request;

use crate::cli::Context;

pub async fn run(ctx: &Context, out: &mut dyn Write) -> anyhow::Result<()> {
    ctx.call(Request::Favorite).await?;
    writeln!(out, "✓ Wallpaper saved to favorites.")?;
    Ok(())
}
