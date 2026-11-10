//! Windows wallpaper setter: a dedicated "desktop thread" owns a single-threaded COM
//! apartment and the `IDesktopWallpaper` object. Requests arrive over an mpsc channel and each
//! gets its result back on a oneshot (the same actor shape as the macOS main-queue setter).
//!
//! `IDesktopWallpaper::SetPosition(fit)` then `SetWallpaper(NULL, path)` sets every monitor. If
//! COM or the interface is unavailable, or a call fails, the fallback writes the legacy
//! `HKCU\Control Panel\Desktop` style values and calls
//! `SystemParametersInfoW(SPI_SETDESKWALLPAPER, …, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)`.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use ::windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use ::windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
use ::windows::Win32::UI::Shell::{
    DESKTOP_WALLPAPER_POSITION, DesktopWallpaper, IDesktopWallpaper,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
};
use ::windows::core::PCWSTR;
use tokio::sync::oneshot;

use super::logic::{DESKTOP_SUBKEY, absolute_image, dwpos, legacy_style, wide};
use crate::api::{BackendInfo, Fit, SetError, WallpaperSetter};

/// One `set` call, handled on the desktop thread.
struct Request {
    image: PathBuf,
    fit: Fit,
    reply: oneshot::Sender<Result<(), SetError>>,
}

/// Sets the wallpaper through the desktop thread. Dropping it ends the thread (the channel closes),
/// which releases the interface and uninitialises COM on that thread.
#[derive(Debug)]
pub struct WinSetter {
    tx: mpsc::Sender<Request>,
    detail: String,
}

impl WinSetter {
    /// Start the desktop thread and wait for it to initialise COM and create `IDesktopWallpaper`.
    ///
    /// Blocks for that initialisation (milliseconds); call it from `spawn_blocking` in async code.
    /// Fails only if the thread can't be spawned. COM failures select the fallback instead.
    pub fn spawn() -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Request>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<String>(1);
        std::thread::Builder::new()
            .name("kakemono-desktop".to_owned())
            .spawn(move || desktop_thread(rx, ready_tx))?;
        let detail = ready_rx
            .recv()
            .map_err(|_| std::io::Error::other("desktop thread exited during startup"))?;
        Ok(Self { tx, detail })
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for WinSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Request {
                image: image.to_path_buf(),
                fit,
                reply,
            })
            .map_err(|_| SetError::os("Windows desktop thread is not running"))?;
        rx.await
            .map_err(|_| SetError::os("Windows desktop thread dropped the request"))?
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "windows".to_owned(),
            detail: self.detail.clone(),
        }
    }
}

/// A COM apartment joined by this thread; uninitialised on drop only if init succeeded
/// (`S_OK` or `S_FALSE`; `RPC_E_CHANGED_MODE` must not be balanced).
///
/// `!Send` (raw-pointer marker), so the `CoUninitialize` in `Drop` always runs on the thread that
/// called `CoInitializeEx`.
pub(crate) struct ComApartment(std::marker::PhantomData<*const ()>);

impl ComApartment {
    /// Join a single-threaded apartment on the current thread.
    pub(crate) fn init_sta() -> ::windows::core::Result<Self> {
        // SAFETY: the reserved pointer must be None, which it is. Initialising COM has no memory
        // preconditions; balancing it is the guard's job (see Drop).
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        hr.ok().map(|()| Self(std::marker::PhantomData))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx in `init_sta` on the same thread. All COM
        // interfaces created in the apartment are dropped before this guard (callers declare the
        // guard first so it drops last).
        unsafe { CoUninitialize() };
    }
}

/// Create the shell's `IDesktopWallpaper` in the current apartment.
pub(crate) fn create_desktop_wallpaper() -> ::windows::core::Result<IDesktopWallpaper> {
    // SAFETY: COM is initialised on this thread (callers hold a `ComApartment`); `DesktopWallpaper`
    // is the CLSID for `IDesktopWallpaper`, no aggregation.
    unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }
}

