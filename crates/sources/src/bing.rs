//! Bing daily wallpaper provider (port of Go `internal/providers/bing`).
//!
//! One request to the HPImageArchive JSON feed returns up to 8 images (one per day). Each image
//! URL is rewritten to its UHD variant. The query is ignored: Bing chooses its own images.
//!
//! Browsing reads the same feed twice, at `idx=0` and `idx=8`, then ends: the archive holds only
//! about 16 images per market. Previews use the small `<urlbase>_400x240.jpg` rendition.

use async_trait::async_trait;
use kakemono_config::schema::BingConfig;
use reqwest::Client;
use serde::Deserialize;

use crate::browse::{
    BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem, sanitize_remote_id,
};
use crate::de;
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

/// Images per feed request (`n`); Bing serves at most 8.
const FEED_LEN: usize = 8;

/// `idx` of the second and last browse batch. Bing keeps about 16 days, so there is no third.
const LAST_BROWSE_IDX: &str = "8";

/// Suffix appended to `urlbase` for a browse preview (a 400×240 JPEG).
const PREVIEW_SUFFIX: &str = "_400x240.jpg";

/// Hosts Bing serves feed images from. `bing.com` also covers `www.bing.com`; both are listed
/// to match the plan's allowlist.
const MEDIA_HOSTS: &[&str] = &["www.bing.com", "bing.com"];

/// The Bing provider.
#[derive(Debug, Clone)]
pub struct Bing {
    cfg: BingConfig,
    client: Client,
    base_url: url::Url,
}

impl Bing {
    /// A provider against the production host ([`BASE_URL`]).
    #[must_use]
    pub fn new(cfg: BingConfig, client: Client) -> Self {
        Self::with_base_url(cfg, client, http::default_base_url(BASE_URL))
    }

    /// A provider against `base_url` (`scheme://host`). Tests point it at a mock. Image URLs
    /// in the feed are relative to it.
    #[must_use]
    pub fn with_base_url(cfg: BingConfig, client: Client, base_url: url::Url) -> Self {
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

    /// The base URL without its trailing `/`. Feed paths are appended to it.
    fn origin(&self) -> &str {
        self.base_url.as_str().trim_end_matches('/')
    }

    /// The feed URL for the batch starting at `idx` (0 is today).
    fn feed_url(&self, idx: &str) -> url::Url {
        let n = FEED_LEN.to_string();
        http::endpoint(
            &self.base_url,
            API_PATH,
            &[
                ("format", "js"),
                ("idx", idx),
                ("n", &n),
                ("mkt", self.market()),
            ],
        )
    }

    /// The UHD image URL of one feed entry, or `None` when it has no `url`.
    fn image_url(&self, img: &BingImage) -> Option<String> {
        if img.url.is_empty() {
            return None;
        }
        // Plain concatenation, like Go: the feed's paths stay byte for byte as sent.
        let origin = self.origin();
        let raw = format!("{origin}{}", img.url);
        let image_url = replace_resolution(&raw);
        // No resolution token in the URL: build the UHD URL from `urlbase` instead.
        if image_url == raw && !img.urlbase.is_empty() {
            return Some(format!("{origin}{}{UHD_SUFFIX}", img.urlbase));
        }
        Some(image_url)
    }

    /// Turns one feed entry into a result, or `None` when it has no `url`.
    fn result(&self, img: BingImage) -> Option<WallpaperResult> {
        let image_url = self.image_url(&img)?;
        Some(WallpaperResult {
            source_url: img.copyrightlink,
            image_url,
            title: img.title,
            author: img.copyright,
            source: "bing".to_owned(),
        })
    }

    /// Turns one feed entry into a browse item, or `None` when it has no `url`.
    ///
    /// The preview is the `_400x240` rendition of `urlbase`. Without a `urlbase` the feed's own
    /// (1920×1080) `url` is the preview and identifies the item.
    fn item(&self, img: BingImage) -> Option<RemoteItem> {
        let full = self.image_url(&img)?;
        let (id, preview_url) = if img.urlbase.is_empty() {
            (img.url.as_str(), format!("{}{}", self.origin(), img.url))
        } else {
            let preview = format!("{}{}{PREVIEW_SUFFIX}", self.origin(), img.urlbase);
            (img.urlbase.as_str(), preview)
        };
        Some(RemoteItem {
            remote_id: sanitize_remote_id(id),
            preview_url,
            image: ImageRef::Direct(full),
            source_url: img.copyrightlink,
            title: img.title,
            author: img.copyright,
            width: None,
            height: None,
            nsfw: false,
        })
    }
}

#[async_trait]
impl Provider for Bing {
    fn name(&self) -> &'static str {
        "bing"
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let url = self.feed_url("0");

