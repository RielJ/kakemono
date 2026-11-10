//! Wallpaper providers behind the `Provider` seam, plus the shared HTTP client, retry, download
//! helpers, per-row online sources and local listing.

pub mod apod;
pub mod bing;
pub mod browse;
mod de;
pub mod download;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
pub mod guard;
pub mod http;
pub mod instance;
pub mod local;
pub mod provider;
pub mod reddit;
pub mod unsplash;
pub mod wallhaven;

pub use browse::{
    BROWSE_PAGE, BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem, sanitize_remote_id,
};
pub use download::{DownloadError, download, is_image_file, sanitize_filename};
pub use guard::{GuardConfig, GuardError, guarded_client};
pub use instance::{OnlineSource, SourceSet, build_online};
pub use provider::{Provider, ProviderError, WallpaperResult};
