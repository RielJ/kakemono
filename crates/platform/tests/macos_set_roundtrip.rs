//! Real-desktop round trip for `MacSetter`: snapshot → set a 1×1 PNG on every screen → read it
//! back → restore the snapshot. CHANGES THE REAL WALLPAPER (then puts it back).
//!
//! `harness = false` (AppKit needs the process main thread, which libtest does not give tests),
//! so `cargo test` runs `main` directly. `main` is a no-op unless BOTH are present:
//! - `--ignored` among the arguments, and
//! - `KAKEMONO_REAL_DESKTOP_TEST=1` in the environment.
//!
//! Run by hand, on a Mac, at the desk:
//!
//! ```sh
//! KAKEMONO_REAL_DESKTOP_TEST=1 cargo test -p kakemono-platform --test macos_set_roundtrip -- --ignored
//! ```

/// Env var that must be `1`, in addition to `--ignored`, before the wallpaper is touched.
const OPT_IN_ENV: &str = "KAKEMONO_REAL_DESKTOP_TEST";

/// True only when the caller asked for ignored tests AND opted in to touching the real desktop.
fn real_desktop_opt_in() -> bool {
    let ignored = std::env::args().skip(1).any(|a| a == "--ignored");
    let env_opt_in = std::env::var_os(OPT_IN_ENV).is_some_and(|v| v == "1");
    ignored && env_opt_in
}

fn main() {
    if !real_desktop_opt_in() {
        println!(
            "macos_set_roundtrip: skipped; pass --ignored to run (and set {OPT_IN_ENV}=1; \
             it changes the real wallpaper)"
        );
        return;
    }
    #[cfg(target_os = "macos")]
    {
        if let Err(err) = mac::run() {
            eprintln!("macos_set_roundtrip: FAILED: {err:#}");
            std::process::exit(1);
        }
        println!("macos_set_roundtrip: ok");
    }
    #[cfg(not(target_os = "macos"))]
    println!("macos_set_roundtrip: skipped; macOS only");
}

#[cfg(target_os = "macos")]
mod mac {
    use anyhow::{Context, ensure};
    use kakemono_config::schema::DesktopConfig;
    use kakemono_platform::macos::setter::MacSetter;
    use kakemono_platform::{Fit, WallpaperSetter};

    /// A valid 1×1 red PNG (checked with `sips`).
    const ONE_PIXEL_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde\x00\x00\x00\x0cIDAT\x08\xd7c\xf8\xcf\xc0\x00\x00\x03\x01\x01\x00\x18\xdd\x8d\xb0\x00\x00\x00\x00IEND\xaeB`\x82";

    /// Runs on the process main thread; `host` drives NSApplication there.
    pub fn run() -> anyhow::Result<()> {
        kakemono_platform::host(&DesktopConfig::default(), |_platform| async move {
            let setter = MacSetter::new();
            let before = setter.snapshot().await.context("snapshot")?;
            ensure!(!before.screens.is_empty(), "no screens");

            let dir = tempfile::tempdir()?;
            let image = dir.path().join("kakemono-roundtrip.png");
            std::fs::write(&image, ONE_PIXEL_PNG)?;
            let image = image.canonicalize()?;

            let set_result = setter.set(&image, Fit::Fill).await;
            let readback = setter.current_images().await;
            // Always restore, even when the set or the read-back failed.
            let restored = setter.restore(&before).await;

            set_result.context("set")?;
            let readback = readback.context("read back")?;
            restored.context("restore")?;
            for (i, path) in readback.iter().enumerate() {
                let path = path.as_deref().map(std::fs::canonicalize).transpose()?;
                ensure!(
                    path.as_deref() == Some(image.as_path()),
                    "screen {i}: expected {}, got {path:?}",
                    image.display()
                );
            }
            Ok(())
        })
    }
}
