//! Reddit provider: image posts from the hot listing of one subreddit (one `[[sources]]` row).
//!
//! Port of Go `internal/providers/reddit/reddit.go`. `fetch` reads the post `url` only, like Go.
//! `browse` also reads `preview` images, whose URLs Reddit HTML-escapes (`&amp;`) unless asked
//! for `raw_json=1`; browse asks and still unescapes.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::browse::{
    BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem, sanitize_remote_id,
};
use crate::de;
use crate::guard;
use crate::http;
use crate::provider::{Provider, ProviderError, WallpaperResult};

/// Production API origin.
pub const BASE_URL: &str = "https://www.reddit.com";

/// Origin used for post pages (`source_url`), independent of the API base URL.
const PAGE_ORIGIN: &str = "https://www.reddit.com";

/// Posts with a lower score are skipped.
pub(crate) const MIN_SCORE: i64 = 100;

/// Posts requested per listing by `fetch`.
const LIMIT: &str = "100";

/// Posts requested per listing by `browse`.
const BROWSE_LIMIT: &str = "25";

/// Narrowest preview resolution `browse` uses, in pixels.
const MIN_PREVIEW_WIDTH: u32 = 320;

/// Hosts of browse previews and full images (see [`Provider::media_hosts`]). Posts whose image
/// or preview is elsewhere are dropped from browse.
const MEDIA_HOSTS: &[&str] = &["i.redd.it", "preview.redd.it", "i.imgur.com"];

/// A valid subreddit name: 1 to 50 of `A-Z a-z 0-9 _ +` (`+` joins subreddits, as Reddit
/// allows). The name goes into the request path, so nothing else can be held here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Subreddit(String);

/// Why a string is not a [`Subreddit`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a subreddit name")]
pub struct InvalidSubreddit(pub String);

impl Subreddit {
    /// The name, without `r/`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for Subreddit {
    type Err = InvalidSubreddit;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let valid = (1..=50).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'+');
        if valid {
            Ok(Self(name.to_owned()))
        } else {
            Err(InvalidSubreddit(name.to_owned()))
        }
    }
}

impl std::fmt::Display for Subreddit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The Reddit provider.
#[derive(Debug, Clone)]
pub struct Reddit {
    subreddit: Subreddit,
    client: Client,
    base_url: url::Url,
}

impl Reddit {
    /// Provider for one subreddit, against [`BASE_URL`].
    #[must_use]
    pub fn for_subreddit(subreddit: Subreddit, client: Client) -> Self {
        Self::for_subreddit_with_base_url(subreddit, client, http::default_base_url(BASE_URL))
    }

    /// [`Reddit::for_subreddit`] against another origin (`scheme://host`), for tests.
    #[must_use]
    pub fn for_subreddit_with_base_url(
        subreddit: Subreddit,
        client: Client,
        base_url: url::Url,
    ) -> Self {
        Self {
            subreddit,
            client,
            base_url,
        }
    }

    /// `{base}/r/{subreddit}/hot.json?limit=100`.
    fn feed_url(&self) -> url::Url {
        let path = format!("/r/{}/hot.json", self.subreddit);
        http::endpoint(&self.base_url, &path, &[("limit", LIMIT)])
    }

    /// `{base}/r/{subreddit}/hot.json?[after={after}&]limit=25&raw_json=1`.
    fn browse_url(&self, after: Option<&str>) -> url::Url {
        let path = format!("/r/{}/hot.json", self.subreddit);
        let mut params = vec![("limit", BROWSE_LIMIT), ("raw_json", "1")];
        if let Some(after) = after {
            params.push(("after", after));
        }
        http::endpoint(&self.base_url, &path, &params)
    }
}

