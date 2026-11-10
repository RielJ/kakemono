//! Wiremock tests for the shared retry helper, `download`, and the case-replay helper in
//! `tests/common`. Run: `cargo test -p kakemono-sources -- http download`.
//!
//! Every test name starts with `http_` or `download_` so that filter selects them. Tests use
//! `#[tokio::test(start_paused = true)]` or `Case::serve` (which pauses), so back-off sleeps
//! advance the clock instantly.

mod common;

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use common::Case;
use kakemono_config::schema as cfg;
use kakemono_sources::download::{DownloadError, MIN_IMAGE_BYTES, download};
use kakemono_sources::http::{self, get_with_retry};
use kakemono_sources::{Provider, ProviderError, WallpaperResult};
use reqwest::StatusCode;
use tokio::time::Instant;
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Client for paused-clock tests: shared UA, no timeout (a pending timeout timer would fire as
/// soon as the paused runtime waits on the socket).
fn paused_client() -> reqwest::Client {
    http::client_builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap()
}

/// Asserts the paused clock moved by `expected`, allowing a few ms of timer-wheel rounding.
#[track_caller]
fn assert_waited(start: Instant, expected: Duration) {
    let waited = start.elapsed();
    assert!(
        waited >= expected && waited <= expected + Duration::from_millis(50),
        "waited {waited:?}, expected {expected:?}"
    );
}

async fn requests_to(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

// ---- retry ----------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn http_429_with_retry_after_seconds_waits_that_long() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "2"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let start = Instant::now();
    let resp = get_with_retry(&paused_client(), server.uri())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_waited(start, Duration::from_secs(2));
    assert_eq!(requests_to(&server).await, 2);
}

#[tokio::test(start_paused = true)]
async fn http_503_with_retry_after_date_waits_until_then() {
    let server = MockServer::start().await;
    let at = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(3));
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", at.as_str()))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let start = Instant::now();
    let resp = get_with_retry(&paused_client(), server.uri())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    // The HTTP-date has 1 s resolution, so "now + 3 s" is 2..=3 s away.
    let waited = start.elapsed();
    assert!(
        waited >= Duration::from_secs(2) && waited <= Duration::from_secs(3),
        "{waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn http_without_retry_after_backs_off_1_2_4_then_returns_last_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let start = Instant::now();
    let resp = get_with_retry(&paused_client(), server.uri())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_waited(start, Duration::from_secs(1 + 2 + 4));
    assert_eq!(requests_to(&server).await, 4);
}

#[tokio::test(start_paused = true)]
async fn http_retry_after_above_cap_is_not_waited_for() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .mount(&server)
        .await;

    let start = Instant::now();
    let resp = get_with_retry(&paused_client(), server.uri())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_waited(start, Duration::ZERO);
    assert_eq!(requests_to(&server).await, 1);
}

#[tokio::test(start_paused = true)]
async fn http_other_errors_are_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let resp = get_with_retry(&paused_client(), server.uri())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(requests_to(&server).await, 1);
}

#[tokio::test]
async fn http_client_sends_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    get_with_retry(&http::client().unwrap(), server.uri())
        .await
        .unwrap();

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs[0].headers.get("user-agent").unwrap(), http::USER_AGENT);
}

// ---- download -------------------------------------------------------------------------------

fn jpeg(bytes: usize) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(vec![0xAB; bytes], "image/jpeg")
}

const BIG: usize = MIN_IMAGE_BYTES as usize + 1;

fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn download_writes_image_with_url_file_name() {
    let server = MockServer::start().await;
    Mock::given(path("/full/ab/wallhaven-ab12cd.jpg"))
        .respond_with(jpeg(BIG))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();

    let url = format!("{}/full/ab/wallhaven-ab12cd.jpg?token=x", server.uri());
    let got = download(&http::client().unwrap(), &url, dir.path())
        .await
        .unwrap();

    assert_eq!(got, dir.path().join("wallhaven-ab12cd.jpg"));
    assert_eq!(std::fs::metadata(&got).unwrap().len(), BIG as u64);
    assert_eq!(
        files_in(dir.path()),
        ["wallhaven-ab12cd.jpg"],
        "no .part file left"
    );
}

