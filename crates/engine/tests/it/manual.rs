//! `Engine::check_source`, `Engine::fetch_url` and `Engine::download_now`.
//! Fakes only (`RecordingSetter`,
//! `ManualSignals`, `StaticProvider`) plus a local wiremock server.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use image::{Rgb, RgbImage};
use kakemono_config::Config;
use kakemono_config::schema::SourceType;
use kakemono_engine::manual::{CHECK_TIMEOUT, MAX_FETCH_BYTES};
use kakemono_engine::{EngineError, EngineEvent};
use kakemono_platform::api::Fit;
use kakemono_sources::fake::StaticProvider;
use kakemono_sources::{Provider, ProviderError, WallpaperResult};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{Builder, Fakes, Fixture, image_server, provider, row, write};

const INTERNET_OFF: &str = "Internet access is off";

/// An engine with Internet access and `<root>/manual` as the manual dir (config edited by
/// `edit`, which gets the temp root) whose online sources come from `fakes`.
fn fixture(fakes: Fakes, edit: impl FnOnce(&mut Config, &Path)) -> Fixture {
    Builder::new()
        .internet(true)
        .manual_dir()
        .fakes(fakes)
        .build(edit)
}

fn png_bytes() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    RgbImage::from_pixel(8, 6, Rgb([90, 120, 160]))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn results(name: &str, n: usize) -> Vec<WallpaperResult> {
    (0..n)
        .map(|i| StaticProvider::result(name, &format!("https://i.test/{i}.jpg")))
        .collect()
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

// ─── check_source ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn check_source_counts_provider_results_and_saves_nothing() {
    let wh = Arc::new(StaticProvider::new("wallhaven", results("wallhaven", 3)));
    let f = fixture(vec![("wh", wh.clone())], |_, _| {});

    let count = f
        .engine
        .check_source(SourceType::Wallhaven, "forest")
        .await
        .map_err(|e| e.to_string());

    assert_eq!(count, Ok(3));
    assert_eq!(wh.queries(), ["forest"]);
    assert_eq!(files_under(&f.downloads()), Vec::<PathBuf>::new());
    assert!(f.setter.calls().is_empty());
}

#[tokio::test]
async fn check_source_with_no_results_is_no_images_found() {
    let wh = Arc::new(StaticProvider::new("wallhaven", Vec::new()));
    let f = fixture(vec![("wh", wh)], |_, _| {});
    assert_eq!(
        f.engine
            .check_source(SourceType::Wallhaven, "zzz")
            .await
            .map_err(|e| e.to_string()),
        Err("no images found".to_owned())
    );
}

#[tokio::test]
async fn check_source_returns_the_provider_error_text() {
    let wh = Arc::new(StaticProvider::failing("wallhaven", "wallhaven: HTTP 401"));
    let f = fixture(vec![("wh", wh)], |_, _| {});
    assert_eq!(
        f.engine
            .check_source(SourceType::Wallhaven, "forest")
            .await
            .map_err(|e| e.to_string()),
        Err("wallhaven: HTTP 401".to_owned())
    );
}

#[tokio::test]
async fn check_source_reports_an_unparsable_location() {
    let un = Arc::new(StaticProvider::new("unsplash", results("unsplash", 1)));
    let f = fixture(vec![("un", un.clone())], |_, _| {});
    let err = f
        .engine
        .check_source(SourceType::Unsplash, "https://example.com/not-unsplash")
        .await
        .map_err(|e| e.to_string())
        .unwrap_err();
    assert!(err.contains("unsplash"), "{err}");
    assert_eq!(un.calls(), 0);
}

#[tokio::test]
async fn check_source_online_with_internet_off_sends_nothing() {
    let wh = Arc::new(StaticProvider::new("wallhaven", results("wallhaven", 3)));
    let f = fixture(vec![("wh", wh.clone())], |c, _| c.daemon.internet = false);
    assert_eq!(
        f.engine
            .check_source(SourceType::Wallhaven, "forest")
            .await
            .map_err(|e| e.to_string()),
        Err(INTERNET_OFF.to_owned())
    );
    assert_eq!(wh.calls(), 0);
}

/// A provider whose fetch never finishes.
#[derive(Debug)]
struct Hang;

