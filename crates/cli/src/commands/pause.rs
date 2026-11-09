//! `pause`: stop auto-rotation (Go `cmd/pause.go`).

use std::io::Write;

use kakemono_protocol::types::Request;

use crate::cli::Context;

pub async fn run(ctx: &Context, out: &mut dyn Write) -> anyhow::Result<()> {
    ctx.call(Request::Pause).await?;
    writeln!(out, "⏸  Auto-rotation paused.")?;
    Ok(())
}