#[async_trait]
impl Provider for Reddit {
    fn name(&self) -> &'static str {
        "reddit"
    }

    /// Fetches the hot listing of the subreddit; `query` is ignored.
    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let response = http::get_with_retry(&self.client, self.feed_url())
            .await
            .map_err(|e| ProviderError::request("reddit: HTTP error", e))?;
        let response = http::check_status(response).map_err(|s| s.into_provider_error("reddit"))?;
        let listing: Listing = http::read_json(response, "reddit").await?;
        Ok(filter_image_posts(listing, self.subreddit.as_str()))
    }

    fn browse_support(&self) -> BrowseSupport {
        BrowseSupport::Paged
    }

    fn media_hosts(&self) -> &'static [&'static str] {
        MEDIA_HOSTS
    }

    /// One hot-listing batch of 25 posts, from `cursor` (Reddit's `after`) on; `query` is
    /// ignored. Ends when the listing's `after` is empty.
    async fn browse(
        &self,
        _query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        let after = cursor.map(|c| c.0.as_str());
        let request = self.client.get(self.browse_url(after));
        let response = http::send_fail_fast(request, "reddit").await?;
        let response = http::check_status(response).map_err(|s| s.into_provider_error("reddit"))?;
        let listing: Listing = http::read_json(response, "reddit").await?;
        Ok(browse_batch(listing, self.subreddit.as_str()))
    }
}

/// The browsable posts of `listing` and the cursor of the next listing.
fn browse_batch(listing: Listing, subreddit: &str) -> BrowseBatch {
    let ListingData { after, children } = listing.data;
    let items = children
        .into_iter()
        .filter_map(|child| browse_item(child.data, subreddit))
        .collect();
    BrowseBatch {
        items,
        next: after.filter(|a| !a.is_empty()).map(BrowseCursor),
    }
}

/// `post` as a browse item, or `None` when it is NSFW, scores below [`MIN_SCORE`], has no
/// name, is not a supported image, or has its image or preview outside [`MEDIA_HOSTS`].
fn browse_item(post: Post, subreddit: &str) -> Option<RemoteItem> {
    if post.over_18 || post.score < MIN_SCORE || post.name.is_empty() {
        return None;
    }
    let image_url = resolve_image_url(&post.url)?;
    let image = post.preview.images.into_iter().next().unwrap_or_default();
    let preview_url = smallest_preview(&image.resolutions).unwrap_or_else(|| image_url.clone());
    if !is_media_url(&image_url) || !is_media_url(&preview_url) {
        return None;
    }
    let size = |n: u32| (n > 0).then_some(n);
    Some(RemoteItem {
        remote_id: sanitize_remote_id(&post.name),
        source_url: source_url(post.permalink, &post.id, subreddit),
        preview_url,
        image: ImageRef::Direct(image_url),
        title: post.title,
        author: post.author,
        width: size(image.source.width),
        height: size(image.source.height),
        // `over_18` posts are dropped above.
        nsfw: false,
    })
}

/// URL of the narrowest resolution at least [`MIN_PREVIEW_WIDTH`] wide, unescaped.
fn smallest_preview(resolutions: &[ImageSource]) -> Option<String> {
    resolutions
        .iter()
        .filter(|r| r.width >= MIN_PREVIEW_WIDTH && !r.url.is_empty())
        .min_by_key(|r| r.width)
        .map(|r| unescape_amp(&r.url))
}

/// `url` with Reddit's HTML escaping of `&` undone.
fn unescape_amp(url: &str) -> String {
    url.replace("&amp;", "&")
}

/// Whether `url` parses and passes the SSRF guard's checks for [`MEDIA_HOSTS`].
fn is_media_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| guard::check_url(&u, MEDIA_HOSTS).is_ok())
}

