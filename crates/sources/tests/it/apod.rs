//! APOD provider against the fixtures in `tests/fixtures/apod` (synthesised from `apod.go`,
//! which has no Go tests), plus hand-written status cases.
//! Run: `cargo test -p kakemono-sources --test apod`.

use crate::common;

use common::Case;
use kakemono_config::schema::ApodConfig;
use kakemono_sources::apod::Apod;
use kakemono_sources::{Provider, ProviderError};

/// Every `*.case.json` in `fixtures/apod`; `every_fixture_case_has_a_test` keeps this in sync.
const CASES: &[&str] = &["mixed_entries", "rate_limited", "video_only"];

async fn run(name: &str) {
    let case = Case::load("apod", name);
    let replay = case.serve().await;
    let apod = Apod::with_base_url(
        case.config::<ApodConfig>(),
        replay.client(),
        replay.base_url(),
    );
    replay.verify(apod.fetch(&case.query).await).await;
}

#[test]
fn every_fixture_case_has_a_test() {
    assert_eq!(Case::names("apod"), CASES);
}

#[tokio::test]
async fn mixed_entries() {
    run("mixed_entries").await;
}

/// 429 is retried by the shared client, then reported with Go's rate-limit text.
#[tokio::test]
async fn rate_limited() {
    run("rate_limited").await;
}

/// A video day yields no results and no error, as in Go.
#[tokio::test]
async fn video_only() {
    run("video_only").await;
}

/// A hand-written case with one route serving `body` with `status`. The temp dir holds the
/// body file and must outlive the replay.
fn custom_case(config: &str, status: u16, body: &str, url: &str) -> (Case, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("body.json"), body).unwrap();
    let json = format!(
        r#"{{
          "go_test": "hand-written", "provider": "apod", "config": {config}, "query": "",
          "routes": [{{"method": "GET", "host": "api.nasa.gov", "path": "/planetary/apod",
                      "status": {status}, "content_type": "application/json", "body_file": "body.json"}}],
          "requests": [{{"method": "GET", "url": "{url}", "user_agent": ""}}],
          "expected": {{"results": []}}
        }}"#
    );
    (
        Case::parse("apod/custom", dir.path().to_owned(), &json),
        dir,
    )
}

const DEMO_URL: &str = "https://api.nasa.gov/planetary/apod?api_key=DEMO_KEY&count=5";

/// Runs a hand-written case and returns the provider's outcome after checking the requests.
async fn fetch_custom(
    config: &str,
    status: u16,
    body: &str,
    url: &str,
) -> Result<Vec<kakemono_sources::WallpaperResult>, ProviderError> {
    let (case, _dir) = custom_case(config, status, body, url);
    let replay = case.serve().await;
    let apod = Apod::with_base_url(
        case.config::<ApodConfig>(),
        replay.client(),
        replay.base_url(),
    );
    let outcome = apod.fetch("").await;
    replay.assert_requests().await;
    // `serve` paused the clock; resume so a test can replay several cases.
    tokio::time::resume();
    outcome
}

#[tokio::test]
async fn empty_api_key_sends_demo_key() {
    let got = fetch_custom(r#"{"enabled": true, "api_key": ""}"#, 200, "[]", DEMO_URL).await;
    assert!(got.unwrap().is_empty());
}

#[tokio::test]
async fn configured_api_key_is_sent() {
    let got = fetch_custom(
        r#"{"enabled": true, "api_key": "abc 123"}"#,
        200,
        "[]",
        "https://api.nasa.gov/planetary/apod?api_key=abc+123&count=5",
    )
    .await;
    assert!(got.unwrap().is_empty());
}

#[tokio::test]
async fn unauthorized_and_forbidden_are_invalid_key() {
    for status in [401, 403] {
        let err = fetch_custom(
            r#"{"enabled": true, "api_key": "bad"}"#,
            status,
            "{}",
            "https://api.nasa.gov/planetary/apod?api_key=bad&count=5",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProviderError::NotConfigured(_)), "{err:?}");
        assert_eq!(
            err.to_string(),
            format!("apod: invalid API key (status {status})")
        );
    }
}

#[tokio::test]
async fn other_status_is_unexpected_status() {
    let err = fetch_custom(r#"{"enabled": true}"#, 500, "oops", DEMO_URL)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "apod: unexpected status 500");
}

#[tokio::test]
async fn rate_limit_is_the_rate_limited_variant() {
    let err = fetch_custom(r#"{"enabled": true}"#, 429, "{}", DEMO_URL)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited(_)), "{err:?}");
}

#[tokio::test]
async fn non_array_body_is_a_decode_error() {
    let err = fetch_custom(r#"{"enabled": true}"#, 200, r#"{"code":400}"#, DEMO_URL)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Decode { .. }), "{err:?}");
    assert!(err.to_string().starts_with("apod: decode: "), "{err}");
}

#[tokio::test]
async fn null_fields_decode_as_empty() {
    let got = fetch_custom(
        r#"{"enabled": true}"#,
        200,
        r#"[{"media_type":"image","url":"https://x/a.jpg","hdurl":null,"title":null,
             "copyright":null,"date":null}]"#,
        DEMO_URL,
    )
    .await
    .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].image_url, "https://x/a.jpg");
    assert_eq!(got[0].source_url, "");
    assert_eq!(got[0].source, "apod");
}

#[test]
fn name_is_apod() {
    let apod = Apod::new(ApodConfig::default(), reqwest::Client::new());
    assert_eq!(apod.name(), "apod");
}
