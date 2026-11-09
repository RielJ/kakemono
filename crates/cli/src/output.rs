//! Human-readable output, ported from Go `cmd/helpers.go` (`printWallpaperInfo`) and
//! `cmd/status.go` (`printStatus`, `status --json`). Byte-for-byte the same text as Go.

use std::io::{self, Write};

use anyhow::Context as _;
use kakemono_protocol::codec;
use kakemono_protocol::types::{Response, StatusData, WallpaperInfo};
use serde_json::value::RawValue;

/// Go `printWallpaperInfo`: one aligned line per non-empty field, or `(wallpaper info empty)`.
pub fn write_wallpaper_info(out: &mut dyn Write, info: Option<&WallpaperInfo>) -> io::Result<()> {
    let Some(w) = info else {
        return writeln!(out, "(no wallpaper information)");
    };
    let mut lines = Vec::new();
    if !w.title.is_empty() {
        lines.push(format!("Title:   {}", w.title));
    }
    if !w.author.is_empty() {
        lines.push(format!("Author:  {}", w.author));
    }
    if !w.source.is_empty() {
        lines.push(format!("Source:  {}", w.source));
    }
    if !w.source_url.is_empty() {
        lines.push(format!("Page:    {}", w.source_url));
    }
    if !w.path.is_empty() {
        lines.push(format!("File:    {}", w.path));
    }
    if w.width > 0 && w.height > 0 {
        lines.push(format!("Size:    {}x{}", w.width, w.height));
    }
    if !w.set_at.is_empty() {
        lines.push(format!("Set at:  {}", w.set_at));
    }
    if lines.is_empty() {
        return writeln!(out, "(wallpaper info empty)");
    }
    writeln!(out, "{}", lines.join("\n"))
}

/// Decodes a `WallpaperInfo` response (`next`, `prev`, `set`) and prints it. A missing or
/// malformed payload is `parsing response: ...`, as Go.
pub fn write_info_response(out: &mut dyn Write, response: &Response) -> anyhow::Result<()> {
    let info: WallpaperInfo = response
        .data_as()
        .context("parsing response")?
        .context("parsing response: no data")?;
    write_wallpaper_info(out, Some(&info))?;
    Ok(())
}

/// Go `printStatus`.
pub fn write_status(out: &mut dyn Write, s: &StatusData) -> io::Result<()> {
    let paused = if s.paused { "paused ⏸" } else { "running" };
    writeln!(out, "Daemon:   {paused}")?;
    writeln!(out, "Theme:    {}", s.theme)?;
    writeln!(out, "Interval: {}", s.interval)?;
    writeln!(
        out,
        "History:  {} entries (index {})",
        s.history_count, s.history_index
    )?;
    match &s.current {
        None => writeln!(out, "Current:  (none)"),
        Some(current) => {
            writeln!(out)?;
            writeln!(out, "Current wallpaper:")?;
            write_wallpaper_info(out, Some(current))
        }
    }
}

/// `status --json`, as Go: the response `data` decoded generically (so keys come out sorted) and
/// re-encoded like `json.MarshalIndent(v, "", "  ")`, plus a newline. If `data` is not valid
/// JSON (or absent), it is printed as is, as Go's fallback did.
pub fn write_status_json(out: &mut dyn Write, data: Option<&RawValue>) -> io::Result<()> {
    let raw = data.map_or("", RawValue::get);
    let pretty = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| codec::to_string_go_pretty(&v).ok());
    match pretty {
        Some(text) => writeln!(out, "{text}"),
        None => writeln!(out, "{raw}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn full() -> WallpaperInfo {
        WallpaperInfo {
            path: "/p/a.jpg".into(),
            source: "bing".into(),
            source_url: "https://example.test/a".into(),
            image_url: "https://example.test/a.jpg".into(),
            title: "A".into(),
            author: "B".into(),
            width: 3840,
            height: 2160,
            set_at: "2024-03-15T10:30:00+08:00".into(),
        }
    }

    #[test]
    fn wallpaper_info_all_fields() {
        let got = text(|o| write_wallpaper_info(o, Some(&full())));
        assert_eq!(
            got,
            "Title:   A\nAuthor:  B\nSource:  bing\nPage:    https://example.test/a\n\
             File:    /p/a.jpg\nSize:    3840x2160\nSet at:  2024-03-15T10:30:00+08:00\n"
        );
    }

    #[test]
    fn wallpaper_info_skips_empty_and_half_sizes() {
        let info = WallpaperInfo {
            path: "/p/a.jpg".into(),
            width: 10,
            ..WallpaperInfo::default()
        };
        assert_eq!(
            text(|o| write_wallpaper_info(o, Some(&info))),
            "File:    /p/a.jpg\n"
        );
        assert_eq!(
            text(|o| write_wallpaper_info(o, Some(&WallpaperInfo::default()))),
            "(wallpaper info empty)\n"
        );
        assert_eq!(
            text(|o| write_wallpaper_info(o, None)),
            "(no wallpaper information)\n"
        );
    }

    #[test]
    fn status_text() {
        let mut s = StatusData {
            current: None,
            paused: false,
            interval: "30m".into(),
            theme: "light".into(),
            history_count: 0,
            history_index: -1,
        };
        assert_eq!(
            text(|o| write_status(o, &s)),
            "Daemon:   running\nTheme:    light\nInterval: 30m\n\
             History:  0 entries (index -1)\nCurrent:  (none)\n"
        );
        s.paused = true;
        s.current = Some(full());
        s.history_count = 3;
        s.history_index = 2;
        assert_eq!(
            text(|o| write_status(o, &s)),
            "Daemon:   paused ⏸\nTheme:    light\nInterval: 30m\nHistory:  3 entries (index 2)\n\n\
             Current wallpaper:\nTitle:   A\nAuthor:  B\nSource:  bing\n\
             Page:    https://example.test/a\nFile:    /p/a.jpg\nSize:    3840x2160\n\
             Set at:  2024-03-15T10:30:00+08:00\n"
        );
    }

    #[test]
    fn status_json_matches_go_fixture_bytes() {
        let fixture = include_str!("../../protocol/tests/fixtures/status_json.txt");
        // The daemon sends compact JSON in struct order; Go re-sorts keys and indents.
        let value: StatusData = serde_json::from_str(fixture).unwrap();
        let compact = codec::to_string_go(&value).unwrap();
        let raw = RawValue::from_string(compact).unwrap();
        let got = text(|o| write_status_json(o, Some(&raw)));
        assert_eq!(got.trim_end(), fixture.trim_end());
        assert!(got.ends_with("}\n"));
    }

    #[test]
    fn status_json_falls_back_to_raw() {
        assert_eq!(text(|o| write_status_json(o, None)), "\n");
    }
}
