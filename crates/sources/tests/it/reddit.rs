//! Reddit provider against the Go-captured fixtures in `tests/fixtures/reddit/`.

use crate::common;

use common::Case;
use kakemono_sources::reddit::Reddit;
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

fn subreddit(case: &Case) -> String {
    let cfg = case.config::<CaseConfig>();
    assert_eq!(
        cfg.subreddits.len(),
        1,
        "{}: one subreddit per row",
        case.id
    );
    cfg.subreddits[0].clone()
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
    let reddit = Reddit::for_subreddit("EarthPorn", reqwest::Client::new());
    assert_eq!(reddit.name(), "reddit");
}

/// An empty subreddit name: error, and no request is sent (Go `TestReddit_NoSubreddits`).
#[tokio::test]
async fn empty_subreddit_is_an_error() {
    let reddit = Reddit::for_subreddit_with_base_url(
        "",
        reqwest::Client::new(),
        // Unroutable: the test fails if a request is attempted.
        "http://127.0.0.1:1",
    );
    let err = reddit.fetch("").await.unwrap_err();
    assert!(matches!(err, ProviderError::Other(_)), "{err:?}");
    assert_eq!(err.to_string(), "reddit: no subreddit configured");
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
