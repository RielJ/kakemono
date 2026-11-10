//! Wallhaven provider (<https://wallhaven.cc>). Port of Go `internal/providers/wallhaven`.
//!
//! Two modes, chosen per fetch by whether `api_key` is set:
//!
//! - API mode ([`api`]): `GET /api/v1/search` with the key.
//! - Scrape mode ([`scrape`]): no key; parses the public `/search` HTML, then up to
//!   [`scrape::MAX_PAGES`] wallpaper pages, waiting `scrape_delay` (plus jitter) before each one.
//!
//! Every request goes through [`http::send_with_retry`] (429/503 back-off on the tokio clock).

use std::time::Duration;

use async_trait::async_trait;
use kakemono_config::schema::WallhavenConfig;
use reqwest::{Client, Response, StatusCode};

use crate::http::{self, BadStatus};
use crate::provider::{Provider, ProviderError, WallpaperResult};

pub mod api;
pub mod scrape;

/// Production origin; the provider appends `/api/v1/search` and `/search`.
pub const DEFAULT_BASE_URL: &str = "https://wallhaven.cc";

/// Base wait before each wallpaper-page fetch in scrape mode (Go: `scrapeDelay` 1000 ms).
/// The actual wait is `delay + random(0..delay)`.
pub const DEFAULT_SCRAPE_DELAY: Duration = Duration::from_millis(1000);

/// Default `purity` when the config value is empty (SFW only).
pub(crate) const DEFAULT_PURITY: &str = "100";
/// Default `sorting` when the config value is empty.
pub const DEFAULT_SORTING: &str = "random";
/// Default `categories` when the config value is empty (general, anime, people).
pub const DEFAULT_CATEGORIES: &str = "111";

/// Query parameter that carries the API key.
pub(crate) const API_KEY_PARAM: &str = "apikey";

use crate::http::{REDACTED, redact_url};

/// Per-instance search options that replace the `[wallhaven]` values, taken from a pasted
/// `wallhaven.cc` URL. `None` (or an empty string) keeps the config value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WallhavenOverrides {
    /// `categories`, e.g. `"100"` (general only).
    pub categories: Option<String>,
    /// `purity`, e.g. `"110"`.
    pub purity: Option<String>,
    /// `sorting`, e.g. `"toplist"`.
    pub sorting: Option<String>,
}

/// The Wallhaven provider. `Debug` shows `api_key` as [`REDACTED`] when it is set.
#[derive(Clone)]
pub struct Wallhaven {
    api_key: String,
    purity: String,
    sorting: String,
    categories: String,
    base_url: url::Url,
    scrape_delay: Duration,
    client: Client,
}

impl Wallhaven {
    /// Provider against [`DEFAULT_BASE_URL`] with [`DEFAULT_SCRAPE_DELAY`].
    #[must_use]
    pub fn new(cfg: WallhavenConfig, client: Client) -> Self {
        Self::with_base_url(cfg, client, http::default_base_url(DEFAULT_BASE_URL))
    }

    /// Provider against `base_url` (`scheme://host`), for tests.
    ///
    /// Empty `purity`, `sorting` and `categories` fall back to `"100"`, `"random"` and `"111"`,
    /// as in Go. The scrape delay stays [`DEFAULT_SCRAPE_DELAY`]; use
    /// [`Wallhaven::with_scrape_delay`] to change it (Go's `NewWithSearchURL` set it to 0).
    #[must_use]
    pub fn with_base_url(cfg: WallhavenConfig, client: Client, base_url: url::Url) -> Self {
        let or_default = |value: String, default: &str| {
            if value.is_empty() {
                default.to_owned()
            } else {
                value
            }
        };
        Self {
            api_key: cfg.api_key,
            purity: or_default(cfg.purity, DEFAULT_PURITY),
            sorting: or_default(cfg.sorting, DEFAULT_SORTING),
            categories: or_default(cfg.categories, DEFAULT_CATEGORIES),
            base_url,
            scrape_delay: DEFAULT_SCRAPE_DELAY,
            client,
        }
    }

