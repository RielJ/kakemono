//! The `WebPage` provider against wiremock: page kinds, size probes, the empty-result messages,
//! the per-link cache, the guarded clients, browse and `resolve_image`.
//! Run: `cargo test -p kakemono-sources --test it web::`.
//!
//! Every test starts its own mock server, so the process-wide cache (keyed by page URL) never
//! shares an entry between tests.

use std::time::Duration;

use kakemono_sources::guard::{GuardConfig, GuardError};
use kakemono_sources::{
    BrowseCursor, BrowseSupport, ImageRef, Provider, ProviderError, WebPage, WebTuning,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NO_IMAGES: &str = "No images on this page. Pages built with JavaScript (Google, Pinterest, Instagram) don't work.";
const NO_SIZES: &str = "Couldn't read the image sizes on this page.";

/// A PNG signature plus an IHDR chunk: enough for the size probe.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
    bytes.extend_from_slice(&[0; 4]);
    bytes
}

fn html(body: &str) -> String {
    format!("<!doctype html><html><head><title>t</title></head><body>{body}</body></html>")
}

/// Serves `body` as HTML at `route`, expecting `times` requests.
async fn mount_page(server: &MockServer, route: &str, body: &str, times: u64) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_raw(html(body), "text/html"))
        .expect(times)
        .mount(server)
        .await;
}

async fn mount_png(server: &MockServer, route: &str, width: u32, height: u32) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_raw(png(width, height), "image/png"))
        .mount(server)
        .await;
}

/// A mock server of its own: `MockServer::start` hands out pooled servers whose address
/// another test may have used, and the page cache is keyed by URL.
async fn fresh_server() -> MockServer {
    MockServer::builder().start().await
}

fn web(url: &str) -> WebPage {
    WebPage::with_guard(url, GuardConfig::permissive_loopback()).unwrap()
}

fn unusable(err: ProviderError) -> String {
    match err {
        ProviderError::Unusable(msg) => msg,
        other => panic!("want Unusable, got {other:?}"),
    }
}

fn image_urls(results: &[kakemono_sources::WallpaperResult]) -> Vec<String> {
    results.iter().map(|r| r.image_url.clone()).collect()
}

// ─── page kinds ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn web_html_page_keeps_full_size_images_in_page_order() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/gallery",
        r#"<img src="/uhd.png"><img src="/fhd.png"><img src="/small.png">"#,
        1,
    )
    .await;
    mount_png(&server, "/uhd.png", 3840, 2160).await;
    mount_png(&server, "/fhd.png", 1920, 1080).await;
    mount_png(&server, "/small.png", 800, 600).await;
    let page = format!("{}/gallery", server.uri());

    let results = web(&page).fetch("").await.unwrap();

    let base = server.uri();
    assert_eq!(
        image_urls(&results),
        [format!("{base}/uhd.png"), format!("{base}/fhd.png")]
    );
}

#[tokio::test]
async fn web_result_names_the_page_host_and_source() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/gallery",
        r#"<img src="/uhd.png" alt="Peaks">"#,
        1,
    )
    .await;
    mount_png(&server, "/uhd.png", 3840, 2160).await;
    let page = format!("{}/gallery", server.uri());

    let results = web(&page).fetch("").await.unwrap();

    assert_eq!(
        (
            results[0].source_url.as_str(),
            results[0].author.as_str(),
            results[0].source.as_str(),
            results[0].title.as_str(),
        ),
        (page.as_str(), "127.0.0.1", "web", "Peaks")
    );
}

#[tokio::test]
async fn web_direct_image_link_is_one_result() {
    let server = fresh_server().await;
    mount_png(&server, "/wall.png", 2560, 1440).await;
    let url = format!("{}/wall.png", server.uri());

    let results = web(&url).fetch("").await.unwrap();

    assert_eq!(image_urls(&results), [url]);
}

#[tokio::test]
async fn web_direct_small_image_names_its_size() {
    let server = fresh_server().await;
    mount_png(&server, "/small.png", 800, 600).await;
    let url = format!("{}/small.png", server.uri());

    let err = web(&url).fetch("").await.unwrap_err();

    assert_eq!(
        unusable(err),
        "This image is 800×600; wallpapers need at least 1080 px on the short side."
    );
}

