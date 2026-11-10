//! Windows system signals and the host thread's message loop.
//!
//! - **Window.** A hidden top-level window on the host (main) thread maps `WM_DISPLAYCHANGE` →
//!   `DisplaysChanged` and `WM_POWERBROADCAST`/`PBT_APMRESUMEAUTOMATIC` → `Resumed`. It is a
//!   never-shown `WS_POPUP` + `WS_EX_TOOLWINDOW` window, not a message-only (`HWND_MESSAGE`) one:
//!   message-only windows do not receive broadcast messages, and both messages are broadcasts.
//! - **Theme.** `HKCU\…\Themes\Personalize\SystemUsesLightTheme`, watched by a blocking
//!   `RegNotifyChangeKeyValue` loop on its own thread. dark-light 3.0 was not used: on Windows it
//!   reads `AppsUseLightTheme` (the app mode, not the shell/taskbar mode). A pinned
//!   `[desktop] theme` wins and disables the watcher.
//!
//! Signals go to one process-wide [`SignalHub`] ([`hub`]), the same type macOS and Linux use.
//! It stays process-wide on Windows: the window proc is a plain function with no state, the
//! theme watcher thread runs once per process, and `open_platform` may run before or without the
//! window.

use std::ffi::c_void;
use std::sync::{Once, OnceLock};

use ::windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use ::windows::Win32::Graphics::Gdi::{
    DEVMODEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS,
    EnumDisplayDevicesW, EnumDisplaySettingsW,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, RRF_RT_REG_DWORD, RegCloseKey,
    RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW,
};
use ::windows::Win32::System::Threading::GetCurrentThreadId;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RegisterClassExW, TranslateMessage,
    WINDOW_EX_STYLE, WM_QUIT, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use ::windows::core::{PCWSTR, w};
use kakemono_config::schema::DesktopConfig;
use tokio::sync::broadcast;

use super::logic::{
    PERSONALIZE_SUBKEY, SYSTEM_USES_LIGHT_THEME, signal_for_message, theme_from_dword, wide,
};
use crate::api::{Signal, SystemSignals, Theme};
use crate::hub::SignalHub;

/// The process-wide hub fed by the window proc and the theme watcher. Its theme starts as the
/// registry's (light if that cannot be read) and is kept current by the watcher.
pub(crate) fn hub() -> &'static SignalHub {
    static HUB: OnceLock<SignalHub> = OnceLock::new();
    HUB.get_or_init(|| SignalHub::new(initial_system_theme()))
}

// ---------------------------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------------------------

/// `SystemUsesLightTheme`, or light (with a warning) when it cannot be read.
fn initial_system_theme() -> Theme {
    read_system_theme().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "cannot read SystemUsesLightTheme; theme falls back to light");
        Theme::Light
    })
}

/// Read `SystemUsesLightTheme`. Errors if the value is missing (e.g. Windows before 10 1903).
pub fn read_system_theme() -> ::windows::core::Result<Theme> {
    let subkey = wide(PERSONALIZE_SUBKEY);
    let name = wide(SYSTEM_USES_LIGHT_THEME);
    let mut value: u32 = 0;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call. RRF_RT_REG_DWORD
    // restricts the read to a 4-byte DWORD, and `value`/`size` point at a live u32 of that size.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut value).cast::<c_void>()),
            Some(&mut size),
        )
    };
    status.ok()?;
    Ok(theme_from_dword(value))
}

/// An open registry key, closed on drop.
struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: `self.0` was opened by RegOpenKeyExW and is closed exactly once, here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// Start the theme watcher thread once per process. The hub reads the theme synchronously
/// first, so `theme()` is right as soon as this returns.
fn start_theme_watcher() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let _ = hub();
        if let Err(e) = std::thread::Builder::new()
            .name("kakemono-theme".to_owned())
            .spawn(watch_theme)
        {
            tracing::warn!(error = %e, "cannot start the theme watcher; ThemeChanged is disabled");
        }
    });
}

