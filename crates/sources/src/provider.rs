//! The `Provider` seam every wallpaper source implements, the candidate type it returns and the
//! shared error type.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::browse::{BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem};

/// One wallpaper candidate returned by a [`Provider`]. Port of Go `providers.WallpaperResult`.
///
/// Serde names match the Go fixture keys (`source_url`, `image_url`, ...), so a fixture's
/// `expected.results` deserializes straight into this type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WallpaperResult {
    /// Human-readable page where the image was found.
    pub source_url: String,
    /// Direct URL of the image file. For the local provider this is an absolute file path.
    pub image_url: String,
    /// Optional title; empty when the source has none.
    pub title: String,
    /// Optional creator; empty when the source has none.
    pub author: String,
    /// Canonical provider name (`"bing"`, `"wallhaven"`, ...), equal to [`Provider::name`].
    pub source: String,
}

/// Implemented by every wallpaper source.
///
/// Cancellation is by dropping the `fetch` future (Go passed a `context.Context`).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Canonical lower-case identifier, e.g. `"wallhaven"`. Matches the config section name.
    fn name(&self) -> &'static str;

    /// Fetches a batch of candidates. `query` may be empty; providers without search ignore it.
    /// An empty `Ok(vec![])` is a valid answer ("nothing matched").
    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError>;

    /// How [`Provider::browse`] pages. The default, [`BrowseSupport::Unsupported`], matches the
    /// default `browse`.
    fn browse_support(&self) -> BrowseSupport {
        BrowseSupport::Unsupported
    }

    /// Host suffixes that preview and full-size image URLs may use, e.g. `wallhaven.cc` (which
    /// also covers `w.wallhaven.cc`). The SSRF guard rejects any other host. Empty by default,
    /// so a provider that does not browse can fetch nothing through the guard.
    fn media_hosts(&self) -> &'static [&'static str] {
        &[]
    }

    /// Host suffixes this provider's image URLs may use right now. A provider whose hosts
    /// come from config or from the pages it reads overrides this. Default:
    /// [`Provider::media_hosts`].
    fn allowed_hosts(&self) -> Vec<String> {
        self.media_hosts().iter().map(|h| (*h).to_owned()).collect()
    }

    /// The client for downloading and previewing this provider's images, already guarded
    /// for [`Provider::allowed_hosts`]. `None` (the default): the caller's client.
    fn media_client(&self) -> Option<reqwest::Client> {
        None
    }

    /// The Settings check: one fetch whose results are counted. Default:
    /// [`Provider::fetch`]. A provider that caches its fetch overrides this to skip the
    /// cache.
    ///
    /// # Errors
    ///
    /// As [`Provider::fetch`].
    async fn check(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        self.fetch(query).await
    }

    /// One batch of browsable images: the first when `cursor` is `None`, otherwise the batch
    /// that `cursor` (from an earlier [`BrowseBatch::next`]) points at. `query` may be empty.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] by default; providers that browse return their usual
    /// errors, with 429/503 as [`ProviderError::RateLimitedFor`] and no retry
    /// ([`crate::http::send_fail_fast`]).
    async fn browse(
        &self,
        _query: &str,
        _cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        Err(ProviderError::Unsupported(self.name().into()))
    }

    /// The URL of the full-size image of `item`. The default returns an
    /// [`ImageRef::Direct`] URL as is.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] for an [`ImageRef::Page`] unless the provider overrides
    /// this to fetch and parse the page.
    async fn resolve_image(&self, item: &RemoteItem) -> Result<String, ProviderError> {
        match &item.image {
            ImageRef::Direct(url) => Ok(url.clone()),
            ImageRef::Page(_) => Err(ProviderError::Unsupported(self.name().into())),
        }
    }
}

/// Why a provider fetch failed.
///
/// `Display` is the user-facing text. Providers choose `context` so the text matches the Go
/// error strings recorded in `tests/fixtures/*/*.case.json` (e.g. `bing: unexpected status 500`).
/// Errors cross IPC as strings, so the text is the contract; variants exist for logging and for
/// tests that match on the kind.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// The provider cannot run with the current config (missing API key, missing directory).
    /// The message is shown as is, e.g. `unsplash requires access_key`.
    #[error("{0}")]
    NotConfigured(String),

    /// The source refused because of a rate limit. The message is shown as is.
    #[error("{0}")]
    RateLimited(String),

    /// The source answered but has nothing kakemono can use. The message names the cause and
    /// is shown as is.
    #[error("{0}")]
    Unusable(String),

    /// Rate limited, with the server's `Retry-After` hint when it sent one. Used by browse
    /// paths ([`crate::http::send_fail_fast`]); the message is shown as is.
    ///
    /// The engine downloader's back-off matches only [`ProviderError::RateLimited`], so fetch
    /// paths must keep using [`crate::http::send_with_retry`] and never return this variant.
    /// Use [`ProviderError::is_rate_limited`] to accept both.
    #[error("{message}")]
    RateLimitedFor {
        /// User-facing text, e.g. `unsplash api: rate limited (status 429)`.
        message: String,
        /// Seconds the server asked to wait, from `Retry-After`; `None` when absent or
        /// unparseable.
        retry_after_secs: Option<u64>,
    },

    /// The provider cannot do what was asked (browse, or resolve a page link). Holds the
    /// provider name; displays as `{name} cannot browse`.
    #[error("{0} cannot browse")]
    Unsupported(String),

    /// The source answered with a status the provider does not handle.
    /// Displays as `{context}: unexpected status {status}`.
    #[error("{context}: unexpected status {status}")]
    Status {
        /// Prefix such as `"bing"` or `"unsplash api"`.
        context: String,
        /// HTTP status code.
        status: u16,
    },

    /// The request failed before a usable response arrived (connect, timeout, body read).
    #[error("{context}: {source}")]
    Request {
        /// Prefix such as `"bing"` or `"reddit: GET https://..."`.
        context: String,
        /// The transport error.
        #[source]
        source: reqwest::Error,
    },

    /// The response body was larger than `limit` bytes ([`crate::http::MAX_API_BODY_BYTES`]).
    #[error("{context}: response larger than {limit} bytes")]
    TooLarge {
        /// Prefix such as `"reddit"`.
        context: String,
        /// The size cap that was exceeded.
        limit: u64,
    },

    /// The response body was not the JSON the provider expected.
    #[error("{context}: decode: {source}")]
    Decode {
        /// Prefix such as `"apod"`.
        context: String,
        /// The parse error.
        #[source]
        source: serde_json::Error,
    },

    /// A filesystem operation failed (local provider).
    #[error("{context}: {source}")]
    Io {
        /// Prefix such as `"local: read /path"`.
        context: String,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Any other provider-specific failure. The message is shown as is, so a provider can
    /// reproduce an exact Go string such as `bing: no images returned from API`.
    #[error("{0}")]
    Other(String),
}

