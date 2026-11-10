//! Unsplash provider: random landscape photos from the official API.
//!
//! Port of Go `internal/providers/unsplash/unsplash.go`, API mode only. Go's keyless mode used
//! `source.unsplash.com`, which Unsplash has shut down, so without an `access_key` `fetch`
//! returns [`ProviderError::NotConfigured`] and sends no request (plan Task 13, risk 6).

use async_trait::async_trait;
use kakemono_config::schema::UnsplashConfig;
use reqwest::Client;
use serde::Deserialize;

use crate::de;
use crate::http::{self, BadStatus};
use crate::provider::{Provider, ProviderError, WallpaperResult};

/// Production API origin.
pub const BASE_URL: &str = "https://api.unsplash.com";

/// Error text of `fetch` when no access key is configured.
pub const NO_ACCESS_KEY: &str = "unsplash requires access_key";

/// Photos requested per fetch.
const COUNT: &str = "10";

/// Appended to image URLs that carry no `fm=` parameter, for maximum resolution (as in Go).
const QUALITY_PARAMS: &str = "fm=jpg&q=95&w=3840";

/// Prefix of API-mode error texts (Go wording).
const CONTEXT: &str = "unsplash api";

/// The Unsplash provider.
#[derive(Debug, Clone)]
pub struct Unsplash {
    cfg: UnsplashConfig,
    client: Client,
    base_url: url::Url,
}

impl Unsplash {
    /// Provider against [`BASE_URL`].
    #[must_use]
    pub fn new(cfg: UnsplashConfig, client: Client) -> Self {
        Self::with_base_url(cfg, client, http::default_base_url(BASE_URL))
    }

    /// Provider against another origin (`scheme://host`), for tests.
    #[must_use]
    pub fn with_base_url(cfg: UnsplashConfig, client: Client, base_url: url::Url) -> Self {
        Self {
            cfg,
            client,
            base_url,
        }
    }
}

#[async_trait]
impl Provider for Unsplash {
    fn name(&self) -> &'static str {
        "unsplash"
    }

    /// `GET {base}/photos/random?client_id=..&count=10&orientation=landscape[&query=..]`.
    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let access_key = self.cfg.access_key.trim();
        if access_key.is_empty() {
            return Err(ProviderError::NotConfigured(NO_ACCESS_KEY.into()));
        }

        let mut params = vec![
            ("count", COUNT),
            ("orientation", "landscape"),
            ("client_id", access_key),
        ];
        if !query.is_empty() {
            params.push(("query", query));
        }
        let url = http::endpoint(&self.base_url, "/photos/random", &params);

        // `without_url`: the URL carries the access key, keep it out of error texts.
        let response = http::get_with_retry(&self.client, url)
            .await
            .map_err(|e| ProviderError::request(format!("{CONTEXT}: GET"), e.without_url()))?;

        let response = http::check_status(response).map_err(|bad| match bad {
            BadStatus::RateLimited => {
                ProviderError::RateLimited(format!("{CONTEXT}: rate limit exceeded"))
            }
            BadStatus::Unauthorized(status) => ProviderError::NotConfigured(format!(
                "{CONTEXT}: invalid or revoked access key (status {})",
                status.as_u16()
            )),
            BadStatus::Other(status) => ProviderError::status(CONTEXT, status),
        })?;

        // With count > 1 the API always returns an array.
        let photos: Vec<Photo> = http::read_json(response, CONTEXT).await?;
        Ok(photos.into_iter().filter_map(to_result).collect())
    }
}

/// One API photo as a result; `None` when it has neither a raw nor a full URL.
fn to_result(photo: Photo) -> Option<WallpaperResult> {
    let mut image_url = if photo.urls.raw.is_empty() {
        photo.urls.full
    } else {
        photo.urls.raw
    };
    if image_url.is_empty() {
        return None;
    }
    if !image_url.contains("fm=") {
        // Go always appended with `&`; use `?` when the URL has no query yet.
        image_url.push(if image_url.contains('?') { '&' } else { '?' });
        image_url.push_str(QUALITY_PARAMS);
    }
    let title = if photo.description.is_empty() {
        photo.alt_description
    } else {
        photo.description
    };
    Some(WallpaperResult {
        source_url: photo.links.html,
        image_url,
        title,
        author: photo.user.name,
        source: "unsplash".into(),
    })
}

/// API photo. Missing or `null` fields (the API sends `"description": null`) decode to `""`,
/// like Go's `encoding/json`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Photo {
    #[serde(deserialize_with = "de::null_default")]
    description: String,
    #[serde(deserialize_with = "de::null_default")]
    alt_description: String,
    #[serde(deserialize_with = "de::null_default")]
    links: Links,
    #[serde(deserialize_with = "de::null_default")]
    urls: Urls,
    #[serde(deserialize_with = "de::null_default")]
    user: User,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Links {
    #[serde(deserialize_with = "de::null_default")]
    html: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Urls {
    #[serde(deserialize_with = "de::null_default")]
    raw: String,
    #[serde(deserialize_with = "de::null_default")]
    full: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct User {
    #[serde(deserialize_with = "de::null_default")]
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(json: &str) -> Photo {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn nulls_decode_as_empty() {
        let p = photo(
            r#"{"description":null,"alt_description":null,"links":{"html":"h"},
                "urls":{"raw":"https://i/x?ixid=1","full":null},"user":{"name":"N"}}"#,
        );
        let r = to_result(p).unwrap();
        assert_eq!(r.title, "");
        assert_eq!(r.image_url, "https://i/x?ixid=1&fm=jpg&q=95&w=3840");
    }

    #[test]
    fn quality_params_start_the_query_when_there_is_none() {
        let r = to_result(photo(r#"{"urls":{"raw":"https://i/x"}}"#)).unwrap();
        assert_eq!(r.image_url, "https://i/x?fm=jpg&q=95&w=3840");
    }

    #[test]
    fn photo_without_urls_is_skipped() {
        assert_eq!(to_result(photo(r#"{"urls":{"raw":"","full":""}}"#)), None);
        assert_eq!(to_result(photo("{}")), None);
    }
}
