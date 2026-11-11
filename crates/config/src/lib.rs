//! `config.toml` schema, Go-compatible defaults, loading, comment-preserving edits, platform paths
//! and the Go-duration parser.

pub mod defaults;
pub mod duration;
pub mod edit;
pub mod error;
pub mod load;
pub mod migrate;
pub mod paths;
pub mod schema;
pub mod sources;

pub use defaults::{defaults, defaults_for_home};
pub use duration::{DurationError, parse_go};
pub use error::ConfigError;
pub use load::{
    FileMigration, Loaded, expand_tilde, load, load_or_default, load_or_default_report,
    load_or_default_report_with_home, load_or_default_with_home, load_report_with_home,
    load_with_home, migrate_config, migrate_file, parse_report, parse_str,
};
pub use migrate::{MigrationReport, migrate_document, needs_migration};
pub use schema::{
    ApodConfig, Appearance, Backend, BingConfig, BrightnessFilter, CommandSpec, CommandSpecError,
    Config, DaemonConfig, DesktopConfig, FavoritesConfig, FiltersConfig, Fit, HooksConfig,
    ManagedBy, PickerConfig, PickerCustom, PickerOrientation, PickerPlacement, PickerPosition,
    SourceEntry, SourceType, ThemeOverride, UiConfig, UnsplashConfig, WallhavenConfig,
};
pub use sources::{
    MAX_ID_LEN, MAX_NAME_LEN, deterministic_id, normalize_name, normalize_sources, valid_id,
};
