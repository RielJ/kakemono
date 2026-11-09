//! `next`: ask the daemon for the next wallpaper and print it (Go `cmd/next.go`).

use std::io::Write;

use kakemono_protocol::types::Request;

use crate::cli::Context;
use crate::output;

pub async fn run(ctx: &Context, out: &mut dyn Write) -> anyhow::Result<()> {
    let response = ctx.call(Request::Next).await?;
    output::write_info_response(out, &response)
}
