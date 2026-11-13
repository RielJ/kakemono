//! Reddit provider against the Go-captured fixtures in `tests/fixtures/reddit/`.

use crate::common;

use common::Case;
use kakemono_sources::reddit::{Reddit, Subreddit};
use kakemono_sources::{Provider, ProviderError};

/// Every case file in `tests/fixtures/reddit/`; one `#[tokio::test]` each below.
const CASES: [&str; 6] = [
    "empty_results",
    "filters_non_images",
    "http_error",
    "imgur_conversion",
    "source_and_permalink",
    "webp_included",
];

/// The case's `config` (Go's `[reddit]` section) as it bears on one row: Go fixtures name one
/// subreddit each.
#[derive(serde::Deserialize)]
struct CaseConfig {
    subreddits: Vec<String>,
}

fn subreddit(case: &Case) -> Subreddit {
    let cfg = case.config::<CaseConfig>();
    assert_eq!(
        cfg.subreddits.len(),
        1,
        "{}: one subreddit per row",
        case.id
    );
    cfg.subreddits[0].parse().expect("valid subreddit")
}

async fn run(name: &str) {
    let case = Case::load("reddit", name);
    let replay = case.serve().await;
    let reddit =
        Reddit::for_subreddit_with_base_url(subreddit(&case), replay.client(), replay.base_url());
    replay.verify(reddit.fetch(&case.query).await).await;
}

#[test]
fn every_fixture_case_has_a_test() {
    assert_eq!(Case::names("reddit"), CASES);
}

#[tokio::test]
async fn empty_results() {
    run("empty_results").await
}

#[tokio::test]
async fn filters_non_images() {
    run("filters_non_images").await
}

#[tokio::test]
async fn http_error() {
    run("http_error").await
}

#[tokio::test]
async fn imgur_conversion() {
    run("imgur_conversion").await
}

#[tokio::test]
async fn source_and_permalink() {
    run("source_and_permalink").await
}

#[tokio::test]
async fn webp_included() {
    run("webp_included").await
}

#[tokio::test]
async fn name_is_reddit() {
    let reddit = Reddit::for_subreddit(
        "EarthPorn".parse().unwrap(),
        kakemono_sources::http::client_builder().build().unwrap(),
    );
    assert_eq!(reddit.name(), "reddit");
}

/// An empty or unsafe subreddit name cannot build a provider, so no request can be sent for it
/// (Go `TestReddit_NoSubreddits` returned an error from `fetch`).
#[test]
fn invalid_subreddit_names_are_rejected() {
    for bad in ["", "a/b", "../x", "a?b", "a b", &"x".repeat(51)] {
        let err = bad.parse::<Subreddit>().unwrap_err();
        assert_eq!(err.to_string(), format!("{bad:?} is not a subreddit name"));
    }
    for good in ["EarthPorn", "a_b", "pics+wallpapers", &"x".repeat(50)] {
        assert_eq!(good.parse::<Subreddit>().unwrap().as_str(), good);
    }
}

/// Every fetch asks for the row's subreddit and nothing else.
#[tokio::test]
async fn fetches_only_its_subreddit() {
    let json = r#"{
        "go_test": "hand-written",
        "provider": "reddit",
        "config": {"subreddits": ["wallpapers"]},
        "query": "",
        "routes": [
            {"method": "GET", "host": "www.reddit.com", "path": "/r/wallpapers/hot.json",
             "status": 200, "content_type": "application/json", "body_file": "empty.json"}
        ],
        "requests": [],
        "expected": {"results": []}
    }"#;
    let case = Case::parse(
        "reddit/one_subreddit",
        common::fixtures_dir().join("reddit"),
        json,
    );
    let replay = case.serve().await;
    let reddit =
        Reddit::for_subreddit_with_base_url(subreddit(&case), replay.client(), replay.base_url());
    for _ in 0..3 {
        assert_eq!(reddit.fetch("").await.unwrap(), vec![]);
    }
    let requests = replay.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    for r in &requests {
        assert_eq!(r.url.path(), "/r/wallpapers/hot.json");
        assert_eq!(r.url.query(), Some("limit=100"));
    }
}