/// Block on `RegNotifyChangeKeyValue` for the Personalize key, re-read on every change and emit
/// `ThemeChanged` only when the theme flips. Runs for the life of the process.
fn watch_theme() {
    let subkey = wide(PERSONALIZE_SUBKEY);
    let mut raw = HKEY::default();
    // SAFETY: `subkey` is NUL-terminated UTF-16 that outlives the call; `raw` is a live out-param.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_NOTIFY,
            &mut raw,
        )
    };
    if let Err(e) = status.ok() {
        tracing::warn!(error = %e, "cannot watch the Personalize key; ThemeChanged is disabled");
        return;
    }
    let key = RegKey(raw);
    loop {
        // SAFETY: `key.0` is an open key with KEY_NOTIFY access, owned by this thread for the whole
        // loop. Synchronous mode (no event, fAsynchronous = false) blocks until a value changes.
        let status = unsafe {
            RegNotifyChangeKeyValue(key.0, false, REG_NOTIFY_CHANGE_LAST_SET, None, false)
        };
        if let Err(e) = status.ok() {
            tracing::warn!(error = %e, "theme watcher stopped");
            return;
        }
        // Any value under Personalize fires this (AppsUseLightTheme, accent colour, ...); dedupe.
        match read_system_theme() {
            Ok(theme) => {
                hub().update_theme(theme);
            }
            Err(e) => tracing::warn!(error = %e, "cannot re-read SystemUsesLightTheme"),
        }
    }
}

/// Windows `SystemSignals`: theme from the registry (or the pinned override), events from [`hub`].
#[derive(Debug)]
pub struct WinSignals {
    pinned: Option<Theme>,
}

impl WinSignals {
    /// Use the `[desktop] theme` override if it pins a theme; otherwise start the registry watcher.
    pub fn open(cfg: &DesktopConfig) -> Self {
        let pinned = Theme::from_override(cfg.theme);
        if pinned.is_none() {
            start_theme_watcher();
        }
        Self { pinned }
    }
}

impl SystemSignals for WinSignals {
    fn theme(&self) -> Theme {
        self.pinned.unwrap_or_else(|| hub().theme())
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        hub().subscribe()
    }

    /// The largest display's current mode in physical pixels ([`crate::screen`]).
    fn screen_size(&self) -> Option<(u32, u32)> {
        screen_cache().get_with(std::time::Instant::now(), crate::screen::TTL, || {
            crate::screen::largest(display_modes())
        })
    }
}

/// The process-wide screen-size cache. The window proc clears it on `WM_DISPLAYCHANGE`, before
/// the `DisplaysChanged` signal goes out, so a reader after the signal sees the new size.
fn screen_cache() -> &'static crate::screen::ScreenCache {
    static CACHE: OnceLock<crate::screen::ScreenCache> = OnceLock::new();
    CACHE.get_or_init(crate::screen::ScreenCache::default)
}

/// The current mode of every display attached to the desktop.
fn display_modes() -> Vec<(u32, u32)> {
    let mut sizes = Vec::new();
    for index in 0.. {
        let mut device = DISPLAY_DEVICEW {
            cb: u32::try_from(std::mem::size_of::<DISPLAY_DEVICEW>()).unwrap_or(u32::MAX),
            ..Default::default()
        };
        // SAFETY: `device` is a live DISPLAY_DEVICEW whose `cb` holds its size, as the API
        // requires; a null device name enumerates the display adapters' outputs.
        let found = unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut device, 0) };
        if !found.as_bool() {
            break;
        }
        if !device
            .StateFlags
            .contains(DISPLAY_DEVICE_ATTACHED_TO_DESKTOP)
        {
            continue;
        }
        let mut mode = DEVMODEW {
            dmSize: u16::try_from(std::mem::size_of::<DEVMODEW>()).unwrap_or(u16::MAX),
            ..Default::default()
        };
        // SAFETY: `DeviceName` is the NUL-terminated name EnumDisplayDevicesW just filled in,
        // and `mode` is a live DEVMODEW whose `dmSize` holds its size.
        let ok = unsafe {
            EnumDisplaySettingsW(
                PCWSTR(device.DeviceName.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
            )
        };
        if ok.as_bool() {
            sizes.push((mode.dmPelsWidth, mode.dmPelsHeight));
        }
    }
    sizes
}

// ---------------------------------------------------------------------------------------------
// Window and message loop (host thread)
// ---------------------------------------------------------------------------------------------

/// The hidden signal window. `!Send` (HWND holds a raw pointer): it lives and dies on the thread
/// that created it and runs the message loop.
#[derive(Debug)]
pub struct SignalWindow {
    hwnd: HWND,
}