#[tokio::test]
async fn web_page_with_only_small_images_names_the_largest() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/page-with-only-small-images-names-the-largest",
        r#"<img src="/a.png"><img src="/b.png">"#,
        1,
    )
    .await;
    mount_png(&server, "/a.png", 800, 600).await;
    mount_png(&server, "/b.png", 1280, 720).await;

    let err = web(&format!(
        "{}/page-with-only-small-images-names-the-largest",
        server.uri()
    ))
    .fetch("")
    .await
    .unwrap_err();

    assert_eq!(
        unusable(err),
        "No full-size images on this page: the largest is 1280×720, and wallpapers need at \
         least 1080 px on the short side."
    );
}

#[tokio::test]
async fn web_page_without_images_says_so() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/page-without-images-says-so",
        "<p>just text</p>",
        1,
    )
    .await;

    let err = web(&format!("{}/page-without-images-says-so", server.uri()))
        .fetch("")
        .await
        .unwrap_err();

    assert_eq!(unusable(err), NO_IMAGES);
}

#[tokio::test]
async fn web_page_with_unreadable_image_sizes_says_so() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/page-with-unreadable-image-sizes-says-so",
        r#"<img src="/a.jpg"><img src="/b.png">"#,
        1,
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/a.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 512], "image/jpeg"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b.png"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let err = web(&format!(
        "{}/page-with-unreadable-image-sizes-says-so",
        server.uri()
    ))
    .fetch("")
    .await
    .unwrap_err();

    assert_eq!(unusable(err), NO_SIZES);
}

#[tokio::test]
async fn web_link_that_is_not_a_page_is_unusable() {
    let server = fresh_server().await;
    Mock::given(method("GET"))
        .and(path("/data"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{}", "application/json"))
        .mount(&server)
        .await;

    let err = web(&format!("{}/data", server.uri()))
        .fetch("")
        .await
        .unwrap_err();

    assert_eq!(
        unusable(err),
        "web: this link isn't an image, a web page or a feed"
    );
}

#[tokio::test]
async fn web_feed_media_content_images_count() {
    let server = fresh_server().await;
    let base = server.uri();
    let rss = format!(
        r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel><title>f</title>
<item><title>One</title><media:content url="{base}/one.png" medium="image"/></item>
</channel></rss>"#
    );
    Mock::given(method("GET"))
        .and(path("/feed"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(rss, "application/rss+xml"))
        .mount(&server)
        .await;
    mount_png(&server, "/one.png", 3000, 2000).await;

    let results = web(&format!("{base}/feed")).fetch("").await.unwrap();

    assert_eq!(image_urls(&results), [format!("{base}/one.png")]);
}

#[tokio::test]
async fn web_status_error_names_the_status() {
    let server = fresh_server().await;
    Mock::given(method("GET"))
        .and(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let err = web(&format!("{}/gone", server.uri()))
        .fetch("")
        .await
        .unwrap_err();

    assert_eq!(err.to_string(), "web: unexpected status 404");
}

// ─── links refused before any request ──────────────────────────────────────────────────────

#[tokio::test]
async fn web_javascript_site_fails_without_a_request() {
    let page = WebPage::new("https://www.google.com/search?q=mountains&tbm=isch").unwrap();

    let err = page.fetch("").await.unwrap_err();

    // A request would have produced a status or a transport error, and a cache entry.
    assert_eq!(
        unusable(err),
        "Google pages are built with JavaScript and can't be read. Paste a direct image link, \
         a gallery page or an RSS feed instead."
    );
    assert!(page.media_client().is_none());
}

#[test]
fn web_http_link_is_refused_in_strict_mode() {
    let err = WebPage::new("http://example.com/gallery").unwrap_err();
    assert_eq!(
        err.to_string(),
        "web: enter a link that starts with https://"
    );
}

#[test]
fn web_loopback_address_is_refused_in_strict_mode() {
    let err = WebPage::new("https://127.0.0.1/gallery").unwrap_err();
    assert!(
        matches!(&err, ProviderError::NotConfigured(m) if m.contains("IP address")),
        "{err:?}"
    );
}

// ─── size probes ───────────────────────────────────────────────────────────────────────────

/// A server that answers every request with `200`, a 5 MiB `Content-Length` (ignoring
/// `Range`), `head` plus 64 KiB of filler, and then holds the connection open without sending
/// the rest. Returns its base URL and a receiver of each request's head.
async fn range_ignoring_server(
    head: Vec<u8>,
) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    stalling_server("image/png", head).await
}

/// [`range_ignoring_server`] with `content_type` as the response's `Content-Type`.
async fn stalling_server(
    content_type: &'static str,
    head: Vec<u8>,
) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let tx = tx.clone();
            let head = head.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&request).to_ascii_lowercase());
                let total = 5 * 1024 * 1024;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {total}\r\n\r\n"
                );
                let mut body = head;
                body.resize(body.len() + 64 * 1024, 0);
                if socket.write_all(response.as_bytes()).await.is_err()
                    || socket.write_all(&body).await.is_err()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
        }
    });
    (format!("http://{addr}"), rx)
}

