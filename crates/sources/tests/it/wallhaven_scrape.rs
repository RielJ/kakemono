//! Wallhaven scrape mode (Task 15): replays the Go `TestWallhavenScrape_*` cases, plus the page
//! cap, the scrape delay on the paused clock, skipped pages and search-page errors.

use crate::common;

use std::path::PathBuf;
use std::time::Duration;

use common::{Case, Replay};
use kakemono_config::schema::WallhavenConfig;
use kakemono_sources::wallhaven::scrape::MAX_PAGES;
use kakemono_sources::wallhaven::{DEFAULT_SCRAPE_DELAY, Wallhaven};
use kakemono_sources::{Provider, ProviderError};

fn provider(case: &Case, replay: &Replay, delay: Duration) -> Wallhaven {
    Wallhaven::with_base_url(
        case.config::<WallhavenConfig>(),
        replay.client(),
        replay.base_url(),
    )
    .with_scrape_delay(delay)
}

async fn run(name: &str) {
    let case = Case::load("wallhaven", name);
    let replay = case.serve().await;
    let wallhaven = provider(&case, &replay, Duration::ZERO);
    assert!(!wallhaven.uses_api(), "{name}: case must be scrape mode");
    replay.verify(wallhaven.fetch(&case.query).await).await;
}

#[tokio::test]
async fn scrape_finds_images() {
    run("scrape_finds_images").await;
}

#[tokio::test]
async fn scrape_no_thumbs() {
    run("scrape_no_thumbs").await;
}

#[test]
fn scrape_every_fixture_case_has_a_test() {
    let scrape: Vec<String> = Case::names("wallhaven")
        .into_iter()
        .filter(|n| n.starts_with("scrape_"))
        .collect();
    assert_eq!(scrape, ["scrape_finds_images", "scrape_no_thumbs"]);
}

/// The default delay (1 s + up to 1 s jitter) is waited before each of the 2 pages.
#[tokio::test]
async fn scrape_waits_the_delay_before_each_page() {
    let case = Case::load("wallhaven", "scrape_finds_images");
    let replay = case.serve().await;
    let wallhaven = Wallhaven::with_base_url(
        case.config::<WallhavenConfig>(),
        replay.client(),
        replay.base_url(),
    );
    let start = tokio::time::Instant::now();
    let outcome = wallhaven.fetch(&case.query).await;
    let elapsed = start.elapsed();
    replay.verify(outcome).await;
    assert!(
        elapsed >= 2 * DEFAULT_SCRAPE_DELAY && elapsed < 4 * DEFAULT_SCRAPE_DELAY,
        "{elapsed:?}"
    );
}

/// Hand-written scrape case. `pages` are `(path, status, body)` routes besides `/search`.
fn scrape_case(
    dir: &tempfile::TempDir,
    search_status: u16,
    search_html: &str,
    pages: &[(&str, u16, &str)],
    requests: &[&str],
    expected: &str,
) -> Case {
    std::fs::write(dir.path().join("search.html"), search_html).unwrap();
    let mut routes = vec![format!(
        r#"{{"method":"GET","host":"wallhaven.cc","path":"/search","status":{search_status},
             "content_type":"text/html","body_file":"search.html"}}"#
    )];
    for (i, (path, status, body)) in pages.iter().enumerate() {
        let file = format!("page{i}.html");
        std::fs::write(dir.path().join(&file), body).unwrap();
        routes.push(format!(
            r#"{{"method":"GET","host":"wallhaven.cc","path":"{path}","status":{status},
                 "content_type":"text/html","body_file":"{file}"}}"#
        ));
    }
    let requests: Vec<String> = requests
        .iter()
        .map(|url| format!(r#"{{"method":"GET","url":"{url}","user_agent":""}}"#))
        .collect();
    let json = format!(
        r#"{{
          "go_test": "none",
          "provider": "wallhaven",
          "config": {{"enabled": true, "api_key": "", "purity": "110", "sorting": "toplist", "categories": "100"}},
          "query": "",
          "routes": [{}],
          "requests": [{}],
          "sort_results": true,
          "expected": {expected}
        }}"#,
        routes.join(","),
        requests.join(",")
    );
    Case::parse("wallhaven/hand", PathBuf::from(dir.path()), &json)
}