#[tokio::test]
async fn download_sanitizes_file_name() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(jpeg(BIG))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();

    let url = format!("{}/img/a:b*c|d.jpg", server.uri());
    let got = download(&http::client().unwrap(), &url, dir.path())
        .await
        .unwrap();

    assert_eq!(got.file_name().unwrap(), "a_b_c_d.jpg");
}

#[tokio::test]
async fn download_adds_extension_from_content_type() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![1; BIG], "image/png"))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();

    let url = format!("{}/photo-123?ixid=abc", server.uri());
    let got = download(&http::client().unwrap(), &url, dir.path())
        .await
        .unwrap();

    assert_eq!(got.file_name().unwrap(), "photo-123.png");
}

#[tokio::test]
async fn download_rejects_non_image() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(vec![b'x'; BIG], "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();

    let err = download(
        &http::client().unwrap(),
        &format!("{}/a.jpg", server.uri()),
        dir.path(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(err, DownloadError::NotImage { ref content_type, .. } if content_type.starts_with("text/html")),
        "{err}"
    );
    assert!(files_in(dir.path()).is_empty());
}

#[tokio::test]
async fn download_rejects_too_small() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(jpeg(MIN_IMAGE_BYTES as usize - 1))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();

    let err = download(
        &http::client().unwrap(),
        &format!("{}/a.jpg", server.uri()),
        dir.path(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(err, DownloadError::TooSmall { bytes, .. } if bytes == MIN_IMAGE_BYTES - 1),
        "{err}"
    );
    assert!(files_in(dir.path()).is_empty(), "partial file removed");
}

#[tokio::test]
async fn download_rejects_error_status() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();

    let err = download(
        &http::client().unwrap(),
        &format!("{}/a.jpg", server.uri()),
        dir.path(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(err, DownloadError::Status { status: 404, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn download_suffixes_colliding_names() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(jpeg(BIG))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let client = http::client().unwrap();
    let url = format!("{}/a.jpg", server.uri());

    let first = download(&client, &url, dir.path()).await.unwrap();
    let second = download(&client, &url, dir.path()).await.unwrap();
    let third = download(&client, &url, dir.path()).await.unwrap();

    assert_eq!(first.file_name().unwrap(), "a.jpg");
    assert_eq!(second.file_name().unwrap(), "a-1.jpg");
    assert_eq!(third.file_name().unwrap(), "a-2.jpg");
}

#[tokio::test(start_paused = true)]
async fn download_retries_429() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "2"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(jpeg(BIG))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();

    let start = Instant::now();
    let got = download(
        &paused_client(),
        &format!("{}/a.jpg", server.uri()),
        dir.path(),
    )
    .await
    .unwrap();

    assert_waited(start, Duration::from_secs(2));
    assert!(got.exists());
}

#[tokio::test]
async fn download_creates_missing_dir() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(jpeg(BIG))
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("cache/wallpapers");

    let got = download(
        &http::client().unwrap(),
        &format!("{}/a.jpg", server.uri()),
        &dir,
    )
    .await
    .unwrap();

    assert_eq!(got, dir.join("a.jpg"));
}

// ---- case-replay helper self-test -----------------------------------------------------------

/// Minimal provider shaped like the real ones: base URL injected, one JSON call, 429 handled
/// by the retry helper, links built from the body.
struct EchoProvider {
    client: reqwest::Client,
    base: String,
}

#[derive(serde::Deserialize)]
struct EchoBody {
    page: String,
    image: String,
}

#[async_trait]
impl Provider for EchoProvider {
    fn name(&self) -> &'static str {
        "echo"
    }
    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        let url = http::url_with_query(
            &format!("{}/api/v1/search", self.base),
            &[("q", query), ("n", "1")],
        )
        .map_err(|e| ProviderError::Other(format!("echo: {e}")))?;
        let resp = http::get_with_retry(&self.client, url)
            .await
            .map_err(|e| ProviderError::request("echo", e))?;
        if !resp.status().is_success() {
            return Err(ProviderError::status("echo", resp.status()));
        }
        let body: EchoBody = http::read_json(resp, "echo").await?;
        Ok(vec![WallpaperResult {
            source_url: body.page,
            image_url: body.image,
            source: self.name().to_owned(),
            ..WallpaperResult::default()
        }])
    }
}

