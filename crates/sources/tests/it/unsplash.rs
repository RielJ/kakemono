//! Unsplash provider against the Go-captured fixtures in `tests/fixtures/unsplash/`.

use crate::common;

use common::Case;
use kakemono_config::schema::UnsplashConfig;
use kakemono_sources::unsplash::{NO_ACCESS_KEY, Unsplash};
use kakemono_sources::{
    BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, Provider, ProviderError, RemoteItem,
};

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

// ---- browse (picker Sources tab) ----

/// A hand-written browse case: `path` on `api.unsplash.com` answers `status` with `body_file`
/// and `headers`; `request` is the one URL the provider must send.
fn browse_case(id: &str, path: &str, status: u16, body_file: &str, request: &str) -> Case {
    browse_case_with_headers(id, path, status, body_file, request, "{}")
}

fn browse_case_with_headers(
    id: &str,
    path: &str,
    status: u16,
    body_file: &str,
    request: &str,
    headers: &str,
) -> Case {
    let json = format!(
        r#"{{
            "go_test": "hand-written",
            "provider": "unsplash",
            "config": {{"enabled": true, "access_key": "test-access-key"}},
            "query": "",
            "routes": [{{"method": "GET", "host": "api.unsplash.com", "path": "{path}",
                         "status": {status}, "content_type": "application/json",
                         "headers": {headers}, "body_file": "{body_file}"}}],
            "requests": [{{"method": "GET", "user_agent": "", "url": "{request}"}}],
            "expected": {{"results": []}}
        }}"#
    );
    Case::parse(id, common::fixtures_dir().join("unsplash"), &json)
}

const EDITORIAL_P1: &str =
    "https://api.unsplash.com/photos?client_id=test-access-key&page=1&per_page=10";
const EDITORIAL_P20: &str =
    "https://api.unsplash.com/photos?client_id=test-access-key&page=20&per_page=10";
const SEARCH_P1: &str = "https://api.unsplash.com/search/photos?client_id=test-access-key\
                         &orientation=landscape&page=1&per_page=10&query=foggy+forest";
const SEARCH_P3: &str = "https://api.unsplash.com/search/photos?client_id=test-access-key\
                         &orientation=landscape&page=3&per_page=10&query=foggy+forest";

fn editorial_case(id: &str, request: &str) -> Case {
    browse_case(id, "/photos", 200, "browse_photos.json", request)
}

fn search_case(id: &str, request: &str) -> Case {
    browse_case(id, "/search/photos", 200, "browse_search.json", request)
}

fn cursor(page: &str) -> BrowseCursor {
    BrowseCursor(page.into())
}

/// Browses `case` with `query` and `cursor`, then checks the provider sent exactly the
/// recorded request.
async fn browse(
    case: &Case,
    query: &str,
    cursor: Option<&BrowseCursor>,
) -> Result<BrowseBatch, ProviderError> {
    let replay = case.serve().await;
    let got = provider(case, &replay).browse(query, cursor).await;
    replay.assert_requests().await;
    got
}

#[tokio::test]
async fn browse_editorial_first_page_points_at_page_2() {
    let case = editorial_case("unsplash/browse_editorial_p1", EDITORIAL_P1);
    let batch = browse(&case, "", None).await.unwrap();
    assert_eq!(batch.next, Some(cursor("2")));
}

#[tokio::test]
async fn browse_editorial_page_20_is_the_last() {
    let case = editorial_case("unsplash/browse_editorial_p20", EDITORIAL_P20);
    let batch = browse(&case, "", Some(&cursor("20"))).await.unwrap();
    assert_eq!(batch.next, None);
}

