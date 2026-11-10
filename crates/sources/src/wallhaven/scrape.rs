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

use std::sync::LazyLock;
use std::time::Duration;

use rand::RngExt;
use rand::seq::SliceRandom;
use scraper::{Html, Selector};

use super::{GetError, Wallhaven};
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
