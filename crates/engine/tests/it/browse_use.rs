//! `Engine::browse_use`: set, favorite or download
//! a browsed remote item. Downloads go through a `permissive_loopback` guarded client to a
//! local wiremock server; the scripted providers come from `browse_session.rs`.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{ImageFormat, Rgb, RgbImage};
use kakemono_config::schema::SourceType;
use kakemono_engine::browse::{BrowseError, BrowseLimits};
use kakemono_engine::files::SEEN_LEDGER;
use kakemono_protocol::types::{BrowseAction, BrowseUseData};
use kakemono_sources::Provider;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::browse_session::{BrowseBuilder, Browsing, Scripted, strings};
use crate::common::row;

/// The host wiremock listens on.
const LOOPBACK: &[&str] = &["127.0.0.1"];

/// A real PNG over the downloader's 10 KiB minimum: noise does not compress.
fn noisy_png() -> Vec<u8> {
    let mut seed: u32 = 7;
    let image = RgbImage::from_fn(96, 64, |_, _| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let [r, g, b, _] = seed.to_le_bytes();
        Rgb([r, g, b])
    });
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).unwrap();
    let bytes = out.into_inner();
    assert!(bytes.len() > 10 * 1024, "{}", bytes.len());
    bytes
}

/// Serves `/img/<id>.jpg` for each id, `expect` times each (checked on drop).
async fn images(ids: &[&str], expect: u64) -> MockServer {
    let server = MockServer::start().await;
    for id in ids {
        Mock::given(method("GET"))
            .and(path(format!("/img/{id}.jpg")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(noisy_png()),
            )
            .expect(expect)
            .mount(&server)
            .await;
    }
    server
}

fn dyn_provider(p: &Arc<Scripted>) -> Arc<dyn Provider> {
    Arc::clone(p) as Arc<dyn Provider>
}

/// A Wallhaven source `wh-1` with items `ids`, served by `server` and allowed by the guard.
fn wallhaven(server: &MockServer, ids: &[&str]) -> Arc<Scripted> {
    Arc::new(Scripted::one_batch("wallhaven", ids).serving(&server.uri(), LOOPBACK))
}

fn engine(p: &Arc<Scripted>) -> BrowseBuilder {
    BrowseBuilder::new(vec![("wh-1", dyn_provider(p))]).loopback(LOOPBACK)
}

/// Browses page 0 so the items are in the index.
async fn browsed(f: &Browsing) {
    f.engine.browse(None, 0, None).await.unwrap();
}

async fn use_item(f: &Browsing, remote_id: &str, action: BrowseAction) -> BrowseUseData {
    f.engine
        .browse_use("wh-1", remote_id, action)
        .await
        .unwrap()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[tokio::test]
async fn set_downloads_into_the_source_folder() {
    let server = images(&["a1"], 1).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let used = use_item(&f, "a1", BrowseAction::Set).await;
    assert_eq!(
        (
            PathBuf::from(&used.path),
            files_in(&f.downloads().join("wh-1"))
        ),
        (
            f.downloads().join("wh-1").join("a1.jpg"),
            strings(&[SEEN_LEDGER, "a1.jpg"])
        )
    );
}

#[tokio::test]
async fn set_records_the_image_url_in_the_seen_ledger() {
    let server = images(&["a1"], 1).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    use_item(&f, "a1", BrowseAction::Set).await;
    let ledger = std::fs::read_to_string(f.downloads().join("wh-1").join(SEEN_LEDGER)).unwrap();
    assert_eq!(ledger, format!("{}/img/a1.jpg\n", server.uri()));
}

#[tokio::test]
async fn set_sets_the_wallpaper_with_the_remote_provenance() {
    let server = images(&["a1"], 1).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let used = use_item(&f, "a1", BrowseAction::Set).await;
    let info = used.wallpaper.unwrap();
    assert_eq!(
        (
            info.source.as_str(),
            info.source_id.as_str(),
            info.title.as_str(),
            info.source_url,
            f.setter.calls().len(),
            f.engine.state().current().map(|c| c.path.clone()),
        ),
        (
            "wallhaven",
            "wh-1",
            "title a1",
            format!("{}/w/a1", server.uri()),
            1,
            Some(used.path.clone())
        )
    );
}

#[tokio::test]
async fn set_then_favorite_downloads_once_and_copies_to_favorites() {
    let server = images(&["a1"], 1).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let set = use_item(&f, "a1", BrowseAction::Set).await;
    let fav = use_item(&f, "a1", BrowseAction::Favorite).await;
    assert_eq!(
        (
            fav.path == set.path,
            fav.wallpaper.is_none(),
            files_in(&f.favorites()),
            p.resolves()
        ),
        (true, true, strings(&["a1.jpg"]), 1)
    );
}

#[tokio::test]
async fn download_neither_sets_nor_favorites() {
    let server = images(&["a1"], 1).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let used = use_item(&f, "a1", BrowseAction::Download).await;
    assert_eq!(
        (
            used.wallpaper.is_none(),
            Path::new(&used.path).exists(),
            f.setter.calls().len(),
            files_in(&f.favorites()).len()
        ),
        (true, true, 0, 0)
    );
}

#[tokio::test]
async fn a_downloaded_file_that_was_removed_is_downloaded_again() {
    let server = images(&["a1"], 2).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let first = use_item(&f, "a1", BrowseAction::Download).await;
    std::fs::remove_file(&first.path).unwrap();
    let second = use_item(&f, "a1", BrowseAction::Download).await;
    assert!(Path::new(&second.path).exists());
}

#[tokio::test]
async fn an_image_the_filters_reject_is_kept() {
    let server = images(&["a1"], 1).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p)
        .edit(|cfg, _| {
            cfg.filters.name_regex_enabled = true;
            cfg.filters.name_regex = "^never$".to_owned();
            cfg.filters.landscape = true;
        })
        .build();
    browsed(&f).await;
    let used = use_item(&f, "a1", BrowseAction::Set).await;
    assert_eq!(
        (Path::new(&used.path).exists(), f.setter.calls().len()),
        (true, 1)
    );
}

#[tokio::test]
async fn an_unlisted_image_host_is_refused_without_a_file() {
    let server = images(&["a1"], 0).await;
    let p = Arc::new(
        Scripted::one_batch("wallhaven", &["a1"]).serving(&server.uri(), &["wallhaven.cc"]),
    );
    let f = engine(&p).build();
    browsed(&f).await;
    let err = f
        .engine
        .browse_use("wh-1", "a1", BrowseAction::Set)
        .await
        .unwrap_err();
    assert_eq!(
        (
            matches!(err, BrowseError::Guard(_)),
            files_in(&f.downloads().join("wh-1")).len()
        ),
        (true, 0),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_redirect_to_an_unlisted_host_is_a_guard_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/img/a1.jpg"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "http://localhost:9/evil.jpg"),
        )
        .mount(&server)
        .await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let err = f
        .engine
        .browse_use("wh-1", "a1", BrowseAction::Download)
        .await
        .unwrap_err();
    assert!(matches!(err, BrowseError::Guard(_)), "{err:?}");
}

#[tokio::test]
async fn an_unknown_item_is_expired() {
    let server = images(&[], 0).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let err = f
        .engine
        .browse_use("wh-1", "zz", BrowseAction::Set)
        .await
        .unwrap_err();
    assert!(matches!(err, BrowseError::ItemExpired), "{err:?}");
}

#[tokio::test]
async fn an_item_evicted_from_the_index_is_expired() {
    let server = images(&[], 0).await;
    let p = wallhaven(&server, &["a1", "a2", "a3"]);
    let f = engine(&p)
        .limits(BrowseLimits {
            index_cap: 2,
            ..BrowseLimits::default()
        })
        .build();
    browsed(&f).await;
    let err = f
        .engine
        .browse_use("wh-1", "a1", BrowseAction::Set)
        .await
        .unwrap_err();
    assert!(matches!(err, BrowseError::ItemExpired), "{err:?}");
}

#[tokio::test]
async fn a_source_disabled_since_browsing_is_unknown() {
    let server = images(&[], 0).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let mut cfg = (*f.engine.config()).clone();
    cfg.sources = vec![kakemono_config::schema::SourceEntry {
        enabled: false,
        ..row("wh-1", SourceType::Wallhaven, "")
    }];
    f.engine.reload(cfg).await.unwrap();
    let err = f
        .engine
        .browse_use("wh-1", "a1", BrowseAction::Set)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, BrowseError::UnknownSource(id) if id == "wh-1"),
        "{err:?}"
    );
}

