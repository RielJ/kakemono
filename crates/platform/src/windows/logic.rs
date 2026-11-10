//! Pure Windows logic: no `windows` crate, so these tests also run on macOS and Linux
//! (`lib.rs` includes this file under `#[cfg(all(test, not(target_os = "windows")))]`).
//!
//! The Win32 numbers are plain integer consts here. `windows/mod.rs` has a Windows-only test that
//! asserts each one equals its `windows` crate constant, so they can't drift.
#![cfg_attr(
    not(target_os = "windows"),
    allow(dead_code, reason = "off Windows only the unit tests use this module")
)]

use std::path::Path;

use crate::api::{Fit, SetError, Signal, Theme};

/// `DESKTOP_WALLPAPER_POSITION` values (`i32`, the type of the `windows` crate newtype's field).
pub const DWPOS_CENTER: i32 = 0;
pub const DWPOS_TILE: i32 = 1;
pub const DWPOS_STRETCH: i32 = 2;
pub const DWPOS_FIT: i32 = 3;
pub const DWPOS_FILL: i32 = 4;

/// Window messages and `WM_POWERBROADCAST` event codes.
pub const WM_DISPLAYCHANGE: u32 = 0x007E;
pub const WM_POWERBROADCAST: u32 = 0x0218;
pub const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;

/// The registry key and value holding the system (taskbar/shell) light/dark mode.
///
/// dark-light 3.0 reads `AppsUseLightTheme` (the app mode) instead, so Task 27 reads this one itself.
pub const PERSONALIZE_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
pub const SYSTEM_USES_LIGHT_THEME: &str = "SystemUsesLightTheme";

/// The legacy wallpaper style key used by the `SystemParametersInfoW` fallback.
pub const DESKTOP_SUBKEY: &str = r"Control Panel\Desktop";

/// `Fit` as an `IDesktopWallpaper::SetPosition` value.
pub fn dwpos(fit: Fit) -> i32 {
    match fit {
        Fit::Fill => DWPOS_FILL,
        Fit::Fit => DWPOS_FIT,
        Fit::Center => DWPOS_CENTER,
        Fit::Stretch => DWPOS_STRETCH,
        Fit::Tile => DWPOS_TILE,
    }
}

/// `Fit` as the legacy `HKCU\Control Panel\Desktop` `(WallpaperStyle, TileWallpaper)` REG_SZ pair,
/// written before `SystemParametersInfoW(SPI_SETDESKWALLPAPER)` in the fallback path.
pub fn legacy_style(fit: Fit) -> (&'static str, &'static str) {
    match fit {
        Fit::Fill => ("10", "0"),
        Fit::Fit => ("6", "0"),
        Fit::Center => ("0", "0"),
        Fit::Stretch => ("2", "0"),
        Fit::Tile => ("0", "1"),
    }
}

/// The `SystemUsesLightTheme` DWORD as a theme: `0` is dark, anything else light.
pub fn theme_from_dword(value: u32) -> Theme {
    if value == 0 {
        Theme::Dark
    } else {
        Theme::Light
    }
}

/// A message-window message as a platform signal, if it is one we report.
///
/// `WM_DISPLAYCHANGE` → `DisplaysChanged`. `WM_POWERBROADCAST` with `PBT_APMRESUMEAUTOMATIC` →
/// `Resumed` (sent on every resume; `PBT_APMRESUMESUSPEND` follows only after user input, so it is
/// ignored to avoid a second `Resumed`).
pub fn signal_for_message(msg: u32, wparam: usize) -> Option<Signal> {
    match msg {
        WM_DISPLAYCHANGE => Some(Signal::DisplaysChanged),
        WM_POWERBROADCAST if wparam == PBT_APMRESUMEAUTOMATIC as usize => Some(Signal::Resumed),
        _ => None,
    }
}

/// `image` if it is absolute. A relative path is rejected, as on macOS and Linux, instead of
/// being resolved against the daemon's working directory.
///
/// # Errors
///
/// `SetError::Os` when `image` is relative.
pub fn absolute_image(image: &Path) -> Result<&Path, SetError> {
    if image.is_absolute() {
        Ok(image)
    } else {
        Err(SetError::os(format!(
            "wallpaper path is not absolute: {}",
            image.display()
        )))
    }
}

/// UTF-16, NUL-terminated, for `PCWSTR` arguments.
pub fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        s.as_ref()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_FITS: [Fit; 5] = [Fit::Fill, Fit::Fit, Fit::Center, Fit::Stretch, Fit::Tile];

    #[test]
    fn fit_maps_to_distinct_dwpos() {
        assert_eq!(dwpos(Fit::Fill), 4);
        assert_eq!(dwpos(Fit::Fit), 3);
        assert_eq!(dwpos(Fit::Center), 0);
        assert_eq!(dwpos(Fit::Stretch), 2);
        assert_eq!(dwpos(Fit::Tile), 1);
        let mut seen: Vec<i32> = ALL_FITS.iter().map(|&f| dwpos(f)).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), ALL_FITS.len());
    }

    #[test]
    fn fit_maps_to_legacy_style() {
        assert_eq!(legacy_style(Fit::Fill), ("10", "0"));
        assert_eq!(legacy_style(Fit::Fit), ("6", "0"));
        assert_eq!(legacy_style(Fit::Center), ("0", "0"));
        assert_eq!(legacy_style(Fit::Stretch), ("2", "0"));
        assert_eq!(legacy_style(Fit::Tile), ("0", "1"));
    }

    #[test]
    fn relative_image_paths_are_rejected() {
        let err = absolute_image(Path::new(r"walls\x.png")).unwrap_err();
        assert_eq!(
            err.to_string(),
            r"wallpaper path is not absolute: walls\x.png"
        );
        let absolute = if cfg!(windows) {
            r"C:\walls\x.png"
        } else {
            "/walls/x.png"
        };
        assert_eq!(
            absolute_image(Path::new(absolute)).unwrap(),
            Path::new(absolute)
        );
    }

    #[test]
    fn registry_dword_to_theme() {
        assert_eq!(theme_from_dword(0), Theme::Dark);
        assert_eq!(theme_from_dword(1), Theme::Light);
        assert_eq!(theme_from_dword(u32::MAX), Theme::Light);
    }

    #[test]
    fn messages_to_signals() {
        assert_eq!(
            signal_for_message(WM_DISPLAYCHANGE, 32),
            Some(Signal::DisplaysChanged)
        );
        assert_eq!(
            signal_for_message(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize),
            Some(Signal::Resumed)
        );
        // PBT_APMRESUMESUSPEND (7), PBT_APMSUSPEND (4), PBT_POWERSETTINGCHANGE (0x8013).
        for wparam in [7, 4, 0x8013] {
            assert_eq!(signal_for_message(WM_POWERBROADCAST, wparam), None);
        }
        // WM_QUIT (0x12, same number as PBT_APMRESUMEAUTOMATIC) and WM_SETTINGCHANGE (0x1A).
        assert_eq!(signal_for_message(0x0012, 0x12), None);
        assert_eq!(signal_for_message(0x001A, 0), None);
    }

    #[test]
    fn wide_is_nul_terminated_utf16() {
        assert_eq!(wide("a\u{e9}"), vec![0x61, 0xE9, 0]);
        assert_eq!(wide(""), vec![0]);
    }
}
