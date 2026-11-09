//! Wallhaven API mode: `GET {base}/api/v1/search?apikey=..&categories=..&purity=..&q=..&sorting=..`.
//! Port of Go `fetchAPI`.

use serde::Deserialize;

use super::Wallhaven;
use crate::http;
use crate::provider::{ProviderError, WallpaperResult};

/// Error-text prefix, as in Go.
const CONTEXT: &str = "wallhaven api";

/// Path of the search endpoint under the base URL.
pub const SEARCH_PATH: &str = "/api/v1/search";

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<ApiWallpaper>,
}

#[derive(Debug, Deserialize)]
struct ApiWallpaper {
    /// Page URL.
    #[serde(default)]
    url: String,
    /// Direct image URL.
    #[serde(default)]
    path: String,
    #[serde(default)]
    uploader: Option<Uploader>,
}

#[derive(Debug, Deserialize)]
struct Uploader {
    #[serde(default)]
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
        let url = http::url_with_query(&format!("{}{SEARCH_PATH}", self.base_url), &params)
            .map_err(|e| ProviderError::Other(format!("{CONTEXT}: build request: {e}")))?;

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
                author: w.uploader.map(|u| u.username).unwrap_or_default(),
                source: "wallhaven".to_owned(),
            })
            .collect())
    }
}
