//! Reddit sorts (`/top/?t=year`, `/new`, ...) reach the listing request.

use kakemono_sources::reddit::{Reddit, Sort, Subreddit, TimeRange};
use kakemono_sources::{BrowseCursor, Provider};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A mock answering every GET with an empty listing.
async fn server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"data": {"children": [], "after": "t3_x"}})),
        )
        .mount(&server)
        .await;
    server
}

fn reddit(server: &MockServer, sort: Sort) -> Reddit {
    let subreddit: Subreddit = "wallpapers".parse().unwrap();
    Reddit::for_subreddit_with_base_url(
        subreddit,
        kakemono_sources::http::client_builder().build().unwrap(),
        server.uri().parse().unwrap(),
    )
    .with_sort(sort)
}

/// `path?query` of every request the server got.
async fn requests(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| format!("{}?{}", r.url.path(), r.url.query().unwrap_or_default()))
        .collect()
}

#[tokio::test]
async fn fetch_top_year_reads_the_top_listing_with_t() {
    let server = server().await;
    reddit(&server, Sort::Top(Some(TimeRange::Year)))
        .fetch("")
        .await
        .unwrap();
    assert_eq!(
        requests(&server).await,
        ["/r/wallpapers/top.json?limit=100&t=year"]
    );
}

#[tokio::test]
async fn fetch_new_reads_the_new_listing_without_t() {
    let server = server().await;
    reddit(&server, Sort::New).fetch("").await.unwrap();
    assert_eq!(
        requests(&server).await,
        ["/r/wallpapers/new.json?limit=100"]
    );
}

#[tokio::test]
async fn fetch_top_without_range_sends_no_t() {
    let server = server().await;
    reddit(&server, Sort::Top(None)).fetch("").await.unwrap();
    assert_eq!(
        requests(&server).await,
        ["/r/wallpapers/top.json?limit=100"]
    );
}

#[tokio::test]
async fn browse_controversial_week_sends_t_on_every_page() {
    let server = server().await;
    let rd = reddit(&server, Sort::Controversial(Some(TimeRange::Week)));
    rd.browse("", None).await.unwrap();
    rd.browse("", Some(&BrowseCursor("t3_x".into())))
        .await
        .unwrap();
    assert_eq!(
        requests(&server).await,
        [
            "/r/wallpapers/controversial.json?limit=25&raw_json=1&t=week",
            "/r/wallpapers/controversial.json?after=t3_x&limit=25&raw_json=1&t=week",
        ]
    );
}

#[tokio::test]
async fn browse_rising_reads_the_rising_listing() {
    let server = server().await;
    reddit(&server, Sort::Rising)
        .browse("", None)
        .await
        .unwrap();
    assert_eq!(
        requests(&server).await,
        ["/r/wallpapers/rising.json?limit=25&raw_json=1"]
    );
}