impl SignalWindow {
    /// Register the window class (once per process) and create the hidden window on this thread.
    pub fn create() -> ::windows::core::Result<Self> {
        let class_name = w!("kakemono-signals");
        // SAFETY: a null module name returns this executable's handle; no pointer arguments.
        let instance = unsafe { GetModuleHandleW(PCWSTR::null())? };
        let class = WNDCLASSEXW {
            cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or(0),
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        // SAFETY: `class` is fully initialised with a correct cbSize; `class_name` is a static
        // NUL-terminated literal, so the class name outlives the registration.
        let atom = unsafe { RegisterClassExW(&class) };
        if atom == 0 {
            // SAFETY: reads this thread's last-error value right after the failing call.
            let err = unsafe { GetLastError() };
            // A second `host()` in the same process (tests) reuses the class.
            if err != ERROR_CLASS_ALREADY_EXISTS {
                return Err(err.into());
            }
        }
        // SAFETY: the class is registered with `wndproc`; the strings are static literals. No
        // parent, menu or create param. The window is never shown (no WS_VISIBLE, no ShowWindow).
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
                class_name,
                w!("kakemono"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )?
        };
        Ok(Self { hwnd })
    }

    #[cfg(test)]
    pub(crate) fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for SignalWindow {
    fn drop(&mut self) {
        // SAFETY: `hwnd` was created on this thread (the type is !Send) and is destroyed once.
        if let Err(e) = unsafe { DestroyWindow(self.hwnd) } {
            tracing::debug!(error = %e, "DestroyWindow failed");
        }
    }
}

/// The window proc: map the message to a signal (if any), then default handling.
///
/// # Safety
///
/// Called only by the system's message dispatch for the signal window's class, with the
/// `hwnd`, `msg`, `wparam` and `lparam` of a message sent to that window.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if let Some(signal) = signal_for_message(msg, wparam.0) {
        tracing::debug!(?signal, "window message");
        if signal == Signal::DisplaysChanged {
            screen_cache().clear();
        }
        hub().send(signal);
    }
    // SAFETY: forwards the unmodified arguments of the message Windows delivered to this proc.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Make sure this thread has a message queue, so `PostThreadMessageW` to it can't fail with
/// "invalid thread id" before the loop starts. Returns the thread id to post to.
pub fn ensure_message_queue() -> u32 {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a live out-param; PM_NOREMOVE leaves any message in the queue. The call's
    // only purpose is its side effect of creating the queue.
    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
    // SAFETY: no arguments, no preconditions.
    unsafe { GetCurrentThreadId() }
}

/// Pump this thread's messages until `WM_QUIT` (or a `GetMessageW` error).
pub fn run_message_loop() {
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a live out-param; a None hwnd reads every message for this thread.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match got.0 {
            0 => break, // WM_QUIT
            -1 => {
                // SAFETY: reads this thread's last-error value right after the failing call.
                let err = unsafe { GetLastError() };
                tracing::error!(error = ?err, "GetMessageW failed; leaving the message loop");
                break;
            }
            _ => {
                // SAFETY: `msg` was just filled by GetMessageW on this thread.
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

/// Posts `WM_QUIT` to the host thread when dropped, so its message loop ends whether the daemon
/// returned or panicked.
#[derive(Debug)]
pub struct QuitOnDrop(pub u32);

impl Drop for QuitOnDrop {
    fn drop(&mut self) {
        // SAFETY: plain value arguments; `self.0` is the host thread's id, whose queue exists
        // (`ensure_message_queue`) and outlives this guard (host joins the worker after the loop).
        if let Err(e) = unsafe { PostThreadMessageW(self.0, WM_QUIT, WPARAM(0), LPARAM(0)) } {
            tracing::error!(error = %e, "cannot post WM_QUIT to the host thread");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_DISPLAYCHANGE};
    use kakemono_config::schema::ThemeOverride;

    #[test]
    fn pinned_theme_wins() {
        let cfg = DesktopConfig {
            theme: ThemeOverride::Dark,
            ..DesktopConfig::default()
        };
        assert_eq!(WinSignals::open(&cfg).theme(), Theme::Dark);
    }

    /// Only reads HKCU; on Windows 10 1903+ the value exists.
    #[test]
    fn system_theme_is_readable() {
        let theme = read_system_theme();
        assert!(theme.is_ok(), "{theme:?}");
    }

    /// Delivers a synthetic WM_DISPLAYCHANGE to the hidden window in-process (no desktop changes).
    #[test]
    fn window_routes_display_change_to_hub() {
        let mut rx = hub().subscribe();
        let window = SignalWindow::create().unwrap();
        // SAFETY: `window` was created on this thread; SendMessageW runs `wndproc` synchronously.
        unsafe {
            SendMessageW(
                window.hwnd(),
                WM_DISPLAYCHANGE,
                Some(WPARAM(32)),
                Some(LPARAM(0)),
            )
        };
        let mut got = Vec::new();
        while let Ok(s) = rx.try_recv() {
            got.push(s);
        }
        assert!(got.contains(&Signal::DisplaysChanged), "{got:?}");
    }
}
