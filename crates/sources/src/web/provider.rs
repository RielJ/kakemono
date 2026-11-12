//! The `WebPage` provider: reads one pasted link, keeps the wallpaper-sized images on it, and
//! caches what it found per link.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use tokio::time::Instant;
use url::{Host, Url};

use super::cache::{self, Entry, Image};
use super::dimensions::dimensions;
use super::extract::{Candidate, PageKind, feed_candidates, file_name, html_candidates, kind_of};
use super::probe::{probe_all, read_prefix};
use super::{MIN_SHORT_SIDE, PROBE_MAX_BYTES, extract::js_only_site};
use crate::browse::{
    BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem, sanitize_remote_id,
};
use crate::guard::{GuardConfig, GuardError, GuardedHttp};
use crate::http::send_with_retry;
use crate::provider::{Provider, ProviderError, WallpaperResult};

/// Provider name, `[[sources]]` type and error prefix.
const NAME: &str = "web";

/// Most bytes of a page read; the rest is ignored.
pub(crate) const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;

/// The message for a location that is not an `https://` link.
pub(crate) const NEED_HTTPS: &str = "web: enter a link that starts with https://";

/// The message when a page has no candidate images at all.
pub(crate) const NO_IMAGES: &str = "No images on this page. Pages built with JavaScript (Google, Pinterest, Instagram) don't work.";

/// The message when no candidate's size could be read.
pub(crate) const NO_SIZES: &str = "Couldn't read the image sizes on this page.";

/// The message when a direct image link's size could not be read.
pub(crate) const NO_IMAGE_SIZE: &str =
    "Couldn't read the size of this image. Use a JPEG, PNG or WebP link.";

/// The message when the link is neither an image, a page nor a feed.
pub(crate) const NOT_A_PAGE: &str = "web: this link isn't an image, a web page or a feed";

/// Timings of a [`WebPage`]. [`WebTuning::default`] is the only setting for real use; tests
/// shorten it.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebTuning {
    /// How long a page's images are reused before the page is read again.
    pub success_ttl: Duration,
    /// How long a failed read is reused before the page is read again.
    pub failure_ttl: Duration,
    /// Longest one image's size probe may take.
    pub probe_timeout: Duration,
    /// Longest a whole read (page plus size probes) may take before the sizes found so far are
    /// used. Kept under the engine's 15 s Settings check timeout.
    pub budget: Duration,
}

impl Default for WebTuning {
    fn default() -> Self {
        Self {
            success_ttl: Duration::from_secs(6 * 60 * 60),
            failure_ttl: Duration::from_secs(10 * 60),
            probe_timeout: Duration::from_secs(5),
            budget: Duration::from_secs(12),
        }
    }
}

/// Full-size images from one pasted link: a direct image, a static HTML page or an RSS/Atom
/// feed. Only JPEG, PNG and WebP images with a short side of at least 1080 px count.
///
/// The page itself is read with [`GuardedHttp::any_public_host`], so its redirects reach any
/// public host as in a browser (the user typed this link). Everything else (size probes,
/// downloads, previews: [`Provider::media_client`]) goes through a [`GuardedHttp`] with a fixed
/// list: the typed page host, the host the page was finally read from, then the hosts of the
/// page's images.
#[derive(Debug, Clone)]
pub struct WebPage {
    page: Url,
    key: String,
    page_host: String,
    author: String,
    guard: GuardConfig,
    tuning: WebTuning,
}

impl WebPage {
    /// The provider for `location`, an `https://` link.
    ///
    /// # Errors
    ///
    /// [`ProviderError::NotConfigured`]: `web: enter a link that starts with https://` for
    /// anything else, `web: blocked ...` for a link the SSRF guard refuses (an IP address,
    /// `localhost`, a port other than 443, credentials).
    pub fn new(location: &str) -> Result<Self, ProviderError> {
        Self::with_guard(location, GuardConfig::strict())
    }

    /// [`WebPage::new`] with the checks of `guard`; tests pass
    /// [`GuardConfig::permissive_loopback`] to reach a mock server.
    ///
    /// # Errors
    ///
    /// See [`WebPage::new`].
    #[doc(hidden)]
    pub fn with_guard(location: &str, guard: GuardConfig) -> Result<Self, ProviderError> {
        let need_https = || ProviderError::NotConfigured(NEED_HTTPS.into());
        let page = Url::parse(location.trim()).map_err(|_| need_https())?;
        let page_host = host_key(&page).ok_or_else(need_https)?;
        match guard.check_url(&page, &[&page_host]) {
            Ok(()) => {}
            Err(GuardError::Scheme { .. }) => return Err(need_https()),
            Err(err) => return Err(ProviderError::NotConfigured(format!("{NAME}: {err}"))),
        }
        Ok(Self {
            key: page.to_string(),
            author: page.host_str().unwrap_or_default().to_owned(),
            page,
            page_host,
            guard,
            tuning: WebTuning::default(),
        })
    }