#[tokio::test]
async fn web_probe_stops_early_when_the_server_ignores_range() {
    let (image_base, mut requests) = range_ignoring_server(png(3840, 2160)).await;
    let server = fresh_server().await;
    mount_page(
        &server,
        "/probe-stops-early-when-the-server-ignores-range",
        &format!(r#"<img src="{image_base}/huge.png">"#),
        1,
    )
    .await;
    let tuning = WebTuning {
        probe_timeout: Duration::from_secs(3),
        ..WebTuning::default()
    };
    let started = std::time::Instant::now();

    let results = web(&format!(
        "{}/probe-stops-early-when-the-server-ignores-range",
        server.uri()
    ))
    .with_tuning(tuning)
    .fetch("")
    .await
    .unwrap();

    // Reading the whole 5 MiB would wait for bytes that never come, until the probe timeout.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(image_urls(&results), [format!("{image_base}/huge.png")]);
    let request = requests.recv().await.unwrap();
    assert!(request.contains("range: bytes=0-65535"), "{request}");
}

/// A thumbnail that links to an HTML page about the image (as on Wikimedia Commons): the probe
/// sees `text/html` and stops at once, without reading the page for an image size.
#[tokio::test]
async fn web_probe_drops_a_candidate_served_as_html_without_reading_it() {
    let (html_base, mut requests) = stalling_server(
        "text/html; charset=UTF-8",
        b"<!doctype html><html>".to_vec(),
    )
    .await;
    let server = fresh_server().await;
    mount_page(
        &server,
        "/probe-drops-a-candidate-served-as-html-without-reading-it",
        &format!(r#"<a href="{html_base}/wiki/File:Lake.jpg"><img src="/thumb.png"></a>"#),
        1,
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/thumb.png"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let tuning = WebTuning {
        probe_timeout: Duration::from_secs(3),
        ..WebTuning::default()
    };
    let started = std::time::Instant::now();

    let err = web(&format!(
        "{}/probe-drops-a-candidate-served-as-html-without-reading-it",
        server.uri()
    ))
    .with_tuning(tuning)
    .fetch("")
    .await
    .unwrap_err();

    // The server sends 64 KiB of a 5 MiB body and stalls: reading on for a size would wait
    // until the probe timeout.
    assert_eq!(
        (unusable(err), started.elapsed() < Duration::from_secs(2)),
        (NO_SIZES.to_owned(), true),
        "{:?}",
        started.elapsed()
    );
    assert!(
        requests
            .recv()
            .await
            .unwrap()
            .contains("/wiki/file:lake.jpg")
    );
}

#[tokio::test]
async fn web_probe_budget_returns_the_sizes_found_so_far() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/probe-budget-returns-the-sizes-found-so-far",
        r#"<img src="/slow.png"><img src="/fast.png">"#,
        1,
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/slow.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(png(3840, 2160), "image/png")
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    mount_png(&server, "/fast.png", 1920, 1080).await;
    let tuning = WebTuning {
        budget: Duration::from_millis(700),
        ..WebTuning::default()
    };
    let started = std::time::Instant::now();

    let results = web(&format!(
        "{}/probe-budget-returns-the-sizes-found-so-far",
        server.uri()
    ))
    .with_tuning(tuning)
    .fetch("")
    .await
    .unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(image_urls(&results), [format!("{}/fast.png", server.uri())]);
}

// ─── cache ─────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn web_two_instances_of_one_link_read_the_page_once() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/two-instances-of-one-link-read-the-page-once",
        r#"<img src="/a.png">"#,
        1,
    )
    .await;
    mount_png(&server, "/a.png", 3840, 2160).await;
    let page = format!(
        "{}/two-instances-of-one-link-read-the-page-once",
        server.uri()
    );

    let first = web(&page).fetch("").await.unwrap();
    let second = web(&page).fetch("").await.unwrap();

    assert_eq!(first, second);
}

