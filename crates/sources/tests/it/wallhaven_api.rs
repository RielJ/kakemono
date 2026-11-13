//! Wallhaven API mode (Task 14): replays the Go `TestWallhavenAPI_*` cases, plus edge cases Go
//! did not cover (rate limit, bad JSON, entries without `path`).

use crate::common;

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
    common::assert_key_absent(&err, "testkey");
    assert!(err.to_string().contains("apikey=REDACTED"), "{err}");

    // Transport failure: the server hangs up unanswered, so reqwest's error carries the URL.
    let base_url = common::hang_up_server().await;
    let cfg = WallhavenConfig {
        api_key: "testkey".to_owned(),
        ..WallhavenConfig::default()
    };
    let err = Wallhaven::with_base_url(
        cfg,
        kakemono_sources::http::client_builder().build().unwrap(),
        base_url,
    )
    .fetch("")
    .await
    .unwrap_err();
    assert!(matches!(err, ProviderError::Request { .. }), "{err:?}");
    common::assert_key_absent(&err, "testkey");
    assert!(err.to_string().contains("apikey=REDACTED"), "{err}");
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

/// `null` decodes as empty, like Go and the other providers (audit M10): one null field used
/// to fail the whole batch with `invalid type: null`.
#[tokio::test]
async fn api_null_fields_decode_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"{"data":[
        {"id":"a","url":"https://wallhaven.cc/w/a","path":null,"uploader":{"username":"x"}},
        {"id":"b","url":null,"path":"https://w.wallhaven.cc/full/b.jpg","uploader":{"username":null}},
        {"id":"c","url":"https://wallhaven.cc/w/c","path":"https://w.wallhaven.cc/full/c.png","uploader":null}
    ]}"#;
    let expected = r#"{"results": [
        {"source_url":"","image_url":"https://w.wallhaven.cc/full/b.jpg","title":"","author":"","source":"wallhaven"},
        {"source_url":"https://wallhaven.cc/w/c","image_url":"https://w.wallhaven.cc/full/c.png","title":"","author":"","source":"wallhaven"}
    ]}"#;
    let case = api_case(&dir, 200, body, expected);
    let replay = case.serve().await;
    let wallhaven = provider(&case, &replay);
    replay.verify(wallhaven.fetch(&case.query).await).await;
}

