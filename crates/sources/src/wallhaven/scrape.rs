//! Wallhaven scrape mode (no API key). Port of Go `fetchScrape` / `scrapeWallpaperPage`.
//!
//! 1. `GET {base}/search?categories=..&purity=..&q=..&sorting=..` and collect the `href` of every
//!    `figure.thumb a.preview` (the wallpaper page links).
//! 2. Shuffle them and keep [`MAX_PAGES`].
//! 3. For each page: wait `scrape_delay + random(0..scrape_delay)`, `GET` it and take
//!    `img#wallpaper[src]` as the image URL. A page that fails is skipped.
//!
//! Like Go, the full-image URL comes from the wallpaper page, not from the thumbnail id: the
//! extension (`.jpg`/`.png`) is only known from the page.
//!
//! Browse fetches only the search page (`&page=N`, cursor `"N"`) and reads each
//! `figure.thumb`: its id, the lazy thumbnail (`img[data-src]`, else `img[src]`), the
//! `a.preview` page link and the purity class. Items are [`ImageRef::Page`]; `resolve_image`
//! fetches one wallpaper page (after `scrape_delay`) when the user picks it.

use std::sync::LazyLock;
use std::time::Duration;

use rand::RngExt;
use rand::seq::SliceRandom;
use scraper::{Html, Selector};

use super::api::parse_resolution;
use super::{GetError, Wallhaven};
use crate::browse::{BrowseBatch, BrowseCursor, ImageRef, RemoteItem, sanitize_remote_id};
use crate::http;
use crate::provider::{ProviderError, WallpaperResult};

/// Most wallpaper pages fetched per scrape (Go: `maxScrapePages`).
pub const MAX_PAGES: usize = 3;

/// Path of the HTML search page under the base URL.
pub(crate) const SEARCH_PATH: &str = "/search";

/// Error-text prefix, as in Go.
const CONTEXT: &str = "wallhaven scrape";

/// Thumbnail links on the search page (same selector as Go).
static PREVIEW_LINKS: LazyLock<Selector> = LazyLock::new(|| selector("figure.thumb a.preview"));

/// The full-size image on a wallpaper page (same selector as Go).
static WALLPAPER_IMG: LazyLock<Selector> = LazyLock::new(|| selector("img#wallpaper"));

/// One result on the search page.
static THUMB: LazyLock<Selector> = LazyLock::new(|| selector("figure.thumb"));

/// The thumbnail inside a [`THUMB`].
static THUMB_IMG: LazyLock<Selector> = LazyLock::new(|| selector("img"));

/// The wallpaper page link inside a [`THUMB`].
static THUMB_PREVIEW: LazyLock<Selector> = LazyLock::new(|| selector("a.preview"));

/// `"1920 x 1080"` inside a [`THUMB`].
static THUMB_RES: LazyLock<Selector> = LazyLock::new(|| selector("span.wall-res"));

/// Parses a constant selector. Both constants are covered by tests, so this cannot fail at
/// runtime.
#[allow(clippy::expect_used)]
fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("constant CSS selector is valid")
}

impl Wallhaven {
    /// Scrape mode. Errors only when the search page fails; failed wallpaper pages are skipped.
    pub(super) async fn fetch_scrape(
        &self,
        query: &str,
    ) -> Result<Vec<WallpaperResult>, ProviderError> {
        let search_url = http::endpoint(&self.base_url, SEARCH_PATH, &self.search_params(query));

        let context = format!("{CONTEXT}: search page");
        let response = self
            .get_ok(search_url.clone())
            .await
            .map_err(|e| e.into_provider_error(&context))?;
        let html = http::read_text(response, &context).await?;

        let mut pages = preview_links(&html, &search_url);
        // `ThreadRng` is not `Send`: keep it out of any `.await`.
        let delays: Vec<Duration> = {
            let mut rng = rand::rng();
            pages.shuffle(&mut rng);
            pages.truncate(MAX_PAGES);
            pages
                .iter()
                .map(|_| jittered(self.scrape_delay, &mut rng))
                .collect()
        };

        let mut results = Vec::with_capacity(pages.len());
        for (page, delay) in pages.into_iter().zip(delays) {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            match self.wallpaper_image(&page).await {
                Ok(image_url) => results.push(WallpaperResult {
                    source_url: page.to_string(),
                    image_url,
                    title: String::new(),
                    author: String::new(),
                    source: "wallhaven".to_owned(),
                }),
                Err(err) => tracing::debug!(page = %page, %err, "wallhaven: skipping page"),
            }
        }
        Ok(results)
    }

    /// Fetches one wallpaper page and returns `img#wallpaper[src]`.
    async fn wallpaper_image(&self, page: &url::Url) -> Result<String, PageError> {
        let response = self.get_ok(page.clone()).await.map_err(PageError::Get)?;
        let html = response.text().await.map_err(PageError::Body)?;
        wallpaper_src(&html).ok_or(PageError::NoImage)
    }
}