/// A 429 that outlasts the retries is `RateLimited`, so the engine applies its rate-limit
/// backoff (audit M1: Reddit used to report it as a plain status error).
#[tokio::test]
async fn rate_limit_is_the_rate_limited_variant() {
    let json = r#"{
        "go_test": "hand-written",
        "provider": "reddit",
        "config": {"subreddits": ["wallpapers"]},
        "query": "",
        "routes": [
            {"method": "GET", "host": "www.reddit.com", "path": "/r/wallpapers/hot.json",
             "status": 429, "content_type": "text/plain", "body_file": "forbidden.txt"}
        ],
        "requests": [{"method": "GET", "user_agent": "",
                      "url": "https://www.reddit.com/r/wallpapers/hot.json?limit=100"}],
        "expected": {"results": []}
    }"#;
    let case = Case::parse(
        "reddit/rate_limited",
        common::fixtures_dir().join("reddit"),
        json,
    );
    let replay = case.serve().await;
    let reddit =
        Reddit::for_subreddit_with_base_url(subreddit(&case), replay.client(), replay.base_url());
    let err = reddit.fetch("").await.unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited(_)), "{err:?}");
    assert_eq!(err.to_string(), "reddit: rate limited (status 429)");
    replay.assert_requests().await;
}

// ---- Browse (picker Sources tab) -------------------------------------------------------------

mod browse {
    use kakemono_sources::reddit::{Reddit, Subreddit};
    use kakemono_sources::{BrowseCursor, BrowseSupport, ImageRef, Provider, ProviderError};
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::common;

    const FEED_PATH: &str = "/r/wallpapers/hot.json";

    fn body(file: &str) -> Vec<u8> {
        let path = common::fixtures_dir().join("reddit").join(file);
        std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    fn json(file: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body(file), "application/json")
    }