/// The post page: `permalink` under [`PAGE_ORIGIN`], an absolute `permalink` as is, or a
/// page built from the post `id` when there is no permalink.
fn source_url(permalink: String, id: &str, subreddit: &str) -> String {
    if permalink.is_empty() {
        format!("{PAGE_ORIGIN}/r/{subreddit}/comments/{id}")
    } else if permalink.starts_with("http") {
        permalink
    } else {
        format!("{PAGE_ORIGIN}{permalink}")
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
            Some(WallpaperResult {
                source_url: source_url(post.permalink, &post.id, subreddit),
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
pub(crate) fn resolve_image_url(post_url: &str) -> Option<String> {
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
    #[serde(deserialize_with = "de::null_default")]
    data: ListingData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListingData {
    /// Fullname of the last post, the `after` of the next listing; `null` at the end.
    #[serde(deserialize_with = "de::null_default")]
    after: Option<String>,
    #[serde(deserialize_with = "de::null_default")]
    children: Vec<Child>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Child {
    #[serde(deserialize_with = "de::null_default")]
    data: Post,
}

/// Missing or `null` fields decode to their zero value, like Go's `encoding/json`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Post {
    #[serde(deserialize_with = "de::null_default")]
    id: String,
    #[serde(deserialize_with = "de::null_default")]
    title: String,
    #[serde(deserialize_with = "de::null_default")]
    author: String,
    #[serde(deserialize_with = "de::null_default")]
    url: String,
    #[serde(deserialize_with = "de::null_default")]
    permalink: String,
    #[serde(deserialize_with = "de::null_default")]
    score: i64,
    /// Fullname, e.g. `t3_abc`: the browse `remote_id`.
    #[serde(deserialize_with = "de::null_default")]
    name: String,
    #[serde(deserialize_with = "de::null_default")]
    over_18: bool,
    /// Lenient, so a `preview` of an unexpected shape cannot fail `fetch`, which ignores it.
    #[serde(deserialize_with = "lenient")]
    preview: Preview,
}

/// Decodes `T`, or `T::default()` when the value is `null` or does not fit `T`.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Preview {
    #[serde(deserialize_with = "de::null_default")]
    images: Vec<PreviewImage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PreviewImage {
    /// The full-size image; only its size is used.
    #[serde(deserialize_with = "de::null_default")]
    source: ImageSource,
    /// Scaled copies, in no guaranteed order.
    #[serde(deserialize_with = "de::null_default")]
    resolutions: Vec<ImageSource>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ImageSource {
    #[serde(deserialize_with = "de::null_default")]
    url: String,
    #[serde(deserialize_with = "de::null_default")]
    width: u32,
    #[serde(deserialize_with = "de::null_default")]
    height: u32,
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

    fn res(url: &str, width: u32) -> ImageSource {
        ImageSource {
            url: url.into(),
            width,
            height: 0,
        }
    }

    #[test]
    fn smallest_preview_picks_narrowest_at_least_320() {
        let resolutions = [res("a", 640), res("b", 216), res("c", 320), res("d", 960)];
        assert_eq!(smallest_preview(&resolutions).as_deref(), Some("c"));
    }

    #[test]
    fn smallest_preview_is_none_when_all_are_narrow() {
        assert_eq!(smallest_preview(&[res("a", 108), res("b", 319)]), None);
    }

    #[test]
    fn smallest_preview_skips_empty_urls() {
        assert_eq!(
            smallest_preview(&[res("", 320), res("b", 640)]).as_deref(),
            Some("b")
        );
    }

    #[test]
    fn smallest_preview_unescapes_amp() {
        let resolutions = [res("https://preview.redd.it/a.jpg?width=320&amp;s=x", 320)];
        assert_eq!(
            smallest_preview(&resolutions).as_deref(),
            Some("https://preview.redd.it/a.jpg?width=320&s=x")
        );
    }

    #[test]
    fn media_url_accepts_listed_https_hosts() {
        for url in [
            "https://i.redd.it/a.jpg",
            "https://preview.redd.it/a.jpg?width=320&s=x",
            "https://i.imgur.com/a.jpg",
        ] {
            assert!(is_media_url(url), "{url}");
        }
    }

    #[test]
    fn media_url_rejects_unlisted_hosts_http_and_garbage() {
        for url in [
            "https://example.com/a.jpg",
            "https://imgur.com/a.jpg",
            "https://external-preview.redd.it/a.jpg",
            "https://i.redd.it.evil.test/a.jpg",
            "http://i.redd.it/a.jpg",
            "not a url",
        ] {
            assert!(!is_media_url(url), "{url}");
        }
    }

    #[test]
    fn malformed_preview_does_not_fail_decoding() {
        let listing: Listing = serde_json::from_str(
            r#"{"data":{"children":[
                {"data":{"id":"a","score":100,"url":"https://e.com/a.jpg",
                         "preview":{"images":"oops"}}}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(filter_image_posts(listing, "pics").len(), 1);
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
