//! Bing provider against the Go-captured fixtures in `tests/fixtures/bing`, plus hand-written
//! cases for behaviour the Go tests did not cover. Run: `cargo test -p kakemono-sources --test bing`.

use crate::common;

use common::Case;
use kakemono_config::schema::BingConfig;
use kakemono_sources::bing::Bing;
use kakemono_sources::{Provider, ProviderError};

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

/// A 429 that outlasts the retries is `RateLimited`, so the engine applies its rate-limit
/// backoff (audit M1: Bing used to report it as a plain status error).
#[tokio::test]
async fn rate_limit_is_the_rate_limited_variant() {
    let (case, _dir) = custom_case(
        r#"{"enabled": true, "market": "en-US"}"#,
        429,
        "",
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
    assert!(matches!(err, ProviderError::RateLimited(_)), "{err:?}");
    assert_eq!(err.to_string(), "bing: rate limited (status 429)");
    replay.assert_requests().await;
}

#[test]
fn name_is_bing() {
    let bing = Bing::new(BingConfig::default(), reqwest::Client::new());
    assert_eq!(bing.name(), "bing");
}

// ---- browse (picker Sources tab) ----

mod browse {
    use std::time::Duration;

    use kakemono_config::schema::BingConfig;
    use kakemono_sources::bing::Bing;
    use kakemono_sources::{
        BrowseCursor, BrowseSupport, ImageRef, Provider, ProviderError, sanitize_remote_id,
    };
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::common::fixtures_dir;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(fixtures_dir().join("bing").join(name)).unwrap()
    }

    /// A mock answering the feed for `idx` with `body`, expected exactly once.
    async fn serve_idx(server: &MockServer, idx: &str, body: String) {
        Mock::given(method("GET"))
            .and(path("/HPImageArchive.aspx"))
            .and(query_param("format", "js"))
            .and(query_param("idx", idx))
            .and(query_param("n", "8"))
            .and(query_param("mkt", "en-US"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(server)
            .await;
    }

    fn bing(server: &MockServer) -> Bing {
        Bing::with_base_url(
            BingConfig::default(),
            reqwest::Client::new(),
            server.uri().parse().unwrap(),
        )
    }

    #[tokio::test]
    async fn browse_first_batch_requests_idx_0_and_points_at_idx_8() {
        let server = MockServer::start().await;
        serve_idx(&server, "0", fixture("browse_idx0.json")).await;
        let batch = bing(&server).browse("ignored", None).await.unwrap();
        assert_eq!(batch.next, Some(BrowseCursor("8".to_owned())));
    }

    #[tokio::test]
    async fn browse_first_batch_has_every_image() {
        let server = MockServer::start().await;
        serve_idx(&server, "0", fixture("browse_idx0.json")).await;
        let batch = bing(&server).browse("", None).await.unwrap();
        assert_eq!(batch.items.len(), 8);
    }

    #[tokio::test]
    async fn browse_second_batch_requests_idx_8_and_ends() {
        let server = MockServer::start().await;
        serve_idx(&server, "8", fixture("browse_idx8.json")).await;
        let cursor = BrowseCursor("8".to_owned());
        let batch = bing(&server).browse("", Some(&cursor)).await.unwrap();
        assert_eq!((batch.items.len(), batch.next), (8, None));
    }

    #[tokio::test]
    async fn browse_item_maps_preview_full_and_metadata() {
        let server = MockServer::start().await;
        serve_idx(&server, "0", fixture("browse_idx0.json")).await;
        let batch = bing(&server).browse("", None).await.unwrap();
        let origin = server.uri();
        let item = &batch.items[0];
        assert_eq!(
            (
                item.remote_id.as_str(),
                item.preview_url.as_str(),
                &item.image,
                item.source_url.as_str(),
                item.title.as_str(),
                item.author.as_str(),
                item.width,
                item.height,
                item.nsfw,
            ),
            (
                sanitize_remote_id("/th?id=OHR.Pic0_EN-US1000").as_str(),
                format!("{origin}/th?id=OHR.Pic0_EN-US1000_400x240.jpg").as_str(),
                &ImageRef::Direct(format!("{origin}/th?id=OHR.Pic0_EN-US1000_UHD.jpg")),
                "https://www.bing.com/search?q=pic0",
                "Title 0",
                "Place 0 (© Photographer 0)",
                None,
                None,
                false,
            )
        );
    }

    /// The full image is the same URL `fetch` would download.
    #[tokio::test]
    async fn browse_full_image_matches_fetch() {
        let server = MockServer::start().await;
        Mock::given(path("/HPImageArchive.aspx"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(fixture("browse_idx0.json"), "application/json"),
            )
            .mount(&server)
            .await;
        let bing = bing(&server);
        let fetched: Vec<ImageRef> = bing
            .fetch("")
            .await
            .unwrap()
            .into_iter()
            .map(|r| ImageRef::Direct(r.image_url))
            .collect();
        let browsed: Vec<ImageRef> = bing
            .browse("", None)
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.image)
            .collect();
        assert_eq!(browsed, fetched);
    }

    #[tokio::test]
    async fn browse_entry_without_urlbase_previews_the_feed_url() {
        let server = MockServer::start().await;
        serve_idx(
            &server,
            "0",
            r#"{"images":[{"title":"T","url":"/th?id=OHR.X_1920x1080.jpg"},{"title":"NoUrl"}]}"#
                .to_owned(),
        )
        .await;
        let batch = bing(&server).browse("", None).await.unwrap();
        let origin = server.uri();
        let got: Vec<(String, String)> = batch
            .items
            .iter()
            .map(|i| (i.remote_id.clone(), i.preview_url.clone()))
            .collect();
        assert_eq!(
            got,
            [(
                sanitize_remote_id("/th?id=OHR.X_1920x1080.jpg"),
                format!("{origin}/th?id=OHR.X_1920x1080.jpg")
            )]
        );
    }

    /// An empty feed is the end of the archive, not an error (unlike `fetch`).
    #[tokio::test]
    async fn browse_empty_feed_ends_cleanly() {
        let server = MockServer::start().await;
        serve_idx(&server, "0", r#"{"images":[]}"#.to_owned()).await;
        let batch = bing(&server).browse("", None).await.unwrap();
        assert_eq!((batch.items.len(), batch.next), (0, None));
    }

    /// A short first batch means there is no second one.
    #[tokio::test]
    async fn browse_short_first_batch_ends() {
        let server = MockServer::start().await;
        serve_idx(
            &server,
            "0",
            r#"{"images":[{"url":"/th?id=OHR.A_1920x1080.jpg","urlbase":"/th?id=OHR.A"}]}"#
                .to_owned(),
        )
        .await;
        let batch = bing(&server).browse("", None).await.unwrap();
        assert_eq!((batch.items.len(), batch.next), (1, None));
    }

    #[tokio::test]
    async fn browse_unknown_cursor_is_an_error_without_a_request() {
        let server = MockServer::start().await;
        let cursor = BrowseCursor("16".to_owned());
        let err = bing(&server).browse("", Some(&cursor)).await.unwrap_err();
        assert_eq!(err.to_string(), "bing: invalid browse cursor \"16\"");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// 429 is reported at once, with the `Retry-After` hint and no retry (`expect(1)`).
    #[tokio::test]
    async fn browse_rate_limit_fails_fast() {
        let server = MockServer::start().await;
        Mock::given(path("/HPImageArchive.aspx"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "60"))
            .expect(1)
            .mount(&server)
            .await;
        let started = std::time::Instant::now();
        let err = bing(&server).browse("", None).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            matches!(
                &err,
                ProviderError::RateLimitedFor {
                    message,
                    retry_after_secs: Some(60),
                } if message == "bing: rate limited (status 429)"
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn browse_server_error_is_unexpected_status() {
        let server = MockServer::start().await;
        Mock::given(path("/HPImageArchive.aspx"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let err = bing(&server).browse("", None).await.unwrap_err();
        assert_eq!(err.to_string(), "bing: unexpected status 500");
    }

    #[test]
    fn browse_support_is_finite() {
        let bing = Bing::new(BingConfig::default(), reqwest::Client::new());
        assert_eq!(bing.browse_support(), BrowseSupport::Finite);
    }

    #[test]
    fn media_hosts_are_bing() {
        let bing = Bing::new(BingConfig::default(), reqwest::Client::new());
        assert_eq!(bing.media_hosts(), ["www.bing.com", "bing.com"]);
    }

    /// Every preview and full URL a production feed yields passes the strict guard.
    #[tokio::test]
    async fn production_urls_pass_the_guard() {
        let server = MockServer::start().await;
        serve_idx(&server, "0", fixture("browse_idx0.json")).await;
        // Fixture URLs are relative, so rebase the mock's answers onto the production origin.
        let batch = bing(&server).browse("", None).await.unwrap();
        let hosts = Bing::new(BingConfig::default(), reqwest::Client::new()).media_hosts();
        let origin = server.uri();
        for item in &batch.items {
            let ImageRef::Direct(full) = &item.image else {
                panic!("{item:?}")
            };
            for url in [&item.preview_url, full] {
                let prod: url::Url = url
                    .replace(&origin, kakemono_sources::bing::BASE_URL)
                    .parse()
                    .unwrap();
                kakemono_sources::GuardConfig::strict()
                    .check_url(&prod, hosts)
                    .unwrap();
            }
        }
    }
}