const SEARCH_URL: &str = "https://wallhaven.cc/search?categories=100&purity=110&sorting=toplist";

fn thumbs(ids: &[&str]) -> String {
    let items: String = ids
        .iter()
        .map(|id| {
            format!(
                r#"<li><figure class="thumb"><a class="preview" href="http://{{{{HOST}}}}/w/{id}"><img src="t.jpg"/></a></figure></li>"#
            )
        })
        .collect();
    format!("<html><body><ul>{items}</ul></body></html>")
}

fn page(src: &str) -> String {
    format!(r#"<html><body><img id="wallpaper" src="{src}"/></body></html>"#)
}

#[tokio::test]
async fn scrape_fetches_at_most_three_pages() {
    let dir = tempfile::tempdir().unwrap();
    let ids = ["a1", "b2", "c3", "d4", "e5"];
    let pages: Vec<(String, String)> = ids
        .iter()
        .map(|id| {
            (
                format!("/w/{id}"),
                page(&format!("https://w.wallhaven.cc/full/{id}.jpg")),
            )
        })
        .collect();
    let routes: Vec<(&str, u16, &str)> = pages
        .iter()
        .map(|(p, b)| (p.as_str(), 200, b.as_str()))
        .collect();
    let case = scrape_case(&dir, 200, &thumbs(&ids), &routes, &[], r#"{"results": []}"#);
    let replay = case.serve().await;
    let results = provider(&case, &replay, Duration::ZERO)
        .fetch(&case.query)
        .await
        .unwrap();

    assert_eq!(results.len(), MAX_PAGES);
    for r in &results {
        let id = r.source_url.rsplit('/').next().unwrap();
        assert!(ids.contains(&id), "{r:?}");
        assert_eq!(r.image_url, format!("https://w.wallhaven.cc/full/{id}.jpg"));
        assert_eq!(r.source, "wallhaven");
    }
    let received = replay.server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1 + MAX_PAGES);
}

#[tokio::test]
async fn scrape_skips_pages_that_fail() {
    let dir = tempfile::tempdir().unwrap();
    let ok = page("https://w.wallhaven.cc/full/ok.png");
    let case = scrape_case(
        &dir,
        200,
        &thumbs(&["ok", "gone", "noimg"]),
        &[
            ("/w/ok", 200, &ok),
            ("/w/gone", 404, "not found"),
            ("/w/noimg", 200, "<html><body><p>no image</p></body></html>"),
        ],
        &[
            SEARCH_URL,
            "http://wallhaven.cc/w/ok",
            "http://wallhaven.cc/w/gone",
            "http://wallhaven.cc/w/noimg",
        ],
        r#"{"results": [{"source_url":"http://wallhaven.cc/w/ok",
            "image_url":"https://w.wallhaven.cc/full/ok.png","title":"","author":"","source":"wallhaven"}]}"#,
    );
    let replay = case.serve().await;
    let outcome = provider(&case, &replay, Duration::ZERO)
        .fetch(&case.query)
        .await;
    replay.verify(outcome).await;
}

#[tokio::test]
async fn scrape_search_page_error_has_go_text() {
    let dir = tempfile::tempdir().unwrap();
    let case = scrape_case(
        &dir,
        500,
        "server error",
        &[],
        &[SEARCH_URL],
        &format!(
            r#"{{"error": "wallhaven scrape: search page: unexpected status 500 for {SEARCH_URL}"}}"#
        ),
    );
    let replay = case.serve().await;
    let outcome = provider(&case, &replay, Duration::ZERO)
        .fetch(&case.query)
        .await;
    replay.verify(outcome).await;
}

/// A 429 on the search page that outlasts the retries is `RateLimited` (audit M1).
#[tokio::test]
async fn scrape_rate_limit_is_the_rate_limited_variant() {
    let dir = tempfile::tempdir().unwrap();
    let case = scrape_case(&dir, 429, "", &[], &[SEARCH_URL], r#"{"results": []}"#);
    let replay = case.serve().await;
    let err = provider(&case, &replay, Duration::ZERO)
        .fetch(&case.query)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "wallhaven scrape: search page: rate limited (status 429)"
    );
    replay.assert_requests().await;
}

