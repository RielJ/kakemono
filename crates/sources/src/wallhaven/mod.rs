//! Wallhaven provider (<https://wallhaven.cc>). Port of Go `internal/providers/wallhaven`.
//!
//! Two modes, chosen per fetch by whether `api_key` is set:
//!
//! - API mode ([`api`]): `GET /api/v1/search` with the key.
//! - Scrape mode ([`scrape`]): no key; parses the public `/search` HTML, then up to
//!   [`scrape::MAX_PAGES`] wallpaper pages, waiting `scrape_delay` (plus jitter) before each one.
//!   Scrape browse reads only the search page; `resolve_image` fetches the one wallpaper page
//!   the user picks.
//!
//! Every `fetch` request goes through [`http::send_with_retry`] (429/503 back-off on the tokio
//! clock). `browse` requests go through [`http::send_fail_fast`]: the picker is waiting, so a
//! 429/503 is an error at once.

use std::time::Duration;

use async_trait::async_trait;
use kakemono_config::schema::WallhavenConfig;
use reqwest::{Client, Response, StatusCode};

use crate::browse::{BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem};
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

/// Hosts of previews (`th.wallhaven.cc`) and full images (`w.wallhaven.cc`), and of the
/// wallpaper pages that scrape mode resolves (`wallhaven.cc`).
const MEDIA_HOSTS: &[&str] = &["wallhaven.cc"];

use crate::http::{REDACTED, redact_url};

/// Per-instance search options taken from a pasted `wallhaven.cc` URL.
///
/// `categories`, `purity` and `sorting` replace the `[wallhaven]` values; `None` (or an empty
/// string) keeps the config value. The other fields have no config value: when set (and not
/// empty) they are sent as the Wallhaven search parameter of the same name, as is, and when
/// `None` the parameter is not sent. Wallhaven checks the values itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WallhavenOverrides {
    /// `categories`, e.g. `"100"` (general only).
    pub categories: Option<String>,
    /// `purity`, e.g. `"110"`.
    pub purity: Option<String>,
    /// `sorting`, e.g. `"toplist"`.
    pub sorting: Option<String>,
    /// `topRange`: `1d`, `3d`, `1w`, `1M`, `3M`, `6M` or `1y`. Wallhaven uses it only with
    /// `sorting=toplist`.
    pub top_range: Option<String>,
    /// `order`: `desc` or `asc`.
    pub order: Option<String>,
    /// `atleast`: minimum resolution, e.g. `2560x1440`.
    pub atleast: Option<String>,
    /// `resolutions`: exact resolutions, e.g. `1920x1080,2560x1440`.
    pub resolutions: Option<String>,
    /// `ratios`: aspect ratios, e.g. `16x9,16x10` or `landscape`.
    pub ratios: Option<String>,
    /// `colors`: a hex color without `#`, e.g. `0066cc`.
    pub colors: Option<String>,
}

/// The Wallhaven provider. `Debug` shows `api_key` as [`REDACTED`] when it is set.
#[derive(Clone)]
pub struct Wallhaven {
    api_key: String,
    purity: String,
    sorting: String,
    categories: String,
    /// Search parameters from a pasted URL with no config value (`topRange`, `order`, ...),
    /// sent as is by both modes. Empty unless [`Wallhaven::apply_overrides`] set some.
    extra_params: Vec<(&'static str, String)>,
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
            extra_params: Vec::new(),
            base_url,
            scrape_delay: DEFAULT_SCRAPE_DELAY,
            client,
        }
    }

    /// Replaces `categories`, `purity` and `sorting` with the non-empty values in `overrides`,
    /// and sends its other non-empty values (`topRange`, `order`, `atleast`, `resolutions`,
    /// `ratios`, `colors`) with every search request.
    #[must_use]
    pub fn apply_overrides(mut self, overrides: &WallhavenOverrides) -> Self {
        fn set(value: &Option<String>) -> Option<&str> {
            value.as_deref().filter(|v| !v.is_empty())
        }
        let pick = |slot: &mut String, value: &Option<String>| {
            if let Some(v) = set(value) {
                v.clone_into(slot);
            }
        };
        pick(&mut self.categories, &overrides.categories);
        pick(&mut self.purity, &overrides.purity);
        pick(&mut self.sorting, &overrides.sorting);
        let extras = [
            ("topRange", &overrides.top_range),
            ("order", &overrides.order),
            ("atleast", &overrides.atleast),
            ("resolutions", &overrides.resolutions),
            ("ratios", &overrides.ratios),
            ("colors", &overrides.colors),
        ];
        for (key, value) in extras {
            self.extra_params.retain(|(k, _)| *k != key);
            if let Some(v) = set(value) {
                self.extra_params.push((key, v.to_owned()));
            }
        }
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

    /// The search parameters both modes send (`purity`, `sorting`, `categories`, `q` when
    /// `query` is not empty, and the URL options set by [`Wallhaven::apply_overrides`]).
    fn search_params<'a>(&'a self, query: &'a str) -> Vec<(&'a str, &'a str)> {
        let mut params = vec![
            ("sorting", self.sorting.as_str()),
            ("purity", self.purity.as_str()),
            ("categories", self.categories.as_str()),
        ];
        if !query.is_empty() {
            params.push(("q", query));
        }
        params.extend(self.extra_params.iter().map(|(k, v)| (*k, v.as_str())));
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

    /// `GET url` once, for browse ([`http::send_fail_fast`]); only 200 counts as success.
    ///
    /// 429/503 become [`ProviderError::RateLimitedFor`]; any other non-200 status gives the
    /// same `{context}: unexpected status {code} for <url>` text as `fetch`. URLs in errors
    /// have the API key redacted.
    async fn get_ok_fail_fast(
        &self,
        url: url::Url,
        context: &str,
    ) -> Result<Response, ProviderError> {
        let response = http::send_fail_fast(self.client.get(url.clone()), context).await?;
        http::check_status(response).map_err(|bad| match bad {
            // `send_fail_fast` already returned 429 as `RateLimitedFor`; kept for totality.
            BadStatus::RateLimited => ProviderError::RateLimitedFor {
                message: format!(
                    "{context}: rate limited (status {})",
                    StatusCode::TOO_MANY_REQUESTS.as_u16()
                ),
                retry_after_secs: None,
            },
            BadStatus::Unauthorized(status) | BadStatus::Other(status) => GetError::Status {
                status,
                url: redact_url(&url),
            }
            .into_provider_error(context),
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
            .field("extra_params", &self.extra_params)
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

    fn browse_support(&self) -> BrowseSupport {
        BrowseSupport::Paged
    }

    fn media_hosts(&self) -> &'static [&'static str] {
        MEDIA_HOSTS
    }

    async fn browse(
        &self,
        query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        if self.uses_api() {
            self.browse_api(query, cursor).await
        } else {
            self.browse_scrape(query, cursor).await
        }
    }

    async fn resolve_image(&self, item: &RemoteItem) -> Result<String, ProviderError> {
        match &item.image {
            ImageRef::Direct(url) => Ok(url.clone()),
            ImageRef::Page(page) => self.resolve_page(page).await,
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
        let debug = format!(
            "{:?}",
            Wallhaven::new(cfg, crate::http::client_builder().build().unwrap())
        );
        assert!(!debug.contains("testkey"), "{debug}");
        assert!(debug.contains("REDACTED"), "{debug}");
    }
}
