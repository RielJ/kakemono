//! Wallhaven API mode (Task 14): replays the Go `TestWallhavenAPI_*` cases, plus edge cases Go
//! did not cover (rate limit, bad JSON, entries without `path`).

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::Case;
use kakemono_config::schema::WallhavenConfig;
use kakemono_sources::wallhaven::Wallhaven;
use kakemono_sources::{Provider, ProviderError};

fn provider(case: &Case, replay: &common::Replay) -> Wallhaven {
    Wallhaven::with_base_url(
        case.config::<WallhavenConfig>(),
        replay.client(),
        replay.base_url(),
    )
    .with_scrape_delay(Duration::ZERO)
}

async fn run(name: &str) {
    let case = Case::load("wallhaven", name);
    let replay = case.serve().await;
    let wallhaven = provider(&case, &replay);
    assert!(wallhaven.uses_api(), "{name}: case must be API mode");
    replay.verify(wallhaven.fetch(&case.query).await).await;
}

#[tokio::test]
async fn api_parses_response() {
    run("api_parses_response").await;
}

#[tokio::test]
async fn api_empty_data() {
    run("api_empty_data").await;
}

/// Go's error text contained the request URL with `apikey=testkey`. The port redacts the key
/// (see `.pi/contracts/sources.md`), so the fixture's expected text is redacted the same way
/// here; the committed fixture keeps Go's text.
#[tokio::test]
async fn api_http_error() {
    let mut case = Case::load("wallhaven", "api_http_error");
    let want = case.expected.error.take().unwrap();
    assert!(want.contains("apikey=testkey&"), "{want}");
    case.expected.error = Some(want.replace("apikey=testkey&", "apikey=REDACTED&"));
    let replay = case.serve().await;
    let wallhaven = provider(&case, &replay);
    assert!(wallhaven.uses_api());
    replay.verify(wallhaven.fetch(&case.query).await).await;
}

/// The key appears in no error text: neither the status error (Display, Debug, source chain)
/// nor a transport error.
#[tokio::test]
async fn api_errors_never_contain_the_key() {
    let case = Case::load("wallhaven", "api_http_error");
    assert_eq!(case.config["api_key"], "testkey");
    let replay = case.serve().await;
    let err = provider(&case, &replay).fetch("").await.unwrap_err();
    assert_key_absent(&err);
    assert!(err.to_string().contains("apikey=REDACTED"), "{err}");

    // Transport failure: nothing listens on the port, so reqwest's error carries the URL.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let cfg = WallhavenConfig {
        enabled: true,
        api_key: "testkey".to_owned(),
        ..WallhavenConfig::default()
    };
    let err = Wallhaven::with_base_url(
        cfg,
        reqwest::Client::new(),
        format!("http://127.0.0.1:{port}"),
    )
    .fetch("")
    .await
    .unwrap_err();
    assert!(matches!(err, ProviderError::Request { .. }), "{err:?}");
    assert_key_absent(&err);
    assert!(err.to_string().contains("apikey=REDACTED"), "{err}");
}

fn assert_key_absent(err: &ProviderError) {
    let mut texts = vec![err.to_string(), format!("{err:?}")];
    let mut source = std::error::Error::source(err);
    while let Some(e) = source {
        texts.push(e.to_string());
        texts.push(format!("{e:?}"));
        source = e.source();
    }
    for text in texts {
        assert!(!text.contains("testkey"), "key leaked: {text}");
    }
}

#[tokio::test]
async fn api_query_param() {
    run("api_query_param").await;
}

#[test]
fn api_every_fixture_case_has_a_test() {
    let api: Vec<String> = Case::names("wallhaven")
        .into_iter()
        .filter(|n| n.starts_with("api_"))
        .collect();
    assert_eq!(
        api,
        [
            "api_empty_data",
            "api_http_error",
            "api_parses_response",
            "api_query_param"
        ]
    );
}

