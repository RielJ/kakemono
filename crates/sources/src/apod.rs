//! NASA Astronomy Picture of the Day provider (port of Go `internal/providers/apod`).
//!
//! One request returns 5 random APOD entries. Only `media_type == "image"` entries are kept
//! (video days are skipped), and `hdurl` is preferred over `url`. The query is ignored.
//!
//! Browsing walks back through the archive in 14-day windows (`start_date..=end_date`), newest
//! first. The cursor is the next window's `end_date`; the walk ends at the first APOD,
//! 1995-06-16.

use async_trait::async_trait;
use chrono::{Days, NaiveDate, Utc};
use kakemono_config::schema::ApodConfig;
use reqwest::Client;
use serde::Deserialize;

use crate::browse::{
    BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem, sanitize_remote_id,
};
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

/// Days in one browse window, `end_date` included.
const WINDOW_DAYS: u64 = 14;

/// The first APOD. Browsing never asks for anything earlier.
const FIRST_APOD: NaiveDate = match NaiveDate::from_ymd_opt(1995, 6, 16) {
    Some(d) => d,
    None => panic!("1995-06-16 is a valid date"),
};

/// Hours subtracted from UTC to get "today" for the API. NASA rejects an `end_date` after its
/// own (US Eastern) date; UTC−5 is never ahead of it, in summer or winter.
const NASA_UTC_OFFSET_HOURS: i64 = 5;

/// Hosts APOD previews and full images come from.
const MEDIA_HOSTS: &[&str] = &["apod.nasa.gov"];

/// Go's error text for a 429 answer.
const RATE_LIMITED: &str =
    "apod: rate limit exceeded (DEMO_KEY allows 30 req/hr; set a real API key)";

/// The APOD provider.
#[derive(Debug, Clone)]
pub struct Apod {
    api_key: String,
    client: Client,
    base_url: url::Url,
    /// Fixed "today" for browsing; `None` reads the clock on every first page.
    today: Option<NaiveDate>,
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
            today: None,
        }
    }

    /// The same provider with browsing pinned to `today` instead of the clock. Tests use it.
    #[must_use]
    pub fn with_today(mut self, today: NaiveDate) -> Self {
        self.today = Some(today);
        self
    }

    /// The newest date browsing may ask for.
    fn today(&self) -> NaiveDate {
        self.today.unwrap_or_else(|| {
            (Utc::now() - chrono::Duration::hours(NASA_UTC_OFFSET_HOURS)).date_naive()
        })
    }

    /// The `end_date` of the window `cursor` points at: today without a cursor. A cursor that
    /// is not a date between [`FIRST_APOD`] and today is an error.
    fn window_end(&self, cursor: Option<&BrowseCursor>) -> Result<NaiveDate, ProviderError> {
        let today = self.today();
        let Some(BrowseCursor(raw)) = cursor else {
            return Ok(today);
        };
        match NaiveDate::parse_from_str(raw, DATE_FORMAT) {
            Ok(end) if (FIRST_APOD..=today).contains(&end) => Ok(end),
            _ => Err(ProviderError::Other(format!(
                "apod: invalid browse cursor {raw:?}"
            ))),
        }
    }
}

/// `YYYY-MM-DD`, the API's date format and the browse cursor's.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// The first day of the window ending at `end`, never before [`FIRST_APOD`].
fn window_start(end: NaiveDate) -> NaiveDate {
    end.checked_sub_days(Days::new(WINDOW_DAYS - 1))
        .map_or(FIRST_APOD, |start| start.max(FIRST_APOD))
}

/// Maps a non-200 status to the provider error, the same for fetch and browse.
fn status_error(bad: BadStatus) -> ProviderError {
    match bad {
        BadStatus::RateLimited => ProviderError::RateLimited(RATE_LIMITED.to_owned()),
        BadStatus::Unauthorized(status) => ProviderError::NotConfigured(format!(
            "apod: invalid API key (status {})",
            status.as_u16()
        )),
        BadStatus::Other(status) => ProviderError::status("apod", status),
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

        let response = http::check_status(response).map_err(status_error)?;

        let entries: Vec<ApodEntry> = http::read_json(response, "apod").await?;
        Ok(entries.into_iter().filter_map(to_result).collect())
    }

    fn browse_support(&self) -> BrowseSupport {
        BrowseSupport::Paged
    }

    fn media_hosts(&self) -> &'static [&'static str] {
        MEDIA_HOSTS
    }

    /// One 14-day window, newest first, videos and entries without an image skipped. The next
    /// cursor is the day before the window; a window that reaches 1995-06-16 ends the archive.
    /// A window with no images (all videos) is empty but still points further back.
    async fn browse(
        &self,
        _query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        let end = self.window_end(cursor)?;
        let start = window_start(end);
        let start_date = start.format(DATE_FORMAT).to_string();
        let end_date = end.format(DATE_FORMAT).to_string();
        let url = http::endpoint(
            &self.base_url,
            API_PATH,
            &[
                ("api_key", &self.api_key),
                ("start_date", &start_date),
                ("end_date", &end_date),
            ],
        );

        // `send_fail_fast` redacts `api_key` from transport errors.
        let response = http::send_fail_fast(self.client.get(url), "apod").await?;
        let response = http::check_status(response).map_err(status_error)?;
        let entries: Vec<ApodEntry> = http::read_json(response, "apod").await?;

        let mut items: Vec<RemoteItem> = entries.into_iter().filter_map(to_item).collect();
        // The API answers oldest first; ids are `YYYY-MM-DD`, so text order is date order.
        items.sort_by(|a, b| b.remote_id.cmp(&a.remote_id));
        let next = start
            .pred_opt()
            .filter(|_| start > FIRST_APOD)
            .map(|d| BrowseCursor(d.format(DATE_FORMAT).to_string()));
        Ok(BrowseBatch { items, next })
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

/// An image entry as a browse item; `None` for videos and entries without any image URL.
/// The preview is `url` (the smaller rendition), the full image `hdurl`; each falls back to
/// the other.
fn to_item(entry: ApodEntry) -> Option<RemoteItem> {
    if entry.media_type != "image" {
        return None;
    }
    let (preview_url, full) = match (entry.url, entry.hdurl) {
        (url, hd) if url.is_empty() && hd.is_empty() => return None,
        (url, hd) if url.is_empty() => (hd.clone(), hd),
        (url, hd) if hd.is_empty() => (url.clone(), url),
        (url, hd) => (url, hd),
    };
    Some(RemoteItem {
        remote_id: sanitize_remote_id(&entry.date),
        preview_url,
        image: ImageRef::Direct(full),
        source_url: page_url(&entry.date),
        title: entry.title,
        // The API wraps credits in newlines (`"\nJane Astro\n"`).
        author: entry.copyright.trim().to_owned(),
        width: None,
        height: None,
        nsfw: false,
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

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn window_start_is_13_days_before_the_end() {
        assert_eq!(window_start(ymd(2024, 1, 5)), ymd(2023, 12, 23));
    }

    #[test]
    fn window_start_never_precedes_the_first_apod() {
        assert_eq!(window_start(ymd(1995, 6, 20)), FIRST_APOD);
    }

    #[test]
    fn clock_today_is_never_after_utc_today() {
        let apod = Apod::new(ApodConfig::default(), Client::new());
        assert!(apod.today() <= Utc::now().date_naive());
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
