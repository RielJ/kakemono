//! The browse preview cache: guarded, capped
//! preview downloads into `<root>/<source_id>/<remote_id>.<ext>`, bounded concurrency and
//! pruning. A local wiremock server (or a raw TCP server for the concurrency count) stands in
//! for the provider CDN, reached through a `permissive_loopback` guarded client.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use image::{ImageFormat, Rgb, RgbImage};
use kakemono_engine::browse::{PreviewCache, PreviewError, PreviewJob, PreviewLimits};
use kakemono_sources::browse::{ImageRef, RemoteItem};
use kakemono_sources::{GuardConfig, GuardError, guarded_client};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{any, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The host list every test job carries: the loopback literal wiremock listens on.
const HOSTS: &[&str] = &["127.0.0.1"];

/// A bound on every test, so a hang fails instead of stalling the run.
const TEST_DEADLINE: Duration = Duration::from_secs(20);

struct Cache {
    _dir: TempDir,
    root: PathBuf,
    cache: PreviewCache,
}

fn cache_with(limits: PreviewLimits) -> Cache {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("previews");
    let guard = GuardConfig::permissive_loopback();
    let client = guarded_client(guard, HOSTS).unwrap();
    let cache = PreviewCache::new(root.clone(), client, guard).with_limits(limits);
    Cache {
        _dir: dir,
        root,
        cache,
    }
}

fn cache() -> Cache {
    cache_with(PreviewLimits::default())
}

fn item(remote_id: &str, preview_url: String) -> RemoteItem {
    RemoteItem {
        remote_id: remote_id.to_owned(),
        image: ImageRef::Direct(preview_url.clone()),
        preview_url,
        source_url: String::new(),
        title: String::new(),
        author: String::new(),
        width: None,
        height: None,
        nsfw: false,
    }
}

fn job<'a>(item: &'a RemoteItem) -> PreviewJob<'a> {
    PreviewJob {
        source_id: "wh-1",
        item,
        hosts: hosts(HOSTS),
        client: None,
    }
}

fn hosts(list: &[&str]) -> Arc<[String]> {
    list.iter().map(|h| (*h).to_owned()).collect()
}

fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    RgbImage::from_pixel(width, height, Rgb([90, 120, 160]))
        .write_to(&mut out, format)
        .unwrap();
    out.into_inner()
}

fn png() -> Vec<u8> {
    encoded(8, 6, ImageFormat::Png)
}

/// A server whose `GET /p` answers `response`, expected `calls` times (checked on drop).
async fn serving(response: ResponseTemplate, calls: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(response)
        .expect(calls)
        .mount(&server)
        .await;
    server
}

/// Every file under `dir`, recursively (none when it doesn't exist).
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out.sort();
    out
}

async fn bounded<F: Future>(fut: F) -> F::Output {
    tokio::time::timeout(TEST_DEADLINE, fut)
        .await
        .expect("test deadline")
}

#[tokio::test]
async fn png_preview_is_stored_as_source_id_slash_remote_id_dot_png() {
    let server = serving(ResponseTemplate::new(200).set_body_bytes(png()), 1).await;
    let c = cache();
    let it = item("abc", format!("{}/p", server.uri()));

    let got = bounded(c.cache.fetch(job(&it))).await;

    assert_eq!(got, Some(c.root.join("wh-1").join("abc.png")));
}

#[tokio::test]
async fn stored_preview_holds_the_served_bytes() {
    let server = serving(ResponseTemplate::new(200).set_body_bytes(png()), 1).await;
    let c = cache();
    let it = item("abc", format!("{}/p", server.uri()));

    let got = bounded(c.cache.fetch(job(&it))).await.unwrap();

    assert_eq!(std::fs::read(got).unwrap(), png());
}

#[tokio::test]
async fn second_fetch_reuses_the_file_without_a_request() {
    // `expect(1)`: the mock fails the test on drop if the second fetch hits the network.
    let server = serving(ResponseTemplate::new(200).set_body_bytes(png()), 1).await;
    let c = cache();
    let it = item("abc", format!("{}/p", server.uri()));

    let first = bounded(c.cache.fetch(job(&it))).await;
    let second = bounded(c.cache.fetch(job(&it))).await;

    assert_eq!(second, first);
}

#[tokio::test]
async fn jpeg_and_webp_previews_get_their_own_extensions() {
    let server = MockServer::start().await;
    for (route, body) in [
        ("/j", encoded(8, 6, ImageFormat::Jpeg)),
        ("/w", encoded(8, 6, ImageFormat::WebP)),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;
    }
    let c = cache();
    let jpeg = item("j", format!("{}/j", server.uri()));
    let webp = item("w", format!("{}/w", server.uri()));

    let got = bounded(c.cache.fetch_all([job(&jpeg), job(&webp)])).await;

    assert_eq!(
        got,
        [
            Some(c.root.join("wh-1/j.jpg")),
            Some(c.root.join("wh-1/w.webp"))
        ]
    );
}