    /// This provider with other timings (tests).
    #[doc(hidden)]
    #[must_use]
    pub fn with_tuning(mut self, tuning: WebTuning) -> Self {
        self.tuning = tuning;
        self
    }

    /// The cached images, reading the page when there is no entry or it has expired.
    async fn images(&self) -> Result<Arc<[Image]>, ProviderError> {
        self.refuse_js_site()?;
        if let Some(entry) = cache::get(&self.key) {
            let ttl = match entry.outcome {
                Ok(_) => self.tuning.success_ttl,
                Err(_) => self.tuning.failure_ttl,
            };
            if entry.at.elapsed() < ttl {
                return entry.outcome.map_err(ProviderError::Unusable);
            }
        }
        self.refresh().await
    }

    /// Reads the page now and stores what it gave.
    async fn refresh(&self) -> Result<Arc<[Image]>, ProviderError> {
        self.refuse_js_site()?;
        let (outcome, http) = self.read().await;
        let outcome = outcome.map(Arc::<[Image]>::from);
        if let Some(http) = http
            && is_cacheable(&outcome)
        {
            let stored = outcome
                .as_ref()
                .map(Arc::clone)
                .map_err(ToString::to_string);
            cache::put(
                &self.key,
                Entry {
                    at: std::time::Instant::now(),
                    outcome: stored,
                    http,
                },
            );
        }
        outcome
    }

    /// Fails at once for a site that only works with JavaScript.
    fn refuse_js_site(&self) -> Result<(), ProviderError> {
        match js_only_site(&self.page) {
            Some(site) => Err(ProviderError::Unusable(format!(
                "{site} pages are built with JavaScript and can't be read. Paste a direct image \
                 link, a gallery page or an RSS feed instead."
            ))),
            None => Ok(()),
        }
    }

    /// One read of the page: the usable images or why there are none, and the client the
    /// images may be fetched with (`None` when no client could be built). After a failure that
    /// client allows the typed page host only.
    async fn read(&self) -> (Result<Vec<Image>, ProviderError>, Option<GuardedHttp>) {
        let deadline = Instant::now() + self.tuning.budget;
        match self.read_page(deadline).await {
            Ok((images, http)) => (Ok(images), Some(http)),
            Err(err) => (
                Err(err),
                GuardedHttp::new(self.guard, &[&self.page_host]).ok(),
            ),
        }
    }

    /// Reads the page with a client that follows its redirects to any public host, as a
    /// browser would: the user typed this link. The images get a client with a fixed list
    /// instead ([`WebPage::media_hosts_for`]).
    async fn read_page(
        &self,
        deadline: Instant,
    ) -> Result<(Vec<Image>, GuardedHttp), ProviderError> {
        let page_http = GuardedHttp::any_public_host(self.guard)
            .map_err(|err| ProviderError::request(NAME, err))?;
        page_http
            .check_url(&self.page)
            .map_err(|err| ProviderError::Other(format!("{NAME}: {err}")))?;
        let response = send_with_retry(page_http.client().get(self.page.clone()))
            .await
            .map_err(|err| ProviderError::request(NAME, err))?;
        if !response.status().is_success() {
            return Err(ProviderError::status(NAME, response.status()));
        }
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = read_prefix(response, MAX_PAGE_BYTES, |buf| {
            kind_of(&content_type, buf) == PageKind::Image
                && (dimensions(buf).is_some() || buf.len() >= PROBE_MAX_BYTES)
        })
        .await
        .map_err(|err| ProviderError::request(NAME, err))?;

        let candidates = match kind_of(&content_type, &body) {
            PageKind::Image => {
                let image = self.direct_image(&final_url, &body)?;
                // A download starts at the typed link and follows the same redirects.
                let http = GuardedHttp::new(self.guard, &self.media_hosts_for(&final_url, &[]))
                    .map_err(|err| ProviderError::request(NAME, err))?;
                return Ok((vec![image], http));
            }
            PageKind::Html => html_candidates(&String::from_utf8_lossy(&body), &final_url),
            PageKind::Feed => feed_candidates(&String::from_utf8_lossy(&body), &final_url)
                .map_err(|err| {
                    tracing::debug!(detail = err.detail(), "web: feed rejected");
                    ProviderError::Unusable(err.to_string())
                })?,
            PageKind::Other => return Err(ProviderError::Unusable(NOT_A_PAGE.into())),
        };
        if candidates.is_empty() {
            return Err(ProviderError::Unusable(NO_IMAGES.into()));
        }
        let media_http =
            GuardedHttp::new(self.guard, &self.media_hosts_for(&final_url, &candidates))
                .map_err(|err| ProviderError::request(NAME, err))?;
        let images = self.sized(&media_http, candidates, deadline).await?;
        Ok((images, media_http))
    }

