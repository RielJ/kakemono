//! Per-instance online sources: one provider per enabled `[[sources]]` row.
//!
//! [`build_online`] turns one row into an [`OnlineSource`]: the row's `location` (keywords, a
//! subreddit or a pasted URL) becomes the provider's input, and the credentials still come from
//! `[wallhaven]`, `[unsplash]`, `[apod]` and `[bing]`. The row's own `enabled` decides whether it is
//! used. [`SourceSet`] holds every enabled online row.
//!
//! How `location` maps, per type:
//!
//! - **Reddit:** `x`, `r/x`, `/r/x` or `https://(www.|old.)reddit.com/r/x/...` give the
//!   subreddit `x`. A URL's sort and `t` (`/r/x/top/?t=year`) pick the listing; hot otherwise.
//! - **Wallhaven:** keywords pass through as the query. A `wallhaven.cc/search?...` URL sets the
//!   query from `q`, overrides `categories`, `purity` and `sorting`, and adds `topRange`,
//!   `order`, `atleast`, `resolutions`, `ratios` and `colors` to every request;
//!   `wallhaven.cc/tag/<n>` (or `/tag/<n>`) becomes the query `id:<n>`.
//! - **Unsplash:** keywords pass through; `unsplash.com/s/photos/<kw>` gives `<kw>` with `-` read
//!   as a space. Any other URL is an error.
//! - **Bing, APOD:** the location is ignored.

use std::sync::Arc;

use kakemono_config::schema::{Config, SourceEntry, SourceType};
use reqwest::Client;

use crate::apod::Apod;
use crate::bing::Bing;
use crate::http::default_base_url;
use crate::provider::{Provider, ProviderError};
use crate::reddit::{Reddit, Sort, Subreddit};
use crate::unsplash::Unsplash;
use crate::wallhaven::{Wallhaven, WallhavenOverrides};

/// One enabled online `[[sources]]` row, ready to fetch.
#[derive(Clone)]
pub struct OnlineSource {
    /// The row's id; names the download folder `downloads/<id>/`.
    pub id: String,
    /// The row's type; always [`SourceType::is_online`].
    pub kind: SourceType,
    /// The provider built for this row.
    pub provider: Arc<dyn Provider>,
    /// The query to pass to [`Provider::fetch`] (empty for Reddit, Bing and APOD).
    pub query: String,
}

impl std::fmt::Debug for OnlineSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnlineSource")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("provider", &self.provider.name())
            .field("query", &self.query)
            .finish()
    }
}

/// A Wallhaven `location` parsed into a query and per-instance option overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WallhavenSearch {
    /// The `q` parameter; empty for "anything".
    pub query: String,
    /// Options that replace the `[wallhaven]` values for this instance.
    pub overrides: WallhavenOverrides,
}

/// Every enabled online `[[sources]]` row, in config order.
#[derive(Clone, Debug, Default)]
pub struct SourceSet {
    /// Enabled online rows only; local, disabled and `unsupported` rows are left out.
    pub online: Vec<OnlineSource>,
}

impl SourceSet {
    /// One [`OnlineSource`] per enabled online row of `cfg.sources`, sharing `http` (build it with
    /// [`crate::http::client`]). Disabled, local and `unsupported` rows are skipped. A row whose
    /// location doesn't parse is skipped with a warning in the log.
    pub fn from_config(cfg: &Config, http: Client) -> Self {
        Self::from_config_at(cfg, http, &Origins::default())
    }

    fn from_config_at(cfg: &Config, http: Client, origins: &Origins) -> Self {
        let online = cfg
            .sources
            .iter()
            .filter(|s| s.enabled && s.kind.is_online())
            .filter_map(
                |entry| match build_online_at(entry, cfg, http.clone(), origins) {
                    Ok(source) => Some(source),
                    Err(err) => {
                        tracing::warn!(id = %entry.id, kind = %entry.kind, %err, "skipping source");
                        None
                    }
                },
            )
            .collect();
        Self { online }
    }

    /// The source with id `id`.
    pub fn get(&self, id: &str) -> Option<&OnlineSource> {
        self.online.iter().find(|s| s.id == id)
    }

    /// Whether there is no enabled online source.
    pub fn is_empty(&self) -> bool {
        self.online.is_empty()
    }
}

