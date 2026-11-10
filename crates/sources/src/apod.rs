//! NASA Astronomy Picture of the Day provider (port of Go `internal/providers/apod`).
//!
//! One request returns 5 random APOD entries. Only `media_type == "image"` entries are kept
//! (video days are skipped), and `hdurl` is preferred over `url`. The query is ignored.

use async_trait::async_trait;
use kakemono_config::schema::ApodConfig;
use reqwest::Client;
use serde::Deserialize;

use crate::de;
use crate::http::{self, BadStatus};
use crate::provider::{Provider, ProviderError, WallpaperResult};

/// Production base URL of the NASA API.
pub const BASE_URL: &str = "https://api.nasa.gov";

/// API path appended to the base URL.
const API_PATH: &str = "/planetary/apod";

/// Key used when the config leaves `api_key` empty. NASA limits it to 30 requests per hour.
pub const DEMO_KEY: &str = "DEMO_KEY";

/// Entries requested per fetch.
const COUNT: &str = "5";

/// Go's error text for a 429 answer.
const RATE_LIMITED: &str =
    "apod: rate limit exceeded (DEMO_KEY allows 30 req/hr; set a real API key)";

/// The APOD provider.
#[derive(Debug, Clone)]
pub struct Apod {
    api_key: String,
    client: Client,
    base_url: url::Url,
}

impl Apod {
    /// A provider against the production NASA API ([`BASE_URL`]).
    #[must_use]
    pub fn new(cfg: ApodConfig, client: Client) -> Self {
        Self::with_base_url(cfg, client, http::default_base_url(BASE_URL))
    }

    /// A provider against `base_url` (usually `scheme://host`). Tests point it at a mock.
    /// An empty `api_key` becomes [`DEMO_KEY`].
    #[must_use]
    pub fn with_base_url(cfg: ApodConfig, client: Client, base_url: url::Url) -> Self {
        let api_key = if cfg.api_key.is_empty() {
            DEMO_KEY.to_owned()
        } else {
            cfg.api_key
        };
        Self {
            api_key,
            client,
            base_url,
        }
    }
}

#[async_trait]
impl Provider for Apod {
    fn name(&self) -> &'static str {
        "apod"
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let url = http::endpoint(
            &self.base_url,
            API_PATH,
            &[("api_key", &self.api_key), ("count", COUNT)],
        );

        // The URL holds the API key, so it is kept out of the error text (Go: `apod: GET: ...`).
        let response = http::get_with_retry(&self.client, url)
            .await
            .map_err(|e| ProviderError::request("apod: GET", e.without_url()))?;

        let response = http::check_status(response).map_err(|bad| match bad {
            BadStatus::RateLimited => ProviderError::RateLimited(RATE_LIMITED.to_owned()),
            BadStatus::Unauthorized(status) => ProviderError::NotConfigured(format!(
                "apod: invalid API key (status {})",
                status.as_u16()
            )),
            BadStatus::Other(status) => ProviderError::status("apod", status),
        })?;

        let entries: Vec<ApodEntry> = http::read_json(response, "apod").await?;
        Ok(entries.into_iter().filter_map(to_result).collect())
    }
}

/// One APOD entry. Missing or `null` fields decode as empty, like Go.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ApodEntry {
    #[serde(deserialize_with = "de::null_default")]
    title: String,
    #[serde(deserialize_with = "de::null_default")]
    date: String,
    #[serde(deserialize_with = "de::null_default")]
    media_type: String,
    #[serde(deserialize_with = "de::null_default")]
    url: String,
    #[serde(deserialize_with = "de::null_default")]
    hdurl: String,
    #[serde(deserialize_with = "de::null_default")]
    copyright: String,
}

/// An image entry as a result; `None` for videos and entries without any image URL.
fn to_result(entry: ApodEntry) -> Option<WallpaperResult> {
    if entry.media_type != "image" {
        return None;
    }
    let image_url = [entry.hdurl, entry.url]
        .into_iter()
        .find(|u| !u.is_empty())?;
    Some(WallpaperResult {
        source_url: page_url(&entry.date),
        image_url,
        title: entry.title,
        author: entry.copyright,
        source: "apod".to_owned(),
    })
}

/// The APOD page for a `YYYY-MM-DD` date, built like Go: `2024-03-15` →
/// `https://apod.nasa.gov/apod/ap20240315.html`. `""` when the date is not 10 bytes long.
fn page_url(date: &str) -> String {
    if date.len() != 10 {
        return String::new();
    }
    match (date.get(0..4), date.get(5..7), date.get(8..10)) {
        (Some(y), Some(m), Some(d)) => format!("https://apod.nasa.gov/apod/ap{y}{m}{d}.html"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_url_from_date() {
        assert_eq!(
            page_url("2024-03-15"),
            "https://apod.nasa.gov/apod/ap20240315.html"
        );
        assert_eq!(page_url("2024-3-19"), "");
        assert_eq!(page_url(""), "");
        // 10 bytes but not on char boundaries: no panic, no URL.
        assert_eq!(page_url("202ü-03-1"), "");
    }

    #[test]
    fn empty_api_key_uses_demo_key() {
        let cfg = ApodConfig {
            api_key: String::new(),
        };
        let apod = Apod::new(cfg, Client::new());
        assert_eq!(apod.api_key, DEMO_KEY);
        assert_eq!(apod.base_url.as_str(), "https://api.nasa.gov/");
    }
}
