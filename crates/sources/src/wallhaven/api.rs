//! Wallhaven API mode: `GET {base}/api/v1/search?apikey=..&categories=..&purity=..&q=..&sorting=..`.
//! Port of Go `fetchAPI`.

use serde::Deserialize;

use super::Wallhaven;
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
