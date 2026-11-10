//! Screen size for the size filter on Linux and Windows. macOS reads `NSScreen` instead
//! (`macos::signals`).
//!
//! - **Linux:** the DRM connectors in sysfs (`/sys/class/drm/card*-*/`). A connector whose
//!   `status` is `connected` lists its modes in `modes`, preferred (native) mode first. This
//!   needs no display connection and works the same under X11 and Wayland. Drivers without
//!   kernel mode setting expose no connectors, and the size stays unknown.
//! - **Windows:** `EnumDisplayDevicesW` plus `EnumDisplaySettingsW(ENUM_CURRENT_SETTINGS)` for
//!   every display attached to the desktop. The mode is in physical pixels whatever the DPI
//!   scale.
//!
//! Both pick the largest display by area, like macOS, and cache the answer for [`TTL`] so
//! [`crate::SystemSignals::screen_size`] stays cheap.

#[cfg(any(target_os = "linux", test))]
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a reading is reused. Plugging in a screen shows up within this time.
pub(crate) const TTL: Duration = Duration::from_secs(60);

/// The largest size by area, or `None` for no sizes. Zero-sized entries are ignored.
pub(crate) fn largest(sizes: impl IntoIterator<Item = (u32, u32)>) -> Option<(u32, u32)> {
    sizes
        .into_iter()
        .filter(|&(w, h)| w > 0 && h > 0)
        .max_by_key(|&(w, h)| u64::from(w) * u64::from(h))
}

#[cfg(any(target_os = "linux", test))]
/// Parses one DRM mode line: `2560x1440`, or `1920x1080i` for an interlaced mode.
pub(crate) fn parse_drm_mode(line: &str) -> Option<(u32, u32)> {
    let (w, h) = line.trim().split_once('x')?;
    let h = h.trim_end_matches(|c: char| !c.is_ascii_digit());
    Some((w.parse().ok()?, h.parse().ok()?))
}

#[cfg(any(target_os = "linux", test))]
/// The preferred mode of every connected DRM connector under `root` (`/sys/class/drm`).
pub(crate) fn drm_sizes(root: &Path) -> Vec<(u32, u32)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        // Connectors are `card0-HDMI-A-1`; `card0` itself and `renderD128` have no modes.
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card") && n.contains('-'))
        })
        .filter(|p| {
            std::fs::read_to_string(p.join("status")).is_ok_and(|s| s.trim() == "connected")
        })
        .filter_map(|p| {
            let modes = std::fs::read_to_string(p.join("modes")).ok()?;
            modes.lines().next().and_then(parse_drm_mode)
        })
        .collect()
}

/// A size reading and when it was taken.
type Reading = (Instant, Option<(u32, u32)>);

/// A reading reused for [`TTL`]. `None` readings are cached too, so a machine without DRM
/// connectors doesn't rescan on every wallpaper change.
#[derive(Debug, Default)]
pub(crate) struct ScreenCache {
    last: Mutex<Option<Reading>>,
}

impl ScreenCache {
    /// The cached size, or a fresh one from `read` when the cache is empty or older than `ttl`.
    pub(crate) fn get_with(
        &self,
        now: Instant,
        ttl: Duration,
        read: impl FnOnce() -> Option<(u32, u32)>,
    ) -> Option<(u32, u32)> {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, size)) = *last
            && now.saturating_duration_since(at) < ttl
        {
            return size;
        }
        let size = read();
        *last = Some((now, size));
        size
    }

    /// Drops the cached reading, e.g. when the OS reports a display change.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn clear(&self) {
        *self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn largest_picks_by_area_and_skips_empty_sizes() {
        assert_eq!(largest([]), None);
        assert_eq!(largest([(0, 0)]), None);
        assert_eq!(
            largest([(1920, 1080), (2560, 1440), (3440, 1440), (1080, 1920)]),
            Some((3440, 1440))
        );
    }

    #[test]
    fn drm_mode_lines() {
        assert_eq!(parse_drm_mode("2560x1440\n"), Some((2560, 1440)));
        assert_eq!(parse_drm_mode("1920x1080i"), Some((1920, 1080)));
        assert_eq!(parse_drm_mode(""), None);
        assert_eq!(parse_drm_mode("garbage"), None);
        assert_eq!(parse_drm_mode("x1080"), None);
    }

    fn connector(root: &Path, name: &str, status: &str, modes: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("status"), status).unwrap();
        fs::write(dir.join("modes"), modes).unwrap();
    }

    #[test]
    fn drm_reads_the_preferred_mode_of_connected_connectors_only() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        connector(root, "card0-eDP-1", "connected\n", "2880x1800\n1920x1200\n");
        connector(
            root,
            "card0-HDMI-A-1",
            "connected\n",
            "3840x2160\n2560x1440\n",
        );
        connector(root, "card0-DP-1", "disconnected\n", "5120x2880\n");
        connector(root, "card1-DP-2", "connected\n", "");
        fs::create_dir_all(root.join("card0")).unwrap();
        fs::create_dir_all(root.join("renderD128")).unwrap();

        let mut sizes = drm_sizes(root);
        sizes.sort_unstable();
        assert_eq!(sizes, vec![(2880, 1800), (3840, 2160)]);
        assert_eq!(largest(sizes), Some((3840, 2160)));
    }

    #[test]
    fn drm_without_sysfs_is_empty() {
        assert!(drm_sizes(Path::new("/nonexistent/kakemono-drm")).is_empty());
    }

    #[test]
    fn cache_reuses_a_reading_until_it_expires_or_is_cleared() {
        let cache = ScreenCache::default();
        let t0 = Instant::now();
        let ttl = Duration::from_secs(60);
        assert_eq!(cache.get_with(t0, ttl, || Some((1, 1))), Some((1, 1)));
        // Within the TTL the reader is not called.
        assert_eq!(
            cache.get_with(t0 + Duration::from_secs(59), ttl, || unreachable!()),
            Some((1, 1))
        );
        // Past it, a fresh reading replaces the old one.
        assert_eq!(cache.get_with(t0 + ttl, ttl, || Some((2, 2))), Some((2, 2)));
        cache.clear();
        assert_eq!(cache.get_with(t0 + ttl, ttl, || None), None);
        // An unknown size is cached as well.
        assert_eq!(cache.get_with(t0 + ttl, ttl, || unreachable!()), None);
    }
}