/// Builds the provider for one online row. `entry.enabled` is not checked; [`SourceSet`] filters
/// disabled rows.
///
/// Errors: [`ProviderError::NotConfigured`] when `entry` is not an online type or its location
/// doesn't parse.
pub fn build_online(
    entry: &SourceEntry,
    cfg: &Config,
    http: Client,
) -> Result<OnlineSource, ProviderError> {
    build_online_at(entry, cfg, http, &Origins::default())
}

/// API origins per provider; production by default, mock servers in tests.
struct Origins {
    wallhaven: url::Url,
    unsplash: url::Url,
    reddit: url::Url,
    bing: url::Url,
    apod: url::Url,
}

impl Default for Origins {
    fn default() -> Self {
        Self {
            wallhaven: default_base_url(crate::wallhaven::DEFAULT_BASE_URL),
            unsplash: default_base_url(crate::unsplash::BASE_URL),
            reddit: default_base_url(crate::reddit::BASE_URL),
            bing: default_base_url(crate::bing::BASE_URL),
            apod: default_base_url(crate::apod::BASE_URL),
        }
    }
}

fn build_online_at(
    entry: &SourceEntry,
    cfg: &Config,
    http: Client,
    origins: &Origins,
) -> Result<OnlineSource, ProviderError> {
    let location = entry.location.trim();
    let (provider, query): (Arc<dyn Provider>, String) = match entry.kind {
        SourceType::Wallhaven => {
            let search = wallhaven_location(location)?;
            let provider =
                Wallhaven::with_base_url(cfg.wallhaven.clone(), http, origins.wallhaven.clone())
                    .apply_overrides(&search.overrides);
            (Arc::new(provider), search.query)
        }
        SourceType::Unsplash => {
            let query = unsplash_location(location)?;
            (
                Arc::new(Unsplash::with_base_url(
                    cfg.unsplash.clone(),
                    http,
                    origins.unsplash.clone(),
                )),
                query,
            )
        }
        SourceType::Reddit => {
            let subreddit = reddit_location(location)?;
            (
                Arc::new(
                    Reddit::for_subreddit_with_base_url(subreddit, http, origins.reddit.clone())
                        .with_sort(reddit_sort(location)),
                ),
                String::new(),
            )
        }
        SourceType::Bing => (
            Arc::new(Bing::with_base_url(
                cfg.bing.clone(),
                http,
                origins.bing.clone(),
            )),
            String::new(),
        ),
        SourceType::Apod => (
            Arc::new(Apod::with_base_url(
                cfg.apod.clone(),
                http,
                origins.apod.clone(),
            )),
            String::new(),
        ),
        SourceType::Folder
        | SourceType::Images
        | SourceType::AlbumName
        | SourceType::AlbumDate
        | SourceType::Unsupported => {
            return Err(ProviderError::NotConfigured(format!(
                "{}: not an online source",
                entry.kind
            )));
        }
    };
    Ok(OnlineSource {
        id: entry.id.clone(),
        kind: entry.kind,
        provider,
        query,
    })
}

/// The listing named by a Reddit `location` URL: `reddit.com/r/x/<sort>/?t=<range>`, where
/// `<sort>` is `hot`, `new`, `rising`, `top` or `controversial` and `t` (`hour`, `day`, `week`,
/// `month`, `year`, `all`) counts for `top` and `controversial` only.
///
/// Anything else is [`Sort::Hot`]: a bare name (`x`, `r/x`), a URL without a sort, a post URL
/// (`/r/x/comments/...`), or a location [`reddit_location`] rejects. An unknown `t` is dropped,
/// so Reddit uses its default range.
pub(crate) fn reddit_sort(location: &str) -> Sort {
    let Ok(Some(url)) = url_like(location.trim(), REDDIT_BARE_HOSTS) else {
        return Sort::Hot;
    };
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let [r, _, sort, ..] = segments.as_slice() else {
        return Sort::Hot;
    };
    if !r.eq_ignore_ascii_case("r") {
        return Sort::Hot;
    }
    let t = url
        .query_pairs()
        .find(|(k, _)| k == "t")
        .map(|(_, v)| v.into_owned());
    Sort::parse(sort, t.as_deref()).unwrap_or_default()
}

