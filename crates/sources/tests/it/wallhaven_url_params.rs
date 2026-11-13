//! Search parameters taken from a pasted `wallhaven.cc` URL ([`WallhavenOverrides`]) reach
//! every request that searches: API and scrape mode, `fetch` and `browse`.

use std::time::Duration;

use kakemono_config::schema::WallhavenConfig;
use kakemono_sources::Provider;
use kakemono_sources::wallhaven::{Wallhaven, WallhavenOverrides};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The search query of every request `server` received, as sent (sorted, encoded).
async fn queries(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.query().unwrap_or_default().to_owned())
        .collect()
}

/// A server that answers every `GET` with an empty result: `{"data":[]}` for the API, an
/// empty search page for scrape mode (both decode as "no wallpapers").
async fn empty_server(api: bool) -> MockServer {
    let server = MockServer::start().await;
    let body = if api {
        ResponseTemplate::new(200).set_body_raw(r#"{"data":[]}"#, "application/json")
    } else {
        ResponseTemplate::new(200).set_body_raw("<html><body></body></html>", "text/html")
    };
    Mock::given(method("GET"))
        .respond_with(body)
        .mount(&server)
        .await;
    server
}

/// The URL `https://wallhaven.cc/search?categories=110&purity=100&topRange=1y&sorting=toplist&order=desc`
/// plus every other supported option, one with characters that must be encoded.
fn overrides() -> WallhavenOverrides {
    WallhavenOverrides {
        categories: Some("110".into()),
        purity: Some("100".into()),
        sorting: Some("toplist".into()),
        top_range: Some("1y".into()),
        order: Some("desc".into()),
        atleast: Some("2560x1440".into()),
        resolutions: Some("1920x1080,2560x1440".into()),
        ratios: Some("16x9,16x10".into()),
        colors: Some("0066cc&x=1".into()),
    }
}

fn wallhaven(server: &MockServer, api_key: &str, overrides: &WallhavenOverrides) -> Wallhaven {
    let cfg = WallhavenConfig {
        api_key: api_key.to_owned(),
        ..WallhavenConfig::default()
    };
    Wallhaven::with_base_url(
        cfg,
        kakemono_sources::http::client_builder().build().unwrap(),
        server.uri().parse().unwrap(),
    )
    .with_scrape_delay(Duration::ZERO)
    .apply_overrides(overrides)
}

/// Query with every [`overrides`] value, sorted by key, before `extra` (also sorted in).
fn want(extra: &[&str]) -> String {
    let mut pairs = vec![
        "atleast=2560x1440",
        "categories=110",
        "colors=0066cc%26x%3D1",
        "order=desc",
        "purity=100",
        "q=forest",
        "ratios=16x9%2C16x10",
        "resolutions=1920x1080%2C2560x1440",
        "sorting=toplist",
        "topRange=1y",
    ];
    pairs.extend_from_slice(extra);
    pairs.sort_by_key(|p| p.split('=').next().unwrap_or_default().to_owned());
    pairs.join("&")
}

#[tokio::test]
async fn api_fetch_sends_url_search_params() {
    let server = empty_server(true).await;
    wallhaven(&server, "k", &overrides())
        .fetch("forest")
        .await
        .unwrap();
    assert_eq!(queries(&server).await, [want(&["apikey=k"])]);
}

#[tokio::test]
async fn api_browse_sends_url_search_params() {
    let server = empty_server(true).await;
    wallhaven(&server, "k", &overrides())
        .browse("forest", None)
        .await
        .unwrap();
    assert_eq!(queries(&server).await, [want(&["apikey=k", "page=1"])]);
}

#[tokio::test]
async fn scrape_fetch_sends_url_search_params() {
    let server = empty_server(false).await;
    wallhaven(&server, "", &overrides())
        .fetch("forest")
        .await
        .unwrap();
    assert_eq!(queries(&server).await, [want(&[])]);
}

#[tokio::test]
async fn scrape_browse_sends_url_search_params() {
    let server = empty_server(false).await;
    wallhaven(&server, "", &overrides())
        .browse("forest", None)
        .await
        .unwrap();
    assert_eq!(queries(&server).await, [want(&["page=1"])]);
}

/// Without URL options the request is the config one, with no extra parameter.
#[tokio::test]
async fn unset_url_options_send_no_extra_params() {
    let server = empty_server(true).await;
    let empty = WallhavenOverrides {
        top_range: Some(String::new()),
        ..WallhavenOverrides::default()
    };
    let provider = wallhaven(&server, "k", &empty);
    provider.fetch("forest").await.unwrap();
    provider.browse("forest", None).await.unwrap();
    assert_eq!(
        queries(&server).await,
        [
            "apikey=k&categories=111&purity=100&q=forest&sorting=random",
            "apikey=k&categories=111&page=1&purity=100&q=forest&sorting=random",
        ]
    );
}
