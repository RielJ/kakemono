//! Defaults copied from Go `config.Defaults()`, plus the new keys.

use std::path::{Path, PathBuf};

use crate::schema::{
    ApodConfig, BingConfig, BrightnessFilter, Config, DaemonConfig, DesktopConfig, FavoritesConfig,
    FiltersConfig, HooksConfig, PickerConfig, PickerOrientation, PickerPlacement, PickerPosition,
    SourceEntry, SourceType, UiConfig, UnsplashConfig, WallhavenConfig,
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
/// `daemon.socket_path` is `None`: the endpoint falls back to `$KAKEMONO_SOCKET`, then the
/// platform default.
pub fn defaults_for_home(home: &Path) -> Config {
    let wallpapers = home.join("Pictures").join("Wallpapers");
    Config {
        daemon: DaemonConfig {
            rotation_enabled: true,
            interval: "30m".to_owned(),
            download_dir: wallpapers.join("cache"),
            trash_dir: wallpapers.join("trash"),
            socket_path: None,
            rotate_on_theme_change: true,
            reapply_on_space_change: false,
            change_on_start: false,
            internet: true,
            download_enabled: true,
            download_interval: "10m".to_owned(),
            download_limit_mb_enabled: true,
            download_limit_mb: 500,
            manual_dir: None,
        },
        wallhaven: WallhavenConfig {
            api_key: String::new(),
            purity: "100".to_owned(),
            sorting: "random".to_owned(),
            categories: "111".to_owned(),
        },
        unsplash: UnsplashConfig {
            access_key: String::new(),
        },
        bing: BingConfig {
            market: "en-US".to_owned(),
        },
        apod: ApodConfig {
            api_key: "DEMO_KEY".to_owned(),
        },
        sources: default_sources(&wallpapers.join("favorites")),
        filters: FiltersConfig {
            landscape: false,
            min_size_enabled: false,
            min_size_percent: 80,
            brightness: BrightnessFilter::Off,
            color_enabled: false,
            color: "#3b82f6".to_owned(),
            rating_enabled: false,
            min_rating: 4,
            name_regex_enabled: false,
            name_regex: String::new(),
        },
        favorites: FavoritesConfig {
            dir: wallpapers.join("favorites"),
        },
        picker: PickerConfig {
            position: PickerPosition::Bottom,
            orientation: PickerOrientation::Auto,
            force_x11: false,
            close_on_blur: true,
            blur_nsfw: true,
            placement: PickerPlacement::Edge,
            custom: None,
        },
        desktop: DesktopConfig::default(),
        hooks: HooksConfig::default(),
        ui: UiConfig::default(),
    }
}

/// The rows a fresh install starts with, all enabled: Wallhaven's hot and top lists, and the
/// favorites folder. The URLs set only the sorting, so `[wallhaven]` purity (SFW) and categories
/// still apply.
pub fn default_sources(favorites: &Path) -> Vec<SourceEntry> {
    let row = |id: &str, name: &str, kind, location: &str| SourceEntry {
        id: id.to_owned(),
        name: name.to_owned(),
        enabled: true,
        kind,
        location: location.to_owned(),
    };
    vec![
        row(
            "wh-hot",
            "Wallhaven hot",
            SourceType::Wallhaven,
            "https://wallhaven.cc/hot",
        ),
        row(
            "wh-top",
            "Wallhaven top",
            SourceType::Wallhaven,
            "https://wallhaven.cc/toplist",
        ),
        row(
            "favorites",
            "",
            SourceType::Folder,
            &favorites.to_string_lossy(),
        ),
    ]
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
section_default!(BingConfig, bing);
section_default!(ApodConfig, apod);
section_default!(FiltersConfig, filters);
section_default!(FavoritesConfig, favorites);
section_default!(PickerConfig, picker);