/// The subreddit named by a Reddit `location`: `x`, `r/x`, `/r/x/` or a `reddit.com/r/x/...`
/// URL (any `*.reddit.com` host; the sort is read by [`reddit_sort`], other path parts are
/// ignored). A name is
/// `[A-Za-z0-9_+]{1,50}` (`+` joins subreddits, as Reddit allows).
///
/// Errors: [`ProviderError::NotConfigured`] (`reddit: ...`) for anything else.
pub(crate) fn reddit_location(location: &str) -> Result<Subreddit, ProviderError> {
    let loc = location.trim();
    let err = || {
        ProviderError::NotConfigured(format!(
            "reddit: {loc:?} is not a subreddit name or reddit.com/r/... URL"
        ))
    };
    let name = match url_like(loc, REDDIT_BARE_HOSTS).map_err(|()| err())? {
        Some(url) => {
            let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
            if host != "reddit.com" && !host.ends_with(".reddit.com") {
                return Err(err());
            }
            let segments: Vec<&str> = url
                .path_segments()
                .map(|s| s.filter(|s| !s.is_empty()).collect())
                .unwrap_or_default();
            match segments.as_slice() {
                [r, name, ..] if r.eq_ignore_ascii_case("r") => (*name).to_owned(),
                _ => return Err(err()),
            }
        }
        None => {
            let rest = loc.trim_start_matches('/');
            let rest = rest
                .strip_prefix("r/")
                .or_else(|| rest.strip_prefix("R/"))
                .unwrap_or(rest);
            rest.trim_end_matches('/').to_owned()
        }
    };
    name.parse().map_err(|_| err())
}

/// Hosts accepted without a scheme, e.g. `www.reddit.com/r/x`.
const REDDIT_BARE_HOSTS: &[&str] = &["reddit.com", "www.reddit.com", "old.reddit.com"];

/// Hosts of Wallhaven URLs, with or without a scheme.
const WALLHAVEN_HOSTS: &[&str] = &["wallhaven.cc", "www.wallhaven.cc"];

/// Hosts of Unsplash URLs, with or without a scheme.
const UNSPLASH_HOSTS: &[&str] = &["unsplash.com", "www.unsplash.com"];

/// The query and option overrides named by a Wallhaven `location`.
///
/// - Keywords pass through as the query, with no overrides.
/// - `wallhaven.cc/search?q=..&categories=..&purity=..&sorting=..` (any of them optional) sets
///   the query from `q` and overrides the other three.
/// - The search parameters `topRange`, `order`, `atleast`, `resolutions`, `ratios` and `colors`
///   are kept as is (not checked; Wallhaven checks them) and sent with every request. Other
///   parameters (`page`, `seed`, ...) and empty values are ignored.
/// - `wallhaven.cc/tag/<n>`, or just `/tag/<n>`, gives the query `id:<n>`.
/// - `wallhaven.cc/latest`, `/toplist`, `/hot` and `/random` set `sorting` to `date_added`,
///   `toplist`, `hot` and `random`; their query parameters are read as above, so
///   `wallhaven.cc/toplist?topRange=1y` keeps its range.
///
/// Errors: [`ProviderError::NotConfigured`] (`wallhaven: ...`) for any other URL.
pub(crate) fn wallhaven_location(location: &str) -> Result<WallhavenSearch, ProviderError> {
    let loc = location.trim();
    let err = || {
        ProviderError::NotConfigured(format!(
            "wallhaven: {loc:?}: only wallhaven.cc search, tag and listing URLs are supported"
        ))
    };
    let url = if loc.starts_with('/') {
        Some(url::Url::parse(&format!("https://wallhaven.cc{loc}")).map_err(|_| err())?)
    } else {
        url_like(loc, WALLHAVEN_HOSTS).map_err(|()| err())?
    };
    let Some(url) = url else {
        return Ok(WallhavenSearch {
            query: loc.to_owned(),
            overrides: WallhavenOverrides::default(),
        });
    };
    if !host_in(&url, WALLHAVEN_HOSTS) {
        return Err(err());
    }

    let mut search = WallhavenSearch::default();
    for (key, value) in url.query_pairs() {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.as_ref() {
            "q" => value.clone_into(&mut search.query),
            "categories" => search.overrides.categories = Some(value.to_owned()),
            "purity" => search.overrides.purity = Some(value.to_owned()),
            "sorting" => search.overrides.sorting = Some(value.to_owned()),
            "topRange" => search.overrides.top_range = Some(value.to_owned()),
            "order" => search.overrides.order = Some(value.to_owned()),
            "atleast" => search.overrides.atleast = Some(value.to_owned()),
            "resolutions" => search.overrides.resolutions = Some(value.to_owned()),
            "ratios" => search.overrides.ratios = Some(value.to_owned()),
            "colors" => search.overrides.colors = Some(value.to_owned()),
            _ => {}
        }
    }

    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    match segments.as_slice() {
        ["search"] => {}
        ["tag", n] if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            search.query = format!("id:{n}");
        }
        [listing] => {
            let sorting = match *listing {
                "latest" => "date_added",
                "toplist" => "toplist",
                "hot" => "hot",
                "random" => "random",
                _ => return Err(err()),
            };
            search.overrides.sorting = Some(sorting.to_owned());
        }
        _ => return Err(err()),
    }
    Ok(search)
}

