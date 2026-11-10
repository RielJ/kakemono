//! Browsing the online sources from the picker (`docs/plans/picker-browse-impl.md`):
//!
//! - [`preview`]: the preview cache that downloads each remote item's small preview to disk,
//!   so the UI can show it through `thumb://` without the webview ever fetching a remote URL.
//! - [`session`]: browse sessions and [`Engine::browse`](crate::Engine::browse).

pub mod preview;
pub mod session;

pub use preview::{PreviewCache, PreviewError, PreviewJob, PreviewLimits};
pub use session::{BrowseLimits, RATE_LIMIT_FALLBACK};

pub use crate::engine::{BrowseDeps, BrowseError};
