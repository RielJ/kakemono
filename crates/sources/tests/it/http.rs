//! Wiremock tests for the shared retry helper, `download`, and the case-replay helper in
//! `tests/common`. Run: `cargo test -p kakemono-sources -- http download`.
//!
//! Every test name starts with `http_` or `download_` so that filter selects them. Tests use
//! `#[tokio::test(start_paused = true)]` or `Case::serve` (which pauses), so back-off sleeps
//! advance the clock instantly.

use crate::common;

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use common::Case;
use kakemono_config::schema as cfg;
use kakemono_sources::download::{DownloadError, MAX_IMAGE_BYTES, MIN_IMAGE_BYTES, download};
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

/// A JSON or HTML body over `MAX_API_BODY_BYTES` is rejected instead of buffered.
#[tokio::test]
async fn http_read_json_rejects_body_over_cap() {
    let server = MockServer::start().await;
    let cap = usize::try_from(http::MAX_API_BODY_BYTES).unwrap();
    let mut body = vec![b' '; cap];
    body.extend_from_slice(b"{}");
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(&server)
        .await;
    let client = http::client().unwrap();

    let response = get_with_retry(&client, server.uri()).await.unwrap();
    let err = http::read_json::<serde_json::Value>(response, "echo")
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::TooLarge { .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!("echo: response larger than {cap} bytes")
    );

    let response = get_with_retry(&client, server.uri()).await.unwrap();
    let err = http::read_text(response, "echo").await.unwrap_err();
    assert!(matches!(err, ProviderError::TooLarge { .. }), "{err:?}");
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

/// `DownloadError` redacts secret query values itself (audit: callers had to patch the text).
#[tokio::test]
async fn download_errors_redact_secret_query_values() {
    let server = MockServer::start().await;
    Mock::given(path("/404.jpg"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(path("/page.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![b'x'; BIG], "text/html"))
        .mount(&server)
        .await;
    Mock::given(path("/tiny.jpg"))
        .respond_with(jpeg(10))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let client = http::client().unwrap();

    for name in ["404.jpg", "page.jpg", "tiny.jpg"] {
        let url = format!("{}/{name}?w=1&X-Amz-Signature=s3cret", server.uri());
        let err = download(&client, &url, dir.path()).await.unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(!text.contains("s3cret"), "{name}: {text}");
        assert!(
            err.to_string()
                .contains(&format!("/{name}?w=1&X-Amz-Signature=REDACTED")),
            "{name}: {err}"
        );
    }

    // An unparsable URL loses its whole query.
    let err = download(&client, "not a url?sig=s3cret", dir.path())
        .await
        .unwrap_err();
    assert!(matches!(err, DownloadError::InvalidUrl { .. }), "{err:?}");
    assert!(!format!("{err} {err:?}").contains("s3cret"), "{err:?}");
}

/// One-shot `image/jpeg` server on a port it keeps bound. It sends `content_length` (if any)
/// in the head, then `body_len` bytes, then closes. Without a length the body ends at close.
async fn raw_jpeg_server(content_length: Option<u64>, body_len: u64) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => request.extend_from_slice(&buf[..n]),
            }
        }
        let length = content_length.map_or(String::new(), |n| format!("Content-Length: {n}\r\n"));
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\n{length}Connection: close\r\n\r\n"
        );
        if socket.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        let chunk = vec![0xAB; 64 * 1024];
        let mut sent = 0;
        while sent < body_len {
            let n = chunk.len().min(usize::try_from(body_len - sent).unwrap());
            if socket.write_all(&chunk[..n]).await.is_err() {
                return; // the client gave up
            }
            sent += n as u64;
        }
    });
    format!("http://{addr}/big.jpg")
}

