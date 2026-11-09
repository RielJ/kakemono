//! Reddit provider: image posts from the hot listing of one random configured subreddit.
//!
//! Port of Go `internal/providers/reddit/reddit.go`. Go read the post `url` only (no
//! `preview` images), so there is nothing to HTML-unescape.

use async_trait::async_trait;
use kakemono_config::schema::RedditConfig;
use rand::seq::IndexedRandom;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Deserializer};

use crate::http;
use crate::provider::{Provider, ProviderError, WallpaperResult};

/// Production API origin.
pub const BASE_URL: &str = "https://www.reddit.com";

/// Origin used for post pages (`source_url`), independent of the API base URL.
const PAGE_ORIGIN: &str = "https://www.reddit.com";

/// Posts with a lower score are skipped.
pub const MIN_SCORE: i64 = 100;

/// Posts requested per listing.
const LIMIT: &str = "100";

/// The Reddit provider.
#[derive(Debug, Clone)]
pub struct Reddit {
    cfg: RedditConfig,
    client: Client,
    base_url: String,
}

impl Reddit {
    /// Provider against [`BASE_URL`].
    pub fn new(cfg: RedditConfig, client: Client) -> Self {
        Self::with_base_url(cfg, client, BASE_URL)
    }

    /// Provider against another origin (`scheme://host`, no trailing slash), for tests.
    pub fn with_base_url(cfg: RedditConfig, client: Client, base_url: impl Into<String>) -> Self {
        Self {
            cfg,
            client,
            base_url: base_url.into(),
        }
    }

    /// `{base}/r/{subreddit}/hot.json?limit=100`.
    fn feed_url(&self, subreddit: &str) -> Result<url::Url, ProviderError> {
        let base = format!("{}/r/{subreddit}/hot.json", self.base_url);
        http::url_with_query(&base, &[("limit", LIMIT)])
            .map_err(|e| ProviderError::Other(format!("reddit: request error: {e}")))
    }
}

#[async_trait]
impl Provider for Reddit {
    fn name(&self) -> &'static str {
        "reddit"
    }

    /// Enabled and at least one subreddit configured (as in Go).
    fn enabled(&self) -> bool {
        self.cfg.enabled && !self.cfg.subreddits.is_empty()
    }

    /// Fetches the hot listing of a random configured subreddit; `query` is ignored.
    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let subreddit = self
            .cfg
            .subreddits
            .choose(&mut rand::rng())
            .ok_or_else(|| ProviderError::Other("reddit: no subreddits configured".into()))?
            .clone();

        let url = self.feed_url(&subreddit)?;
        let response = http::get_with_retry(&self.client, url)
            .await
            .map_err(|e| ProviderError::request("reddit: HTTP error", e))?;
        if response.status() != StatusCode::OK {
            return Err(ProviderError::status("reddit", response.status()));
        }
        let listing: Listing = http::read_json(response, "reddit").await?;
        Ok(filter_image_posts(listing, &subreddit))
    }
}

/// Image posts with a score of at least [`MIN_SCORE`], in listing order.
fn filter_image_posts(listing: Listing, subreddit: &str) -> Vec<WallpaperResult> {
    listing
        .data
        .children
        .into_iter()
        .map(|child| child.data)
        .filter(|post| post.score >= MIN_SCORE)
        .filter_map(|post| {
            let image_url = resolve_image_url(&post.url)?;
            let source_url = if post.permalink.is_empty() {
                format!("{PAGE_ORIGIN}/r/{subreddit}/comments/{}", post.id)
            } else if post.permalink.starts_with("http") {
                post.permalink
            } else {
                format!("{PAGE_ORIGIN}{}", post.permalink)
            };
            Some(WallpaperResult {
                source_url,
                image_url,
                title: post.title,
                author: post.author,
                source: "reddit".into(),
            })
        })
        .collect()
}

/// A direct image URL for a post URL, or `None` when the post does not link to a supported
/// image. Direct `.jpg/.jpeg/.png/.webp` links pass through; `imgur.com/<id>` pages become
/// `https://i.imgur.com/<id>.jpg`; other imgur links (not albums) pass through.
pub fn resolve_image_url(post_url: &str) -> Option<String> {
    let lower = post_url.to_lowercase();

    if [".jpg", ".jpeg", ".png", ".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        return Some(post_url.to_owned());
    }

    if lower.contains("imgur.com/") && !lower.contains("imgur.com/a/") {
        if !lower.contains("i.imgur.com") {
            let id = post_url.rsplit('/').next().unwrap_or_default();
            if !id.is_empty() && !id.contains('.') {
                return Some(format!("https://i.imgur.com/{id}.jpg"));
            }
        }
        return Some(post_url.to_owned());
    }

    None
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Listing {
    #[serde(deserialize_with = "null_default")]
    data: ListingData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListingData {
    #[serde(deserialize_with = "null_default")]
    children: Vec<Child>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Child {
    #[serde(deserialize_with = "null_default")]
    data: Post,
}

/// Missing or `null` fields decode to their zero value, like Go's `encoding/json`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Post {
    #[serde(deserialize_with = "null_default")]
    id: String,
    #[serde(deserialize_with = "null_default")]
    title: String,
    #[serde(deserialize_with = "null_default")]
    author: String,
    #[serde(deserialize_with = "null_default")]
    url: String,
    #[serde(deserialize_with = "null_default")]
    permalink: String,
    #[serde(deserialize_with = "null_default")]
    score: i64,
}

/// Decodes `null` as `T::default()`.
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_image_extensions_pass_through_any_case() {
        for url in [
            "https://e.com/a.jpg",
            "https://e.com/a.JPEG",
            "https://e.com/a.png",
            "https://e.com/a.webp",
        ] {
            assert_eq!(resolve_image_url(url).as_deref(), Some(url));
        }
    }

    #[test]
    fn non_images_and_gifs_are_rejected() {
        assert_eq!(resolve_image_url("https://e.com/article"), None);
        assert_eq!(resolve_image_url("https://e.com/anim.gif"), None);
        assert_eq!(resolve_image_url(""), None);
    }

    #[test]
    fn imgur_rules_match_go() {
        assert_eq!(
            resolve_image_url("https://imgur.com/abc123").as_deref(),
            Some("https://i.imgur.com/abc123.jpg")
        );
        // Albums are not images.
        assert_eq!(resolve_image_url("https://imgur.com/a/xyz"), None);
        // i.imgur.com links without a known extension pass through.
        assert_eq!(
            resolve_image_url("https://i.imgur.com/abc.gifv").as_deref(),
            Some("https://i.imgur.com/abc.gifv")
        );
    }

    #[test]
    fn permalink_variants() {
        let listing: Listing = serde_json::from_str(
            r#"{"data":{"children":[
                {"data":{"id":"a","permalink":"","score":100,"url":"https://e.com/a.jpg","title":null}},
                {"data":{"id":"b","permalink":"https://old.reddit.com/r/x/comments/b/","score":100,"url":"https://e.com/b.jpg"}},
                {"data":{"id":"c","score":99,"url":"https://e.com/c.jpg"}}
            ]}}"#,
        )
        .unwrap();
        let results = filter_image_posts(listing, "pics");
        let urls: Vec<_> = results.iter().map(|r| r.source_url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://www.reddit.com/r/pics/comments/a",
                "https://old.reddit.com/r/x/comments/b/"
            ]
        );
        assert_eq!(results[0].title, "");
    }
}
