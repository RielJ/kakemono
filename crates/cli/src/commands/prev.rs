//! `prev`: step back in the wallpaper history and print it (Go `cmd/prev.go`).

use std::io::Write;

use kakemono_protocol::types::Request;

use crate::cli::Context;
use crate::output;

pub async fn run(ctx: &Context, out: &mut dyn Write) -> anyhow::Result<()> {
    let response = ctx.call(Request::Prev).await?;
    output::write_info_response(out, &response)
}