#[tokio::test]
async fn internet_off_refuses_to_use_an_item() {
    let server = images(&[], 0).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p).build();
    browsed(&f).await;
    let mut cfg = (*f.engine.config()).clone();
    cfg.daemon.internet = false;
    f.engine.reload(cfg).await.unwrap();
    let err = f
        .engine
        .browse_use("wh-1", "a1", BrowseAction::Set)
        .await
        .unwrap_err();
    assert!(matches!(err, BrowseError::InternetOff), "{err:?}");
}

#[tokio::test]
async fn a_page_link_item_is_resolved_before_download() {
    let server = images(&["a1"], 1).await;
    let p = Arc::new(
        Scripted::one_batch("wallhaven", &["a1"])
            .serving(&server.uri(), LOOPBACK)
            .page_images(),
    );
    let f = engine(&p).build();
    browsed(&f).await;
    let used = use_item(&f, "a1", BrowseAction::Download).await;
    assert_eq!((Path::new(&used.path).exists(), p.resolves()), (true, 1));
}

#[tokio::test]
async fn the_download_limit_spares_the_new_file() {
    let server = images(&["a1"], 1).await;
    let p = wallhaven(&server, &["a1"]);
    let f = engine(&p)
        .edit(|cfg, root| {
            cfg.daemon.download_limit_mb_enabled = true;
            cfg.daemon.download_limit_mb = 0;
            crate::common::write(&root.join("downloads/wh-1/old.jpg"), &[1; 2048]);
        })
        .build();
    browsed(&f).await;
    let used = use_item(&f, "a1", BrowseAction::Download).await;
    assert_eq!(
        (
            Path::new(&used.path).exists(),
            f.downloads().join("wh-1/old.jpg").exists()
        ),
        (true, false)
    );
}

/// The daemon runs every request on a spawned task, so both futures must be `Send` for any
/// borrow lifetime. A closure over a borrowed stream item breaks that while each call alone
/// still compiles; this only has to build.
#[expect(dead_code, reason = "a compile-time check, never called")]
fn browse_futures_are_spawnable(engine: kakemono_engine::Engine) {
    tokio::spawn(async move {
        let _ = engine.browse(None, 0, None).await;
        let _ = engine.browse_use("wh", "1", BrowseAction::Set).await;
    });
}
