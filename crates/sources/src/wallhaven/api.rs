//! Wallhaven API mode: `GET {base}/api/v1/search?apikey=..&categories=..&purity=..&q=..&sorting=..`.
//! Port of Go `fetchAPI`.
//!
//! Browse adds `page=N` and, after the first page, the `seed` the API returned (random sort
//! stays stable across pages). The cursor is `"{page}:{seed}"`; the end is `meta.last_page`.

use serde::Deserialize;

use super::Wallhaven;
use crate::browse::{BrowseBatch, BrowseCursor, ImageRef, RemoteItem, sanitize_remote_id};
use crate::de;
use crate::http;
use crate::provider::{ProviderError, WallpaperResult};

/// Error-text prefix, as in Go.
const CONTEXT: &str = "wallhaven api";

/// Path of the search endpoint under the base URL.
pub(crate) const SEARCH_PATH: &str = "/api/v1/search";

/// Search response. Missing or `null` fields decode as empty, like Go.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SearchResponse {
    #[serde(deserialize_with = "de::null_default")]
    data: Vec<ApiWallpaper>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ApiWallpaper {
    /// Page URL.
    #[serde(deserialize_with = "de::null_default")]
    url: String,
    /// Direct image URL.
    #[serde(deserialize_with = "de::null_default")]
    path: String,
    #[serde(deserialize_with = "de::null_default")]
    uploader: Uploader,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Uploader {
    #[serde(deserialize_with = "de::null_default")]
    username: String,
}

impl Wallhaven {
    /// API mode. Entries without a `path` are skipped.
    pub(super) async fn fetch_api(
        &self,
        query: &str,
    ) -> Result<Vec<WallpaperResult>, ProviderError> {
        let mut params = self.search_params(query);
        params.push((super::API_KEY_PARAM, self.api_key.as_str()));
        let url = http::endpoint(&self.base_url, SEARCH_PATH, &params);

        let response = self
            .get_ok(url)
            .await
            .map_err(|e| e.into_provider_error(CONTEXT))?;
        // `read_json` redacts the key from body-read errors.
        let body: SearchResponse = http::read_json(response, CONTEXT).await?;

        Ok(body
            .data
            .into_iter()
            .filter(|w| !w.path.is_empty())
            .map(|w| WallpaperResult {
                source_url: w.url,
                image_url: w.path,
                title: String::new(),
                author: w.uploader.username,
                source: "wallhaven".to_owned(),
            })
            .collect())
    }
}

/// Browse response. Kept apart from [`SearchResponse`] so a surprise in the browse-only fields
/// (`thumbs`, `meta`, ...) can never fail `fetch`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BrowseResponse {
    #[serde(deserialize_with = "de::null_default")]
    data: Vec<BrowseWallpaper>,
    #[serde(deserialize_with = "de::null_default")]
    meta: Meta,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BrowseWallpaper {
    #[serde(deserialize_with = "de::null_default")]
    id: String,
    /// Page URL.
    #[serde(deserialize_with = "de::null_default")]
    url: String,
    /// Direct image URL.
    #[serde(deserialize_with = "de::null_default")]
    path: String,
    /// `sfw`, `sketchy` or `nsfw`.
    #[serde(deserialize_with = "de::null_default")]
    purity: String,
    /// `"{width}x{height}"`, e.g. `"1920x1080"`.
    #[serde(deserialize_with = "de::null_default")]
    resolution: String,
    #[serde(deserialize_with = "de::null_default")]
    thumbs: Thumbs,
    #[serde(deserialize_with = "de::null_default")]
    uploader: Uploader,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Thumbs {
    /// About 300×200; the preview.
    #[serde(deserialize_with = "de::null_default")]
    large: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Meta {
    /// `None` when absent: then the batch after an empty one is the end.
    last_page: Option<u64>,
    /// Set for `sorting=random`; sending it back keeps the random order stable across pages.
    #[serde(deserialize_with = "de::null_default")]
    seed: String,
}

/// The API browse cursor, `"{page}:{seed}"` (the seed may be empty).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ApiCursor {
    /// 1-based page number.
    page: u64,
    seed: String,
}

impl ApiCursor {
    const FIRST: Self = Self {
        page: 1,
        seed: String::new(),
    };

    fn parse(cursor: &BrowseCursor) -> Result<Self, ProviderError> {
        let invalid = || ProviderError::Other(format!("{CONTEXT}: invalid browse cursor"));
        let (page, seed) = cursor.0.split_once(':').ok_or_else(invalid)?;
        let page = page
            .parse::<u64>()
            .ok()
            .filter(|&p| p >= 1)
            .ok_or_else(invalid)?;
        Ok(Self {
            page,
            seed: seed.to_owned(),
        })
    }

    fn encode(&self) -> BrowseCursor {
        BrowseCursor(format!("{}:{}", self.page, self.seed))
    }
}

impl Wallhaven {
    /// API-mode browse: one `GET` of page `N` (with the seed of earlier pages), no retry.
    pub(super) async fn browse_api(
        &self,
        query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        let at = cursor.map_or(Ok(ApiCursor::FIRST), ApiCursor::parse)?;
        let page = at.page.to_string();
        let mut params = self.search_params(query);
        params.push((super::API_KEY_PARAM, self.api_key.as_str()));
        params.push(("page", page.as_str()));
        if !at.seed.is_empty() {
            params.push(("seed", at.seed.as_str()));
        }
        let url = http::endpoint(&self.base_url, SEARCH_PATH, &params);

        let response = self.get_ok_fail_fast(url, CONTEXT).await?;
        let body: BrowseResponse = http::read_json(response, CONTEXT).await?;

        let seed = if body.meta.seed.is_empty() {
            at.seed
        } else {
            body.meta.seed
        };
        let more = match body.meta.last_page {
            Some(last) => at.page < last,
            None => !body.data.is_empty(),
        };
        let next = at
            .page
            .checked_add(1)
            .filter(|_| more)
            .map(|page| ApiCursor { page, seed }.encode());
        let items = body.data.into_iter().filter_map(remote_item).collect();
        Ok(BrowseBatch { items, next })
    }
}

/// `w` as a [`RemoteItem`]; `None` without an id, a preview (`thumbs.large`) or a full image.
/// Any purity but `sfw` (including a missing one) counts as NSFW.
fn remote_item(w: BrowseWallpaper) -> Option<RemoteItem> {
    if w.id.is_empty() || w.thumbs.large.is_empty() || w.path.is_empty() {
        return None;
    }
    let (width, height) = parse_resolution(&w.resolution).unzip();
    Some(RemoteItem {
        remote_id: sanitize_remote_id(&w.id),
        preview_url: w.thumbs.large,
        image: ImageRef::Direct(w.path),
        source_url: w.url,
        title: String::new(),
        author: w.uploader.username,
        width,
        height,
        nsfw: w.purity != "sfw",
    })
}

/// `"1920x1080"` as `(1920, 1080)`; `None` unless both sides are numbers.
pub(super) fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_resolution_reads_width_and_height() {
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
    }

    #[test]
    fn parse_resolution_rejects_garbage() {
        for text in ["", "1920", "x1080", "1920x", "axb", "-1x2"] {
            assert_eq!(parse_resolution(text), None, "{text}");
        }
    }

    #[test]
    fn cursor_round_trips() {
        let cursor = ApiCursor {
            page: 7,
            seed: "Ab12".to_owned(),
        };
        assert_eq!(ApiCursor::parse(&cursor.encode()).unwrap(), cursor);
    }

    #[test]
    fn cursor_rejects_page_zero_and_missing_separator() {
        for raw in ["0:x", "3", ":x", "a:b"] {
            assert!(
                ApiCursor::parse(&BrowseCursor(raw.to_owned())).is_err(),
                "{raw}"
            );
        }
    }
}
