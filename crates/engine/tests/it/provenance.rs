//! History provenance of explicit picks (`docs/plans/picker-browse-impl.md` Task 10):
//! `set_file` infers the source from the folder under `download_dir`, and `set_file_with`
//! records exactly the provenance it is given.

use std::path::Path;

use kakemono_config::Config;
use kakemono_config::schema::SourceType;
use kakemono_engine::Provenance;
use kakemono_engine::state::State;

use crate::common::{Builder, Fixture, provider, row, write};

/// An engine with an enabled Wallhaven row `wh-1a2b` (and whatever `edit` changes).
fn fixture(edit: impl FnOnce(&mut Config)) -> Fixture {
    Builder::new()
        .fakes(vec![(
            "wh-1a2b",
            provider("wallhaven", "https://example.invalid/x.jpg"),
        )])
        .build(|cfg, _| {
            cfg.sources = vec![row("wh-1a2b", SourceType::Wallhaven, "")];
            edit(cfg);
        })
}

fn source_pair(f: &Fixture) -> (String, String) {
    let current = f.engine.status().current.unwrap();
    (current.source, current.source_id)
}

#[tokio::test]
async fn set_file_in_an_online_source_folder_records_that_source() {
    let f = fixture(|_| {});
    let img = write(&f.downloads().join("wh-1a2b/x.jpg"), b"fake image data");
    f.engine.set_file(&img).await.unwrap();
    assert_eq!(
        source_pair(&f),
        ("wallhaven".to_owned(), "wh-1a2b".to_owned())
    );
}

#[tokio::test]
async fn set_file_in_an_online_source_folder_keeps_title_and_path() {
    let f = fixture(|_| {});
    let img = write(&f.downloads().join("wh-1a2b/x.jpg"), b"fake image data");
    let info = f.engine.set_file(&img).await.unwrap();
    assert_eq!(
        (
            info.path.as_str(),
            info.title.as_str(),
            info.source_url.as_str()
        ),
        (img.to_str().unwrap(), "x.jpg", "")
    );
}

#[tokio::test]
async fn set_file_through_a_non_canonical_path_still_infers_the_source() {
    let f = fixture(|_| {});
    write(&f.downloads().join("wh-1a2b/x.jpg"), b"fake image data");
    let img = f.downloads().join("wh-1a2b/../wh-1a2b/x.jpg");
    f.engine.set_file(&img).await.unwrap();
    assert_eq!(
        source_pair(&f),
        ("wallhaven".to_owned(), "wh-1a2b".to_owned())
    );
}

#[tokio::test]
async fn set_file_outside_every_source_folder_records_local() {
    let f = fixture(|_| {});
    let img = f.image("a.jpg");
    f.engine.set_file(&img).await.unwrap();
    assert_eq!(source_pair(&f), ("local".to_owned(), String::new()));
}

#[tokio::test]
async fn set_file_in_a_nested_folder_of_a_source_records_local() {
    let f = fixture(|_| {});
    let img = write(
        &f.downloads().join("wh-1a2b/nested/x.jpg"),
        b"fake image data",
    );
    f.engine.set_file(&img).await.unwrap();
    assert_eq!(source_pair(&f), ("local".to_owned(), String::new()));
}

#[tokio::test]
async fn set_file_in_a_disabled_source_folder_records_local() {
    let f = fixture(|cfg| cfg.sources[0].enabled = false);
    let img = write(&f.downloads().join("wh-1a2b/x.jpg"), b"fake image data");
    f.engine.set_file(&img).await.unwrap();
    assert_eq!(source_pair(&f), ("local".to_owned(), String::new()));
}

#[tokio::test]
async fn set_file_in_a_folder_without_a_source_row_records_local() {
    let f = fixture(|_| {});
    let img = write(&f.downloads().join("wh-gone/x.jpg"), b"fake image data");
    f.engine.set_file(&img).await.unwrap();
    assert_eq!(source_pair(&f), ("local".to_owned(), String::new()));
}

#[tokio::test]
async fn set_file_with_records_exactly_the_given_provenance() {
    let f = fixture(|_| {});
    let img = f.image("b.jpg");
    let info = f
        .engine
        .set_file_with(
            &img,
            Provenance {
                source: "unsplash".to_owned(),
                source_id: "us-77".to_owned(),
                title: "Morning fog".to_owned(),
                source_url: "https://unsplash.com/photos/abc".to_owned(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (
            info.source.as_str(),
            info.source_id.as_str(),
            info.title.as_str(),
            info.source_url.as_str(),
        ),
        (
            "unsplash",
            "us-77",
            "Morning fog",
            "https://unsplash.com/photos/abc"
        )
    );
}

#[tokio::test]
async fn set_file_with_pushes_the_entry_to_saved_history() {
    let f = fixture(|_| {});
    let img = f.image("b.jpg");
    let info = f
        .engine
        .set_file_with(
            &img,
            Provenance {
                source: "unsplash".to_owned(),
                source_id: "us-77".to_owned(),
                title: "Morning fog".to_owned(),
                source_url: String::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(State::load(&f.state_path()).unwrap().current(), Some(&info));
}

#[tokio::test]
async fn set_file_with_a_missing_file_sets_nothing() {
    let f = fixture(|_| {});
    let err = f
        .engine
        .set_file_with(Path::new("/nonexistent/x.jpg"), Provenance::default())
        .await
        .unwrap_err();
    assert_eq!(
        (err.to_string(), f.setter.attempts()),
        (
            "manager: file not found: stat /nonexistent/x.jpg: no such file or directory"
                .to_owned(),
            0
        )
    );
}
