//! `status [--json]`: daemon and wallpaper state (Go `cmd/status.go`).

use std::io::Write;

use anyhow::Context as _;
use kakemono_protocol::types::{Request, StatusData};

use crate::cli::Context;
use crate::output;

pub async fn run(ctx: &Context, out: &mut dyn Write, json: bool) -> anyhow::Result<()> {
    let response = ctx.call(Request::Status).await?;
    if json {
        output::write_status_json(out, response.data.as_deref())?;
        return Ok(());
    }
    let status: StatusData = response
        .data_as()
        .context("parsing status")?
        .context("parsing status: no data")?;
    output::write_status(out, &status)?;
    Ok(())
}
