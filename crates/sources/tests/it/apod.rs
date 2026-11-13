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
    let apod = Apod::new(
        ApodConfig::default(),
        kakemono_sources::http::client_builder().build().unwrap(),
    );
    assert_eq!(apod.name(), "apod");
}

/// A transport error never shows the API key (it travels in the query).
#[tokio::test]
async fn transport_error_hides_the_key() {
    let cfg: ApodConfig =
        serde_json::from_str(r#"{"enabled": true, "api_key": "testkey"}"#).unwrap();
    let apod = Apod::with_base_url(
        cfg,
        kakemono_sources::http::client_builder().build().unwrap(),
        common::hang_up_server().await,
    );
    let err = apod.fetch("").await.unwrap_err();
    assert!(matches!(err, ProviderError::Request { .. }), "{err:?}");
    common::assert_key_absent(&err, "testkey");
}

// ---- browse (picker Sources tab) ----

mod browse {
    use std::time::Duration;

    use chrono::NaiveDate;
    use kakemono_config::schema::ApodConfig;
    use kakemono_sources::apod::Apod;
    use kakemono_sources::{
        BrowseCursor, BrowseSupport, ImageRef, Provider, ProviderError, RemoteItem,
        sanitize_remote_id,
    };
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::common::fixtures_dir;

    fn fixture() -> String {
        std::fs::read_to_string(fixtures_dir().join("apod").join("browse_range.json")).unwrap()
    }

    fn date(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn cursor(s: &str) -> BrowseCursor {
        BrowseCursor(s.to_owned())
    }

    /// A mock answering the `start..=end` window with `body`, expected exactly once.
    async fn serve_window(server: &MockServer, start: &str, end: &str, body: String) {
        Mock::given(method("GET"))
            .and(path("/planetary/apod"))
            .and(query_param("api_key", "DEMO_KEY"))
            .and(query_param("start_date", start))
            .and(query_param("end_date", end))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(server)
            .await;
    }

    /// Any request to the APOD path answered with `status`, expected exactly `times` times.
    async fn serve_status(server: &MockServer, status: u16, times: u64) {
        Mock::given(path("/planetary/apod"))
            .respond_with(ResponseTemplate::new(status).insert_header("Retry-After", "60"))
            .expect(times)
            .mount(server)
            .await;
    }

    fn apod_with_key(server: &MockServer, api_key: &str, today: &str) -> Apod {
        Apod::with_base_url(
            ApodConfig {
                api_key: api_key.to_owned(),
            },
            kakemono_sources::http::client_builder().build().unwrap(),
            server.uri().parse().unwrap(),
        )
        .with_today(date(today))
    }

    fn apod(server: &MockServer, today: &str) -> Apod {
        apod_with_key(server, "", today)
    }

    /// The first batch of the fixture window (today 2024-03-20).
    async fn first_batch_items() -> Vec<RemoteItem> {
        let server = MockServer::start().await;
        serve_window(&server, "2024-03-07", "2024-03-20", fixture()).await;
        apod(&server, "2024-03-20")
            .browse("ignored", None)
            .await
            .unwrap()
            .items
    }

    fn item<'a>(items: &'a [RemoteItem], day: &str) -> &'a RemoteItem {
        items
            .iter()
            .find(|i| i.remote_id == day)
            .unwrap_or_else(|| panic!("no item {day} in {items:?}"))
    }

    #[tokio::test]
    async fn browse_first_window_is_14_days_ending_today_and_points_at_the_day_before() {
        let server = MockServer::start().await;
        serve_window(&server, "2024-03-07", "2024-03-20", fixture()).await;
        let batch = apod(&server, "2024-03-20").browse("", None).await.unwrap();
        assert_eq!(batch.next, Some(cursor("2024-03-06")));
    }

    #[tokio::test]
    async fn browse_cursor_is_the_end_of_the_next_window() {
        let server = MockServer::start().await;
        serve_window(&server, "2024-02-22", "2024-03-06", "[]".to_owned()).await;
        let batch = apod(&server, "2024-03-20")
            .browse("", Some(&cursor("2024-03-06")))
            .await
            .unwrap();
        assert_eq!(batch.next, Some(cursor("2024-02-21")));
    }

    #[tokio::test]
    async fn browse_window_crosses_a_leap_day() {
        let server = MockServer::start().await;
        serve_window(&server, "2024-02-19", "2024-03-03", "[]".to_owned()).await;
        let batch = apod(&server, "2024-03-03").browse("", None).await.unwrap();
        assert_eq!(batch.next, Some(cursor("2024-02-18")));
    }

    #[tokio::test]
    async fn browse_window_is_clamped_to_the_first_apod_and_ends() {
        let server = MockServer::start().await;
        serve_window(&server, "1995-06-16", "1995-06-20", "[]".to_owned()).await;
        let batch = apod(&server, "2024-03-20")
            .browse("", Some(&cursor("1995-06-20")))
            .await
            .unwrap();
        assert_eq!(batch.next, None);
    }

    #[tokio::test]
    async fn browse_window_starting_exactly_on_the_first_apod_ends() {
        let server = MockServer::start().await;
        serve_window(&server, "1995-06-16", "1995-06-29", "[]".to_owned()).await;
        let batch = apod(&server, "2024-03-20")
            .browse("", Some(&cursor("1995-06-29")))
            .await
            .unwrap();
        assert_eq!(batch.next, None);
    }

    #[tokio::test]
    async fn browse_window_one_day_after_the_first_apod_has_one_more_window() {
        let server = MockServer::start().await;
        serve_window(&server, "1995-06-17", "1995-06-30", "[]".to_owned()).await;
        let batch = apod(&server, "2024-03-20")
            .browse("", Some(&cursor("1995-06-30")))
            .await
            .unwrap();
        assert_eq!(batch.next, Some(cursor("1995-06-16")));
    }

    #[tokio::test]
    async fn browse_cursor_before_the_first_apod_is_an_error_without_a_request() {
        let server = MockServer::start().await;
        serve_status(&server, 200, 0).await;
        let err = apod(&server, "2024-03-20")
            .browse("", Some(&cursor("1995-06-15")))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)), "{err:?}");
    }

    #[tokio::test]
    async fn browse_cursor_after_today_is_an_error_without_a_request() {
        let server = MockServer::start().await;
        serve_status(&server, 200, 0).await;
        let err = apod(&server, "2024-03-20")
            .browse("", Some(&cursor("2024-03-21")))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)), "{err:?}");
    }

    #[tokio::test]
    async fn browse_malformed_cursor_is_an_error_without_a_request() {
        let server = MockServer::start().await;
        serve_status(&server, 200, 0).await;
        let err = apod(&server, "2024-03-20")
            .browse("", Some(&cursor("next")))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)), "{err:?}");
    }

    #[tokio::test]
    async fn browse_items_come_newest_first() {
        let ids: Vec<String> = first_batch_items()
            .await
            .into_iter()
            .map(|i| i.remote_id)
            .collect();
        assert_eq!(
            ids,
            [
                "2024-03-20",
                "2024-03-18",
                "2024-03-17",
                "2024-03-16",
                "2024-03-15",
                "2024-03-13",
                "2024-03-12",
                "2024-03-11",
                "2024-03-10",
                "2024-03-08",
                "2024-03-07",
            ]
        );
    }

    #[tokio::test]
    async fn browse_skips_videos() {
        let items = first_batch_items().await;
        assert!(items.iter().all(|i| i.remote_id != "2024-03-14"));
    }

    #[tokio::test]
    async fn browse_skips_other_media_and_entries_without_an_image() {
        let items = first_batch_items().await;
        assert!(
            items
                .iter()
                .all(|i| i.remote_id != "2024-03-19" && i.remote_id != "2024-03-09")
        );
    }

    #[tokio::test]
    async fn browse_item_maps_preview_full_and_metadata() {
        let items = first_batch_items().await;
        assert_eq!(
            item(&items, "2024-03-18"),
            &RemoteItem {
                remote_id: "2024-03-18".to_owned(),
                preview_url: "https://apod.nasa.gov/apod/image/2403/sky18_1024.jpg".to_owned(),
                image: ImageRef::Direct(
                    "https://apod.nasa.gov/apod/image/2403/sky18_4096.jpg".to_owned()
                ),
                source_url: "https://apod.nasa.gov/apod/ap20240318.html".to_owned(),
                title: "Sky 18".to_owned(),
                author: "Astro Person 18".to_owned(),
                width: None,
                height: None,
                nsfw: false,
            }
        );
    }

    #[tokio::test]
    async fn browse_remote_id_is_the_date_and_already_sanitized() {
        let items = first_batch_items().await;
        assert!(
            items
                .iter()
                .all(|i| sanitize_remote_id(&i.remote_id) == i.remote_id)
        );
    }

    #[tokio::test]
    async fn browse_author_is_empty_without_copyright() {
        let items = first_batch_items().await;
        assert_eq!(item(&items, "2024-03-17").author, "");
    }

    #[tokio::test]
    async fn browse_full_image_falls_back_to_url_without_hdurl() {
        let items = first_batch_items().await;
        assert_eq!(
            item(&items, "2024-03-16").image,
            ImageRef::Direct("https://apod.nasa.gov/apod/image/2403/sky16_1024.jpg".to_owned())
        );
    }

    #[tokio::test]
    async fn browse_preview_falls_back_to_hdurl_without_url() {
        let items = first_batch_items().await;
        assert_eq!(
            item(&items, "2024-03-11").preview_url,
            "https://apod.nasa.gov/apod/image/2403/sky11_4096.jpg"
        );
    }

    #[tokio::test]
    async fn browse_window_of_only_videos_is_empty_but_keeps_going() {
        let server = MockServer::start().await;
        let body = r#"[{"date":"2024-03-20","media_type":"video","title":"V","url":"https://www.youtube.com/embed/x"}]"#;
        serve_window(&server, "2024-03-07", "2024-03-20", body.to_owned()).await;
        let batch = apod(&server, "2024-03-20").browse("", None).await.unwrap();
        assert_eq!(
            (batch.items.len(), batch.next),
            (0, Some(cursor("2024-03-06")))
        );
    }

    #[tokio::test]
    async fn browse_sends_the_configured_api_key() {
        let server = MockServer::start().await;
        Mock::given(path("/planetary/apod"))
            .and(query_param("api_key", "s3cret"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("[]", "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        apod_with_key(&server, "s3cret", "2024-03-20")
            .browse("", None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn browse_rate_limit_fails_fast() {
        let server = MockServer::start().await;
        serve_status(&server, 429, 1).await;
        let started = std::time::Instant::now();
        let err = apod(&server, "2024-03-20")
            .browse("", None)
            .await
            .unwrap_err();
        // The server asks for a 60 s wait; under 30 s proves there was none. A tighter bound
        // fails when the whole workspace's tests load the machine.
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{:?}",
            started.elapsed()
        );
        assert!(
            matches!(
                &err,
                ProviderError::RateLimitedFor {
                    message,
                    retry_after_secs: Some(60),
                } if message == "apod: rate limited (status 429)"
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn browse_bad_key_is_not_configured() {
        let server = MockServer::start().await;
        serve_status(&server, 403, 1).await;
        let err = apod_with_key(&server, "s3cret", "2024-03-20")
            .browse("", None)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::NotConfigured(_)), "{err:?}");
    }

    #[tokio::test]
    async fn browse_server_error_is_unexpected_status() {
        let server = MockServer::start().await;
        serve_status(&server, 500, 1).await;
        let err = apod(&server, "2024-03-20")
            .browse("", None)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Status { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn browse_transport_error_hides_the_api_key() {
        // Nothing listens on a dropped server's port.
        let server = MockServer::start().await;
        let apod = apod_with_key(&server, "s3cret", "2024-03-20");
        drop(server);
        let err = apod.browse("", None).await.unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(!text.contains("s3cret"), "{text}");
    }

    #[test]
    fn browse_support_is_paged() {
        let apod = Apod::new(
            ApodConfig::default(),
            kakemono_sources::http::client_builder().build().unwrap(),
        );
        assert_eq!(apod.browse_support(), BrowseSupport::Paged);
    }

    #[test]
    fn media_hosts_are_apod() {
        let apod = Apod::new(
            ApodConfig::default(),
            kakemono_sources::http::client_builder().build().unwrap(),
        );
        assert_eq!(apod.media_hosts(), ["apod.nasa.gov"]);
    }

    #[tokio::test]
    async fn fixture_urls_pass_the_guard() {
        let hosts = Apod::new(
            ApodConfig::default(),
            kakemono_sources::http::client_builder().build().unwrap(),
        )
        .media_hosts();
        for item in first_batch_items().await {
            let ImageRef::Direct(full) = &item.image else {
                panic!("{item:?}")
            };
            for url in [&item.preview_url, full] {
                kakemono_sources::GuardConfig::strict()
                    .check_url(&url.parse().unwrap(), hosts)
                    .unwrap();
            }
        }
    }
}