/// An endless (here: just over the cap) body with no `Content-Length` is cut off at
/// `MAX_IMAGE_BYTES` and leaves nothing on disk.
#[tokio::test]
async fn download_stops_streaming_at_size_cap() {
    let url = raw_jpeg_server(None, MAX_IMAGE_BYTES + 64 * 1024).await;
    let dir = tempfile::tempdir().unwrap();

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        download(&http::client().unwrap(), &url, dir.path()),
    )
    .await
    .expect("test deadline");

    let err = result.expect_err("an image over the cap must be rejected");
    assert!(
        matches!(err, DownloadError::TooLarge { limit, .. } if limit == MAX_IMAGE_BYTES),
        "{err}"
    );
    assert!(files_in(dir.path()).is_empty(), "partial file removed");
}

/// A `Content-Length` over the cap is rejected from the head, before any body byte is read:
/// the server here never sends the body it announced.
#[tokio::test]
async fn download_rejects_announced_size_over_cap() {
    let url = raw_jpeg_server(Some(MAX_IMAGE_BYTES + 1), 0).await;
    let dir = tempfile::tempdir().unwrap();

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        download(&http::client().unwrap(), &url, dir.path()),
    )
    .await
    .expect("test deadline");

    let err = result.expect_err("an announced size over the cap must be rejected");
    assert!(
        matches!(err, DownloadError::TooLarge { limit, .. } if limit == MAX_IMAGE_BYTES),
        "{err}"
    );
    assert!(files_in(dir.path()).is_empty());
}

// ---- case-replay helper self-test -----------------------------------------------------------

/// Minimal provider shaped like the real ones: base URL injected, one JSON call, 429 handled
/// by the retry helper, links built from the body.
struct EchoProvider {
    client: reqwest::Client,
    base: url::Url,
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
        let url = http::endpoint(&self.base, "/api/v1/search", &[("q", query), ("n", "1")]);
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

/// A server answering every request with `status` and `Retry-After: 60`, expecting `expect`
/// requests.
async fn fail_fast_server(status: u16, expect: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(status).insert_header("Retry-After", "60"))
        .expect(expect)
        .mount(&server)
        .await;
    server
}

// Real clock on purpose: a paused clock would hide a sleep instead of failing.
#[tokio::test]
async fn http_send_fail_fast_returns_429_at_once_with_hint() {
    let server = fail_fast_server(429, 1).await;
    let start = std::time::Instant::now();
    let err = http::send_fail_fast(paused_client().get(server.uri()), "unsplash api")
        .await
        .unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert!(
        matches!(
            &err,
            ProviderError::RateLimitedFor { message, retry_after_secs: Some(60) }
                if message == "unsplash api: rate limited (status 429)"
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn http_send_fail_fast_maps_503_to_rate_limited() {
    let server = fail_fast_server(503, 1).await;
    let err = http::send_fail_fast(paused_client().get(server.uri()), "bing")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "bing: rate limited (status 503)");
}

#[tokio::test]
async fn http_send_fail_fast_without_header_has_no_hint() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let err = http::send_fail_fast(paused_client().get(server.uri()), "bing")
        .await
        .unwrap_err();
    assert!(
        matches!(
            &err,
            ProviderError::RateLimitedFor { message, retry_after_secs: None }
                if message == "bing: rate limited (status 429)"
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn http_send_fail_fast_passes_other_statuses_through() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    let response = http::send_fail_fast(paused_client().get(server.uri()), "bing")
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn http_send_fail_fast_transport_error_is_request_with_redacted_url() {
    let base = common::hang_up_server().await;
    let url = format!("{base}/x?apikey=s3cret");
    let err = http::send_fail_fast(paused_client().get(url), "wallhaven")
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Request { .. }), "{err:?}");
    common::assert_key_absent(&err, "s3cret");
}

#[tokio::test]
async fn http_retry_after_secs_reads_response_header() {
    let server = fail_fast_server(429, 1).await;
    let response = paused_client().get(server.uri()).send().await.unwrap();
    assert_eq!(http::retry_after_secs(response.headers()), Some(60));
}