#[async_trait]
impl Provider for Hang {
    fn name(&self) -> &'static str {
        "bing"
    }
    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn check_source_times_out() {
    let f = fixture(vec![("b", Arc::new(Hang))], |_, _| {});
    let started = tokio::time::Instant::now();
    let err = f
        .engine
        .check_source(SourceType::Bing, "")
        .await
        .map_err(|e| e.to_string())
        .unwrap_err();
    assert_eq!(err, "check timed out after 15s");
    assert_eq!(started.elapsed(), CHECK_TIMEOUT);
}

#[tokio::test]
async fn check_source_counts_a_folder_recursively_without_internet() {
    let f = fixture(Vec::new(), |c, _| c.daemon.internet = false);
    let pics = f.root.join("pics");
    write(&pics.join("a.jpg"), b"x");
    write(&pics.join("sub/b.png"), b"x");
    write(&pics.join("notes.txt"), b"x");
    let loc = pics.to_str().unwrap();

    assert_eq!(
        f.engine
            .check_source(SourceType::Folder, loc)
            .await
            .map_err(|e| e.to_string()),
        Ok(2)
    );
    // Albums use the folder itself, not its subfolders.
    assert_eq!(
        f.engine
            .check_source(SourceType::AlbumName, loc)
            .await
            .map_err(|e| e.to_string()),
        Ok(1)
    );
    assert_eq!(
        f.engine
            .check_source(SourceType::AlbumDate, loc)
            .await
            .map_err(|e| e.to_string()),
        Ok(1)
    );
    let file = pics.join("a.jpg");
    assert_eq!(
        f.engine
            .check_source(SourceType::Images, file.to_str().unwrap())
            .await
            .map_err(|e| e.to_string()),
        Ok(1)
    );
    assert!(f.setter.calls().is_empty());
}

#[tokio::test]
async fn check_source_local_with_no_images_is_no_images_found() {
    let f = fixture(Vec::new(), |_, _| {});
    let empty = f.root.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let txt = write(&f.root.join("a.txt"), b"x");
    for (kind, loc) in [
        (SourceType::Folder, empty.clone()),
        (SourceType::Folder, f.root.join("missing")),
        (SourceType::AlbumName, empty.clone()),
        (SourceType::Images, txt),
    ] {
        assert_eq!(
            f.engine
                .check_source(kind, loc.to_str().unwrap())
                .await
                .map_err(|e| e.to_string()),
            Err("no images found".to_owned()),
            "{kind} {loc:?}"
        );
    }
}

#[tokio::test]
async fn check_source_unsupported_type_is_an_error() {
    let f = fixture(Vec::new(), |_, _| {});
    assert_eq!(
        f.engine
            .check_source(SourceType::Unsupported, "x")
            .await
            .map_err(|e| e.to_string()),
        Err("unsupported source type".to_owned())
    );
}

// ─── fetch_url ────────────────────────────────────────────────────────────────────────────

async fn png_server(name: &str, content_type: &str, body: Vec<u8>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/img/{name}")))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", content_type)
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn fetch_url_saves_into_manual_dir_and_sets_it() {
    let server = png_server("sunset.png", "image/png", png_bytes()).await;
    let f = fixture(Vec::new(), |_, _| {});
    let mut events = f.engine.subscribe();
    let url = format!("{}/img/sunset.png", server.uri());

    let info = f.engine.fetch_url(&url).await.unwrap();

    let saved = f.manual().join("sunset.png");
    assert_eq!(Path::new(&info.path), saved);
    assert_eq!(std::fs::read(&saved).unwrap(), png_bytes());
    assert_eq!(info.source, "manual");
    assert_eq!(info.source_id, "manual");
    assert_eq!(info.image_url, url);
    assert_eq!(info.title, "sunset.png");
    assert_eq!(info.set_at, "2026-03-04T05:06:07+08:00");
    assert_eq!(f.setter.calls(), vec![(saved.clone(), Fit::Center)]);
    let state = f.engine.state();
    assert_eq!(state.history(), std::slice::from_ref(&info));
    assert_eq!(f.engine.status().current.as_ref(), Some(&info));
    assert_eq!(events.try_recv().unwrap(), EngineEvent::Changed(info));
}

