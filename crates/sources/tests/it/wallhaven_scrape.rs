//! Wallhaven scrape mode (Task 15): replays the Go `TestWallhavenScrape_*` cases, plus the page
//! cap, the scrape delay on the paused clock, skipped pages and search-page errors.

use crate::common;

use std::path::PathBuf;
use std::time::Duration;

use common::{Case, Replay};
use kakemono_config::schema::WallhavenConfig;
use kakemono_sources::Provider;
use kakemono_sources::wallhaven::scrape::MAX_PAGES;
use kakemono_sources::wallhaven::{DEFAULT_SCRAPE_DELAY, Wallhaven};

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