        let response = http::get_with_retry(&self.client, url.clone())
            .await
            // reqwest's text repeats the URL; Go's had it once, in the prefix.
            .map_err(|e| ProviderError::request(format!("bing: GET {url}"), e.without_url()))?;
        let response = http::check_status(response).map_err(|s| s.into_provider_error("bing"))?;

        let feed: BingResponse = http::read_json(response, "bing").await?;
        let results: Vec<WallpaperResult> = feed
            .images
            .into_iter()
            .filter_map(|img| self.result(img))
            .collect();
        if results.is_empty() {
            return Err(ProviderError::Other(NO_IMAGES.to_owned()));
        }
        Ok(results)
    }

    fn browse_support(&self) -> BrowseSupport {
        BrowseSupport::Finite
    }

    fn media_hosts(&self) -> &'static [&'static str] {
        MEDIA_HOSTS
    }

    /// `idx=0`, then `idx=8` (cursor `"8"`), then the end. A first batch shorter than 8
    /// images, or an empty feed, also ends the archive: unlike `fetch`, no images is not an
    /// error here.
    async fn browse(
        &self,
        _query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        let idx = match cursor {
            None => "0",
            Some(BrowseCursor(c)) if c == LAST_BROWSE_IDX => LAST_BROWSE_IDX,
            Some(BrowseCursor(c)) => {
                return Err(ProviderError::Other(format!(
                    "bing: invalid browse cursor {c:?}"
                )));
            }
        };

        let response = http::send_fail_fast(self.client.get(self.feed_url(idx)), "bing").await?;
        let response = http::check_status(response).map_err(|s| s.into_provider_error("bing"))?;
        let feed: BingResponse = http::read_json(response, "bing").await?;

        let full_batch = feed.images.len() >= FEED_LEN;
        let items: Vec<RemoteItem> = feed
            .images
            .into_iter()
            .filter_map(|img| self.item(img))
            .collect();
        let next = (idx != LAST_BROWSE_IDX && full_batch)
            .then(|| BrowseCursor(LAST_BROWSE_IDX.to_owned()));
        Ok(BrowseBatch { items, next })
    }
}

/// Top-level feed JSON. Missing or `null` fields decode as empty, like Go.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BingResponse {
    #[serde(deserialize_with = "de::null_default")]
    images: Vec<BingImage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BingImage {
    #[serde(deserialize_with = "de::null_default")]
    url: String,
    #[serde(deserialize_with = "de::null_default")]
    urlbase: String,
    #[serde(deserialize_with = "de::null_default")]
    title: String,
    #[serde(deserialize_with = "de::null_default")]
    copyright: String,
    #[serde(deserialize_with = "de::null_default")]
    copyrightlink: String,
}

/// Replaces every resolution token `_<digits>x<digits>.jpg` with `_UHD.jpg`, then drops anything
/// after the first `_UHD.jpg&` (extra query parameters some Bing URLs carry).
///
/// Port of Go `replaceResolution` (regex `_\d+x\d+\.jpg`), without a regex dependency.
pub(crate) fn replace_resolution(url: &str) -> String {
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
