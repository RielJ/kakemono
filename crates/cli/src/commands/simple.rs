//! The commands that send one request and print one reply (Go `cmd/next.go`, `prev.go`,
//! `pause.go`, `resume.go`, `favorite.go`, `trash.go`, `status.go`).

use std::io::Write;

use kakemono_protocol::types::{Request, WallpaperInfo};

use crate::cli::Context;
use crate::output;

/// A one-request command: what it sends and what it prints on `ok`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Simple {
    /// `next`: the next wallpaper.
    Next,
    /// `prev`: one step back in the history.
    Prev,
    /// `pause`: stop auto-rotation.
    Pause,
    /// `resume`: restart auto-rotation.
    Resume,
    /// `favorite`: copy the current wallpaper to the favorites dir.
    Favorite,
    /// `trash`: move the current wallpaper to the trash dir; the daemon advances.
    Trash,
    /// `quit`: stop the daemon.
    Quit,
}

/// What a [`Simple`] command prints after the daemon answered `ok`.
enum Reply {
    /// The wallpaper in the response. A missing or malformed payload is an error.
    Wallpaper,
    /// A fixed line.
    Line(&'static str),
    /// A fixed line, then the wallpaper in the response if it decodes (Go skipped a bad one).
    LineThenWallpaper(&'static str),
}

impl Simple {
    fn request(self) -> Request {
        match self {
            Simple::Next => Request::Next,
            Simple::Prev => Request::Prev,
            Simple::Pause => Request::Pause,
            Simple::Resume => Request::Resume,
            Simple::Favorite => Request::Favorite,
            Simple::Trash => Request::Trash { expected: None },
            Simple::Quit => Request::Quit,
        }
    }

    fn reply(self) -> Reply {
        match self {
            Simple::Next | Simple::Prev => Reply::Wallpaper,
            Simple::Pause => Reply::Line("⏸  Auto-rotation paused."),
            Simple::Resume => Reply::Line("▶  Auto-rotation resumed."),
            Simple::Favorite => Reply::Line("✓ Wallpaper saved to favorites."),
            Simple::Trash => Reply::LineThenWallpaper("🗑  Wallpaper trashed.  Switching to next…"),
            Simple::Quit => Reply::Line("■  Daemon stopping."),
        }
    }
}

/// Sends `command`'s request and prints its reply.
pub async fn run(ctx: &Context, out: &mut dyn Write, command: Simple) -> anyhow::Result<()> {
    let response = ctx.call(command.request()).await?;
    match command.reply() {
        Reply::Wallpaper => output::write_info_response(out, &response)?,
        Reply::Line(line) => writeln!(out, "{line}")?,
        Reply::LineThenWallpaper(line) => {
            writeln!(out, "{line}")?;
            if let Ok(Some(info)) = response.data_as::<WallpaperInfo>() {
                output::write_wallpaper_info(out, Some(&info))?;
            }
        }
    }
    Ok(())
}

/// `status [--json]`: daemon and wallpaper state. `--json` prints the payload as the daemon sent
/// it.
pub async fn status(ctx: &Context, out: &mut dyn Write, json: bool) -> anyhow::Result<()> {
    if json {
        let response = ctx.call(Request::Status).await?;
        output::write_status_json(out, response.data.as_deref())?;
        return Ok(());
    }
    output::write_status(out, &ctx.status().await?)?;
    Ok(())
}