    /// The link itself is an image: its size comes from the bytes already read.
    fn direct_image(&self, url: &Url, prefix: &[u8]) -> Result<Image, ProviderError> {
        let (width, height) =
            dimensions(prefix).ok_or_else(|| ProviderError::Unusable(NO_IMAGE_SIZE.into()))?;
        if width.min(height) < MIN_SHORT_SIDE {
            return Err(ProviderError::Unusable(format!(
                "This image is {width}×{height}; wallpapers need at least {MIN_SHORT_SIDE} px \
                 on the short side."
            )));
        }
        Ok(Image {
            url: self.page.to_string(),
            preview: None,
            title: file_name(url),
            width,
            height,
        })
    }

    /// The typed page host, the host the page was finally read from (`final_url`, after
    /// redirects), then the hosts of every candidate and preview, in page order.
    fn media_hosts_for(&self, final_url: &Url, candidates: &[Candidate]) -> Vec<String> {
        let found = std::iter::once(final_url)
            .chain(
                candidates
                    .iter()
                    .flat_map(|c| std::iter::once(&c.url).chain(c.preview.as_ref())),
            )
            .filter_map(host_key)
            // A single-label host such as `com` would allow a whole TLD as a suffix. `localhost`
            // stays: strict mode refuses it anyway, and tests reach a mock server with it.
            .filter(|h| h.contains('.') || h.contains(':') || h == "localhost");
        std::iter::once(self.page_host.clone())
            .chain(found)
            .collect()
    }

    /// The candidates whose size could be read and is wallpaper-sized, in page order; the
    /// right [`ProviderError::Unusable`] when there are none.
    async fn sized(
        &self,
        http: &GuardedHttp,
        candidates: Vec<Candidate>,
        deadline: Instant,
    ) -> Result<Vec<Image>, ProviderError> {
        // A URL the client refuses (a host past the cap, http in strict mode) is never sent.
        let candidates: Vec<Candidate> = candidates
            .into_iter()
            .filter(|c| http.check_url(&c.url).is_ok())
            .collect();
        let urls: Vec<Url> = candidates.iter().map(|c| c.url.clone()).collect();
        let sizes = probe_all(http.client(), &urls, self.tuning.probe_timeout, deadline).await;

        let mut largest: Option<(u32, u32)> = None;
        let mut images = Vec::new();
        for (candidate, size) in candidates.into_iter().zip(sizes) {
            let Some((width, height)) = size else {
                continue;
            };
            if largest.is_none_or(|(w, h)| {
                u64::from(width) * u64::from(height) > u64::from(w) * u64::from(h)
            }) {
                largest = Some((width, height));
            }
            if width.min(height) < MIN_SHORT_SIDE {
                continue;
            }
            let preview = candidate
                .preview
                .filter(|p| http.check_url(p).is_ok())
                .map(String::from);
            images.push(Image {
                url: candidate.url.to_string(),
                preview,
                title: candidate.title,
                width,
                height,
            });
        }
        match (images.is_empty(), largest) {
            (false, _) => Ok(images),
            (true, None) => Err(ProviderError::Unusable(NO_SIZES.into())),
            (true, Some((w, h))) => Err(ProviderError::Unusable(format!(
                "No full-size images on this page: the largest is {w}×{h}, and wallpapers need \
                 at least {MIN_SHORT_SIDE} px on the short side."
            ))),
        }
    }

    fn result(&self, image: &Image) -> WallpaperResult {
        WallpaperResult {
            source_url: self.page.to_string(),
            image_url: image.url.clone(),
            title: image.title.clone(),
            author: self.author.clone(),
            source: NAME.into(),
        }
    }

    fn results(&self, images: &[Image]) -> Vec<WallpaperResult> {
        images.iter().map(|i| self.result(i)).collect()
    }
}

