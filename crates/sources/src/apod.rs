//! NASA Astronomy Picture of the Day provider (port of Go `internal/providers/apod`).
//!
//! One request returns 5 random APOD entries. Only `media_type == "image"` entries are kept
//! (video days are skipped), and `hdurl` is preferred over `url`. The query is ignored.

use async_trait::async_trait;
use kakemono_config::schema::ApodConfig;
use reqwest::{Client, StatusCode};
use serde::Deserialize;

use crate::http;
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
    enabled: bool,
    api_key: String,
    client: Client,
    base_url: String,
}

impl Apod {
    /// A provider against the production NASA API.
    pub fn new(cfg: ApodConfig, client: Client) -> Self {
        Self::with_base_url(cfg, client, BASE_URL)
    }

    /// A provider against `base_url` (`scheme://host`, no path). Tests point it at a mock.
    /// A trailing `/` is removed. An empty `api_key` becomes [`DEMO_KEY`].
    pub fn with_base_url(cfg: ApodConfig, client: Client, base_url: impl Into<String>) -> Self {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        let api_key = if cfg.api_key.is_empty() {
            DEMO_KEY.to_owned()
        } else {
            cfg.api_key
        };
        Self {
            enabled: cfg.enabled,
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

    fn enabled(&self) -> bool {
        self.enabled
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let base = format!("{}{API_PATH}", self.base_url);
        let url = http::url_with_query(&base, &[("api_key", &self.api_key), ("count", COUNT)])
            .map_err(|e| ProviderError::Other(format!("apod: build request: {e}")))?;

        // The URL holds the API key, so it is kept out of the error text (Go: `apod: GET: ...`).
        let response = http::get_with_retry(&self.client, url)
            .await
            .map_err(|e| ProviderError::request("apod: GET", e.without_url()))?;

        let status = response.status();
        match status {
            StatusCode::OK => {}
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(ProviderError::RateLimited(RATE_LIMITED.to_owned()));
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(ProviderError::NotConfigured(format!(
                    "apod: invalid API key (status {})",
                    status.as_u16()
                )));
            }
            _ => return Err(ProviderError::status("apod", status)),
        }

        let entries: Vec<ApodEntry> = http::read_json(response, "apod").await?;
        Ok(entries.into_iter().filter_map(to_result).collect())
    }
}

/// One APOD entry. Missing or `null` fields decode as empty, like Go.
#[derive(Debug, Deserialize)]
struct ApodEntry {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    hdurl: Option<String>,
    #[serde(default)]
    copyright: Option<String>,
}

/// An image entry as a result; `None` for videos and entries without any image URL.
fn to_result(entry: ApodEntry) -> Option<WallpaperResult> {
    if entry.media_type.as_deref() != Some("image") {
        return None;
    }
    let image_url = [entry.hdurl, entry.url]
        .into_iter()
        .flatten()
        .find(|u| !u.is_empty())?;
    Some(WallpaperResult {
        source_url: page_url(entry.date.as_deref().unwrap_or_default()),
        image_url,
        title: entry.title.unwrap_or_default(),
        author: entry.copyright.unwrap_or_default(),
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
            enabled: true,
            api_key: String::new(),
        };
        let apod = Apod::new(cfg, Client::new());
        assert_eq!(apod.api_key, DEMO_KEY);
        assert_eq!(apod.base_url, BASE_URL);
    }
}