#[tokio::test]
async fn web_check_reads_the_page_again() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/check-reads-the-page-again",
        r#"<img src="/a.png">"#,
        2,
    )
    .await;
    mount_png(&server, "/a.png", 3840, 2160).await;
    let provider = web(&format!("{}/check-reads-the-page-again", server.uri()));

    provider.fetch("").await.unwrap();
    let checked = provider.check("").await.unwrap();

    assert_eq!(checked.len(), 1);
}

#[tokio::test]
async fn web_failure_is_cached() {
    let server = fresh_server().await;
    mount_page(&server, "/failure-is-cached", "<p>nothing</p>", 1).await;
    let page = format!("{}/failure-is-cached", server.uri());

    let first = web(&page).fetch("").await.unwrap_err().to_string();
    let second = web(&page).fetch("").await.unwrap_err().to_string();

    assert_eq!((first.as_str(), second.as_str()), (NO_IMAGES, NO_IMAGES));
}

#[tokio::test]
async fn web_expired_entry_reads_the_page_again() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/expired-entry-reads-the-page-again",
        r#"<img src="/a.png">"#,
        2,
    )
    .await;
    mount_png(&server, "/a.png", 3840, 2160).await;
    let tuning = WebTuning {
        success_ttl: Duration::ZERO,
        ..WebTuning::default()
    };
    let provider = web(&format!(
        "{}/expired-entry-reads-the-page-again",
        server.uri()
    ))
    .with_tuning(tuning);

    provider.fetch("").await.unwrap();
    provider.fetch("").await.unwrap();
}

// ─── guarded clients ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn web_before_a_read_only_the_page_host_is_allowed() {
    let provider = web("http://127.0.0.1:9/never-read");
    assert_eq!(
        (provider.allowed_hosts(), provider.media_client().is_none()),
        (vec!["127.0.0.1".to_owned()], true)
    );
}