// ---------------------------------------------------------------------------------------------
// Browse (picker Sources tab): one search-page request per batch, `ImageRef::Page` items that
// `resolve_image` turns into the full-image URL at use time.
// ---------------------------------------------------------------------------------------------

mod browse {
    use std::time::Duration;

    use kakemono_config::schema::WallhavenConfig;
    use kakemono_sources::wallhaven::Wallhaven;
    use kakemono_sources::{BrowseCursor, ImageRef, Provider, ProviderError, RemoteItem};
    use wiremock::matchers::{method, path, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::common;

    /// The fixture page, with `{{HOST}}` pointing at `server`.
    fn fixture(server: &MockServer) -> String {
        let file = common::fixtures_dir()
            .join("wallhaven")
            .join("scrape_browse.html");
        let html =
            std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        html.replace("{{HOST}}", &server.address().to_string())
    }

    fn keyless(server: &MockServer, delay: Duration) -> Wallhaven {
        let cfg = WallhavenConfig {
            api_key: String::new(),
            purity: "100".to_owned(),
            sorting: "random".to_owned(),
            categories: "111".to_owned(),
        };
        Wallhaven::with_base_url(
            cfg,
            kakemono_sources::http::client_builder().build().unwrap(),
            server.uri().parse().unwrap(),
        )
        .with_scrape_delay(delay)
    }

    /// `/search` with the config params; `page` is added by the caller.
    fn search() -> wiremock::MockBuilder {
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("purity", "100"))
            .and(query_param("sorting", "random"))
            .and(query_param("categories", "111"))
            .and(query_param("q", "nature"))
    }

