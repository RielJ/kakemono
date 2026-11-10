//! [`EngineError`]: why an op failed.

use std::io;
use std::path::PathBuf;

use kakemono_platform::api::SetError;
use kakemono_sources::{DownloadError, GuardError, ProviderError};
use tokio::task::JoinError;

use crate::downloader::DownloadFailure;
use crate::files::FilesError;
use crate::manual::{CHECK_TIMEOUT, ManualError};

/// Why an op failed. Crosses IPC as its `Display` string; messages keep Go's `manager: ` prefix
/// and wording where Go had the same failure.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// `favorite`/`trash` with no current wallpaper.
    #[error("manager: no current wallpaper")]
    NoCurrent,
    /// `trash` with an `expected` path that isn't the current wallpaper (Task 45b).
    #[error("manager: current wallpaper changed: expected {expected}, current {current}")]
    NotCurrent { expected: String, current: String },
    /// `prev` at the oldest history entry (or with empty history).
    #[error("manager: already at the beginning of history")]
    AtStart,
    /// `set_file` on a path that can't be stat'ed. The payload is Go's `os.Stat` error text,
    /// `stat <path>: <reason>`.
    #[error("manager: file not found: {0}")]
    FileNotFound(String),
    /// The platform setter failed.
    #[error("manager: set wallpaper: {0}")]
    Set(#[source] SetError),
    /// `reload` with a `[desktop]` that cannot be used (Task 37b). The old config and setter
    /// stay.
    #[error("[desktop]: {0}")]
    Desktop(#[source] SetError),
    /// A change found no image on disk and couldn't download one: `[daemon] internet` is off
    /// or no online source is enabled. Go had no such failure; the text is meant for the UI.
    #[error("No images yet. Add a source or turn on Internet access.")]
    NoImages,
    /// The pool was empty and every on-demand download failed. One message per online source,
    /// in config order, without the `manager: ` prefix (e.g. `provider bing fetch: …`,
    /// `download image: …`). URLs in download errors are redacted. With one source the text
    /// equals Go's.
    #[error("manager: {}", join_failures(.0))]
    Fetch(Vec<DownloadFailure>),
    /// Creating one of the engine's directories failed.
    #[error("manager: create dir {}: {source}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Favorite/trash file operations (Task 20). The payload has no `manager: ` prefix.
    #[error("manager: {0}")]
    Files(#[from] FilesError),
    /// `fetch_url` failed (Task 16): a bad input, `[daemon] internet` off for a URL, a download
    /// or copy failure, not a JPEG/PNG/WebP, or over the size cap. Shown as is, no prefix.
    #[error(transparent)]
    Manual(#[from] ManualError),
}

/// Why `check_source`, `download_one` or `download_now` failed. Crosses IPC as its `Display`
/// string, shown as is (no `manager: ` prefix).
#[derive(Debug, thiserror::Error)]
pub enum SourceOpError {
    /// An online op with `[daemon] internet` off.
    #[error("Internet access is off")]
    InternetOff,
    /// `download_one` for an id that isn't an enabled online `[[sources]]` row.
    #[error("no enabled online source {0:?}")]
    UnknownSource(String),
    /// `download_one`'s download failed.
    #[error(transparent)]
    Download(#[from] DownloadFailure),
    /// `download_now` without an id: every download failed. One text per source, in config
    /// order, joined with `"; "`.
    #[error("{}", join_failures(.0))]
    Downloads(Vec<DownloadFailure>),
    /// Listing the empty download folders panicked or was cancelled.
    #[error("download failed: {0}")]
    List(#[source] JoinError),
    /// `check_source` found zero images.
    #[error("no images found")]
    NoImagesFound,
    /// `check_source`'s provider query took longer than [`CHECK_TIMEOUT`].
    #[error("check timed out after {}s", CHECK_TIMEOUT.as_secs())]
    CheckTimeout,
    /// `check_source`'s location doesn't parse for its type (e.g. `unsplash: ...`), or the
    /// provider query failed. The provider's text, as is.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The sources factory built no source from a row that parsed.
    #[error("{0}: can't build the source")]
    CantBuild(kakemono_config::schema::SourceType),
    /// Counting a local source's images panicked or was cancelled.
    #[error("check failed: {0}")]
    Check(#[source] JoinError),
    /// `check_source` for a type that is neither local nor online.
    #[error("unsupported source type")]
    Unsupported,
}

/// Why `browse` or `browse_use` failed. Crosses IPC as its `Display` string, shown as is.
#[derive(Debug, thiserror::Error)]
pub enum BrowseError {
    /// `[daemon] internet` is off.
    #[error("Internet access is off")]
    InternetOff,
    /// The engine was built without [`BrowseDeps`](crate::engine::BrowseDeps).
    #[error("browsing is not available in this daemon")]
    NotAvailable,
    /// The source filter or item names an id that isn't an enabled online `[[sources]]` row.
    #[error("no enabled online source {0:?}")]
    UnknownSource(String),
    /// A page past the next one of the session. `produced` pages exist (`0..produced`).
    #[error("browse page {page} is out of range: the session has {produced} page(s)")]
    PageOutOfRange { page: u32, produced: u32 },
    /// `browse_use` for an item no longer in the item index (evicted, or never browsed).
    #[error("That preview expired — scroll or reopen to refresh.")]
    ItemExpired,
    /// The item's source id can't name a download folder.
    #[error("invalid source id {0:?}")]
    InvalidId(String),
    /// The provider couldn't give the full image URL (e.g. a Wallhaven page scrape failed).
    #[error(transparent)]
    Resolve(ProviderError),
    /// The provider gave something that isn't a URL.
    #[error("{0}: invalid image URL")]
    BadUrl(&'static str),
    /// The SSRF guard refused the image URL or a redirect.
    #[error(transparent)]
    Guard(#[from] GuardError),
    /// The full-image download failed.
    #[error("download image: {0}")]
    Download(#[source] DownloadError),
    /// The download took longer than `BrowseLimits::use_timeout`.
    #[error("download image: no answer within {}s", .0.as_secs())]
    DownloadTimeout(std::time::Duration),
    /// Setting the wallpaper or copying to favorites failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

/// The download failures' texts joined with `"; "`.
fn join_failures(failures: &[DownloadFailure]) -> String {
    failures
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use kakemono_config::schema::SourceType;
    use kakemono_sources::DownloadError;

    use super::*;
    use crate::manual::FetchFailure;

    fn nothing_new(name: &'static str) -> DownloadFailure {
        DownloadFailure::NothingNew {
            name,
            query: String::new(),
        }
    }

    /// A cancelled task's `JoinError` (`task <id> was cancelled`).
    async fn cancelled() -> JoinError {
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        task.await.unwrap_err()
    }

    /// The daemon sends these texts as wire errors; they were `String`s before the typed
    /// errors and must read exactly as they did.
    #[tokio::test]
    async fn source_op_error_texts_are_pinned() {
        let cases = [
            (SourceOpError::InternetOff, "Internet access is off"),
            (
                SourceOpError::UnknownSource("bing".into()),
                "no enabled online source \"bing\"",
            ),
            (
                SourceOpError::Download(nothing_new("bing")),
                "provider bing has nothing new for query \"\"",
            ),
            (
                SourceOpError::Downloads(vec![nothing_new("bing"), nothing_new("apod")]),
                "provider bing has nothing new for query \"\"; \
                 provider apod has nothing new for query \"\"",
            ),
            (SourceOpError::NoImagesFound, "no images found"),
            (SourceOpError::CheckTimeout, "check timed out after 15s"),
            (
                SourceOpError::Provider(ProviderError::NotConfigured(
                    "unsplash requires access_key".into(),
                )),
                "unsplash requires access_key",
            ),
            (
                SourceOpError::CantBuild(SourceType::Wallhaven),
                "wallhaven: can't build the source",
            ),
            (SourceOpError::Unsupported, "unsupported source type"),
        ];
        for (err, text) in cases {
            assert_eq!(err.to_string(), text);
        }
        assert_eq!(crate::manual::INTERNET_OFF, "Internet access is off");
        assert_eq!(crate::manual::NO_IMAGES_FOUND, "no images found");

        let check = SourceOpError::Check(cancelled().await).to_string();
        assert!(check.starts_with("check failed: task ") && check.ends_with(" was cancelled"));
        let list = SourceOpError::List(cancelled().await).to_string();
        assert!(list.starts_with("download failed: task "), "{list}");
    }

    #[test]
    fn fetch_error_text_is_pinned() {
        let one = EngineError::Fetch(vec![nothing_new("bing")]);
        assert_eq!(
            one.to_string(),
            "manager: provider bing has nothing new for query \"\""
        );
        let url = "https://x/a.jpg?key=s3cret";
        let two = EngineError::Fetch(vec![
            nothing_new("bing"),
            DownloadFailure::Download {
                url: url.into(),
                error: DownloadError::Status {
                    url: url.into(),
                    status: 404,
                },
            },
        ]);
        assert_eq!(
            two.to_string(),
            "manager: provider bing has nothing new for query \"\"; \
             download image: download: GET https://x/a.jpg?key=REDACTED returned status 404"
        );
    }

    #[tokio::test]
    async fn manual_error_texts_are_pinned() {
        let io = || io::Error::from(io::ErrorKind::NotFound);
        let cases = [
            (
                ManualError::BadInput("ftp://x".into()),
                "not an http(s) URL or absolute file path: \"ftp://x\"".to_owned(),
            ),
            (
                ManualError::InternetOff,
                "Internet access is off".to_owned(),
            ),
            (
                ManualError::Download {
                    url: "https://x/a.png".into(),
                    reason: FetchFailure::Status(404),
                },
                "download https://x/a.png: HTTP 404".to_owned(),
            ),
            (
                ManualError::NotAnImage,
                "not a JPEG, PNG or WebP image".to_owned(),
            ),
            (
                ManualError::TooLarge,
                "image is larger than 64 MB".to_owned(),
            ),
            (
                ManualError::Stat {
                    path: "/nope.png".into(),
                    source: io(),
                },
                "stat /nope.png: no such file or directory".to_owned(),
            ),
            (
                ManualError::NotAFile("/tmp".into()),
                "/tmp is not a file".to_owned(),
            ),
            (
                ManualError::Read {
                    path: "/a.png".into(),
                    source: io(),
                },
                format!("read /a.png: {}", io()),
            ),
            (
                ManualError::Save {
                    path: Path::new("/m/a.png").into(),
                    source: io(),
                },
                format!("save /m/a.png: {}", io()),
            ),
            (
                ManualError::NoFreeName("/m".into()),
                "save /m: no free file name".to_owned(),
            ),
        ];
        for (err, text) in cases {
            // `EngineError::Manual` adds no prefix.
            assert_eq!(EngineError::from(err).to_string(), text);
        }
        let task = ManualError::Task(cancelled().await).to_string();
        assert!(task.starts_with("save failed: task "), "{task}");
    }
}