/// Preview is `urls.small`, the full image the raw-or-full URL with the fetch quality
/// parameters, and items without a preview or a full URL are dropped.
#[tokio::test]
async fn browse_editorial_maps_items() {
    let case = editorial_case("unsplash/browse_editorial_items", EDITORIAL_P1);
    let batch = browse(&case, "", None).await.unwrap();
    assert_eq!(
        batch.items,
        [
            RemoteItem {
                remote_id: "Ed1tAaa".into(),
                preview_url: "https://images.unsplash.com/photo-ed1taaa?ixid=e1&w=400".into(),
                image: ImageRef::Direct(
                    "https://images.unsplash.com/photo-ed1taaa?ixid=e1&fm=jpg&q=95&w=3840".into()
                ),
                source_url: "https://unsplash.com/photos/Ed1tAaa".into(),
                title: "Harbour at dusk".into(),
                author: "Eda Harbour".into(),
                width: Some(6000),
                height: Some(4000),
                nsfw: false,
            },
            RemoteItem {
                remote_id: "Ed1t_Bbb".into(),
                preview_url: "https://plus.unsplash.com/premium_photo-ed1tbbb?ixid=e2&w=400".into(),
                image: ImageRef::Direct(
                    "https://plus.unsplash.com/premium_photo-ed1tbbb?ixid=e2&fm=png".into()
                ),
                source_url: "https://unsplash.com/photos/Ed1t_Bbb".into(),
                title: "a desert road".into(),
                author: "Ben Road".into(),
                width: None,
                height: None,
                nsfw: false,
            },
        ]
    );
}

#[tokio::test]
async fn browse_editorial_empty_page_ends() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/photos"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&server)
        .await;
    let unsplash = Unsplash::with_base_url(
        UnsplashConfig {
            access_key: "k".into(),
        },
        reqwest::Client::new(),
        server.uri().parse().unwrap(),
    );
    let batch = unsplash.browse("", Some(&cursor("7"))).await.unwrap();
    assert_eq!(batch, BrowseBatch::default());
}

#[tokio::test]
async fn browse_search_first_page_points_at_page_2() {
    let case = search_case("unsplash/browse_search_p1", SEARCH_P1);
    let batch = browse(&case, "foggy forest", None).await.unwrap();
    assert_eq!(batch.next, Some(cursor("2")));
}

#[tokio::test]
async fn browse_search_ends_at_total_pages() {
    let case = search_case("unsplash/browse_search_p3", SEARCH_P3);
    let batch = browse(&case, "foggy forest", Some(&cursor("3")))
        .await
        .unwrap();
    assert_eq!(batch.next, None);
}

#[tokio::test]
async fn browse_search_maps_items() {
    let case = search_case("unsplash/browse_search_items", SEARCH_P1);
    let batch = browse(&case, "foggy forest", None).await.unwrap();
    assert_eq!(
        batch.items,
        [
            RemoteItem {
                remote_id: "Srch1Aa".into(),
                preview_url: "https://images.unsplash.com/photo-srch1aa?ixid=s1&w=400".into(),
                image: ImageRef::Direct(
                    "https://images.unsplash.com/photo-srch1aa?ixid=s1&fm=jpg&q=95&w=3840".into()
                ),
                source_url: "https://unsplash.com/photos/Srch1Aa".into(),
                title: "foggy forest".into(),
                author: "Fern Fog".into(),
                width: Some(5000),
                height: Some(3333),
                nsfw: false,
            },
            RemoteItem {
                remote_id: "Srch2Bb".into(),
                preview_url: "https://images.unsplash.com/photo-srch2bb?ixid=s2&w=400".into(),
                image: ImageRef::Direct(
                    "https://images.unsplash.com/photo-srch2bb?ixid=s2&fm=jpg&q=95&w=3840".into()
                ),
                source_url: "https://unsplash.com/photos/Srch2Bb".into(),
                title: "Pines".into(),
                author: String::new(),
                width: None,
                height: None,
                nsfw: false,
            },
        ]
    );
}

#[tokio::test]
async fn browse_without_key_is_not_configured() {
    let unsplash = Unsplash::with_base_url(
        UnsplashConfig::default(),
        reqwest::Client::new(),
        // Unroutable: the test fails if a request is attempted.
        "http://127.0.0.1:1".parse().unwrap(),
    );
    let err = unsplash.browse("", None).await.unwrap_err();
    assert!(matches!(err, ProviderError::NotConfigured(_)), "{err:?}");
}