/// The search keywords named by an Unsplash `location`: keywords pass through;
/// `unsplash.com/s/photos/<kw>` gives `<kw>`, percent-decoded, with `-` read as a space.
///
/// Errors: [`ProviderError::NotConfigured`] (`unsplash: only search URLs are supported`) for
/// any other URL.
pub(crate) fn unsplash_location(location: &str) -> Result<String, ProviderError> {
    let loc = location.trim();
    let err = || ProviderError::NotConfigured("unsplash: only search URLs are supported".into());
    let Some(url) = url_like(loc, UNSPLASH_HOSTS).map_err(|()| err())? else {
        return Ok(loc.to_owned());
    };
    if !host_in(&url, UNSPLASH_HOSTS) {
        return Err(err());
    }
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    match segments.as_slice() {
        ["s", "photos", keywords] => {
            let keywords = percent_decode(keywords).replace('-', " ");
            let keywords = keywords.split_whitespace().collect::<Vec<_>>().join(" ");
            if keywords.is_empty() {
                Err(err())
            } else {
                Ok(keywords)
            }
        }
        _ => Err(err()),
    }
}

/// `Ok(Some(url))` when `loc` is a URL: it starts with `http://` or `https://`, or with one of
/// `bare_hosts` followed by `/` (read as `https://`). `Ok(None)` for plain text. `Err(())` when
/// it looks like a URL but doesn't parse.
fn url_like(loc: &str, bare_hosts: &[&str]) -> Result<Option<url::Url>, ()> {
    let lower = loc.to_ascii_lowercase();
    let text = if lower.starts_with("http://") || lower.starts_with("https://") {
        loc.to_owned()
    } else if bare_hosts
        .iter()
        .any(|h| lower.starts_with(&format!("{h}/")))
    {
        format!("https://{loc}")
    } else {
        return Ok(None);
    };
    url::Url::parse(&text).map(Some).map_err(|_| ())
}

/// Whether `url`'s host is one of `hosts` (case-insensitive).
fn host_in(url: &url::Url, hosts: &[&str]) -> bool {
    let host = url.host_str().unwrap_or_default();
    hosts.iter().any(|h| host.eq_ignore_ascii_case(h))
}