    /// Replaces `categories`, `purity` and `sorting` with the non-empty values in `overrides`.
    #[must_use]
    pub fn apply_overrides(mut self, overrides: &WallhavenOverrides) -> Self {
        let pick = |slot: &mut String, value: &Option<String>| {
            if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
                v.clone_into(slot);
            }
        };
        pick(&mut self.categories, &overrides.categories);
        pick(&mut self.purity, &overrides.purity);
        pick(&mut self.sorting, &overrides.sorting);
        self
    }

    /// Sets the base wait before each wallpaper-page fetch in scrape mode. `Duration::ZERO`
    /// disables the wait (and its jitter).
    #[must_use]
    pub fn with_scrape_delay(mut self, delay: Duration) -> Self {
        self.scrape_delay = delay;
        self
    }

    /// Whether `fetch` uses API mode (an API key is set) rather than scrape mode.
    pub fn uses_api(&self) -> bool {
        !self.api_key.is_empty()
    }

    /// The search parameters both modes send (`purity`, `sorting`, `categories`, and `q` when
    /// `query` is not empty).
    fn search_params<'a>(&'a self, query: &'a str) -> Vec<(&'a str, &'a str)> {
        let mut params = vec![
            ("sorting", self.sorting.as_str()),
            ("purity", self.purity.as_str()),
            ("categories", self.categories.as_str()),
        ];
        if !query.is_empty() {
            params.push(("q", query));
        }
        params
    }

    /// `GET url` with retry; only 200 counts as success (Go's `doWithRetry`).
    ///
    /// Every URL in an error from this function has the API key replaced by [`REDACTED`]:
    /// `send_with_retry` redacts transport errors, the status error is redacted here, and
    /// `http::read_json` redacts body-read errors.
    async fn get_ok(&self, url: url::Url) -> Result<Response, GetError> {
        let response = http::send_with_retry(self.client.get(url.clone()))
            .await
            .map_err(GetError::Request)?;
        http::check_status(response).map_err(|bad| match bad {
            BadStatus::RateLimited => GetError::RateLimited,
            BadStatus::Unauthorized(status) | BadStatus::Other(status) => GetError::Status {
                status,
                url: redact_url(&url),
            },
        })
    }
}

impl std::fmt::Debug for Wallhaven {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let api_key = if self.api_key.is_empty() {
            ""
        } else {
            REDACTED
        };
        f.debug_struct("Wallhaven")
            .field("api_key", &api_key)
            .field("purity", &self.purity)
            .field("sorting", &self.sorting)
            .field("categories", &self.categories)
            .field("base_url", &self.base_url.as_str())
            .field("scrape_delay", &self.scrape_delay)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Provider for Wallhaven {
    fn name(&self) -> &'static str {
        "wallhaven"
    }

    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        if self.uses_api() {
            self.fetch_api(query).await
        } else {
            self.fetch_scrape(query).await
        }
    }
}

/// Why [`Wallhaven::get_ok`] failed. Turned into a [`ProviderError`] with a mode prefix.
#[derive(Debug)]
enum GetError {
    /// Transport failure (connect, timeout); its URL is already redacted.
    Request(reqwest::Error),
    /// Still 429 after the retries.
    RateLimited,
    /// Any other non-200 status. `url` is already redacted.
    Status { status: StatusCode, url: url::Url },
}

impl GetError {
    /// Go's texts: `{context}: unexpected status 500 for <url>`; rate limit and transport errors
    /// keep the same prefix.
    fn into_provider_error(self, context: &str) -> ProviderError {
        match self {
            Self::Request(e) => ProviderError::request(context, e),
            Self::RateLimited => http::rate_limited(context),
            Self::Status { status, url } => ProviderError::Other(format!(
                "{context}: unexpected status {} for {url}",
                status.as_u16()
            )),
        }
    }
}

impl std::fmt::Display for GetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(e) => write!(f, "{e}"),
            Self::RateLimited => f.write_str("rate limited (status 429)"),
            Self::Status { status, url } => {
                write!(f, "unexpected status {} for {url}", status.as_u16())
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_replaces_only_the_key_and_keeps_order() {
        let url = url::Url::parse(
            "https://wallhaven.cc/api/v1/search?apikey=s3cr%26t&categories=111&q=a+b&sorting=random",
        )
        .unwrap();
        let redacted = redact_url(&url);
        assert_eq!(
            redacted.as_str(),
            "https://wallhaven.cc/api/v1/search?apikey=REDACTED&categories=111&q=a+b&sorting=random"
        );
        assert!(!redacted.as_str().contains("s3cr"));
    }

    #[test]
    fn redact_url_without_key_is_unchanged() {
        let url = url::Url::parse("https://wallhaven.cc/search?categories=111&q=a%20b").unwrap();
        assert_eq!(redact_url(&url), url);
    }

    #[test]
    fn debug_hides_the_key() {
        let cfg = WallhavenConfig {
            api_key: "testkey".to_owned(),
            ..WallhavenConfig::default()
        };
        let debug = format!("{:?}", Wallhaven::new(cfg, Client::new()));
        assert!(!debug.contains("testkey"), "{debug}");
        assert!(debug.contains("REDACTED"), "{debug}");
    }
}
