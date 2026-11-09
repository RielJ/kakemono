//! Reddit provider against the Go-captured fixtures in `tests/fixtures/reddit/`.

mod common;

use std::collections::BTreeSet;

use common::Case;
use kakemono_config::schema::RedditConfig;
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

async fn run(name: &str) {
    let case = Case::load("reddit", name);
    let replay = case.serve().await;
    let reddit = Reddit::with_base_url(
        case.config::<RedditConfig>(),
        replay.client(),
        replay.base_url(),
    );
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
    let reddit = Reddit::new(RedditConfig::default(), reqwest::Client::new());
    assert_eq!(reddit.name(), "reddit");
}

/// Go `TestReddit_Enabled`: needs `enabled` and at least one subreddit.
#[test]
fn enabled_requires_subreddits() {
    let cfg = |enabled, subreddits: &[&str]| RedditConfig {
        enabled,
        subreddits: subreddits.iter().map(|s| (*s).to_owned()).collect(),
    };
    let client = reqwest::Client::new();
    assert!(Reddit::new(cfg(true, &["EarthPorn"]), client.clone()).enabled());
    assert!(!Reddit::new(cfg(true, &[]), client.clone()).enabled());
    assert!(!Reddit::new(cfg(false, &["EarthPorn"]), client).enabled());
}

/// Go `TestReddit_NoSubreddits`: error, and no request is sent.
#[tokio::test]
async fn no_subreddits_is_an_error() {
    let reddit = Reddit::with_base_url(
        RedditConfig {
            enabled: true,
            subreddits: vec![],
        },
        reqwest::Client::new(),
        // Unroutable: the test fails if a request is attempted.
        "http://127.0.0.1:1",
    );
    let err = reddit.fetch("").await.unwrap_err();
    assert!(matches!(err, ProviderError::Other(_)), "{err:?}");
    assert_eq!(err.to_string(), "reddit: no subreddits configured");
}

/// Each fetch asks for one of the configured subreddits, and over many fetches all of them.
#[tokio::test]
async fn picks_a_random_configured_subreddit() {
    let json = r#"{
        "go_test": "hand-written",
        "provider": "reddit",
        "config": {"enabled": true, "subreddits": ["EarthPorn", "wallpapers"]},
        "query": "",
        "routes": [
            {"method": "GET", "host": "www.reddit.com", "path": "/r/EarthPorn/hot.json",
             "status": 200, "content_type": "application/json", "body_file": "empty.json"},
            {"method": "GET", "host": "www.reddit.com", "path": "/r/wallpapers/hot.json",
             "status": 200, "content_type": "application/json", "body_file": "empty.json"}
        ],
        "requests": [],
        "expected": {"results": []}
    }"#;
    let case = Case::parse(
        "reddit/random_subreddit",
        common::fixtures_dir().join("reddit"),
        json,
    );
    let replay = case.serve().await;
    let reddit = Reddit::with_base_url(
        case.config::<RedditConfig>(),
        replay.client(),
        replay.base_url(),
    );
    for _ in 0..40 {
        assert_eq!(reddit.fetch("").await.unwrap(), vec![]);
    }
    let requests = replay.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 40);
    let paths: BTreeSet<String> = requests
        .iter()
        .map(|r| {
            assert_eq!(r.url.query(), Some("limit=100"));
            r.url.path().to_owned()
        })
        .collect();
    // Two choices, 40 draws: missing one has probability 2^-39.
    assert_eq!(
        paths,
        BTreeSet::from([
            "/r/EarthPorn/hot.json".to_owned(),
            "/r/wallpapers/hot.json".to_owned()
        ])
    );
}