/// Decodes `%XX` escapes; invalid escapes stay as they are and invalid UTF-8 is replaced.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            // Two hex digits fit in a byte.
            out.push(u8::try_from(hi * 16 + lo).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn not_configured(err: ProviderError) -> String {
        match err {
            ProviderError::NotConfigured(msg) => msg,
            other => panic!("want NotConfigured, got {other:?}"),
        }
    }

    // ---- reddit_location ----

    #[test]
    fn reddit_url_gives_the_subreddit() {
        assert_eq!(
            reddit_location("https://www.reddit.com/r/EarthPorn/top/?t=week")
                .unwrap()
                .as_str(),
            "EarthPorn"
        );
        assert_eq!(
            reddit_location("https://old.reddit.com/r/wallpapers")
                .unwrap()
                .as_str(),
            "wallpapers"
        );
        assert_eq!(
            reddit_location("http://reddit.com/r/pics/")
                .unwrap()
                .as_str(),
            "pics"
        );
        assert_eq!(
            reddit_location("www.reddit.com/r/EarthPorn/")
                .unwrap()
                .as_str(),
            "EarthPorn"
        );
    }

    #[test]
    fn reddit_name_forms() {
        assert_eq!(
            reddit_location("r/earthporn").unwrap().as_str(),
            "earthporn"
        );
        assert_eq!(
            reddit_location("/r/earthporn/").unwrap().as_str(),
            "earthporn"
        );
        assert_eq!(reddit_location("earthporn").unwrap().as_str(), "earthporn");
        assert_eq!(
            reddit_location("  earthporn  ").unwrap().as_str(),
            "earthporn"
        );
    }

    #[test]
    fn reddit_sort_comes_from_the_url() {
        use crate::reddit::TimeRange;
        for (loc, want) in [
            (
                "https://www.reddit.com/r/EarthPorn/top/?t=year",
                Sort::Top(Some(TimeRange::Year)),
            ),
            ("reddit.com/r/wallpapers/top", Sort::Top(None)),
            ("https://old.reddit.com/r/wallpapers/new/", Sort::New),
            ("https://www.reddit.com/r/wallpapers/rising", Sort::Rising),
            (
                "https://www.reddit.com/r/wallpapers/controversial/?t=WEEK",
                Sort::Controversial(Some(TimeRange::Week)),
            ),
            ("https://www.reddit.com/r/wallpapers/hot/?t=year", Sort::Hot),
            (
                "https://www.reddit.com/r/wallpapers/top/?t=forever",
                Sort::Top(None),
            ),
            ("https://www.reddit.com/r/wallpapers/", Sort::Hot),
            (
                "https://www.reddit.com/r/wallpapers/comments/abc/title/",
                Sort::Hot,
            ),
            ("r/wallpapers", Sort::Hot),
            ("wallpapers", Sort::Hot),
        ] {
            assert_eq!(reddit_sort(loc), want, "{loc}");
        }
    }

    #[tokio::test]
    async fn reddit_row_with_top_url_fetches_the_top_listing() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/r/EarthPorn/top.json"))
            .and(query_param("t", "month"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": {"children": []}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let origins = Origins {
            reddit: server.uri().parse().unwrap(),
            ..Origins::default()
        };
        let row = entry(
            "rd-1",
            SourceType::Reddit,
            "https://www.reddit.com/r/EarthPorn/top/?t=month",
        );
        let src = build_online_at(&row, &Config::default(), Client::new(), &origins).unwrap();
        src.provider.fetch(&src.query).await.unwrap();
    }

    #[test]
    fn reddit_rejects_non_subreddits() {
        for bad in [
            "",
            "r/",
            "https://example.com/r/pics",
            "https://www.reddit.com/user/someone",
            "two words",
            "../etc",
        ] {
            let msg = not_configured(reddit_location(bad).unwrap_err());
            assert!(msg.starts_with("reddit: "), "{bad:?}: {msg}");
        }
    }

    // ---- wallhaven_location ----

    #[test]
    fn wallhaven_search_url_sets_query_and_overrides() {
        let got = wallhaven_location(
            "https://wallhaven.cc/search?q=forest&categories=100&purity=100&sorting=toplist",
        )
        .unwrap();
        assert_eq!(
            got,
            WallhavenSearch {
                query: "forest".into(),
                overrides: WallhavenOverrides {
                    categories: Some("100".into()),
                    purity: Some("100".into()),
                    sorting: Some("toplist".into()),
                    ..WallhavenOverrides::default()
                },
            }
        );
    }

    #[test]
    fn wallhaven_search_url_keeps_range_order_and_filters() {
        let got = wallhaven_location(
            "https://wallhaven.cc/search?categories=110&purity=100&topRange=1y&sorting=toplist\
             &order=desc&atleast=2560x1440&resolutions=1920x1080%2C2560x1440&ratios=16x9,16x10\
             &colors=0066cc",
        )
        .unwrap();
        assert_eq!(
            got,
            WallhavenSearch {
                query: String::new(),
                overrides: WallhavenOverrides {
                    categories: Some("110".into()),
                    purity: Some("100".into()),
                    sorting: Some("toplist".into()),
                    top_range: Some("1y".into()),
                    order: Some("desc".into()),
                    atleast: Some("2560x1440".into()),
                    resolutions: Some("1920x1080,2560x1440".into()),
                    ratios: Some("16x9,16x10".into()),
                    colors: Some("0066cc".into()),
                },
            }
        );
    }

    #[test]
    fn wallhaven_toplist_listing_keeps_top_range() {
        let got = wallhaven_location("https://wallhaven.cc/toplist?topRange=1y&page=2").unwrap();
        assert_eq!(
            got.overrides,
            WallhavenOverrides {
                sorting: Some("toplist".into()),
                top_range: Some("1y".into()),
                ..WallhavenOverrides::default()
            }
        );
    }

    #[test]
    fn wallhaven_search_url_decodes_query_and_leaves_missing_options() {
        let got = wallhaven_location("https://wallhaven.cc/search?q=misty+forest%21&page=3&seed=x")
            .unwrap();
        assert_eq!(got.query, "misty forest!");
        assert_eq!(got.overrides, WallhavenOverrides::default());
    }

    #[test]
    fn wallhaven_tag_becomes_id_query() {
        for loc in [
            "/tag/37",
            "https://wallhaven.cc/tag/37",
            "wallhaven.cc/tag/37",
            "https://www.wallhaven.cc/tag/37/",
        ] {
            let got = wallhaven_location(loc).unwrap();
            assert_eq!(got.query, "id:37", "{loc}");
            assert_eq!(got.overrides, WallhavenOverrides::default(), "{loc}");
        }
    }

    #[test]
    fn wallhaven_listing_urls_set_sorting() {
        for (loc, sorting) in [
            ("https://wallhaven.cc/latest", "date_added"),
            ("https://wallhaven.cc/toplist?page=2", "toplist"),
            ("https://wallhaven.cc/hot", "hot"),
            ("wallhaven.cc/random", "random"),
        ] {
            let got = wallhaven_location(loc).unwrap();
            assert_eq!(got.query, "", "{loc}");
            assert_eq!(got.overrides.sorting.as_deref(), Some(sorting), "{loc}");
        }
    }

    #[test]
    fn wallhaven_keywords_pass_through() {
        let got = wallhaven_location("nature mountains").unwrap();
        assert_eq!(got.query, "nature mountains");
        assert_eq!(got.overrides, WallhavenOverrides::default());
        assert_eq!(wallhaven_location("").unwrap(), WallhavenSearch::default());
    }

    #[test]
    fn wallhaven_rejects_other_urls() {
        for bad in [
            "https://example.com/search?q=x",
            "https://wallhaven.cc/w/abc123",
            "https://wallhaven.cc/tag/notanumber",
        ] {
            let msg = not_configured(wallhaven_location(bad).unwrap_err());
            assert!(msg.starts_with("wallhaven: "), "{bad:?}: {msg}");
        }
    }

    // ---- unsplash_location ----

    #[test]
    fn unsplash_search_url_gives_keywords() {
        assert_eq!(
            unsplash_location("https://unsplash.com/s/photos/mountain-lake").unwrap(),
            "mountain lake"
        );
        assert_eq!(
            unsplash_location("unsplash.com/s/photos/new%20york?orientation=landscape").unwrap(),
            "new york"
        );
    }

    #[test]
    fn unsplash_keywords_pass_through() {
        assert_eq!(unsplash_location("mountain lake").unwrap(), "mountain lake");
        assert_eq!(unsplash_location("").unwrap(), "");
    }

    #[test]
    fn unsplash_other_urls_are_not_configured() {
        for bad in [
            "https://unsplash.com/photos/abc123",
            "https://unsplash.com/s/photos/",
            "https://example.com/s/photos/cats",
        ] {
            assert_eq!(
                not_configured(unsplash_location(bad).unwrap_err()),
                "unsplash: only search URLs are supported",
                "{bad:?}"
            );
        }
    }

    // ---- build_online / SourceSet ----

    fn entry(id: &str, kind: SourceType, location: &str) -> SourceEntry {
        SourceEntry {
            id: id.into(),
            enabled: true,
            kind,
            location: location.into(),
        }
    }

    #[test]
    fn build_online_builds_every_online_type() {
        let cfg = Config::default();
        for (kind, location, query) in [
            (SourceType::Wallhaven, "forest", "forest"),
            (
                SourceType::Unsplash,
                "https://unsplash.com/s/photos/sea-cliff",
                "sea cliff",
            ),
            (SourceType::Reddit, "r/EarthPorn", ""),
            (SourceType::Bing, "ignored", ""),
            (SourceType::Apod, "", ""),
        ] {
            let src = build_online(&entry("x-1", kind, location), &cfg, Client::new()).unwrap();
            assert_eq!(src.provider.name(), kind.as_str());
            assert_eq!(src.kind, kind);
            assert_eq!(src.id, "x-1");
            assert_eq!(src.query, query, "{kind}");
        }
    }

    #[test]
    fn build_online_refuses_local_and_unsupported_types() {
        for kind in [
            SourceType::Folder,
            SourceType::Images,
            SourceType::AlbumName,
            SourceType::AlbumDate,
            SourceType::Unsupported,
        ] {
            let err = build_online(&entry("x", kind, "/tmp"), &Config::default(), Client::new())
                .unwrap_err();
            assert_eq!(not_configured(err), format!("{kind}: not an online source"));
        }
    }

    #[test]
    fn source_set_keeps_enabled_online_rows_that_build() {
        let mut cfg = Config::default();
        let mut off = entry("us-off", SourceType::Unsplash, "cats");
        off.enabled = false;
        cfg.sources = vec![
            entry("wh-1", SourceType::Wallhaven, "forest"),
            off,
            entry("dir-1", SourceType::Folder, "/tmp"),
            entry("unk-1", SourceType::Unsupported, "x"),
            entry("rd-bad", SourceType::Reddit, "https://example.com/nope"),
            entry("rd-1", SourceType::Reddit, "r/pics"),
            entry("bing-1", SourceType::Bing, ""),
        ];
        let set = SourceSet::from_config(&cfg, Client::new());
        let ids: Vec<&str> = set.online.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["wh-1", "rd-1", "bing-1"]);
        assert_eq!(set.get("wh-1").unwrap().query, "forest");
        assert!(set.get("us-off").is_none());
        assert!(set.get("nope").is_none());
        assert!(!set.is_empty());
        assert!(SourceSet::default().is_empty());
    }

    /// A Wallhaven instance built from a pasted search URL sends that URL's options, not the
    /// `[wallhaven]` ones.
    #[tokio::test]
    async fn wallhaven_instance_from_url_sends_overridden_params() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/search"))
            .and(query_param("q", "forest"))
            .and(query_param("categories", "010"))
            .and(query_param("purity", "110"))
            .and(query_param("sorting", "toplist"))
            .and(query_param("topRange", "1y"))
            .and(query_param("order", "desc"))
            .and(query_param("apikey", "k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"url": "https://wallhaven.cc/w/a1", "path": "https://w.wallhaven.cc/full/a1.jpg"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.wallhaven.api_key = "k".into();
        cfg.wallhaven.categories = "111".into();
        cfg.wallhaven.purity = "100".into();
        cfg.wallhaven.sorting = "random".into();
        let origins = Origins {
            wallhaven: server.uri().parse().unwrap(),
            ..Origins::default()
        };
        let row = entry(
            "wh-1",
            SourceType::Wallhaven,
            "https://wallhaven.cc/search?q=forest&categories=010&purity=110&sorting=toplist\
             &topRange=1y&order=desc",
        );
        let src = build_online_at(&row, &cfg, Client::new(), &origins).unwrap();
        let results = src.provider.fetch(&src.query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].image_url, "https://w.wallhaven.cc/full/a1.jpg");
    }

    /// A Reddit instance fetches only its own subreddit.
    #[tokio::test]
    async fn reddit_instance_fetches_its_subreddit() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/r/EarthPorn/hot.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": {"children": []}})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let origins = Origins {
            reddit: server.uri().parse().unwrap(),
            ..Origins::default()
        };
        let row = entry(
            "rd-1",
            SourceType::Reddit,
            "https://www.reddit.com/r/EarthPorn/",
        );
        let src = build_online_at(&row, &Config::default(), Client::new(), &origins).unwrap();
        assert!(src.provider.fetch(&src.query).await.unwrap().is_empty());
    }
}