#[tokio::test]
async fn web_after_a_read_the_image_host_is_allowed_with_a_client() {
    let server = fresh_server().await;
    let port = server.address().port();
    // The image is on another host name (`localhost`) of the same server.
    mount_page(
        &server,
        "/after-a-read-the-image-host-is-allowed-with-a-client",
        &format!(r#"<img src="http://localhost:{port}/a.png">"#),
        1,
    )
    .await;
    mount_png(&server, "/a.png", 3840, 2160).await;
    let provider = web(&format!(
        "{}/after-a-read-the-image-host-is-allowed-with-a-client",
        server.uri()
    ));

    provider.fetch("").await.unwrap();

    assert_eq!(
        (provider.allowed_hosts(), provider.media_client().is_some()),
        (vec!["127.0.0.1".to_owned(), "localhost".to_owned()], true)
    );
}

#[tokio::test]
async fn web_media_client_refuses_a_redirect_to_a_host_the_page_never_named() {
    let server = fresh_server().await;
    let port = server.address().port();
    mount_page(
        &server,
        "/media-client-refuses-a-redirect-to-a-host-the-page-never-named",
        r#"<img src="/a.png">"#,
        1,
    )
    .await;
    mount_png(&server, "/a.png", 3840, 2160).await;
    Mock::given(method("GET"))
        .and(path("/hop.png"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("http://localhost:{port}/a.png")),
        )
        .mount(&server)
        .await;
    let provider = web(&format!(
        "{}/media-client-refuses-a-redirect-to-a-host-the-page-never-named",
        server.uri()
    ));
    provider.fetch("").await.unwrap();
    let client = provider.media_client().unwrap();

    let err = client
        .get(format!("{}/hop.png", server.uri()))
        .send()
        .await
        .unwrap_err();

    assert!(
        matches!(
            GuardError::find_in(&err),
            Some(GuardError::UnlistedHost { host, .. }) if host == "localhost"
        ),
        "{err:?}"
    );
}

/// The typed page answers `301` to a page on another host (as `www.site.com` -> `site.com`,
/// or `apod.nasa.gov` -> `science.nasa.gov`), whose images are on a third name. Mounts it all
/// and returns the typed link, its server and the page's server.
async fn mount_redirected_page(name: &str) -> (String, MockServer, MockServer) {
    let typed = fresh_server().await;
    let page = fresh_server().await;
    let page_port = page.address().port();
    Mock::given(method("GET"))
        .and(path(format!("/{name}")))
        .respond_with(ResponseTemplate::new(301).insert_header(
            "location",
            format!("http://localhost:{page_port}/{name}-moved"),
        ))
        .expect(1)
        .mount(&typed)
        .await;
    mount_page(
        &page,
        &format!("/{name}-moved"),
        &format!(r#"<img src="http://127.0.0.1:{page_port}/big.png">"#),
        1,
    )
    .await;
    mount_png(&page, "/big.png", 3840, 2160).await;
    (format!("{}/{name}", typed.uri()), typed, page)
}

/// The page redirect is followed; the images come from the final page; the cache stays keyed
/// by the typed link (the second instance reads nothing); the image client allows the typed
/// host, the final host and the image host.
#[tokio::test]
async fn web_page_redirect_to_another_host_is_followed() {
    let (link, _typed, page) = mount_redirected_page("page-redirect-to-another-host").await;
    let port = page.address().port();

    let first = web(&link).fetch("").await.unwrap();
    let second = web(&link);
    let again = second.fetch("").await.unwrap();

    let want = vec![format!("http://127.0.0.1:{port}/big.png")];
    assert_eq!(
        (
            image_urls(&first),
            image_urls(&again),
            second.allowed_hosts()
        ),
        (
            want.clone(),
            want,
            vec!["127.0.0.1".to_owned(), "localhost".to_owned()]
        )
    );
}

/// After a redirected read, the image client still refuses a host that was neither typed,
/// redirected to nor named by the page.
#[tokio::test]
async fn web_media_client_after_a_page_redirect_refuses_other_hosts() {
    let (link, _typed, page) = mount_redirected_page("media-client-after-a-page-redirect").await;
    let port = page.address().port();
    Mock::given(method("GET"))
        .and(path("/hop.png"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("http://127.0.0.2:{port}/big.png")),
        )
        .mount(&page)
        .await;
    let provider = web(&link);
    provider.fetch("").await.unwrap();
    let client = provider.media_client().unwrap();

    let err = client
        .get(format!("http://localhost:{port}/hop.png"))
        .send()
        .await
        .unwrap_err();

    assert!(
        matches!(
            GuardError::find_in(&err),
            Some(GuardError::UnlistedHost { host, .. }) if host == "127.0.0.2"
        ),
        "{err:?}"
    );
}

// ─── browse ────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn web_browse_is_one_finite_batch_with_previews() {
    let server = fresh_server().await;
    let base = server.uri();
    mount_page(
        &server,
        "/browse-is-one-finite-batch-with-previews",
        r#"<a href="/full.png"><img src="/thumb.png"></a>"#,
        1,
    )
    .await;
    mount_png(&server, "/full.png", 3840, 2160).await;
    let provider = web(&format!("{base}/browse-is-one-finite-batch-with-previews"));

    let batch = provider.browse("", None).await.unwrap();
    let item = &batch.items[0];

    assert_eq!(
        (
            provider.browse_support(),
            batch.items.len(),
            batch.next.is_none(),
            item.preview_url.clone(),
            item.image.clone(),
            (item.width, item.height),
        ),
        (
            BrowseSupport::Finite,
            1,
            true,
            format!("{base}/thumb.png"),
            ImageRef::Direct(format!("{base}/full.png")),
            (Some(3840), Some(2160)),
        )
    );
}

#[tokio::test]
async fn web_browse_past_the_first_batch_is_empty() {
    let provider = web("http://127.0.0.1:9/never-read");
    let batch = provider
        .browse("", Some(&BrowseCursor("1".into())))
        .await
        .unwrap();
    assert!(batch.items.is_empty() && batch.next.is_none());
}

#[tokio::test]
async fn web_resolve_image_accepts_only_cached_urls() {
    let server = fresh_server().await;
    mount_page(
        &server,
        "/resolve-image-accepts-only-cached-urls",
        r#"<img src="/a.png">"#,
        1,
    )
    .await;
    mount_png(&server, "/a.png", 3840, 2160).await;
    let provider = web(&format!(
        "{}/resolve-image-accepts-only-cached-urls",
        server.uri()
    ));
    let batch = provider.browse("", None).await.unwrap();
    let known = batch.items[0].clone();
    let mut forged = known.clone();
    forged.image = ImageRef::Direct("https://evil.example/x.png".into());

    let (ok, refused) = (
        provider.resolve_image(&known).await,
        provider.resolve_image(&forged).await,
    );

    assert_eq!(ok.unwrap(), format!("{}/a.png", server.uri()));
    assert!(refused.is_err());
}
