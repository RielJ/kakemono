//! `MacSetter`: NSWorkspace `setDesktopImageURL:forScreen:options:error:` for every screen, run on
//! the main queue (plan §F).
//!
//! macOS has no API to set every Space: this changes the current Space of each screen, which is
//! what the Go daemon's `tell every desktop` did.

use std::path::{Path, PathBuf};
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSImageScaling, NSScreen, NSWorkspace, NSWorkspaceDesktopImageAllowClippingKey,
    NSWorkspaceDesktopImageOptionKey, NSWorkspaceDesktopImageScalingKey,
};
use objc2_foundation::{NSDictionary, NSNumber, NSURL};

use super::mainloop::on_main;
use crate::api::{BackendInfo, Fit, SetError, WallpaperSetter};

/// How long a set may wait for the main thread before it fails.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Sets the wallpaper on every screen through NSWorkspace. Needs the macOS [`host`] running.
///
/// [`host`]: crate::host::host
#[derive(Clone, Copy, Debug, Default)]
pub struct MacSetter;

/// The desktop image of each screen, as captured by [`MacSetter::snapshot`]. Screens are matched
/// by their position in `NSScreen.screens` when restoring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopSnapshot {
    /// One entry per screen.
    pub screens: Vec<ScreenImage>,
}

/// One screen's desktop image and the scaling options it was set with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenImage {
    /// `None` when AppKit reports no file URL for the screen.
    pub path: Option<PathBuf>,
    /// Raw `NSImageScaling` value, when the options had one.
    pub scaling: Option<usize>,
    /// `NSWorkspaceDesktopImageAllowClippingKey`, when the options had it.
    pub allow_clipping: Option<bool>,
}

impl MacSetter {
    pub fn new() -> Self {
        Self
    }

    /// The image file of each screen's current Space. Read-only.
    pub async fn current_images(&self) -> Result<Vec<Option<PathBuf>>, SetError> {
        let snapshot = self.snapshot().await?;
        Ok(snapshot.screens.into_iter().map(|s| s.path).collect())
    }

    /// Capture each screen's desktop image and options. Read-only.
    pub async fn snapshot(&self) -> Result<DesktopSnapshot, SetError> {
        on_main(MAIN_THREAD_TIMEOUT, snapshot_on_main)
            .await
            .map_err(|e| SetError::os_from(e.to_string(), e))
    }

    /// Put back the images from `snapshot`, screen by screen. Screens without a recorded path,
    /// and screens added since the snapshot, are left alone.
    pub async fn restore(&self, snapshot: &DesktopSnapshot) -> Result<(), SetError> {
        let snapshot = snapshot.clone();
        on_main(MAIN_THREAD_TIMEOUT, move |mtm| {
            restore_on_main(mtm, &snapshot)
        })
        .await
        .map_err(|e| SetError::os_from(e.to_string(), e))?
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for MacSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        let path = checked_image_path(image).await?;
        on_main(MAIN_THREAD_TIMEOUT, move |mtm| set_on_main(mtm, &path, fit))
            .await
            .map_err(|e| SetError::os_from(e.to_string(), e))?
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "macos".to_owned(),
            detail: "NSWorkspace setDesktopImageURL (current Space of every screen)".to_owned(),
        }
    }
}

/// AppKit accepts a URL to a missing file without an error, so check it here: the path must be
/// absolute and name an existing file.
async fn checked_image_path(image: &Path) -> Result<PathBuf, SetError> {
    if !image.is_absolute() {
        return Err(SetError::os(format!(
            "wallpaper path is not absolute: {}",
            image.display()
        )));
    }
    let meta = tokio::fs::metadata(image).await?;
    if !meta.is_file() {
        return Err(SetError::os(format!("not a file: {}", image.display())));
    }
    Ok(image.to_path_buf())
}

/// `NSImageScaling` and "allow clipping" for each [`Fit`]. AppKit has no tiling option, so
/// `Tile` shows the image once at its own size, like `Center`.
pub(crate) fn fit_options(fit: Fit) -> (NSImageScaling, bool) {
    match fit {
        // Cover the screen, cropping the overflow.
        Fit::Fill => (NSImageScaling::ScaleProportionallyUpOrDown, true),
        // Whole image visible, letterboxed.
        Fit::Fit => (NSImageScaling::ScaleProportionallyUpOrDown, false),
        Fit::Stretch => (NSImageScaling::ScaleAxesIndependently, false),
        Fit::Center | Fit::Tile => (NSImageScaling::ScaleNone, true),
    }
}

fn desktop_options(
    scaling: NSImageScaling,
    allow_clipping: bool,
) -> Retained<NSDictionary<NSWorkspaceDesktopImageOptionKey, AnyObject>> {
    let scaling = NSNumber::new_usize(scaling.0);
    let clipping = NSNumber::new_bool(allow_clipping);
    // SAFETY: AppKit's exported option-key constants; valid for the life of the process.
    let keys = unsafe {
        [
            NSWorkspaceDesktopImageScalingKey,
            NSWorkspaceDesktopImageAllowClippingKey,
        ]
    };
    let values: [&AnyObject; 2] = [scaling.as_ref(), clipping.as_ref()];
    NSDictionary::from_slices(&keys, &values)
}