/// A hand-written API-mode case: one route on `/api/v1/search` serving `body` with `status`.
fn api_case(dir: &tempfile::TempDir, status: u16, body: &str, expected: &str) -> Case {
    std::fs::write(dir.path().join("body"), body).unwrap();
    let json = format!(
        r#"{{
          "go_test": "none",
          "provider": "wallhaven",
          "config": {{"enabled": true, "api_key": "k", "purity": "", "sorting": "", "categories": ""}},
          "query": "red sky",
          "routes": [{{"method": "GET", "host": "wallhaven.cc", "path": "/api/v1/search",
                       "status": {status}, "content_type": "application/json", "body_file": "body"}}],
          "requests": [{{"method": "GET",
            "url": "https://wallhaven.cc/api/v1/search?apikey=k&categories=111&purity=100&q=red+sky&sorting=random",
            "user_agent": ""}}],
          "expected": {expected}
        }}"#
    );
    Case::parse("wallhaven/hand", PathBuf::from(dir.path()), &json)
}

#[tokio::test]
async fn api_empty_config_values_use_go_defaults_and_skip_pathless_entries() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"{"data":[
        {"id":"a","url":"https://wallhaven.cc/w/a","path":"","uploader":{"username":"x"}},
        {"id":"b","url":"https://wallhaven.cc/w/b","path":"https://w.wallhaven.cc/full/b.jpg"},
        {"id":"c","url":"https://wallhaven.cc/w/c","path":"https://w.wallhaven.cc/full/c.png","uploader":{"username":"u"}}
    ]}"#;
    let expected = r#"{"results": [
        {"source_url":"https://wallhaven.cc/w/b","image_url":"https://w.wallhaven.cc/full/b.jpg","title":"","author":"","source":"wallhaven"},
        {"source_url":"https://wallhaven.cc/w/c","image_url":"https://w.wallhaven.cc/full/c.png","title":"","author":"u","source":"wallhaven"}
    ]}"#;
    let case = api_case(&dir, 200, body, expected);
    let replay = case.serve().await;
    let wallhaven = provider(&case, &replay);
    replay.verify(wallhaven.fetch(&case.query).await).await;
}

#[tokio::test]
async fn api_bad_json_is_a_decode_error() {
    let dir = tempfile::tempdir().unwrap();
    let case = api_case(&dir, 200, "not json", r#"{"error": "unused"}"#);
    let replay = case.serve().await;
    let err = provider(&case, &replay)
        .fetch(&case.query)
        .await
        .unwrap_err();
    assert!(
        err.to_string().starts_with("wallhaven api: decode: "),
        "{err}"
    );
    replay.assert_requests().await;
}

#[tokio::test]
async fn api_rate_limited_after_retries() {
    let dir = tempfile::tempdir().unwrap();
    let case = api_case(&dir, 429, "", r#"{"error": "unused"}"#);
    let replay = case.serve().await;
    let start = tokio::time::Instant::now();
    let err = provider(&case, &replay)
        .fetch(&case.query)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "wallhaven api: rate limited (status 429)");
    assert!(matches!(err, ProviderError::RateLimited(_)));
    // 1 + 3 retries with 1 s, 2 s, 4 s back-off on the paused clock.
    let received = replay.server.received_requests().await.unwrap();
    assert_eq!(received.len(), 4);
    assert!(start.elapsed() >= Duration::from_secs(7));
    replay.assert_requests().await;
}

#[test]
fn name_and_enabled() {
    let client = kakemono_sources::http::client().unwrap();
    let on = Wallhaven::new(
        WallhavenConfig {
            enabled: true,
            ..WallhavenConfig::default()
        },
        client.clone(),
    );
    let off = Wallhaven::new(
        WallhavenConfig {
            enabled: false,
            ..WallhavenConfig::default()
        },
        client,
    );
    assert_eq!(on.name(), "wallhaven");
    assert!(on.enabled());
    assert!(!off.enabled());
}

#[test]
fn api_key_selects_mode() {
    let client = kakemono_sources::http::client().unwrap();
    let keyed = Wallhaven::new(
        WallhavenConfig {
            api_key: "k".into(),
            ..WallhavenConfig::default()
        },
        client.clone(),
    );
    let keyless = Wallhaven::new(WallhavenConfig::default(), client);
    assert!(keyed.uses_api());
    assert!(!keyless.uses_api());
}