#[tokio::test]
async fn fetch_url_keeps_names_unique_and_uses_the_detected_type() {
    // Served as octet-stream with no extension: the magic bytes decide.
    let server = png_server("photo", "application/octet-stream", png_bytes()).await;
    let f = fixture(Vec::new(), |_, _| {});
    write(&f.manual().join("photo.png"), b"older");
    let url = format!("{}/img/photo", server.uri());

    let info = f.engine.fetch_url(&url).await.unwrap();

    assert_eq!(Path::new(&info.path), f.manual().join("photo-2.png"));
    assert_eq!(
        std::fs::read(f.manual().join("photo.png")).unwrap(),
        b"older"
    );
}

#[tokio::test]
async fn fetch_url_rejects_a_body_that_is_not_jpg_png_or_webp() {
    let server = png_server("a.jpg", "text/html", b"<html>nope</html>".to_vec()).await;
    let f = fixture(Vec::new(), |_, _| {});
    let err = f
        .engine
        .fetch_url(&format!("{}/img/a.jpg", server.uri()))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Manual(_)), "{err:?}");
    assert_eq!(err.to_string(), "not a JPEG, PNG or WebP image");
    assert_eq!(files_under(&f.manual()), Vec::<PathBuf>::new());
    assert!(f.setter.calls().is_empty());
    assert!(f.engine.state().history().is_empty());
}

#[tokio::test]
async fn fetch_url_reports_an_http_error_status() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let f = fixture(Vec::new(), |_, _| {});
    let err = f
        .engine
        .fetch_url(&format!("{}/img/a.jpg?key=S3CRET", server.uri()))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("404"), "{err}");
    assert!(!err.contains("S3CRET"), "{err}");
}

#[tokio::test]
async fn fetch_url_with_internet_off_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let f = fixture(Vec::new(), |c, _| c.daemon.internet = false);
    let err = f
        .engine
        .fetch_url(&format!("{}/img/a.png", server.uri()))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), INTERNET_OFF);
    server.verify().await;
}

#[tokio::test]
async fn fetch_url_copies_an_absolute_path_into_manual_dir() {
    // Local files work with Internet access off.
    let f = fixture(Vec::new(), |c, _| c.daemon.internet = false);
    let original = write(&f.root.join("elsewhere/My Pic.png"), &png_bytes());

    let info = f
        .engine
        .fetch_url(original.to_str().unwrap())
        .await
        .unwrap();

    let saved = f.manual().join("My Pic.png");
    assert_eq!(Path::new(&info.path), saved);
    assert_eq!(std::fs::read(&saved).unwrap(), png_bytes());
    assert!(original.exists(), "the original stays");
    assert_eq!(
        (info.source.as_str(), info.source_id.as_str()),
        ("manual", "manual")
    );
    assert_eq!(info.image_url, "");
    assert_eq!(f.setter.calls(), vec![(saved, Fit::Center)]);
    assert_eq!(f.engine.state().history().len(), 1);
}

#[tokio::test]
async fn fetch_url_accepts_a_file_url() {
    let f = fixture(Vec::new(), |_, _| {});
    let original = write(&f.root.join("in/a b.png"), &png_bytes());
    let url = format!("file://{}", original.to_str().unwrap().replace(' ', "%20"));
    let info = f.engine.fetch_url(&url).await.unwrap();
    assert_eq!(Path::new(&info.path), f.manual().join("a b.png"));
}

#[tokio::test]
async fn fetch_url_sets_a_file_already_in_manual_dir_without_copying() {
    let f = fixture(Vec::new(), |_, _| {});
    let inside = write(&f.manual().join("keep.png"), &png_bytes());
    let info = f.engine.fetch_url(inside.to_str().unwrap()).await.unwrap();
    assert_eq!(Path::new(&info.path), inside);
    assert_eq!(files_under(&f.manual()), vec![inside]);
}

