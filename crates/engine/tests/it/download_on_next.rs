//! `next` with background downloads off (`[daemon] download_enabled = false`), Internet on and
//! an online source: each change downloads one fresh image instead of recycling the files
//! already on disk, and falls back to the disk when the download fails. Fakes plus a local
//! wiremock server.

use std::path::Path;
use std::sync::Arc;

use kakemono_config::Config;
use kakemono_config::schema::SourceType;
use kakemono_engine::EngineError;
use kakemono_sources::fake::StaticProvider;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{Builder, Fakes, Fixture, image_server, provider, row, write};

/// An engine with Internet on and background downloads off, over `edit`'s rows.
fn downloads_off(fakes: Fakes, edit: impl FnOnce(&mut Config, &Path)) -> Fixture {
    Builder::new().internet(true).fakes(fakes).build(|c, root| {
        c.daemon.download_enabled = false;
        edit(c, root);
    })
}

#[tokio::test]
async fn next_with_downloads_off_downloads_a_new_image_even_with_files_on_disk() {
    let server = image_server(&["new.jpg"]).await;
    let bing = provider("bing", &format!("{}/img/new.jpg", server.uri()));
    let f = downloads_off(vec![("bing", bing.clone())], |c, _| {
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });
    let old = write(&f.downloads().join("bing/old.jpg"), b"fake image data");

    let info = f.engine.next().await.unwrap();

    let new = f.downloads().join("bing/new.jpg");
    assert_eq!(
        Path::new(&info.path),
        new,
        "not the file already on disk ({old:?})"
    );
    assert_eq!(
        (info.source.as_str(), info.source_id.as_str()),
        ("bing", "bing")
    );
    assert_eq!(bing.calls(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(f.engine.state().history().last(), Some(&info));
}

#[tokio::test]
async fn next_with_downloads_off_falls_back_to_disk_when_the_download_fails() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let bing = provider("bing", &format!("{}/img/new.jpg", server.uri()));
    let f = downloads_off(vec![("bing", bing.clone())], |c, _| {
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });
    let old = write(&f.downloads().join("bing/old.jpg"), b"fake image data");

    let info = f.engine.next().await.unwrap();

    assert_eq!(Path::new(&info.path), old);
    assert_eq!(bing.calls(), 1, "tried the download first");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn next_with_downloads_off_tries_every_online_source_then_reports_fetch() {
    let fakes: Fakes = vec![
        ("bing", Arc::new(StaticProvider::new("bing", vec![]))),
        (
            "apod",
            Arc::new(StaticProvider::failing("apod", "apod: boom")),
        ),
    ];
    let f = downloads_off(fakes, |c, _| {
        c.sources = vec![
            row("bing", SourceType::Bing, ""),
            row("apod", SourceType::Apod, ""),
        ];
    });

    let err = f.engine.next().await.unwrap_err();

    let EngineError::Fetch(failures) = &err else {
        panic!("{err:?}");
    };
    assert_eq!(failures.len(), 2, "{err}");
    assert!(f.setter.calls().is_empty());
}

#[tokio::test]
async fn next_with_downloads_off_and_only_a_local_source_makes_no_request() {
    let server = MockServer::start().await;
    let f = downloads_off(Vec::new(), |c, root| {
        c.sources = vec![row(
            "pics",
            SourceType::Folder,
            root.join("pics").to_str().unwrap(),
        )];
    });
    let pic = write(&f.root.join("pics/p.png"), b"p");

    let info = f.engine.next().await.unwrap();

    assert_eq!(Path::new(&info.path), pic);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn next_with_downloads_off_picks_local_and_online_sources_alike() {
    // One online and one local source: each change downloads with chance 1/2. Over 40 changes,
    // never or always downloading has a chance of 2 × 2^-40.
    let server = image_server(&["new.jpg"]).await;
    let bing = provider("bing", &format!("{}/img/new.jpg", server.uri()));
    let f = downloads_off(vec![("bing", bing.clone())], |c, root| {
        c.sources = vec![
            row("bing", SourceType::Bing, ""),
            row(
                "pics",
                SourceType::Folder,
                root.join("pics").to_str().unwrap(),
            ),
        ];
    });
    for name in ["a.png", "b.png", "c.png"] {
        write(&f.root.join("pics").join(name), b"p");
    }

    for _ in 0..40 {
        f.engine.next().await.unwrap();
    }

    let fetches = bing.calls();
    assert!(
        0 < fetches && fetches < 40,
        "{fetches} fetches in 40 changes"
    );
}

#[tokio::test]
async fn next_with_downloads_off_trims_the_download_folder_to_the_cap() {
    let server = image_server(&["new.jpg"]).await;
    let bing = provider("bing", &format!("{}/img/new.jpg", server.uri()));
    let f = downloads_off(vec![("bing", bing.clone())], |c, _| {
        c.daemon.download_limit_mb_enabled = true;
        c.daemon.download_limit_mb = 1;
        c.sources = vec![row("bing", SourceType::Bing, "")];
    });
    let old = write(&f.downloads().join("bing/old.jpg"), &vec![0; 1024 * 1024]);

    let info = f.engine.next().await.unwrap();

    assert_eq!(Path::new(&info.path), f.downloads().join("bing/new.jpg"));
    assert!(!old.exists(), "the old file over the 1 MB cap is trimmed");
}