/// Whether a failed read may be reused: not for transport errors or rate limits, which say
/// nothing about the page.
fn is_cacheable<T>(outcome: &Result<T, ProviderError>) -> bool {
    !matches!(
        outcome,
        Err(ProviderError::Request { .. }
            | ProviderError::RateLimited(_)
            | ProviderError::RateLimitedFor { .. })
    )
}

/// The host as [`GuardConfig::check_url`] matches it: a domain without its trailing dot, an IP
/// address without brackets.
fn host_key(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Domain(name) => {
            let name = name.strip_suffix('.').unwrap_or(name);
            (!name.is_empty()).then(|| name.to_ascii_lowercase())
        }
        Host::Ipv4(ip) => Some(ip.to_string()),
        Host::Ipv6(ip) => Some(ip.to_string()),
    }
}

#[async_trait]
impl Provider for WebPage {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        Ok(self.results(&self.images().await?))
    }

    /// Reads the page again, skipping the cache, and stores the result.
    async fn check(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        Ok(self.results(&self.refresh().await?))
    }

    fn browse_support(&self) -> BrowseSupport {
        BrowseSupport::Finite
    }

    /// The typed page host before the first read; then the typed host, the final page host
    /// (after redirects) and the images' hosts.
    fn allowed_hosts(&self) -> Vec<String> {
        cache::get(&self.key).map_or_else(
            || vec![self.page_host.clone()],
            |entry| entry.http.hosts().to_vec(),
        )
    }

    fn media_client(&self) -> Option<reqwest::Client> {
        cache::get(&self.key).map(|entry| entry.http.client().clone())
    }

    /// Every usable image in one batch; a cursor gets an empty last batch.
    async fn browse(
        &self,
        _query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        if cursor.is_some() {
            return Ok(BrowseBatch::default());
        }
        let images = self.images().await?;
        let items = images
            .iter()
            .map(|image| RemoteItem {
                remote_id: sanitize_remote_id(&image.url),
                preview_url: image.preview.clone().unwrap_or_else(|| image.url.clone()),
                image: ImageRef::Direct(image.url.clone()),
                source_url: self.page.to_string(),
                title: image.title.clone(),
                author: self.author.clone(),
                width: Some(image.width),
                height: Some(image.height),
                nsfw: false,
            })
            .collect();
        Ok(BrowseBatch { items, next: None })
    }

    /// The item's URL when it is one of this page's cached images; an error otherwise, so a
    /// forged item cannot reach a host the page never named.
    async fn resolve_image(&self, item: &RemoteItem) -> Result<String, ProviderError> {
        let ImageRef::Direct(url) = &item.image else {
            return Err(ProviderError::Unsupported(NAME.into()));
        };
        let known = cache::get(&self.key)
            .and_then(|entry| entry.outcome.ok())
            .is_some_and(|images| images.iter().any(|i| &i.url == url));
        if known {
            Ok(url.clone())
        } else {
            Err(ProviderError::Unusable(
                "web: this image is no longer on the page".into(),
            ))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn not_configured(location: &str) -> String {
        match WebPage::new(location).unwrap_err() {
            ProviderError::NotConfigured(msg) => msg,
            other => panic!("want NotConfigured, got {other:?}"),
        }
    }

    #[test]
    fn new_accepts_an_https_link() {
        assert!(WebPage::new("  https://example.com/gallery  ").is_ok());
    }

    #[test]
    fn new_refuses_http() {
        assert_eq!(not_configured("http://example.com/gallery"), NEED_HTTPS);
    }

    #[test]
    fn new_refuses_text_that_is_not_a_link() {
        assert_eq!(not_configured("not a url"), NEED_HTTPS);
    }

    #[test]
    fn new_refuses_an_ip_address() {
        let msg = not_configured("https://127.0.0.1/gallery");
        assert!(msg.contains("IP address"), "{msg}");
    }

    #[test]
    fn new_refuses_localhost() {
        let msg = not_configured("https://localhost/gallery");
        assert!(msg.contains("localhost"), "{msg}");
    }

    #[test]
    fn media_hosts_skip_single_label_hosts() {
        let page = WebPage::new("https://example.com/").unwrap();
        let candidate = |url: &str| Candidate {
            url: Url::parse(url).unwrap(),
            preview: None,
            title: String::new(),
        };
        let hosts = page.media_hosts_for(
            &Url::parse("https://www.example.com/").unwrap(),
            &[
                candidate("https://com/a.jpg"),
                candidate("https://img.example.org/b.jpg"),
            ],
        );
        assert_eq!(hosts, ["example.com", "www.example.com", "img.example.org"]);
    }
}