#[tokio::test]
async fn fetch_url_rejects_a_local_file_over_the_cap() {
    let f = fixture(Vec::new(), |_, _| {});
    let big = f.root.join("big.png");
    let file = std::fs::File::create(&big).unwrap();
    file.set_len(MAX_FETCH_BYTES + 1).unwrap(); // sparse: no real 64 MB write
    let err = f.engine.fetch_url(big.to_str().unwrap()).await.unwrap_err();
    assert_eq!(err.to_string(), "image is larger than 64 MB");
    assert_eq!(files_under(&f.manual()), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn fetch_url_rejects_a_local_non_image_and_a_missing_file() {
    let f = fixture(Vec::new(), |_, _| {});
    let txt = write(&f.root.join("a.png"), b"not really a png");
    assert_eq!(
        f.engine
            .fetch_url(txt.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string(),
        "not a JPEG, PNG or WebP image"
    );
    let missing = f.root.join("missing.png");
    let err = f
        .engine
        .fetch_url(missing.to_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("missing.png"), "{err}");
    assert_eq!(files_under(&f.manual()), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn fetch_url_rejects_relative_paths_and_other_schemes() {
    let f = fixture(Vec::new(), |_, _| {});
    for input in ["a.png", "ftp://x/a.png", "  "] {
        let err = f.engine.fetch_url(input).await.unwrap_err().to_string();
        assert!(
            err.starts_with("not an http(s) URL or absolute file path"),
            "{input}: {err}"
        );
    }
}

// ─── download_now ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn download_now_with_an_id_downloads_for_that_source() {
    let server = image_server(&["w.jpg"]).await;
    let wh = provider("wallhaven", &format!("{}/img/w.jpg", server.uri()));
    let f = fixture(vec![("wh-1", wh.clone())], |c, _| {
        c.sources = vec![row("wh-1", SourceType::Wallhaven, "forest")];
    });

    let path = f
        .engine
        .download_now(Some("wh-1"))
        .await
        .map_err(|e| e.to_string())
        .unwrap();

    assert_eq!(path, Some(f.downloads().join("wh-1/w.jpg")));
    assert_eq!(wh.calls(), 1);
    assert!(f.setter.calls().is_empty(), "download_now doesn't set");
}

#[tokio::test]
async fn download_now_without_an_id_fills_only_empty_sources() {
    let server = image_server(&["w.jpg", "b.jpg"]).await;
    let wh = provider("wallhaven", &format!("{}/img/w.jpg", server.uri()));
    let bing = provider("bing", &format!("{}/img/b.jpg", server.uri()));
    let f = fixture(
        vec![("wh-1", wh.clone()), ("bing", bing.clone())],
        |c, _| {
            c.sources = vec![
                row("wh-1", SourceType::Wallhaven, "forest"),
                row("bing", SourceType::Bing, ""),
            ];
        },
    );
    write(&f.downloads().join("wh-1/old.jpg"), b"x");

    let path = f
        .engine
        .download_now(None)
        .await
        .map_err(|e| e.to_string())
        .unwrap();

    assert_eq!(path, Some(f.downloads().join("bing/b.jpg")));
    assert_eq!(wh.calls(), 0, "wh-1 already has an image");
    assert_eq!(bing.calls(), 1);
}

#[tokio::test]
async fn download_now_without_an_id_and_nothing_empty_is_null() {
    let wh = provider("wallhaven", "https://i.test/w.jpg");
    let f = fixture(vec![("wh-1", wh.clone())], |c, _| {
        c.sources = vec![row("wh-1", SourceType::Wallhaven, "forest")];
    });
    write(&f.downloads().join("wh-1/old.jpg"), b"x");
    assert_eq!(
        f.engine.download_now(None).await.map_err(|e| e.to_string()),
        Ok(None)
    );
    assert_eq!(wh.calls(), 0);
}

#[tokio::test]
async fn download_now_reports_failures_and_internet_off() {
    let wh = Arc::new(StaticProvider::failing("wallhaven", "wallhaven: boom"));
    let f = fixture(vec![("wh-1", wh)], |c, _| {
        c.sources = vec![row("wh-1", SourceType::Wallhaven, "forest")];
    });
    assert_eq!(
        f.engine.download_now(None).await.map_err(|e| e.to_string()),
        Err("provider wallhaven fetch: wallhaven: boom".to_owned())
    );
    assert_eq!(
        f.engine
            .download_now(Some("nope"))
            .await
            .map_err(|e| e.to_string()),
        Err("no enabled online source \"nope\"".to_owned())
    );

    let f = fixture(Vec::new(), |c, _| c.daemon.internet = false);
    assert_eq!(
        f.engine.download_now(None).await.map_err(|e| e.to_string()),
        Err(INTERNET_OFF.to_owned())
    );
}
