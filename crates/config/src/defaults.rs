//! Defaults copied from Go `config.Defaults()` (plan §I), plus the new keys.

use std::path::{Path, PathBuf};

use crate::schema::{
    ApodConfig, BingConfig, Config, DaemonConfig, DesktopConfig, FavoritesConfig, HooksConfig,
    LocalConfig, PickerConfig, PickerPlacement, PickerPosition, RedditConfig, ThemeConfig,
    UiConfig, UnsplashConfig, WallhavenConfig,
};

/// The current user's home directory, or an empty path if it cannot be found (Go ignored the error
/// the same way).
pub fn home_dir() -> PathBuf {
    etcetera::home_dir().unwrap_or_default()
}

/// Defaults for the current user's home directory.
pub fn defaults() -> Config {
    defaults_for_home(&home_dir())
}

/// Defaults with paths rooted at `home`. Pure; tests use it with a fixed home.
///
/// Deviation from Go: `daemon.socket_path` is `None` (platform default, plan §I) instead of
/// `~/.config/wallpaperd/daemon.sock`. On macOS the platform default is that same path.
pub fn defaults_for_home(home: &Path) -> Config {
    let wallpapers = home.join("Pictures").join("Wallpapers");
    Config {
        daemon: DaemonConfig {
            rotation_enabled: true,
            interval: "30m".to_owned(),
            download_dir: wallpapers.join("cache"),
            cache_size: 50,
            trash_dir: wallpapers.join("trash"),
            socket_path: None,
            rotate_on_theme_change: true,
            reapply_on_space_change: false,
        },
        wallhaven: WallhavenConfig {
            enabled: false,
            api_key: String::new(),
            purity: "100".to_owned(),
            sorting: "random".to_owned(),
            categories: "111".to_owned(),
        },
        unsplash: UnsplashConfig {
            enabled: false,
            access_key: String::new(),
        },
        reddit: RedditConfig {
            enabled: false,
            subreddits: strings(&["EarthPorn", "wallpapers"]),
        },
        bing: BingConfig {
            enabled: true,
            market: "en-US".to_owned(),
        },
        apod: ApodConfig {
            enabled: false,
            api_key: "DEMO_KEY".to_owned(),
        },
        local: LocalConfig {
            enabled: false,
            dirs: Vec::new(),
            recursive: true,
        },
        light: light_theme(),
        dark: dark_theme(),
        favorites: FavoritesConfig {
            dir: wallpapers.join("favorites"),
        },
        picker: PickerConfig {
            position: PickerPosition::Bottom,
            force_x11: false,
            close_on_blur: true,
            placement: PickerPlacement::Edge,
            custom: None,
        },
        desktop: DesktopConfig::default(),
        hooks: HooksConfig::default(),
        ui: UiConfig::default(),
    }
}

/// Go's `[light]` defaults.
pub fn light_theme() -> ThemeConfig {
    ThemeConfig {
        sources: strings(&["bing"]),
        queries: strings(&["landscape", "nature"]),
    }
}

/// Go's `[dark]` defaults.
pub fn dark_theme() -> ThemeConfig {
    ThemeConfig {
        sources: strings(&["bing"]),
        queries: strings(&["space", "night", "dark"]),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

// Per-section `Default` impls delegate to `defaults()`, so a missing section and a missing key
// inside a present section both take Go's values.
macro_rules! section_default {
    ($ty:ty, $field:ident) => {
        impl Default for $ty {
            fn default() -> Self {
                defaults().$field
            }
        }
    };
}

section_default!(DaemonConfig, daemon);
section_default!(WallhavenConfig, wallhaven);
section_default!(UnsplashConfig, unsplash);
section_default!(RedditConfig, reddit);
section_default!(BingConfig, bing);
section_default!(ApodConfig, apod);
section_default!(LocalConfig, local);
section_default!(FavoritesConfig, favorites);
section_default!(PickerConfig, picker);
