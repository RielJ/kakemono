//! Wallpaper providers behind the `Provider` seam, plus the shared HTTP client, retry, download
//! helpers and the config-driven registry (plan §A, §G).

pub mod apod;
pub mod bing;
pub mod download;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
pub mod http;
pub mod local;
pub mod provider;
pub mod reddit;
pub mod registry;
pub mod unsplash;
pub mod wallhaven;

pub use download::{DownloadError, download, is_image_file, sanitize_filename};
pub use provider::{Provider, ProviderError, WallpaperResult};
