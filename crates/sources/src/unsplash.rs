//! Unsplash provider: random landscape photos from the official API.
//!
//! Port of Go `internal/providers/unsplash/unsplash.go`, API mode only. Go's keyless mode used
//! `source.unsplash.com`, which Unsplash has shut down, so without an `access_key` `fetch`
//! returns [`ProviderError::NotConfigured`] and sends no request.
//!
//! Browsing (picker Sources tab) pages through the editorial feed (`/photos`) for an empty
//! query and through `/search/photos` otherwise. The cursor is the next page number.

use async_trait::async_trait;
use kakemono_config::schema::UnsplashConfig;
use reqwest::Client;
use serde::Deserialize;

use crate::browse::{
    BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem, sanitize_remote_id,
};
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

/// Photos per browse request; Unsplash's own default page size.
const BROWSE_PER_PAGE: &str = "10";

/// Last editorial page browse asks for: 200 photos, so an idle scroll cannot drain the hourly
/// rate limit (50 requests for demo apps).
const EDITORIAL_LAST_PAGE: u32 = 20;

/// Hosts of Unsplash's image CDN, for `urls.small`, `urls.raw` and `urls.full`
/// (`plus.unsplash.com` serves Unsplash+ photos).
const MEDIA_HOSTS: &[&str] = &["images.unsplash.com", "plus.unsplash.com"];

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

    /// The configured access key, trimmed; [`ProviderError::NotConfigured`] when blank.
    fn access_key(&self) -> Result<&str, ProviderError> {
        let access_key = self.cfg.access_key.trim();
        if access_key.is_empty() {
            return Err(ProviderError::NotConfigured(NO_ACCESS_KEY.into()));
        }
        Ok(access_key)
    }

    /// `GET {base}{path}?{params}` sent once ([`http::send_fail_fast`]), decoded as `T`.
    async fn get_fail_fast<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, ProviderError> {
        let url = http::endpoint(&self.base_url, path, params);
        let response = http::send_fail_fast(self.client.get(url), CONTEXT).await?;
        let response = http::check_status(response).map_err(status_error)?;
        http::read_json(response, CONTEXT).await
    }
}

#[async_trait]
impl Provider for Unsplash {
    fn name(&self) -> &'static str {
        "unsplash"
    }

    /// `GET {base}/photos/random?client_id=..&count=10&orientation=landscape[&query=..]`.
    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let access_key = self.access_key()?;

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

        let response = http::check_status(response).map_err(status_error)?;

        // With count > 1 the API always returns an array.
        let photos: Vec<Photo> = http::read_json(response, CONTEXT).await?;
        Ok(photos.into_iter().filter_map(to_result).collect())
    }

    fn browse_support(&self) -> BrowseSupport {
        BrowseSupport::Paged
    }

    fn media_hosts(&self) -> &'static [&'static str] {
        MEDIA_HOSTS
    }

    /// Empty query: `GET {base}/photos?client_id=..&page=N&per_page=10` (editorial feed, newest
    /// first), ending after [`EDITORIAL_LAST_PAGE`] or at an empty page.
    ///
    /// Otherwise: `GET {base}/search/photos?client_id=..&orientation=landscape&page=N&per_page=10
    /// &query=..`, ending at the response's `total_pages`.
    async fn browse(
        &self,
        query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        let access_key = self.access_key()?;
        let page = cursor.map_or(Ok(1), parse_cursor)?;
        let page_param = page.to_string();
        let mut params = vec![
            ("client_id", access_key),
            ("page", page_param.as_str()),
            ("per_page", BROWSE_PER_PAGE),
        ];

        let query = query.trim();
        let (photos, last_page) = if query.is_empty() {
            let photos: Vec<Photo> = self.get_fail_fast("/photos", &params).await?;
            (photos, EDITORIAL_LAST_PAGE)
        } else {
            params.extend([("orientation", "landscape"), ("query", query)]);
            let found: SearchResponse = self.get_fail_fast("/search/photos", &params).await?;
            (found.results, found.total_pages)
        };

        let next =
            (!photos.is_empty() && page < last_page).then(|| BrowseCursor((page + 1).to_string()));
        Ok(BrowseBatch {
            items: photos.into_iter().filter_map(to_item).collect(),
            next,
        })
    }
}

/// The page number a browse cursor holds: a decimal of at least 1.
fn parse_cursor(cursor: &BrowseCursor) -> Result<u32, ProviderError> {
    cursor
        .0
        .parse()
        .ok()
        .filter(|&page| page >= 1)
        .ok_or_else(|| {
            ProviderError::Other(format!("unsplash: invalid browse cursor {:?}", cursor.0))
        })
}

