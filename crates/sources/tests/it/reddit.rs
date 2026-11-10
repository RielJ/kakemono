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
    let reddit = Reddit::for_subreddit("EarthPorn".parse().unwrap(), reqwest::Client::new());
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