    fn html(body: String) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body, "text/html")
    }

    /// Fails `server.verify()` if any wallpaper page is fetched.
    async fn no_page_fetches(server: &MockServer) {
        Mock::given(path_regex("^/w/"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(server)
            .await;
    }

    fn item(image: ImageRef) -> RemoteItem {
        RemoteItem {
            remote_id: "w01".to_owned(),
            preview_url: "https://th.wallhaven.cc/small/w01/w01.jpg".to_owned(),
            image,
            source_url: String::new(),
            title: String::new(),
            author: String::new(),
            width: None,
            height: None,
            nsfw: false,
        }
    }

    #[tokio::test]
    async fn browse_first_call_makes_one_request_for_page_1_and_cursor_is_2() {
        let server = MockServer::start().await;
        search()
            .and(query_param("page", "1"))
            .respond_with(html(fixture(&server)))
            .expect(1)
            .mount(&server)
            .await;
        no_page_fetches(&server).await;
        let batch = keyless(&server, Duration::ZERO)
            .browse("nature", None)
            .await
            .unwrap();
        assert_eq!(batch.next, Some(BrowseCursor("2".to_owned())));
        server.verify().await;
    }

    #[tokio::test]
    async fn browse_next_call_sends_page_from_cursor() {
        let server = MockServer::start().await;
        search()
            .and(query_param("page", "3"))
            .respond_with(html(fixture(&server)))
            .expect(1)
            .mount(&server)
            .await;
        let batch = keyless(&server, Duration::ZERO)
            .browse("nature", Some(&BrowseCursor("3".to_owned())))
            .await
            .unwrap();
        assert_eq!(batch.next, Some(BrowseCursor("4".to_owned())));
        server.verify().await;
    }

    #[tokio::test]
    async fn browse_returns_page_items_and_drops_thumbs_without_an_image() {
        let server = MockServer::start().await;
        search()
            .respond_with(html(fixture(&server)))
            .mount(&server)
            .await;
        let batch = keyless(&server, Duration::ZERO)
            .browse("nature", None)
            .await
            .unwrap();
        let ids: Vec<&str> = batch.items.iter().map(|i| i.remote_id.as_str()).collect();
        let mut expected: Vec<String> = (1..=22).map(|i| format!("w{i:02}")).collect();
        expected.push(kakemono_sources::sanitize_remote_id("../w24"));
        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn browse_maps_a_data_src_thumb() {
        let server = MockServer::start().await;
        search()
            .respond_with(html(fixture(&server)))
            .mount(&server)
            .await;
        let batch = keyless(&server, Duration::ZERO)
            .browse("nature", None)
            .await
            .unwrap();
        let page = format!("{}/w/01", server.uri());
        assert_eq!(
            batch.items[0],
            RemoteItem {
                remote_id: "w01".to_owned(),
                preview_url: "https://th.wallhaven.cc/small/w01/w01.jpg".to_owned(),
                image: ImageRef::Page(page.clone()),
                source_url: page,
                title: String::new(),
                author: String::new(),
                width: Some(1920),
                height: Some(1080),
                nsfw: false,
            }
        );
    }

    #[tokio::test]
    async fn browse_falls_back_to_src_and_flags_sketchy_and_nsfw() {
        let server = MockServer::start().await;
        search()
            .respond_with(html(fixture(&server)))
            .mount(&server)
            .await;
        let batch = keyless(&server, Duration::ZERO)
            .browse("nature", None)
            .await
            .unwrap();
        let w22 = batch.items.iter().find(|i| i.remote_id == "w22").unwrap();
        assert_eq!(w22.preview_url, "https://th.wallhaven.cc/small/w22/w22.jpg");
        let flags: Vec<bool> = batch.items.iter().map(|i| i.nsfw).collect();
        let mut expected = vec![false; 21];
        expected.extend([true, true]);
        assert_eq!(flags, expected);
    }

    #[tokio::test]
    async fn browse_page_without_thumbs_has_no_next() {
        let server = MockServer::start().await;
        search()
            .respond_with(html("<html><body><ul></ul></body></html>".to_owned()))
            .expect(1)
            .mount(&server)
            .await;
        let batch = keyless(&server, Duration::ZERO)
            .browse("nature", Some(&BrowseCursor("9".to_owned())))
            .await
            .unwrap();
        assert!(batch.items.is_empty() && batch.next.is_none(), "{batch:?}");
    }

    /// The scrape delay belongs to `resolve_image`; a browse batch is one quick request. The
    /// delay is long so a loaded machine can't make the request itself look like a wait (the
    /// default 1 s once failed at 1.04 s next to a release build).
    #[tokio::test]
    async fn browse_does_not_wait_the_scrape_delay() {
        const DELAY: Duration = Duration::from_secs(30);
        let server = MockServer::start().await;
        search()
            .respond_with(html(fixture(&server)))
            .mount(&server)
            .await;
        let start = std::time::Instant::now();
        keyless(&server, DELAY)
            .browse("nature", None)
            .await
            .unwrap();
        assert!(start.elapsed() < DELAY, "{:?}", start.elapsed());
    }

    #[tokio::test]
    async fn browse_rate_limit_fails_fast_without_retry() {
        let server = MockServer::start().await;
        search()
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "30"))
            .expect(1)
            .mount(&server)
            .await;
        let err = keyless(&server, Duration::ZERO)
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
        assert_eq!(
            err.to_string(),
            "wallhaven scrape: search page: rate limited (status 429)"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn browse_status_error_names_the_search_page() {
        let server = MockServer::start().await;
        search()
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let err = keyless(&server, Duration::ZERO)
            .browse("nature", None)
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("wallhaven scrape: search page: unexpected status 500 for "),
            "{err}"
        );
    }

    #[tokio::test]
    async fn browse_rejects_a_malformed_cursor_without_a_request() {
        let server = MockServer::start().await;
        for raw in ["0", "", "2:seed", "x"] {
            let err = keyless(&server, Duration::ZERO)
                .browse("nature", Some(&BrowseCursor(raw.to_owned())))
                .await
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                "wallhaven scrape: invalid browse cursor",
                "{raw}"
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn resolve_image_fetches_the_page_and_returns_the_wallpaper_src() {
        let server = MockServer::start().await;
        search()
            .respond_with(html(fixture(&server)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/w/01"))
            .respond_with(html(
                r#"<html><body><img id="wallpaper" src="https://w.wallhaven.cc/full/w0/wallhaven-w01.png"></body></html>"#
                    .to_owned(),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let wallhaven = keyless(&server, Duration::ZERO);
        let batch = wallhaven.browse("nature", None).await.unwrap();
        let url = wallhaven.resolve_image(&batch.items[0]).await.unwrap();
        assert_eq!(url, "https://w.wallhaven.cc/full/w0/wallhaven-w01.png");
        server.verify().await;
    }

    #[tokio::test]
    async fn resolve_image_waits_the_scrape_delay_first() {
        let server = MockServer::start().await;
        Mock::given(path("/w/01"))
            .respond_with(html(
                r#"<img id="wallpaper" src="https://w.wallhaven.cc/full/a.jpg">"#.to_owned(),
            ))
            .mount(&server)
            .await;
        let delay = Duration::from_millis(50);
        let start = std::time::Instant::now();
        keyless(&server, delay)
            .resolve_image(&item(ImageRef::Page(format!("{}/w/01", server.uri()))))
            .await
            .unwrap();
        assert!(start.elapsed() >= delay, "{:?}", start.elapsed());
    }

    #[tokio::test]
    async fn resolve_image_without_a_wallpaper_img_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(path("/w/01"))
            .respond_with(html("<p>removed</p>".to_owned()))
            .mount(&server)
            .await;
        let err = keyless(&server, Duration::ZERO)
            .resolve_image(&item(ImageRef::Page(format!("{}/w/01", server.uri()))))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "wallhaven scrape: wallpaper page: img#wallpaper not found"
        );
    }

    #[tokio::test]
    async fn resolve_image_rate_limit_fails_fast_without_retry() {
        let server = MockServer::start().await;
        Mock::given(path("/w/01"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let err = keyless(&server, Duration::ZERO)
            .resolve_image(&item(ImageRef::Page(format!("{}/w/01", server.uri()))))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::RateLimitedFor { .. }),
            "{err:?}"
        );
        server.verify().await;
    }

    /// Only pages on the configured site are fetched, so an item cannot point the provider at
    /// another host.
    #[tokio::test]
    async fn resolve_image_rejects_a_page_on_another_origin_without_a_request() {
        let server = MockServer::start().await;
        let err = keyless(&server, Duration::ZERO)
            .resolve_image(&item(ImageRef::Page(
                "https://evil.example/w/01".to_owned(),
            )))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "wallhaven scrape: wallpaper page is not on the wallhaven site"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn resolve_image_returns_a_direct_url_as_is() {
        let server = MockServer::start().await;
        let url = keyless(&server, Duration::ZERO)
            .resolve_image(&item(ImageRef::Direct(
                "https://w.wallhaven.cc/full/a.jpg".to_owned(),
            )))
            .await
            .unwrap();
        assert_eq!(url, "https://w.wallhaven.cc/full/a.jpg");
    }

    /// Manual: checks that the live search page still uses lazy `data-src` thumbs.
    /// `cargo nextest run -p kakemono-sources --run-ignored only browse_live_wallhaven_scrape`
    #[tokio::test]
    #[ignore = "network: hits wallhaven.cc"]
    async fn browse_live_wallhaven_scrape() {
        let wallhaven = Wallhaven::new(
            WallhavenConfig::default(),
            kakemono_sources::http::client_builder().build().unwrap(),
        );
        let batch = wallhaven.browse("", None).await.unwrap();
        assert!(batch.items.len() >= 20, "{batch:?}");
        assert!(
            batch
                .items
                .iter()
                .all(|i| i.preview_url.starts_with("https://th.wallhaven.cc/")),
            "{batch:?}"
        );
        assert_eq!(batch.next, Some(BrowseCursor("2".to_owned())));
        let url = wallhaven.resolve_image(&batch.items[0]).await.unwrap();
        assert!(url.starts_with("https://w.wallhaven.cc/full/"), "{url}");
    }
}