impl Wallhaven {
    /// Scrape-mode browse: one `GET` of search page `N`, no wallpaper pages, no delay, no retry.
    pub(super) async fn browse_scrape(
        &self,
        query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        let page = cursor.map_or(Ok(1), parse_cursor)?;
        let page_text = page.to_string();
        let mut params = self.search_params(query);
        params.push(("page", page_text.as_str()));
        let search_url = http::endpoint(&self.base_url, SEARCH_PATH, &params);

        let context = format!("{CONTEXT}: search page");
        let response = self.get_ok_fail_fast(search_url.clone(), &context).await?;
        let html = http::read_text(response, &context).await?;

        let (thumbs, items) = browse_items(&html, &search_url);
        // An empty page is past the end. A page of only unreadable thumbs is not.
        let next = page
            .checked_add(1)
            .filter(|_| thumbs > 0)
            .map(|n| BrowseCursor(n.to_string()));
        Ok(BrowseBatch { items, next })
    }

    /// The full-image URL behind a wallpaper page from [`Wallhaven::browse_scrape`]: waits
    /// `scrape_delay` (plus jitter), then `GET`s the page once and takes `img#wallpaper[src]`.
    ///
    /// Only pages on the configured site (same origin as the base URL) are fetched.
    pub(super) async fn resolve_page(&self, page: &str) -> Result<String, ProviderError> {
        let page = url::Url::parse(page)
            .ok()
            .filter(|url| url.origin() == self.base_url.origin())
            .ok_or_else(|| {
                ProviderError::Other(format!(
                    "{CONTEXT}: wallpaper page is not on the wallhaven site"
                ))
            })?;
        let delay = jittered(self.scrape_delay, &mut rand::rng());
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        let context = format!("{CONTEXT}: wallpaper page");
        let response = self.get_ok_fail_fast(page, &context).await?;
        let html = http::read_text(response, &context).await?;
        wallpaper_src(&html)
            .ok_or_else(|| ProviderError::Other(format!("{context}: {}", PageError::NoImage)))
    }
}

/// The page number in a scrape cursor (`"N"`, `N >= 1`).
fn parse_cursor(cursor: &BrowseCursor) -> Result<u64, ProviderError> {
    cursor
        .0
        .parse::<u64>()
        .ok()
        .filter(|&page| page >= 1)
        .ok_or_else(|| ProviderError::Other(format!("{CONTEXT}: invalid browse cursor")))
}

/// The number of `figure.thumb`s on a search page, and the ones that make a [`RemoteItem`].
///
/// A thumb needs a `data-wallpaper-id`, a thumbnail (`data-src`, else `src`; Wallhaven loads
/// thumbs lazily and leaves `src` empty) and an `a.preview` link. Links resolve against `base`.
/// Only the `thumb-sfw` class counts as SFW; `thumb-sketchy`, `thumb-nsfw` or none is NSFW.
fn browse_items(html: &str, base: &url::Url) -> (usize, Vec<RemoteItem>) {
    let doc = Html::parse_document(html);
    let mut thumbs = 0;
    let items = doc
        .select(&THUMB)
        .inspect(|_| thumbs += 1)
        .filter_map(|figure| {
            let id = figure
                .value()
                .attr("data-wallpaper-id")
                .filter(|id| !id.is_empty())?;
            let img = figure.select(&THUMB_IMG).next()?;
            let preview = ["data-src", "src"]
                .into_iter()
                .filter_map(|attr| img.value().attr(attr))
                .find(|src| !src.is_empty())
                .and_then(|src| base.join(src).ok())?;
            let page = figure
                .select(&THUMB_PREVIEW)
                .next()
                .and_then(|a| a.value().attr("href"))
                .filter(|href| !href.is_empty())
                .and_then(|href| base.join(href).ok())?;
            let resolution = figure
                .select(&THUMB_RES)
                .next()
                .map(|span| span.text().collect::<String>())
                .and_then(|text| parse_resolution(&text));
            let (width, height) = resolution.unzip();
            let sfw = figure.value().classes().any(|class| class == "thumb-sfw");
            Some(RemoteItem {
                remote_id: sanitize_remote_id(id),
                preview_url: preview.into(),
                image: ImageRef::Page(page.to_string()),
                source_url: page.into(),
                title: String::new(),
                author: String::new(),
                width,
                height,
                nsfw: !sfw,
            })
        })
        .collect();
    (thumbs, items)
}

/// `delay + random(0..delay)` (Go: `scrapeDelay + rand.Intn(scrapeDelay)`); zero stays zero.
fn jittered(delay: Duration, rng: &mut impl RngExt) -> Duration {
    if delay.is_zero() {
        return Duration::ZERO;
    }
    let max_nanos = u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX);
    delay + Duration::from_nanos(rng.random_range(0..max_nanos))
}

/// The non-empty `href`s of `figure.thumb a.preview`, resolved against `base` (absolute links,
/// as Wallhaven serves them, are kept as is). Links that do not resolve are dropped.
fn preview_links(html: &str, base: &url::Url) -> Vec<url::Url> {
    Html::parse_document(html)
        .select(&PREVIEW_LINKS)
        .filter_map(|a| a.value().attr("href"))
        .filter(|href| !href.is_empty())
        .filter_map(|href| base.join(href).ok())
        .collect()
}

