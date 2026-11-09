//! `resume`: restart auto-rotation (Go `cmd/resume.go`).

use std::io::Write;

use kakemono_protocol::types::Request;

use crate::cli::Context;

pub async fn run(ctx: &Context, out: &mut dyn Write) -> anyhow::Result<()> {
    ctx.call(Request::Resume).await?;
    writeln!(out, "▶  Auto-rotation resumed.")?;
    Ok(())
}