/// `{"data": null}` is an empty page, not a decode error.
#[tokio::test]
async fn api_null_data_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let case = api_case(&dir, 200, r#"{"data":null}"#, r#"{"results": []}"#);
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
fn name_is_wallhaven() {
    let client = kakemono_sources::http::client().unwrap();
    let wallhaven = Wallhaven::new(WallhavenConfig::default(), client);
    assert_eq!(wallhaven.name(), "wallhaven");
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

// ---------------------------------------------------------------------------------------------
// Browse (picker Sources tab): `page`/`seed` paging, fail-fast 429, field mapping.
// ---------------------------------------------------------------------------------------------

mod browse {
    use kakemono_config::schema::WallhavenConfig;
    use kakemono_sources::wallhaven::Wallhaven;
    use kakemono_sources::{
        BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, Provider, ProviderError, RemoteItem,
    };
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::common;

    const KEY: &str = "testkey";

    fn fixture(name: &str) -> String {
        let file = common::fixtures_dir().join("wallhaven").join(name);
        std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
    }

    fn keyed(base_url: url::Url) -> Wallhaven {
        let cfg = WallhavenConfig {
            api_key: KEY.to_owned(),
            purity: "110".to_owned(),
            sorting: "random".to_owned(),
            categories: "111".to_owned(),
        };
        Wallhaven::with_base_url(
            cfg,
            kakemono_sources::http::client_builder().build().unwrap(),
            base_url,
        )
    }

    fn base(server: &MockServer) -> url::Url {
        server.uri().parse().unwrap()
    }

    /// `/api/v1/search` with the key and the config params; `page` and `seed` are added by the
    /// caller.
    fn search() -> wiremock::MockBuilder {
        Mock::given(method("GET"))
            .and(path("/api/v1/search"))
            .and(query_param("apikey", KEY))
            .and(query_param("purity", "110"))
            .and(query_param("sorting", "random"))
            .and(query_param("categories", "111"))
            .and(query_param("q", "nature"))
    }

    fn ok(body: String) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body, "application/json")
    }

    async fn first_page() -> (MockServer, BrowseBatch) {
        let server = MockServer::start().await;
        search()
            .and(query_param("page", "1"))
            .and(query_param_is_missing("seed"))
            .respond_with(ok(fixture("api_browse_p1.json")))
            .expect(1)
            .mount(&server)
            .await;
        let batch = keyed(base(&server)).browse("nature", None).await.unwrap();
        (server, batch)
    }

    #[tokio::test]
    async fn browse_first_call_sends_page_1_without_seed_and_cursor_holds_seed() {
        let (server, batch) = first_page().await;
        assert_eq!(batch.next, Some(BrowseCursor("2:Xy12Ab".to_owned())));
        server.verify().await;
    }

    #[tokio::test]
    async fn browse_next_call_sends_page_and_seed_from_cursor() {
        let server = MockServer::start().await;
        search()
            .and(query_param("page", "2"))
            .and(query_param("seed", "Xy12Ab"))
            .respond_with(ok(fixture("api_browse_p1.json")))
            .expect(1)
            .mount(&server)
            .await;
        let cursor = BrowseCursor("2:Xy12Ab".to_owned());
        let batch = keyed(base(&server))
            .browse("nature", Some(&cursor))
            .await
            .unwrap();
        assert_eq!(batch.next, Some(BrowseCursor("3:Xy12Ab".to_owned())));
        server.verify().await;
    }

    #[tokio::test]
    async fn browse_last_page_has_no_next() {
        let server = MockServer::start().await;
        search()
            .and(query_param("page", "3"))
            .and(query_param("seed", "Xy12Ab"))
            .respond_with(ok(fixture("api_browse_last.json")))
            .expect(1)
            .mount(&server)
            .await;
        let cursor = BrowseCursor("3:Xy12Ab".to_owned());
        let batch = keyed(base(&server))
            .browse("nature", Some(&cursor))
            .await
            .unwrap();
        assert_eq!(batch.next, None);
        assert_eq!(batch.items.len(), 1);
        assert!(batch.items[0].nsfw, "purity nsfw: {:?}", batch.items[0]);
        server.verify().await;
    }

    /// A sort without a seed (e.g. `toplist`) answers `"seed": null`: the cursor carries an
    /// empty seed and the next request sends none.
    #[tokio::test]
    async fn browse_without_seed_sends_no_seed_on_next_page() {
        let server = MockServer::start().await;
        search()
            .and(query_param("page", "2"))
            .and(query_param_is_missing("seed"))
            .respond_with(ok(fixture("api_browse_last.json")
                .replace(r#""current_page": 3"#, r#""current_page": 2"#)))
            .expect(1)
            .mount(&server)
            .await;
        let cursor = BrowseCursor("2:".to_owned());
        let batch = keyed(base(&server))
            .browse("nature", Some(&cursor))
            .await
            .unwrap();
        assert_eq!(batch.next, Some(BrowseCursor("3:".to_owned())));
        server.verify().await;
    }

    #[tokio::test]
    async fn browse_maps_fields_and_drops_items_without_thumbs() {
        let (_server, batch) = first_page().await;
        let want = vec![
            RemoteItem {
                remote_id: "abc123".to_owned(),
                preview_url: "https://th.wallhaven.cc/lg/ab/abc123.jpg".to_owned(),
                image: ImageRef::Direct(
                    "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg".to_owned(),
                ),
                source_url: "https://wallhaven.cc/w/abc123".to_owned(),
                title: String::new(),
                author: "artist1".to_owned(),
                width: Some(1920),
                height: Some(1080),
                nsfw: false,
            },
            RemoteItem {
                remote_id: "def456".to_owned(),
                preview_url: "https://th.wallhaven.cc/lg/de/def456.jpg".to_owned(),
                image: ImageRef::Direct(
                    "https://w.wallhaven.cc/full/de/wallhaven-def456.png".to_owned(),
                ),
                source_url: "https://wallhaven.cc/w/def456".to_owned(),
                title: String::new(),
                author: String::new(),
                width: Some(3840),
                height: Some(2160),
                nsfw: true,
            },
        ];
        assert_eq!(batch.items, want);
    }

    #[tokio::test]
    async fn browse_sanitizes_remote_id() {
        let server = MockServer::start().await;
        let body = r#"{"data":[{"id":"../x","url":"https://wallhaven.cc/w/x","purity":"sfw",
            "path":"https://w.wallhaven.cc/full/x.jpg",
            "thumbs":{"large":"https://th.wallhaven.cc/lg/x.jpg"}}],
            "meta":{"current_page":1,"last_page":1,"seed":null}}"#;
        search()
            .respond_with(ok(body.to_owned()))
            .mount(&server)
            .await;
        let batch = keyed(base(&server)).browse("nature", None).await.unwrap();
        let id = &batch.items[0].remote_id;
        assert_eq!(id, &kakemono_sources::browse::sanitize_remote_id("../x"));
    }

    #[tokio::test]
    async fn browse_rate_limit_fails_fast_without_retry() {
        let server = MockServer::start().await;
        search()
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "30"))
            .expect(1)
            .mount(&server)
            .await;
        let err = keyed(base(&server))
            .browse("nature", None)
            .await
            .unwrap_err();
        assert!(
            matches!(
                &err,
                ProviderError::RateLimitedFor {
                    retry_after_secs: Some(30),
                    ..
                }
            ),
            "{err:?}"
        );
        assert_eq!(err.to_string(), "wallhaven api: rate limited (status 429)");
        server.verify().await;
    }

    #[tokio::test]
    async fn browse_status_error_never_contains_the_key() {
        let server = MockServer::start().await;
        search()
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let err = keyed(base(&server))
            .browse("nature", None)
            .await
            .unwrap_err();
        common::assert_key_absent(&err, KEY);
        assert!(
            err.to_string()
                .starts_with("wallhaven api: unexpected status 500 for "),
            "{err}"
        );
        assert!(err.to_string().contains("apikey=REDACTED"), "{err}");
    }

    #[tokio::test]
    async fn browse_transport_error_never_contains_the_key() {
        let err = keyed(common::hang_up_server().await)
            .browse("nature", None)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Request { .. }), "{err:?}");
        common::assert_key_absent(&err, KEY);
    }

    #[tokio::test]
    async fn browse_rejects_a_malformed_cursor_without_a_request() {
        let server = MockServer::start().await;
        search()
            .respond_with(ok(fixture("api_browse_p1.json")))
            .expect(0)
            .mount(&server)
            .await;
        let cursor = BrowseCursor("not-a-page".to_owned());
        let err = keyed(base(&server))
            .browse("nature", Some(&cursor))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)), "{err:?}");
        server.verify().await;
    }

    #[test]
    fn browse_support_is_paged() {
        let wallhaven = keyed("https://wallhaven.cc".parse().unwrap());
        assert_eq!(wallhaven.browse_support(), BrowseSupport::Paged);
    }

    #[test]
    fn media_hosts_is_wallhaven_cc() {
        let wallhaven = keyed("https://wallhaven.cc".parse().unwrap());
        assert_eq!(wallhaven.media_hosts(), ["wallhaven.cc"]);
    }
}