    /// A mock serving `browse_p1.json` without `after` and `browse_end.json` for `after=t3_x`.
    async fn server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(FEED_PATH))
            .and(query_param_is_missing("after"))
            .respond_with(json("browse_p1.json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(FEED_PATH))
            .and(query_param("after", "t3_x"))
            .respond_with(json("browse_end.json"))
            .mount(&server)
            .await;
        server
    }

    fn reddit(server: &MockServer) -> Reddit {
        let subreddit: Subreddit = "wallpapers".parse().unwrap();
        Reddit::for_subreddit_with_base_url(
            subreddit,
            kakemono_sources::http::client_builder().build().unwrap(),
            server.uri().parse().unwrap(),
        )
    }

    fn cursor(after: &str) -> BrowseCursor {
        BrowseCursor(after.into())
    }

    async fn queries(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| r.url.query().unwrap_or_default().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn browse_first_request_asks_for_25_raw_json_posts() {
        let server = server().await;
        reddit(&server).browse("", None).await.unwrap();
        assert_eq!(queries(&server).await, ["limit=25&raw_json=1"]);
    }

    #[tokio::test]
    async fn browse_first_request_uses_the_hot_listing_of_the_row() {
        let server = server().await;
        reddit(&server).browse("ignored", None).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].url.path(), FEED_PATH);
    }

    #[tokio::test]
    async fn browse_next_cursor_is_the_listing_after() {
        let server = server().await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        assert_eq!(batch.next, Some(cursor("t3_x")));
    }

    #[tokio::test]
    async fn browse_with_cursor_sends_after() {
        let server = server().await;
        reddit(&server)
            .browse("", Some(&cursor("t3_x")))
            .await
            .unwrap();
        assert_eq!(queries(&server).await, ["after=t3_x&limit=25&raw_json=1"]);
    }

    #[tokio::test]
    async fn browse_after_null_ends() {
        let server = server().await;
        let batch = reddit(&server)
            .browse("", Some(&cursor("t3_x")))
            .await
            .unwrap();
        assert_eq!(batch.next, None);
    }

    #[tokio::test]
    async fn browse_empty_after_ends() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"data":{"after":"","children":[]}}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        assert_eq!(batch.next, None);
    }

    /// `over_18`, score below `MIN_SCORE`, an image or preview on an unlisted host, and a
    /// non-image link are all dropped; the rest keep listing order.
    #[tokio::test]
    async fn browse_keeps_only_listed_sfw_image_posts_in_order() {
        let server = server().await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        let ids: Vec<_> = batch.items.iter().map(|i| i.remote_id.as_str()).collect();
        assert_eq!(ids, ["t3_good", "t3_imgur", "t3_small"]);
    }

    #[tokio::test]
    async fn browse_end_batch_has_its_post() {
        let server = server().await;
        let batch = reddit(&server)
            .browse("", Some(&cursor("t3_x")))
            .await
            .unwrap();
        let ids: Vec<_> = batch.items.iter().map(|i| i.remote_id.as_str()).collect();
        assert_eq!(ids, ["t3_last"]);
    }

    /// The smallest resolution at least 320 px wide, with `&amp;` unescaped, even when the
    /// resolutions are out of order.
    #[tokio::test]
    async fn browse_preview_is_smallest_resolution_at_least_320_wide() {
        let server = server().await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        assert_eq!(
            batch.items[0].preview_url,
            "https://preview.redd.it/good.jpg?width=320&s=c"
        );
    }

    #[tokio::test]
    async fn browse_preview_falls_back_to_image_without_preview() {
        let server = server().await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        assert_eq!(batch.items[1].preview_url, "https://i.imgur.com/abc123.jpg");
    }

    #[tokio::test]
    async fn browse_preview_falls_back_to_image_when_all_resolutions_are_small() {
        let server = server().await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        assert_eq!(batch.items[2].preview_url, "https://i.redd.it/small.png");
    }

    #[tokio::test]
    async fn browse_item_fields_come_from_the_post() {
        let server = server().await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        let item = &batch.items[0];
        assert_eq!(
            (
                &item.image,
                item.source_url.as_str(),
                item.title.as_str(),
                item.author.as_str(),
                item.width,
                item.height,
                item.nsfw,
            ),
            (
                &ImageRef::Direct("https://i.redd.it/good.jpg".into()),
                "https://www.reddit.com/r/wallpapers/comments/good/mountain_lake/",
                "Mountain lake",
                "user1",
                Some(3840),
                Some(2160),
                false,
            )
        );
    }

    #[tokio::test]
    async fn browse_item_without_preview_has_no_size() {
        let server = server().await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        assert_eq!((batch.items[1].width, batch.items[1].height), (None, None));
    }

    #[tokio::test]
    async fn browse_unsafe_name_is_hashed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"data":{"after":null,"children":[{"data":{"name":"../t3_x","score":100,
                    "url":"https://i.redd.it/x.jpg"}}]}}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        assert_eq!(
            batch.items[0].remote_id,
            kakemono_sources::sanitize_remote_id("../t3_x")
        );
    }

    #[tokio::test]
    async fn browse_post_without_name_is_dropped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"data":{"after":null,"children":[{"data":{"score":100,
                    "url":"https://i.redd.it/x.jpg"}}]}}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let batch = reddit(&server).browse("", None).await.unwrap();
        assert!(batch.items.is_empty(), "{:?}", batch.items);
    }

    /// 429 fails at once (one request, no retry sleep) with the `Retry-After` hint.
    #[tokio::test]
    async fn browse_rate_limit_fails_fast_with_hint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
            .expect(1)
            .mount(&server)
            .await;
        let err = reddit(&server).browse("", None).await.unwrap_err();
        assert!(
            matches!(
                &err,
                ProviderError::RateLimitedFor {
                    retry_after_secs: Some(120),
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn browse_rate_limit_message() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let err = reddit(&server).browse("", None).await.unwrap_err();
        assert_eq!(err.to_string(), "reddit: rate limited (status 503)");
    }

    #[tokio::test]
    async fn browse_server_error_is_a_status_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let err = reddit(&server).browse("", None).await.unwrap_err();
        assert_eq!(err.to_string(), "reddit: unexpected status 500");
    }

    #[test]
    fn browse_support_is_paged() {
        let reddit = Reddit::for_subreddit(
            "pics".parse().unwrap(),
            kakemono_sources::http::client_builder().build().unwrap(),
        );
        assert_eq!(reddit.browse_support(), BrowseSupport::Paged);
    }

    #[test]
    fn media_hosts_are_reddit_and_imgur_image_hosts() {
        let reddit = Reddit::for_subreddit(
            "pics".parse().unwrap(),
            kakemono_sources::http::client_builder().build().unwrap(),
        );
        assert_eq!(
            reddit.media_hosts(),
            ["i.redd.it", "preview.redd.it", "i.imgur.com"]
        );
    }

    /// The background `fetch` path is unchanged: 100 posts, no `raw_json`.
    #[tokio::test]
    async fn fetch_still_asks_for_100_posts() {
        let server = server().await;
        reddit(&server).fetch("").await.unwrap();
        assert_eq!(queries(&server).await, ["limit=100"]);
    }
}