fn set_url_on_screen(
    workspace: &NSWorkspace,
    url: &NSURL,
    screen: &NSScreen,
    options: &NSDictionary<NSWorkspaceDesktopImageOptionKey, AnyObject>,
) -> Result<(), String> {
    // SAFETY: every value in `options` has the type AppKit documents for its key (NSNumber for
    // the scaling and clipping keys); see `desktop_options`. We are on the main thread (callers
    // hold a `MainThreadMarker`).
    unsafe { workspace.setDesktopImageURL_forScreen_options_error(url, screen, options) }
        .map_err(|err| err.localizedDescription().to_string())
}

fn set_on_main(mtm: MainThreadMarker, path: &Path, fit: Fit) -> Result<(), SetError> {
    let url = NSURL::from_file_path(path)
        .ok_or_else(|| SetError::os(format!("not a valid file path: {}", path.display())))?;
    let (scaling, clipping) = fit_options(fit);
    let options = desktop_options(scaling, clipping);
    let workspace = NSWorkspace::sharedWorkspace();
    let screens = NSScreen::screens(mtm);
    if screens.is_empty() {
        return Err(SetError::os("no screens are attached"));
    }
    let failures: Vec<String> = screens
        .iter()
        .enumerate()
        .filter_map(|(i, screen)| {
            set_url_on_screen(&workspace, &url, &screen, &options)
                .err()
                .map(|e| format!("screen {i}: {e}"))
        })
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(SetError::os(format!(
            "setDesktopImageURL failed: {}",
            failures.join("; ")
        )))
    }
}

fn snapshot_on_main(mtm: MainThreadMarker) -> DesktopSnapshot {
    let workspace = NSWorkspace::sharedWorkspace();
    let screens = NSScreen::screens(mtm)
        .iter()
        .map(|screen| {
            let path = workspace
                .desktopImageURLForScreen(&screen)
                .and_then(|url| url.to_file_path());
            let options = workspace.desktopImageOptionsForScreen(&screen);
            // SAFETY: AppKit's exported option-key constants; valid for the life of the process.
            let (scaling_key, clipping_key) = unsafe {
                (
                    NSWorkspaceDesktopImageScalingKey,
                    NSWorkspaceDesktopImageAllowClippingKey,
                )
            };
            let number = |key: &NSWorkspaceDesktopImageOptionKey| {
                options
                    .as_ref()
                    .and_then(|o| o.objectForKey(key))
                    .and_then(|v| v.downcast::<NSNumber>().ok())
            };
            ScreenImage {
                path,
                scaling: number(scaling_key).map(|n| n.unsignedIntegerValue()),
                allow_clipping: number(clipping_key).map(|n| n.boolValue()),
            }
        })
        .collect();
    DesktopSnapshot { screens }
}

fn restore_on_main(mtm: MainThreadMarker, snapshot: &DesktopSnapshot) -> Result<(), SetError> {
    let workspace = NSWorkspace::sharedWorkspace();
    let (default_scaling, default_clipping) = fit_options(Fit::Fill);
    let mut failures = Vec::new();
    for (i, (screen, saved)) in NSScreen::screens(mtm)
        .iter()
        .zip(&snapshot.screens)
        .enumerate()
    {
        let Some(url) = saved.path.as_deref().and_then(NSURL::from_file_path) else {
            continue;
        };
        let options = desktop_options(
            saved.scaling.map_or(default_scaling, NSImageScaling),
            saved.allow_clipping.unwrap_or(default_clipping),
        );
        if let Err(e) = set_url_on_screen(&workspace, &url, &screen, &options) {
            failures.push(format!("screen {i}: {e}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(SetError::os(format!(
            "restore failed: {}",
            failures.join("; ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // None of these reach AppKit's setter: they fail before the main-thread hop.

    #[test]
    fn fit_maps_to_scaling_and_clipping() {
        use NSImageScaling as S;
        assert_eq!(
            fit_options(Fit::Fill),
            (S::ScaleProportionallyUpOrDown, true)
        );
        assert_eq!(
            fit_options(Fit::Fit),
            (S::ScaleProportionallyUpOrDown, false)
        );
        assert_eq!(
            fit_options(Fit::Stretch),
            (S::ScaleAxesIndependently, false)
        );
        assert_eq!(fit_options(Fit::Center), (S::ScaleNone, true));
        assert_eq!(fit_options(Fit::Tile), (S::ScaleNone, true));
    }

    #[test]
    fn options_dictionary_holds_both_keys() {
        let dict = desktop_options(NSImageScaling::ScaleAxesIndependently, false);
        assert_eq!(dict.count(), 2);
        // SAFETY: AppKit's exported constant.
        let key = unsafe { NSWorkspaceDesktopImageScalingKey };
        let scaling = dict
            .objectForKey(key)
            .unwrap()
            .downcast::<NSNumber>()
            .unwrap();
        assert_eq!(scaling.unsignedIntegerValue(), 1);
    }

    #[test]
    fn info_names_the_backend() {
        assert_eq!(MacSetter::new().info().name, "macos");
    }

    #[tokio::test]
    async fn relative_path_is_rejected() {
        let err = MacSetter::new()
            .set(Path::new("wall.jpg"), Fit::Fill)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not absolute"), "{err}");
    }

    #[tokio::test]
    async fn missing_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = MacSetter::new()
            .set(&dir.path().join("missing.png"), Fit::Fill)
            .await
            .unwrap_err();
        assert!(matches!(err, SetError::Io(_)), "{err}");
    }

    #[tokio::test]
    async fn directory_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = MacSetter::new()
            .set(dir.path(), Fit::Fill)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a file"), "{err}");
    }
}