fn echo_case(dir: &std::path::Path, status: u16, expected: &str) -> Case {
    std::fs::write(
        dir.join("body.json"),
        r#"{"page":"http://{{HOST}}/w/abc","image":"https://w.example/full/abc.jpg"}"#,
    )
    .unwrap();
    // Go recorded https:// URLs and its own UA; neither should matter to the comparison.
    let json = format!(
        r#"{{
          "go_test": "TestEcho", "provider": "echo", "config": {{"enabled": true, "api_key": "k"}},
          "query": "nature",
          "routes": [
            {{"method": "GET", "host": "echo.example", "path": "/api/v1/search",
              "status": {status}, "content_type": "application/json",
              "headers": {{"Retry-After": "2"}}, "body_file": "body.json"}}
          ],
          "requests": [
            {{"method": "GET", "url": "https://echo.example/api/v1/search?n=1&q=nature",
              "user_agent": "wallpaperd/1.0"}}
          ],
          "expected": {expected}
        }}"#
    );
    Case::parse("echo/selftest", dir.to_owned(), &json)
}

#[tokio::test]
async fn http_case_replay_helper_matches_results() {
    let tmp = tempfile::tempdir().unwrap();
    let case = echo_case(
        tmp.path(),
        200,
        r#"{"results": [{"source_url": "https://echo.example/w/abc",
            "image_url": "https://w.example/full/abc.jpg", "title": "", "author": "",
            "source": "echo"}]}"#,
    );
    assert_eq!(case.config::<serde_json::Value>()["api_key"], "k");

    let replay = case.serve().await;
    let p = EchoProvider {
        client: replay.client(),
        base: replay.base_url(),
    };
    replay.verify(p.fetch(&case.query).await).await;
}

#[tokio::test]
async fn http_case_replay_helper_matches_errors_after_retries() {
    let tmp = tempfile::tempdir().unwrap();
    let case = echo_case(
        tmp.path(),
        429,
        r#"{"error": "echo: unexpected status 429"}"#,
    );

    let replay = case.serve().await;
    let p = EchoProvider {
        client: replay.client(),
        base: replay.base_url(),
    };
    let start = Instant::now();
    replay.verify(p.fetch(&case.query).await).await;
    // Retry-After: 2 on every attempt: 3 retries, 6 s of paused time, 4 identical requests
    // that the set comparison collapses into Go's one.
    assert_waited(start, Duration::from_secs(6));
    assert_eq!(replay.server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
#[should_panic(expected = "requests (method, host, path, query)")]
async fn http_case_replay_helper_catches_wrong_query() {
    let tmp = tempfile::tempdir().unwrap();
    let case = echo_case(
        tmp.path(),
        500,
        r#"{"error": "echo: unexpected status 500"}"#,
    );
    let replay = case.serve().await;
    let p = EchoProvider {
        client: replay.client(),
        base: replay.base_url(),
    };
    p.fetch("other").await.unwrap_err();
    replay.assert_requests().await;
}

#[tokio::test]
async fn http_case_replay_helper_loads_go_fixtures() {
    let names = Case::names("bing");
    assert!(names.iter().any(|n| n == "parses_json"), "{names:?}");
    for provider in ["apod", "bing", "reddit", "unsplash", "wallhaven"] {
        for name in Case::names(provider) {
            let case = Case::load(provider, &name);
            assert_eq!(case.provider, provider, "{}", case.id);
            match provider {
                "apod" => drop(case.config::<cfg::ApodConfig>()),
                "bing" => drop(case.config::<cfg::BingConfig>()),
                // Reddit has no config section: a row names one subreddit.
                "reddit" => assert!(
                    case.config::<serde_json::Value>()["subreddits"].is_array(),
                    "{}",
                    case.id
                ),
                "unsplash" => drop(case.config::<cfg::UnsplashConfig>()),
                _ => drop(case.config::<cfg::WallhavenConfig>()),
            }
            assert!(
                case.expected.results.is_some() != case.expected.error.is_some(),
                "{}: exactly one of results/error",
                case.id
            );
            for route in &case.routes {
                if let Some(f) = &route.body_file {
                    case.body(f);
                }
            }
        }
    }
}
