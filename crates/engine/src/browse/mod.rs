//! Browsing the online sources from the picker (`docs/plans/picker-browse-impl.md`): the
//! preview cache that downloads each remote item's small preview to disk, so the UI can show it
//! through `thumb://` without the webview ever fetching a remote URL.

pub mod preview;

pub use preview::{PreviewCache, PreviewError, PreviewJob, PreviewLimits};
