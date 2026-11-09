//! `trash`: move the current wallpaper to the trash dir and advance (Go `cmd/trash.go`).

use std::io::Write;

use kakemono_protocol::types::{Request, WallpaperInfo};

use crate::cli::Context;
use crate::output;

pub async fn run(ctx: &Context, out: &mut dyn Write) -> anyhow::Result<()> {
    let response = ctx.call(Request::Trash).await?;
    writeln!(out, "🗑  Wallpaper trashed.  Switching to next…")?;
    // The daemon auto-advances after trash; the response carries the new wallpaper. As Go, an
    // undecodable payload is skipped silently.
    if let Ok(Some(info)) = response.data_as::<WallpaperInfo>() {
        output::write_wallpaper_info(out, Some(&info))?;
    }
    Ok(())
}
