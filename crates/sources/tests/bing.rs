//! Bing provider against the Go-captured fixtures in `tests/fixtures/bing`, plus hand-written
//! cases for behaviour the Go tests did not cover. Run: `cargo test -p kakemono-sources --test bing`.

mod common;

use common::Case;
use kakemono_config::schema::BingConfig;
use kakemono_sources::Provider;
use kakemono_sources::bing::Bing;

/// Every `*.case.json` in `fixtures/bing`; `every_fixture_case_has_a_test` keeps this in sync.
const CASES: &[&str] = &[
    "empty_images",
    "http_error",
    "parses_json",
    "uhd_resolution",
];

async fn replay((case, _dir): (Case, tempfile::TempDir)) {
    let replay = case.serve().await;
    let bing = Bing::with_base_url(
        case.config::<BingConfig>(),
        replay.client(),
        replay.base_url(),
    );
    replay.verify(bing.fetch(&case.query).await).await;
}

async fn run(name: &str) {
    let case = Case::load("bing", name);
    let replay = case.serve().await;
    let bing = Bing::with_base_url(
        case.config::<BingConfig>(),
        replay.client(),
        replay.base_url(),
    );
    replay.verify(bing.fetch(&case.query).await).await;
}

#[test]
fn every_fixture_case_has_a_test() {
    assert_eq!(Case::names("bing"), CASES);
}

#[tokio::test]
async fn empty_images() {
    run("empty_images").await;
}

#[tokio::test]
async fn http_error() {
    run("http_error").await;
}

#[tokio::test]
async fn parses_json() {
    run("parses_json").await;
}

#[tokio::test]
async fn uhd_resolution() {
    run("uhd_resolution").await;
}

/// A hand-written case whose single route serves `body` from a temp dir.
/// Returns the temp dir too: it must outlive the replay.
fn custom_case(
    config: &str,
    status: u16,
    body: &str,
    requests: &str,
    expected: &str,
) -> (Case, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("body.json"), body).unwrap();
    let json = format!(
        r#"{{
          "go_test": "hand-written", "provider": "bing", "config": {config}, "query": "",
          "routes": [{{"method": "GET", "host": "www.bing.com", "path": "/HPImageArchive.aspx",
                      "status": {status}, "content_type": "application/json", "body_file": "body.json"}}],
          "requests": {requests},
          "expected": {expected}
        }}"#
    );
    (
        Case::parse("bing/custom", dir.path().to_owned(), &json),
        dir,
    )
}

const DEFAULT_REQUEST: &str = r#"[{"method": "GET", "user_agent": "",
  "url": "https://www.bing.com/HPImageArchive.aspx?format=js&idx=0&n=8&mkt=en-US"}]"#;

#[tokio::test]
async fn empty_market_falls_back_to_en_us() {
    replay(custom_case(
        r#"{"enabled": true, "market": ""}"#,
        200,
        r#"{"images":[{"title":"T","url":"/th?id=OHR.X_1920x1080.jpg"}]}"#,
        DEFAULT_REQUEST,
        r#"{"results": [{"source_url": "", "image_url": "https://www.bing.com/th?id=OHR.X_UHD.jpg",
            "title": "T", "author": "", "source": "bing"}]}"#,
    ))
    .await;
}

#[tokio::test]
async fn market_is_sent_as_mkt() {
    replay(custom_case(
        r#"{"enabled": true, "market": "ja-JP"}"#,
        200,
        r#"{"images":[{"title":"T","url":"/th?id=OHR.X_1920x1080.jpg"}]}"#,
        r#"[{"method": "GET", "user_agent": "",
             "url": "https://www.bing.com/HPImageArchive.aspx?format=js&idx=0&n=8&mkt=ja-JP"}]"#,
        r#"{"results": [{"source_url": "", "image_url": "https://www.bing.com/th?id=OHR.X_UHD.jpg",
            "title": "T", "author": "", "source": "bing"}]}"#,
    ))
    .await;
}

#[tokio::test]
async fn urlbase_used_when_url_has_no_resolution_and_entries_without_url_skipped() {
    replay(custom_case(
        r#"{"enabled": true, "market": "en-US"}"#,
        200,
        r#"{"images":[
            {"title":"NoUrl","urlbase":"/th?id=OHR.Skip"},
            {"title":"Base","url":"/th?id=OHR.Base.jpg","urlbase":"/th?id=OHR.Base",
             "copyright":"© C","copyrightlink":"https://www.bing.com/search?q=base"},
            {"title":"Plain","url":"/th?id=OHR.Plain.jpg"}
        ]}"#,
        DEFAULT_REQUEST,
        r#"{"results": [
            {"source_url": "https://www.bing.com/search?q=base",
             "image_url": "https://www.bing.com/th?id=OHR.Base_UHD.jpg",
             "title": "Base", "author": "© C", "source": "bing"},
            {"source_url": "", "image_url": "https://www.bing.com/th?id=OHR.Plain.jpg",
             "title": "Plain", "author": "", "source": "bing"}
        ]}"#,
    ))
    .await;
}

#[tokio::test]
async fn null_images_is_no_images_error() {
    replay(custom_case(
        r#"{"enabled": true, "market": "en-US"}"#,
        200,
        r#"{"images":null}"#,
        DEFAULT_REQUEST,
        r#"{"error": "bing: no images returned from API"}"#,
    ))
    .await;
}

#[tokio::test]
async fn invalid_json_is_a_decode_error() {
    let (case, _dir) = custom_case(
        r#"{"enabled": true, "market": "en-US"}"#,
        200,
        "not json",
        DEFAULT_REQUEST,
        r#"{"results": []}"#,
    );
    let replay = case.serve().await;
    let bing = Bing::with_base_url(
        case.config::<BingConfig>(),
        replay.client(),
        replay.base_url(),
    );
    let err = bing.fetch("").await.unwrap_err();
    assert!(
        err.to_string().starts_with("bing: decode: "),
        "unexpected error: {err}"
    );
    replay.assert_requests().await;
}

#[tokio::test]
async fn persistent_503_is_retried_then_reported() {
    replay(custom_case(
        r#"{"enabled": true, "market": "en-US"}"#,
        503,
        "",
        DEFAULT_REQUEST,
        r#"{"error": "bing: unexpected status 503"}"#,
    ))
    .await;
}

#[test]
fn name_and_enabled_come_from_config() {
    let client = reqwest::Client::new();
    let on = Bing::new(BingConfig::default(), client.clone());
    assert_eq!(on.name(), "bing");
    assert!(on.enabled(), "Bing is enabled by default");
    let off = Bing::new(
        BingConfig {
            enabled: false,
            ..BingConfig::default()
        },
        client,
    );
    assert!(!off.enabled());
}
