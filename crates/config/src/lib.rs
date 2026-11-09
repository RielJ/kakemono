//! `config.toml` schema, Go-compatible defaults, loading, comment-preserving edits, platform paths
//! and the Go-duration parser (plan §A, §I).

pub mod defaults;
pub mod duration;
pub mod edit;
pub mod error;
pub mod load;
pub mod paths;
pub mod schema;

pub use defaults::{defaults, defaults_for_home};
pub use duration::{DurationError, parse_go};
pub use error::ConfigError;
pub use load::{
    expand_tilde, load, load_or_default, load_or_default_with_home, load_with_home, parse_str,
};
pub use schema::{
    ApodConfig, Backend, BingConfig, CommandSpec, CommandSpecError, Config, DaemonConfig,
    DesktopConfig, FavoritesConfig, Fit, HooksConfig, LocalConfig, ManagedBy, PickerConfig,
    PickerCustom, PickerPlacement, PickerPosition, RedditConfig, ThemeConfig, ThemeOverride,
    UiConfig, UnsplashConfig, WallhavenConfig,
};