impl ProviderError {
    /// `ProviderError::Status` from a context and a status code.
    pub fn status(context: impl Into<String>, status: reqwest::StatusCode) -> Self {
        Self::Status {
            context: context.into(),
            status: status.as_u16(),
        }
    }

    /// `ProviderError::Request` from a context and a reqwest error.
    pub fn request(context: impl Into<String>, source: reqwest::Error) -> Self {
        Self::Request {
            context: context.into(),
            source,
        }
    }

    /// `ProviderError::Decode` from a context and a serde_json error.
    pub fn decode(context: impl Into<String>, source: serde_json::Error) -> Self {
        Self::Decode {
            context: context.into(),
            source,
        }
    }

    /// The `Retry-After` hint of [`ProviderError::RateLimitedFor`]; `None` for every other
    /// variant, including [`ProviderError::RateLimited`].
    #[must_use]
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimitedFor {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => None,
        }
    }

    /// Whether this is a rate limit: [`ProviderError::RateLimited`] or
    /// [`ProviderError::RateLimitedFor`].
    #[must_use]
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::RateLimited(_) | Self::RateLimitedFor { .. })
    }

    /// `ProviderError::Io` from a context and an I/O error.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_error_text_matches_go() {
        let err = ProviderError::status("bing", reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.to_string(), "bing: unexpected status 500");
    }

    fn rate_limited_for(retry_after_secs: Option<u64>) -> ProviderError {
        ProviderError::RateLimitedFor {
            message: "unsplash api: rate limited (status 429)".into(),
            retry_after_secs,
        }
    }

    #[test]
    fn rate_limited_for_displays_message_only() {
        assert_eq!(
            rate_limited_for(Some(60)).to_string(),
            "unsplash api: rate limited (status 429)"
        );
    }

    #[test]
    fn retry_after_secs_reads_rate_limited_for_hint() {
        assert_eq!(rate_limited_for(Some(60)).retry_after_secs(), Some(60));
    }

    #[test]
    fn retry_after_secs_is_none_without_hint() {
        assert_eq!(rate_limited_for(None).retry_after_secs(), None);
    }

    #[test]
    fn retry_after_secs_is_none_for_rate_limited() {
        assert_eq!(
            ProviderError::RateLimited("retry after 60 s".into()).retry_after_secs(),
            None
        );
    }

    #[test]
    fn is_rate_limited_for_both_variants() {
        assert!(ProviderError::RateLimited("r".into()).is_rate_limited());
        assert!(rate_limited_for(None).is_rate_limited());
    }

    #[test]
    fn is_rate_limited_false_for_other_variants() {
        assert!(
            !ProviderError::status("x", reqwest::StatusCode::SERVICE_UNAVAILABLE).is_rate_limited()
        );
        assert!(!ProviderError::NotConfigured("x".into()).is_rate_limited());
        assert!(!ProviderError::Unsupported("x".into()).is_rate_limited());
    }

    #[test]
    fn message_variants_display_verbatim() {
        let msg = "apod: rate limit exceeded (DEMO_KEY allows 30 req/hr; set a real API key)";
        assert_eq!(ProviderError::RateLimited(msg.into()).to_string(), msg);
        assert_eq!(
            ProviderError::NotConfigured("unsplash requires access_key".into()).to_string(),
            "unsplash requires access_key"
        );
    }

    #[test]
    fn unusable_displays_message_only() {
        let msg = "example.com: no images on the page";
        assert_eq!(ProviderError::Unusable(msg.into()).to_string(), msg);
    }

    #[test]
    fn result_deserializes_from_fixture_keys() {
        let r: WallpaperResult = serde_json::from_str(
            r#"{"source_url":"s","image_url":"i","title":"t","author":"a","source":"bing"}"#,
        )
        .unwrap();
        assert_eq!(r.source, "bing");
        assert_eq!(r.image_url, "i");
    }
}