/// The desktop thread body: COM init, interface creation, then the request loop.
fn desktop_thread(rx: mpsc::Receiver<Request>, ready: mpsc::SyncSender<String>) {
    // Declared first so it drops last, after `wallpaper` is released.
    let com = ComApartment::init_sta();
    let wallpaper = match &com {
        Ok(_) => create_desktop_wallpaper().map_err(|e| format!("CoCreateInstance: {e}")),
        Err(e) => Err(format!("CoInitializeEx: {e}")),
    };
    let detail = match &wallpaper {
        Ok(_) => "IDesktopWallpaper (all monitors)".to_owned(),
        Err(e) => {
            tracing::warn!(error = %e, "IDesktopWallpaper unavailable; using SystemParametersInfoW");
            format!("SystemParametersInfoW (IDesktopWallpaper unavailable: {e})")
        }
    };
    let wallpaper = wallpaper.ok();
    if ready.send(detail).is_err() {
        return;
    }
    // This STA blocks on the channel instead of pumping messages. That's fine: it hosts no COM
    // objects that others call into, and COM's modal loop pumps during our outgoing calls.
    for req in rx {
        let result = apply(wallpaper.as_ref(), &req.image, req.fit);
        let _ = req.reply.send(result);
    }
    drop(wallpaper);
    drop(com);
}

/// Set `image` with `fit` on every monitor: `IDesktopWallpaper` first, else the legacy fallback.
fn apply(wallpaper: Option<&IDesktopWallpaper>, image: &Path, fit: Fit) -> Result<(), SetError> {
    // Both APIs want an absolute path to an existing file.
    let path = absolute_image(image)?;
    std::fs::metadata(path)?;
    let wide_path = wide(path.as_os_str());

    let primary_err = match wallpaper {
        Some(wp) => match set_with_desktop_wallpaper(wp, &wide_path, fit) {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(error = %e, "IDesktopWallpaper failed; trying SystemParametersInfoW");
                Some(e)
            }
        },
        None => None,
    };
    set_with_system_parameters(wide_path, fit).map_err(|fallback| {
        let message = match primary_err {
            Some(primary) => {
                format!("IDesktopWallpaper: {primary}; SystemParametersInfoW: {fallback}")
            }
            None => format!("SystemParametersInfoW: {fallback}"),
        };
        SetError::os_from(message, fallback)
    })
}

fn set_with_desktop_wallpaper(
    wp: &IDesktopWallpaper,
    wide_path: &[u16],
    fit: Fit,
) -> ::windows::core::Result<()> {
    // SAFETY: `wp` lives in this thread's apartment (created on the desktop thread, never moved).
    unsafe { wp.SetPosition(DESKTOP_WALLPAPER_POSITION(dwpos(fit)))? };
    // SAFETY: a null monitor id means "all monitors"; `wide_path` is NUL-terminated UTF-16 that
    // outlives the call.
    unsafe { wp.SetWallpaper(PCWSTR::null(), PCWSTR(wide_path.as_ptr())) }
}

fn set_with_system_parameters(mut wide_path: Vec<u16>, fit: Fit) -> ::windows::core::Result<()> {
    let (style, tile) = legacy_style(fit);
    // Best effort: the image still changes if the style can't be written.
    for (name, value) in [("WallpaperStyle", style), ("TileWallpaper", tile)] {
        if let Err(e) = set_reg_sz(DESKTOP_SUBKEY, name, value) {
            tracing::warn!(error = %e, name, "could not write the wallpaper style");
        }
    }
    // SAFETY: `wide_path` is a NUL-terminated UTF-16 buffer that outlives the call; for
    // SPI_SETDESKWALLPAPER the pointer is read only (the API takes `*mut` for its other actions).
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide_path.as_mut_ptr().cast::<c_void>()),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    }
}

