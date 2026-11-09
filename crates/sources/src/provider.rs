//! The `Provider` seam every wallpaper source implements, the candidate type it returns and the
//! shared error type (plan §A, §G).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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

    /// Whether the provider is turned on in the config.
    fn enabled(&self) -> bool;

    /// Fetches a batch of candidates. `query` may be empty; providers without search ignore it.
    /// An empty `Ok(vec![])` is a valid answer ("nothing matched").
    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError>;
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
    fn result_deserializes_from_fixture_keys() {
        let r: WallpaperResult = serde_json::from_str(
            r#"{"source_url":"s","image_url":"i","title":"t","author":"a","source":"bing"}"#,
        )
        .unwrap();
        assert_eq!(r.source, "bing");
        assert_eq!(r.image_url, "i");
    }
}