/// 429 fails at once (one request, no back-off) and carries the `Retry-After` hint.
#[tokio::test]
async fn browse_rate_limited_fails_fast_with_hint() {
    let case = browse_case_with_headers(
        "unsplash/browse_rate_limited",
        "/photos",
        429,
        "unauthorized.json",
        EDITORIAL_P1,
        r#"{"Retry-After": "120"}"#,
    );
    let replay = case.serve().await;
    let err = provider(&case, &replay).browse("", None).await.unwrap_err();
    let requests = replay.server.received_requests().await.unwrap().len();
    assert_eq!(
        (err.to_string(), err.retry_after_secs(), requests),
        (
            "unsplash api: rate limited (status 429)".to_owned(),
            Some(120),
            1
        )
    );
}

#[tokio::test]
async fn browse_unauthorized_is_invalid_key() {
    let case = browse_case(
        "unsplash/browse_unauthorized",
        "/photos",
        401,
        "unauthorized.json",
        EDITORIAL_P1,
    );
    let err = browse(&case, "", None).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsplash api: invalid or revoked access key (status 401)"
    );
}

#[tokio::test]
async fn browse_other_status_is_unexpected() {
    let case = browse_case(
        "unsplash/browse_server_error",
        "/search/photos",
        500,
        "unauthorized.json",
        SEARCH_P1,
    );
    let err = browse(&case, "foggy forest", None).await.unwrap_err();
    assert_eq!(err.to_string(), "unsplash api: unexpected status 500");
}

#[tokio::test]
async fn browse_rejects_a_foreign_cursor() {
    let unsplash = Unsplash::with_base_url(
        UnsplashConfig {
            access_key: "k".into(),
        },
        reqwest::Client::new(),
        "http://127.0.0.1:1".parse().unwrap(),
    );
    let err = unsplash
        .browse("", Some(&cursor("2:seed")))
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsplash: invalid browse cursor \"2:seed\""
    );
}

/// A transport error never shows the access key.
#[tokio::test]
async fn browse_transport_error_hides_the_key() {
    let unsplash = Unsplash::with_base_url(
        UnsplashConfig {
            access_key: "testkey".into(),
        },
        reqwest::Client::new(),
        common::hang_up_server().await,
    );
    let err = unsplash.browse("forest", None).await.unwrap_err();
    assert!(matches!(err, ProviderError::Request { .. }), "{err:?}");
    common::assert_key_absent(&err, "testkey");
}

#[test]
fn browse_support_is_paged() {
    let unsplash = Unsplash::new(UnsplashConfig::default(), reqwest::Client::new());
    assert_eq!(unsplash.browse_support(), BrowseSupport::Paged);
}

#[test]
fn media_hosts_are_the_image_cdns() {
    let unsplash = Unsplash::new(UnsplashConfig::default(), reqwest::Client::new());
    assert_eq!(
        unsplash.media_hosts(),
        ["images.unsplash.com", "plus.unsplash.com"]
    );
}

/// Live smoke test against api.unsplash.com: `UNSPLASH_ACCESS_KEY=.. cargo test -p
/// kakemono-sources -- --ignored browse_live_unsplash`. Checks the documented response shape
/// (`urls.small`, `total_pages`) still parses into items on the media hosts.
#[tokio::test]
#[ignore = "network; needs UNSPLASH_ACCESS_KEY"]
async fn browse_live_unsplash() {
    let access_key = std::env::var("UNSPLASH_ACCESS_KEY").expect("UNSPLASH_ACCESS_KEY");
    let unsplash = Unsplash::new(
        UnsplashConfig { access_key },
        kakemono_sources::http::client().unwrap(),
    );
    for query in ["", "mountains"] {
        let batch = unsplash.browse(query, None).await.unwrap();
        assert!(!batch.items.is_empty(), "{query:?}: no items");
        assert_eq!(batch.next, Some(cursor("2")), "{query:?}");
        for item in &batch.items {
            let host = url::Url::parse(&item.preview_url).unwrap();
            let host = host.host_str().unwrap_or_default();
            assert!(unsplash.media_hosts().contains(&host), "{query:?}: {host}");
        }
    }
}
