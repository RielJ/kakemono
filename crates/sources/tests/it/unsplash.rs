//! Unsplash provider against the Go-captured fixtures in `tests/fixtures/unsplash/`.

use crate::common;

use common::Case;
use kakemono_config::schema::UnsplashConfig;
use kakemono_sources::unsplash::{NO_ACCESS_KEY, Unsplash};
use kakemono_sources::{Provider, ProviderError};

/// Every case file in `tests/fixtures/unsplash/`; one `#[tokio::test]` each below.
const CASES: [&str; 3] = ["api_random", "api_unauthorized", "nokey_redirect"];

fn provider(case: &Case, replay: &common::Replay) -> Unsplash {
    Unsplash::with_base_url(
        case.config::<UnsplashConfig>(),
        replay.client(),
        replay.base_url(),
    )
}

async fn run(name: &str) {
    let case = Case::load("unsplash", name);
    let replay = case.serve().await;
    replay
        .verify(provider(&case, &replay).fetch(&case.query).await)
        .await;
}

/// A hand-written API case answering `/photos/random` with `status` and `body_file`.
fn api_case(id: &str, status: u16, body_file: &str, expected: &str) -> Case {
    let json = format!(
        r#"{{
            "go_test": "hand-written",
            "provider": "unsplash",
            "config": {{"enabled": true, "access_key": "test-access-key"}},
            "query": "",
            "routes": [{{"method": "GET", "host": "api.unsplash.com", "path": "/photos/random",
                         "status": {status}, "content_type": "application/json",
                         "body_file": "{body_file}"}}],
            "requests": [{{"method": "GET", "user_agent": "",
                "url": "https://api.unsplash.com/photos/random?client_id=test-access-key&count=10&orientation=landscape"}}],
            "expected": {expected}
        }}"#
    );
    Case::parse(id, common::fixtures_dir().join("unsplash"), &json)
}

#[test]
fn every_fixture_case_has_a_test() {
    assert_eq!(Case::names("unsplash"), CASES);
}

#[tokio::test]
async fn api_random() {
    run("api_random").await
}

#[tokio::test]
async fn api_unauthorized() {
    let case = Case::load("unsplash", "api_unauthorized");
    let replay = case.serve().await;
    let err = provider(&case, &replay)
        .fetch(&case.query)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::NotConfigured(_)), "{err:?}");
    replay.verify(Err::<Vec<_>, _>(err)).await;
}

/// Deliberate deviation from Go: the keyless `source.unsplash.com` endpoint is dead, so the
/// recorded Go behaviour (HEAD + `Location`) is not reproduced. Without a key `fetch` fails
/// with `NotConfigured` and sends no request.
#[tokio::test]
async fn nokey_redirect() {
    let case = Case::load("unsplash", "nokey_redirect");
    assert!(case.expected.results.is_some(), "Go recorded a result here");
    let replay = case.serve().await;
    let err = provider(&case, &replay)
        .fetch(&case.query)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::NotConfigured(_)), "{err:?}");
    assert_eq!(err.to_string(), NO_ACCESS_KEY);
    assert!(replay.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn blank_access_key_is_not_configured() {
    let unsplash = Unsplash::with_base_url(
        UnsplashConfig {
            access_key: "  ".into(),
        },
        reqwest::Client::new(),
        // Unroutable: the test fails if a request is attempted.
        "http://127.0.0.1:1".parse().unwrap(),
    );
    let err = unsplash.fetch("forest").await.unwrap_err();
    assert_eq!(err.to_string(), "unsplash requires access_key");
}

#[tokio::test]
async fn forbidden_is_invalid_key() {
    let case = api_case(
        "unsplash/forbidden",
        403,
        "unauthorized.json",
        r#"{"error": "unsplash api: invalid or revoked access key (status 403)"}"#,
    );
    let replay = case.serve().await;
    replay
        .verify(provider(&case, &replay).fetch(&case.query).await)
        .await;
}

/// 429 is retried by `send_with_retry` (paused clock), then reported with Go's text.
#[tokio::test]
async fn rate_limited() {
    let case = api_case(
        "unsplash/rate_limited",
        429,
        "unauthorized.json",
        r#"{"error": "unsplash api: rate limit exceeded"}"#,
    );
    let replay = case.serve().await;
    let err = provider(&case, &replay)
        .fetch(&case.query)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited(_)), "{err:?}");
    replay.verify(Err::<Vec<_>, _>(err)).await;
    assert_eq!(replay.server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn other_status_is_unexpected() {
    let case = api_case(
        "unsplash/server_error",
        500,
        "unauthorized.json",
        r#"{"error": "unsplash api: unexpected status 500"}"#,
    );
    let replay = case.serve().await;
    replay
        .verify(provider(&case, &replay).fetch(&case.query).await)
        .await;
}

#[tokio::test]
async fn non_array_body_is_a_decode_error() {
    // `unauthorized.json` is an object, not the array `/photos/random?count=10` returns.
    let case = api_case(
        "unsplash/bad_body",
        200,
        "unauthorized.json",
        r#"{"results": []}"#,
    );
    let replay = case.serve().await;
    let err = provider(&case, &replay).fetch("").await.unwrap_err();
    assert!(matches!(err, ProviderError::Decode { .. }), "{err:?}");
    assert!(
        err.to_string().starts_with("unsplash api: decode: "),
        "{err}"
    );
    replay.assert_requests().await;
}

#[test]
fn name_is_unsplash() {
    let unsplash = Unsplash::new(UnsplashConfig::default(), reqwest::Client::new());
    assert_eq!(unsplash.name(), "unsplash");
}

/// A transport error never shows the access key (it travels in the query as `client_id`).
#[tokio::test]
async fn transport_error_hides_the_key() {
    let unsplash = Unsplash::with_base_url(
        UnsplashConfig {
            access_key: "testkey".into(),
        },
        reqwest::Client::new(),
        common::hang_up_server().await,
    );
    let err = unsplash.fetch("forest").await.unwrap_err();
    assert!(matches!(err, ProviderError::Request { .. }), "{err:?}");
    common::assert_key_absent(&err, "testkey");
}