#[tokio::test]
async fn body_over_four_mib_is_refused() {
    let mut body = png();
    body.resize(5 * 1024 * 1024, 0);
    let server = serving(ResponseTemplate::new(200).set_body_bytes(body), 1).await;
    let c = cache();
    let it = item("big", format!("{}/p", server.uri()));

    let got = bounded(c.cache.try_fetch(job(&it))).await;

    assert!(matches!(got, Err(PreviewError::TooLarge)), "{got:?}");
}

#[tokio::test]
async fn refused_body_leaves_no_file() {
    let mut body = png();
    body.resize(5 * 1024 * 1024, 0);
    let server = serving(ResponseTemplate::new(200).set_body_bytes(body), 1).await;
    let c = cache();
    let it = item("big", format!("{}/p", server.uri()));

    let got = bounded(c.cache.fetch(job(&it))).await;

    assert_eq!((got, files_under(&c.root)), (None, vec![]));
}

#[tokio::test]
async fn html_body_is_refused_and_leaves_no_file() {
    let html = ResponseTemplate::new(200)
        .insert_header("content-type", "text/html")
        .set_body_string("<html><body>not an image</body></html>");
    let server = serving(html, 1).await;
    let c = cache();
    let it = item("page", format!("{}/p", server.uri()));

    let got = bounded(c.cache.try_fetch(job(&it))).await;

    assert!(
        matches!(got, Err(PreviewError::NotAnImage)) && files_under(&c.root).is_empty(),
        "{got:?}, files: {:?}",
        files_under(&c.root)
    );
}

#[tokio::test]
async fn header_wider_than_4096_is_refused_and_leaves_no_file() {
    let wide = encoded(5000, 10, ImageFormat::Png);
    let server = serving(ResponseTemplate::new(200).set_body_bytes(wide), 1).await;
    let c = cache();
    let it = item("wide", format!("{}/p", server.uri()));

    let got = bounded(c.cache.try_fetch(job(&it))).await;

    assert!(
        matches!(
            got,
            Err(PreviewError::Dimensions {
                width: 5000,
                height: 10
            })
        ) && files_under(&c.root).is_empty(),
        "{got:?}, files: {:?}",
        files_under(&c.root)
    );
}

#[tokio::test]
async fn error_status_is_refused() {
    let server = serving(ResponseTemplate::new(404), 1).await;
    let c = cache();
    let it = item("gone", format!("{}/p", server.uri()));

    let got = bounded(c.cache.try_fetch(job(&it))).await;

    assert!(matches!(got, Err(PreviewError::Status(404))), "{got:?}");
}

#[tokio::test]
async fn slow_server_past_the_timeout_gives_none() {
    let slow = ResponseTemplate::new(200)
        .set_body_bytes(png())
        .set_delay(Duration::from_secs(5));
    let server = serving(slow, 1).await;
    let c = cache_with(PreviewLimits {
        timeout: Duration::from_millis(200),
        ..PreviewLimits::default()
    });
    let it = item("slow", format!("{}/p", server.uri()));

    let got = bounded(c.cache.try_fetch(job(&it))).await;

    assert!(matches!(got, Err(PreviewError::Timeout)), "{got:?}");
}

#[tokio::test]
async fn url_outside_the_host_list_is_refused_without_a_request() {
    let server = serving(ResponseTemplate::new(200).set_body_bytes(png()), 0).await;
    let c = cache();
    let it = item("abc", format!("{}/p", server.uri()));
    let elsewhere = PreviewJob {
        hosts: hosts(&["wallhaven.cc"]),
        ..job(&it)
    };

    let got = bounded(c.cache.try_fetch(elsewhere)).await;

    assert!(
        matches!(
            got,
            Err(PreviewError::Guard(GuardError::UnlistedHost { .. }))
        ),
        "{got:?}"
    );
}

/// A source with its own media client: the preview goes through it, never the cache's client.
#[tokio::test]
async fn job_with_its_own_client_downloads_through_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .and(header("x-client", "row"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .with_priority(10)
        .expect(0)
        .mount(&server)
        .await;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-client", "row".parse().unwrap());
    let own = kakemono_sources::http::client_builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let c = cache();
    let it = item("abc", format!("{}/p", server.uri()));
    let with_client = PreviewJob {
        client: Some(own),
        ..job(&it)
    };

    let got = bounded(c.cache.try_fetch(with_client)).await;

    assert!(got.is_ok_and(|p| p.is_file()));
    server.verify().await;
}

#[tokio::test]
async fn redirect_off_the_client_host_list_is_refused() {
    let server = MockServer::start().await;
    // `localhost` is allowed in permissive mode but is not on the client's list.
    let target = format!("http://localhost:{}/p", server.address().port());
    Mock::given(method("GET"))
        .and(path("/r"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", target.as_str()))
        .expect(1)
        .mount(&server)
        .await;
    let c = cache();
    let it = item("abc", format!("{}/r", server.uri()));

    let got = bounded(c.cache.try_fetch(job(&it))).await;

    assert!(
        matches!(
            got,
            Err(PreviewError::Guard(GuardError::UnlistedHost { .. }))
        ),
        "{got:?}"
    );
}

#[tokio::test]
async fn fetch_all_keeps_input_order_and_reports_failures_as_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png()))
        .mount(&server)
        .await;
    let c = cache();
    let ok = item("ok", format!("{}/ok", server.uri()));
    let missing = item("missing", format!("{}/missing", server.uri()));

    let got = bounded(c.cache.fetch_all([job(&missing), job(&ok)])).await;

    assert_eq!(got, [None, Some(c.root.join("wh-1/ok.png"))]);
}