/// `img#wallpaper[src]` when present and non-empty.
fn wallpaper_src(html: &str) -> Option<String> {
    Html::parse_document(html)
        .select(&WALLPAPER_IMG)
        .next()
        .and_then(|img| img.value().attr("src"))
        .filter(|src| !src.is_empty())
        .map(str::to_owned)
}

/// Why one wallpaper page was skipped (logged, never returned).
#[derive(Debug)]
enum PageError {
    Get(GetError),
    Body(reqwest::Error),
    NoImage,
}

impl std::fmt::Display for PageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Get(e) => write!(f, "{e}"),
            Self::Body(e) => write!(f, "read body: {e}"),
            Self::NoImage => f.write_str("img#wallpaper not found"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_parse() {
        LazyLock::force(&PREVIEW_LINKS);
        LazyLock::force(&WALLPAPER_IMG);
        LazyLock::force(&THUMB);
        LazyLock::force(&THUMB_IMG);
        LazyLock::force(&THUMB_PREVIEW);
        LazyLock::force(&THUMB_RES);
    }

    #[test]
    fn browse_items_count_thumbs_that_make_no_item() {
        let base = url::Url::parse("https://wallhaven.cc/search?page=1").unwrap();
        let html = r#"<ul>
          <li><figure class="thumb thumb-sfw" data-wallpaper-id="a1"><img data-src="/t/a1.jpg" src=""><a class="preview" href="/w/a1"></a></figure></li>
          <li><figure class="thumb thumb-sfw"><img data-src="/t/b2.jpg"><a class="preview" href="/w/b2"></a></figure></li>
          <li><figure class="thumb thumb-sfw" data-wallpaper-id="c3"><img data-src="" src=""><a class="preview" href="/w/c3"></a></figure></li>
          <li><figure class="thumb thumb-sfw" data-wallpaper-id="d4"><img data-src="/t/d4.jpg"></figure></li>
        </ul>"#;
        let (thumbs, items) = browse_items(html, &base);
        assert_eq!(thumbs, 4);
        let pages: Vec<(&str, &str, &ImageRef)> = items
            .iter()
            .map(|i| (i.remote_id.as_str(), i.preview_url.as_str(), &i.image))
            .collect();
        assert_eq!(
            pages,
            [(
                "a1",
                "https://wallhaven.cc/t/a1.jpg",
                &ImageRef::Page("https://wallhaven.cc/w/a1".to_owned())
            )]
        );
    }

    #[test]
    fn browse_items_without_a_purity_class_are_nsfw() {
        let base = url::Url::parse("https://wallhaven.cc/search").unwrap();
        let html = r#"<figure class="thumb" data-wallpaper-id="a1"><img src="/t.jpg"><a class="preview" href="/w/a1"></a></figure>"#;
        let (_, items) = browse_items(html, &base);
        assert!(items[0].nsfw);
    }

    #[test]
    fn parse_cursor_rejects_zero_and_text() {
        for raw in ["0", "", "-1", "2:seed", "x"] {
            assert!(
                parse_cursor(&BrowseCursor(raw.to_owned())).is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn preview_links_keep_order_skip_empty_and_resolve_relative() {
        let base = url::Url::parse("https://wallhaven.cc/search?q=x").unwrap();
        let html = r#"<ul>
          <li><figure class="thumb"><a class="preview" href="https://wallhaven.cc/w/a1">x</a></figure></li>
          <li><figure class="thumb"><a class="preview" href="">x</a></figure></li>
          <li><figure class="thumb"><a class="preview">x</a></figure></li>
          <li><figure class="thumb"><a class="preview" href="/w/b2">x</a></figure></li>
          <li><figure class="other"><a class="preview" href="https://wallhaven.cc/w/c3">x</a></figure></li>
          <li><figure class="thumb"><a class="jsAnchor" href="https://wallhaven.cc/w/d4">x</a></figure></li>
        </ul>"#;
        let links: Vec<String> = preview_links(html, &base)
            .iter()
            .map(url::Url::to_string)
            .collect();
        assert_eq!(
            links,
            ["https://wallhaven.cc/w/a1", "https://wallhaven.cc/w/b2"]
        );
    }

    #[test]
    fn wallpaper_src_needs_the_wallpaper_id() {
        assert_eq!(
            wallpaper_src(r#"<img id="wallpaper" src="https://w.wallhaven.cc/full/a.png">"#)
                .as_deref(),
            Some("https://w.wallhaven.cc/full/a.png")
        );
        assert_eq!(wallpaper_src(r#"<img id="wallpaper" src="">"#), None);
        assert_eq!(wallpaper_src(r#"<img id="other" src="x.jpg">"#), None);
        assert_eq!(wallpaper_src("<p>nothing</p>"), None);
    }

    #[test]
    fn jitter_stays_in_delay_to_twice_delay() {
        let mut rng = rand::rng();
        assert_eq!(jittered(Duration::ZERO, &mut rng), Duration::ZERO);
        let delay = Duration::from_millis(1000);
        for _ in 0..200 {
            let d = jittered(delay, &mut rng);
            assert!(d >= delay && d < 2 * delay, "{d:?}");
        }
    }
}