/// Write an `HKCU\<subkey>` REG_SZ value.
fn set_reg_sz(subkey: &str, name: &str, value: &str) -> ::windows::core::Result<()> {
    let subkey = wide(subkey);
    let name = wide(name);
    let data = wide(value);
    let bytes = u32::try_from(std::mem::size_of_val(data.as_slice())).map_err(|_| {
        ::windows::core::Error::from_hresult(::windows::Win32::Foundation::E_INVALIDARG)
    })?;
    // SAFETY: all three buffers are NUL-terminated UTF-16 that outlive the call; `bytes` is the
    // exact size of `data` including its terminator, as REG_SZ requires.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr().cast::<c_void>()),
            bytes,
        )
    };
    status.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::windows::Win32::System::Com::CoTaskMemFree;

    /// A 2×2 24-bit BMP (rows padded to 4 bytes), written by hand so the test needs no encoder.
    fn tiny_bmp(rgb: [u8; 3]) -> Vec<u8> {
        let row = [rgb[2], rgb[1], rgb[0], rgb[2], rgb[1], rgb[0], 0, 0]; // BGR ×2 + padding
        let pixels: Vec<u8> = row.iter().chain(row.iter()).copied().collect();
        let size = u32::try_from(54 + pixels.len()).unwrap();
        let mut b = Vec::new();
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&size.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&54u32.to_le_bytes());
        b.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
        b.extend_from_slice(&2i32.to_le_bytes());
        b.extend_from_slice(&2i32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&[0u8; 24]); // BI_RGB, sizes, resolution, palette
        b.extend_from_slice(&pixels);
        b
    }

    /// Read monitor 0's wallpaper path with `GetMonitorDevicePathAt(0)` + `GetWallpaper`.
    fn monitor0_wallpaper(wp: &IDesktopWallpaper) -> String {
        // SAFETY: `wp` belongs to this thread's apartment. Both returned PWSTRs are CoTaskMem
        // allocations owned by us; each is copied into a String and then freed exactly once.
        unsafe {
            let monitor = wp.GetMonitorDevicePathAt(0).unwrap();
            let path = wp.GetWallpaper(PCWSTR(monitor.0)).unwrap();
            let out = path.to_string().unwrap();
            CoTaskMemFree(Some(path.0.cast_const().cast()));
            CoTaskMemFree(Some(monitor.0.cast_const().cast()));
            out
        }
    }

    #[tokio::test]
    async fn unreadable_image_is_an_io_error() {
        let setter = WinSetter::spawn().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = setter
            .set(&dir.path().join("missing.bmp"), Fit::Fill)
            .await
            .unwrap_err();
        assert!(matches!(err, SetError::Io(_)), "{err:?}");
        assert_eq!(setter.info().name, "windows");
    }

    /// Changes the real desktop, so it is ignored. Run it by hand on Windows 10/11 (ideally with two
    /// monitors, and check both change):
    /// `cargo test -p kakemono-platform -- --ignored windows_set_roundtrip --nocapture`.
    /// It restores monitor 0's previous wallpaper afterwards.
    #[tokio::test]
    #[ignore = "changes the real Windows desktop wallpaper"]
    async fn windows_set_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("kakemono-roundtrip.bmp");
        std::fs::write(&image, tiny_bmp([0x20, 0x80, 0xC0])).unwrap();

        let previous = tokio::task::spawn_blocking(|| {
            let _com = ComApartment::init_sta().unwrap();
            let wp = create_desktop_wallpaper().unwrap();
            monitor0_wallpaper(&wp)
        })
        .await
        .unwrap();

        let setter = WinSetter::spawn().unwrap();
        assert!(setter.info().detail.starts_with("IDesktopWallpaper"));
        setter.set(&image, Fit::Fit).await.unwrap();

        let now = tokio::task::spawn_blocking(|| {
            let _com = ComApartment::init_sta().unwrap();
            let wp = create_desktop_wallpaper().unwrap();
            monitor0_wallpaper(&wp)
        })
        .await
        .unwrap();
        assert!(
            now.eq_ignore_ascii_case(&image.display().to_string()),
            "monitor 0 shows {now:?}, expected {image:?}"
        );

        if !previous.is_empty() {
            setter.set(Path::new(&previous), Fit::Fill).await.unwrap();
        }
    }
}