/// A non-200 API status as the provider's error (Go wording).
///
/// A 429 reaches this only from `fetch`: browse's [`http::send_fail_fast`] turns it into
/// [`ProviderError::RateLimitedFor`] first.
fn status_error(bad: BadStatus) -> ProviderError {
    match bad {
        BadStatus::RateLimited => {
            ProviderError::RateLimited(format!("{CONTEXT}: rate limit exceeded"))
        }
        BadStatus::Unauthorized(status) => ProviderError::NotConfigured(format!(
            "{CONTEXT}: invalid or revoked access key (status {})",
            status.as_u16()
        )),
        BadStatus::Other(status) => ProviderError::status(CONTEXT, status),
    }
}

/// One API photo as a result; `None` when it has neither a raw nor a full URL.
fn to_result(photo: Photo) -> Option<WallpaperResult> {
    let image_url = full_image_url(photo.urls.raw, photo.urls.full)?;
    Some(WallpaperResult {
        source_url: photo.links.html,
        image_url,
        title: title(photo.description, photo.alt_description),
        author: photo.user.name,
        source: "unsplash".into(),
    })
}

/// One API photo as a browse item; `None` when it has no id, no `urls.small` preview, or
/// neither a raw nor a full URL. A missing id would hash every such photo to the same
/// `remote_id`.
fn to_item(photo: Photo) -> Option<RemoteItem> {
    if photo.id.is_empty() || photo.urls.small.is_empty() {
        return None;
    }
    let image_url = full_image_url(photo.urls.raw, photo.urls.full)?;
    Some(RemoteItem {
        remote_id: sanitize_remote_id(&photo.id),
        preview_url: photo.urls.small,
        image: ImageRef::Direct(image_url),
        source_url: photo.links.html,
        title: title(photo.description, photo.alt_description),
        author: photo.user.name,
        width: photo.width,
        height: photo.height,
        nsfw: false,
    })
}

/// The full-size image URL: `raw`, else `full`, with [`QUALITY_PARAMS`] appended when it names
/// no format. `None` when both are empty.
fn full_image_url(raw: String, full: String) -> Option<String> {
    let mut image_url = if raw.is_empty() { full } else { raw };
    if image_url.is_empty() {
        return None;
    }
    if !image_url.contains("fm=") {
        // Go always appended with `&`; use `?` when the URL has no query yet.
        image_url.push(if image_url.contains('?') { '&' } else { '?' });
        image_url.push_str(QUALITY_PARAMS);
    }
    Some(image_url)
}

/// `description`, else `alt_description`.
fn title(description: String, alt_description: String) -> String {
    if description.is_empty() {
        alt_description
    } else {
        description
    }
}

/// `/search/photos` response.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SearchResponse {
    #[serde(deserialize_with = "de::null_default")]
    total_pages: u32,
    #[serde(deserialize_with = "de::null_default")]
    results: Vec<Photo>,
}

/// API photo. Missing or `null` fields (the API sends `"description": null`) decode to `""`,
/// like Go's `encoding/json`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Photo {
    #[serde(deserialize_with = "de::null_default")]
    id: String,
    /// Full-size width in pixels; only browse reads it.
    width: Option<u32>,
    /// Full-size height in pixels; only browse reads it.
    height: Option<u32>,
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
    /// 400 px wide preview; only browse reads it.
    #[serde(deserialize_with = "de::null_default")]
    small: String,
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

    fn item_photo(id: &str) -> Photo {
        photo(&format!(
            r#"{{"id":{id:?},"urls":{{"raw":"https://i/x?fm=jpg","small":"https://i/s"}}}}"#
        ))
    }

    #[test]
    fn item_keeps_a_safe_id() {
        assert_eq!(to_item(item_photo("Ab_9-z")).unwrap().remote_id, "Ab_9-z");
    }

    #[test]
    fn item_id_with_path_characters_is_hashed() {
        assert_eq!(
            to_item(item_photo("../x")).unwrap().remote_id,
            sanitize_remote_id("../x")
        );
    }

    #[test]
    fn item_without_id_is_skipped() {
        assert_eq!(to_item(item_photo("")), None);
    }

    #[test]
    fn cursor_zero_is_invalid() {
        assert!(parse_cursor(&BrowseCursor("0".into())).is_err());
    }

    #[test]
    fn cursor_reads_page_number() {
        assert_eq!(parse_cursor(&BrowseCursor("7".into())).unwrap(), 7);
    }

    #[test]
    fn search_without_total_pages_decodes_as_zero() {
        let found: SearchResponse = serde_json::from_str(r#"{"total_pages":null}"#).unwrap();
        assert_eq!((found.total_pages, found.results.len()), (0, 0));
    }

    #[test]
    fn photo_without_urls_is_skipped() {
        assert_eq!(to_result(photo(r#"{"urls":{"raw":"","full":""}}"#)), None);
        assert_eq!(to_result(photo("{}")), None);
    }
}