/// The most requests a raw HTTP server had open at once over `jobs` fetches. Each request is
/// held for a while before the server answers, so overlapping fetches overlap on the server.
async fn max_in_flight(jobs: usize) -> usize {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let open = Arc::new(AtomicUsize::new(0));
    let max = Arc::new(AtomicUsize::new(0));
    let body = png();
    let server = {
        let (open, max) = (Arc::clone(&open), Arc::clone(&max));
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (open, max, body) = (Arc::clone(&open), Arc::clone(&max), body.clone());
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0_u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        request.extend_from_slice(&buf[..n]);
                    }
                    let now = open.fetch_add(1, Ordering::SeqCst) + 1;
                    max.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    open.fetch_sub(1, Ordering::SeqCst);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    socket.write_all(head.as_bytes()).await.unwrap();
                    socket.write_all(&body).await.unwrap();
                    socket.shutdown().await.unwrap();
                });
            }
        })
    };
    let c = cache();
    let items: Vec<RemoteItem> = (0..jobs)
        .map(|i| item(&format!("i{i}"), format!("http://{addr}/{i}")))
        .collect();

    let got = bounded(c.cache.fetch_all(items.iter().map(job))).await;

    server.abort();
    assert!(got.iter().all(Option::is_some), "{got:?}");
    max.load(Ordering::SeqCst)
}

#[tokio::test]
async fn fetch_all_runs_at_most_four_fetches_at_once() {
    // Exactly 4: more would break the bound, fewer would mean the fetches ran one by one.
    assert_eq!(max_in_flight(10).await, 4);
}

/// Writes `<root>/<source>/<name>` with its modification time `age` ago.
fn aged(root: &Path, source: &str, name: &str, age: Duration) -> PathBuf {
    let dir = root.join(source);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let file = std::fs::File::create(&p).unwrap();
    file.set_modified(SystemTime::now() - age).unwrap();
    p
}

const HOUR: Duration = Duration::from_secs(3600);

#[tokio::test]
async fn prune_removes_files_older_than_a_day() {
    let c = cache();
    let fresh = aged(&c.root, "wh-1", "new.png", HOUR);
    aged(&c.root, "wh-1", "old.png", 25 * HOUR);
    aged(&c.root, "bing-2", "old.jpg", 30 * HOUR);
    aged(&c.root, "bing-2", "x.jpg.0.part", 48 * HOUR);

    bounded(c.cache.prune()).await.unwrap();

    assert_eq!(files_under(&c.root), [fresh]);
}

#[tokio::test]
async fn prune_keeps_only_the_newest_files_up_to_the_cap() {
    let c = cache_with(PreviewLimits {
        max_files: 2,
        ..PreviewLimits::default()
    });
    let newest = aged(&c.root, "wh-1", "a.png", HOUR);
    aged(&c.root, "wh-1", "b.png", 3 * HOUR);
    let second = aged(&c.root, "bing-2", "c.jpg", 2 * HOUR);
    aged(&c.root, "bing-2", "d.jpg", 4 * HOUR);

    bounded(c.cache.prune()).await.unwrap();

    let mut want = vec![newest, second];
    want.sort();
    assert_eq!(files_under(&c.root), want);
}

#[tokio::test]
async fn prune_reports_how_many_files_it_removed() {
    let c = cache();
    aged(&c.root, "wh-1", "old.png", 25 * HOUR);
    aged(&c.root, "wh-1", "new.png", HOUR);

    let removed = bounded(c.cache.prune()).await.unwrap();

    assert_eq!(removed, 1);
}

#[tokio::test]
async fn reused_preview_survives_the_next_prune() {
    // `expect(0)`: the old file is reused, not fetched again.
    let server = serving(ResponseTemplate::new(200).set_body_bytes(png()), 0).await;
    let c = cache();
    let old = aged(&c.root, "wh-1", "abc.png", 25 * HOUR);
    let it = item("abc", format!("{}/p", server.uri()));
    bounded(c.cache.fetch(job(&it))).await;

    bounded(c.cache.prune()).await.unwrap();

    assert_eq!(files_under(&c.root), [old]);
}

#[tokio::test]
async fn prune_of_a_missing_root_removes_nothing() {
    let c = cache();

    let removed = bounded(c.cache.prune()).await.unwrap();

    assert_eq!(removed, 0);
}
