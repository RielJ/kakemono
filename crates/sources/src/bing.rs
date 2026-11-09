//! Bing daily wallpaper provider (port of Go `internal/providers/bing`).
//!
//! One request to the HPImageArchive JSON feed returns up to 8 images (one per day). Each image
//! URL is rewritten to its UHD variant. The query is ignored: Bing chooses its own images.

use async_trait::async_trait;
use kakemono_config::schema::BingConfig;
use reqwest::{Client, StatusCode};
use serde::Deserialize;

use crate::http;
use crate::provider::{Provider, ProviderError, WallpaperResult};

/// Production base URL.
pub const BASE_URL: &str = "https://www.bing.com";

/// Path of the image archive feed, appended to the base URL.
const API_PATH: &str = "/HPImageArchive.aspx";

/// Market used when the config leaves `market` empty.
const DEFAULT_MARKET: &str = "en-US";

/// Error text when the feed has no usable image (Go's wording).
const NO_IMAGES: &str = "bing: no images returned from API";

/// Suffix that replaces the resolution token (`_1920x1080.jpg`).
const UHD_SUFFIX: &str = "_UHD.jpg";

/// The Bing provider.
#[derive(Debug, Clone)]
pub struct Bing {
    cfg: BingConfig,
    client: Client,
    base_url: String,
}

impl Bing {
    /// A provider against the production host.
    pub fn new(cfg: BingConfig, client: Client) -> Self {
        Self::with_base_url(cfg, client, BASE_URL)
    }

    /// A provider against `base_url` (`scheme://host`, no path). Tests point it at a mock.
    /// A trailing `/` is removed.
    pub fn with_base_url(cfg: BingConfig, client: Client, base_url: impl Into<String>) -> Self {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        Self {
            cfg,
            client,
            base_url,
        }
    }

    fn market(&self) -> &str {
        if self.cfg.market.is_empty() {
            DEFAULT_MARKET
        } else {
            &self.cfg.market
        }
    }

    /// Turns one feed entry into a result, or `None` when it has no `url`.
    fn result(&self, img: BingImage) -> Option<WallpaperResult> {
        let rel = img.url.filter(|u| !u.is_empty())?;
        let raw = format!("{}{rel}", self.base_url);
        let mut image_url = replace_resolution(&raw);
        // No resolution token in the URL: build the UHD URL from `urlbase` instead.
        if image_url == raw
            && let Some(base) = img.urlbase.filter(|b| !b.is_empty())
        {
            image_url = format!("{}{base}{UHD_SUFFIX}", self.base_url);
        }
        Some(WallpaperResult {
            source_url: img.copyrightlink.unwrap_or_default(),
            image_url,
            title: img.title.unwrap_or_default(),
            author: img.copyright.unwrap_or_default(),
            source: "bing".to_owned(),
        })
    }
}

#[async_trait]
impl Provider for Bing {
    fn name(&self) -> &'static str {
        "bing"
    }

    fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let base = format!("{}{API_PATH}", self.base_url);
        let url = http::url_with_query(
            &base,
            &[
                ("format", "js"),
                ("idx", "0"),
                ("n", "8"),
                ("mkt", self.market()),
            ],
        )
        .map_err(|e| ProviderError::Other(format!("bing: build request: {e}")))?;

        let response = http::get_with_retry(&self.client, url.clone())
            .await
            // reqwest's text repeats the URL; Go's had it once, in the prefix.
            .map_err(|e| ProviderError::request(format!("bing: GET {url}"), e.without_url()))?;
        if response.status() != StatusCode::OK {
            return Err(ProviderError::status("bing", response.status()));
        }

        let feed: BingResponse = http::read_json(response, "bing").await?;
        let results: Vec<WallpaperResult> = feed
            .images
            .unwrap_or_default()
            .into_iter()
            .filter_map(|img| self.result(img))
            .collect();
        if results.is_empty() {
            return Err(ProviderError::Other(NO_IMAGES.to_owned()));
        }
        Ok(results)
    }
}

/// Top-level feed JSON. Missing or `null` fields decode as empty, like Go.
#[derive(Debug, Deserialize)]
struct BingResponse {
    #[serde(default)]
    images: Option<Vec<BingImage>>,
}

#[derive(Debug, Deserialize)]
struct BingImage {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    urlbase: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    copyright: Option<String>,
    #[serde(default)]
    copyrightlink: Option<String>,
}

/// Replaces every resolution token `_<digits>x<digits>.jpg` with `_UHD.jpg`, then drops anything
/// after the first `_UHD.jpg&` (extra query parameters some Bing URLs carry).
///
/// Port of Go `replaceResolution` (regex `_\d+x\d+\.jpg`), without a regex dependency.
pub fn replace_resolution(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(pos) = rest.find('_') {
        out.push_str(&rest[..pos]);
        let candidate = &rest[pos..];
        match resolution_token_len(candidate) {
            Some(len) => {
                out.push_str(UHD_SUFFIX);
                rest = &candidate[len..];
            }
            None => {
                out.push('_');
                rest = &candidate[1..];
            }
        }
    }
    out.push_str(rest);

    let marker = "_UHD.jpg&";
    if let Some(idx) = out.find(marker) {
        out.truncate(idx + UHD_SUFFIX.len());
    }
    out
}

/// Byte length of a `_<digits>x<digits>.jpg` token at the start of `s`, if there is one.
fn resolution_token_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let digits = |from: usize| {
        bytes
            .get(from..)
            .map_or(0, |b| b.iter().take_while(|c| c.is_ascii_digit()).count())
    };
    if bytes.first() != Some(&b'_') {
        return None;
    }
    let width = digits(1);
    if width == 0 || bytes.get(1 + width) != Some(&b'x') {
        return None;
    }
    let height_at = 2 + width;
    let height = digits(height_at);
    if height == 0 {
        return None;
    }
    let ext_at = height_at + height;
    if bytes.get(ext_at..ext_at + 4) == Some(b".jpg") {
        Some(ext_at + 4)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_resolution_and_strips_trailing_query() {
        assert_eq!(
            replace_resolution("https://b/th?id=OHR.Pic_1920x1080.jpg&rf=LaDigue.jpg"),
            "https://b/th?id=OHR.Pic_UHD.jpg"
        );
        assert_eq!(
            replace_resolution("https://b/th?id=OHR.Pic_1920x1080.jpg"),
            "https://b/th?id=OHR.Pic_UHD.jpg"
        );
    }

    #[test]
    fn leaves_urls_without_a_token_alone() {
        for url in [
            "https://b/th?id=OHR.Pic.jpg",
            "https://b/a_b_c.jpg",
            "https://b/a_1920x.jpg",
            "https://b/a_x1080.jpg",
            "https://b/a_1920x1080.png",
            "https://b/a_1920x1080.jp",
            "_",
            "",
        ] {
            assert_eq!(replace_resolution(url), url, "{url}");
        }
    }

    #[test]
    fn replaces_every_token_like_regex_replace_all() {
        assert_eq!(
            replace_resolution("x_1x2.jpg_y_3x4.jpg"),
            "x_UHD.jpg_y_UHD.jpg"
        );
        // A failed match at one `_` still finds the token at the next.
        assert_eq!(replace_resolution("a__10x20.jpg"), "a__UHD.jpg");
        assert_eq!(replace_resolution("ü_1x1.jpgé"), "ü_UHD.jpgé");
    }
}
