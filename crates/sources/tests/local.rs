//! Local provider against temp-dir trees. Run: `cargo test -p kakemono-sources --test local`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use kakemono_config::schema::LocalConfig;
use kakemono_sources::local::Local;
use kakemono_sources::{Provider, WallpaperResult};

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, b"x").unwrap();
}

/// A tree with images at three depths and files that must be ignored:
///
/// ```text
/// a.jpg  B.PNG  c.webp  .hidden.jpg  notes.txt  noext  folder.jpg/inner.png
/// sub/d.jpeg  sub/deeper/e.TIFF  sub/readme.md
/// ```
fn tree() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for f in [
        "a.jpg",
        "B.PNG",
        "c.webp",
        ".hidden.jpg",
        "notes.txt",
        "noext",
        "folder.jpg/inner.png",
        "sub/d.jpeg",
        "sub/deeper/e.TIFF",
        "sub/readme.md",
    ] {
        touch(&root.join(f));
    }
    tmp
}

fn local(dirs: Vec<PathBuf>, recursive: bool) -> Local {
    Local::new(LocalConfig {
        enabled: true,
        dirs,
        recursive,
    })
}

/// Paths relative to `root`, sorted (fetch shuffles).
fn rel_paths(results: &[WallpaperResult], root: &Path) -> BTreeSet<String> {
    results
        .iter()
        .map(|r| {
            Path::new(&r.image_url)
                .strip_prefix(root)
                .unwrap_or_else(|_| panic!("{} is not under {}", r.image_url, root.display()))
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[tokio::test]
async fn recursive_finds_images_at_every_depth() {
    let tmp = tree();
    let got = local(vec![tmp.path().to_owned()], true)
        .fetch("")
        .await
        .unwrap();
    assert_eq!(
        rel_paths(&got, tmp.path()),
        set(&[
            ".hidden.jpg",
            "B.PNG",
            "a.jpg",
            "c.webp",
            "folder.jpg/inner.png",
            "sub/d.jpeg",
            "sub/deeper/e.TIFF",
        ])
    );
}

#[tokio::test]
async fn non_recursive_reads_only_direct_children() {
    let tmp = tree();
    let got = local(vec![tmp.path().to_owned()], false)
        .fetch("")
        .await
        .unwrap();
    assert_eq!(
        rel_paths(&got, tmp.path()),
        set(&[".hidden.jpg", "B.PNG", "a.jpg", "c.webp"])
    );
}

#[tokio::test]
async fn result_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("My Photo.v2.jpg");
    touch(&file);
    let got = local(vec![tmp.path().to_owned()], true)
        .fetch("ignored query")
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    let r = &got[0];
    assert!(Path::new(&r.image_url).is_absolute(), "{}", r.image_url);
    assert_eq!(Path::new(&r.image_url), file);
    assert_eq!(r.title, "My Photo.v2");
    assert_eq!(r.author, "");
    assert_eq!(r.source, "local");
    assert_eq!(
        r.source_url,
        url::Url::from_file_path(&file).unwrap().as_str()
    );
    assert!(r.source_url.starts_with("file://"), "{}", r.source_url);
    assert!(
        r.source_url.ends_with("/My%20Photo.v2.jpg"),
        "{}",
        r.source_url
    );
}

#[tokio::test]
async fn several_dirs_are_combined_and_bad_ones_skipped() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    touch(&one.path().join("1.jpg"));
    touch(&two.path().join("2.png"));
    let file_not_dir = one.path().join("1.jpg");
    let missing = one.path().join("does-not-exist");

    let got = local(
        vec![
            missing,
            one.path().to_owned(),
            file_not_dir,
            two.path().to_owned(),
        ],
        true,
    )
    .fetch("")
    .await
    .unwrap();
    let names: BTreeSet<String> = got.iter().map(|r| r.title.clone()).collect();
    assert_eq!(names, set(&["1", "2"]));
}

#[tokio::test]
async fn no_dirs_or_no_images_is_empty_ok() {
    assert!(local(vec![], true).fetch("").await.unwrap().is_empty());

    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("notes.txt"));
    assert!(
        local(
            vec![tmp.path().to_owned(), tmp.path().join("missing")],
            true
        )
        .fetch("")
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn order_is_shuffled() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..12 {
        touch(&tmp.path().join(format!("{i:02}.jpg")));
    }
    let provider = local(vec![tmp.path().to_owned()], true);
    let first: Vec<String> = provider
        .fetch("")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.image_url)
        .collect();
    assert_eq!(first.len(), 12);
    // 12! orderings: twenty identical fetches in a row means no shuffle.
    let mut differs = false;
    for _ in 0..20 {
        let next: Vec<String> = provider
            .fetch("")
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.image_url)
            .collect();
        assert_eq!(
            next.iter().collect::<BTreeSet<_>>(),
            first.iter().collect::<BTreeSet<_>>()
        );
        differs |= next != first;
    }
    assert!(differs, "20 fetches returned the same order");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_to_files_kept_broken_and_dir_links_not_followed() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    touch(&outside.path().join("target.jpg"));
    touch(&outside.path().join("nested/inside.jpg"));

    let root = tmp.path();
    symlink(outside.path().join("target.jpg"), root.join("link.jpg")).unwrap();
    symlink(outside.path().join("gone.jpg"), root.join("broken.jpg")).unwrap();
    symlink(outside.path().join("nested"), root.join("dirlink")).unwrap();

    let got = local(vec![root.to_owned()], true).fetch("").await.unwrap();
    assert_eq!(rel_paths(&got, root), set(&["link.jpg"]));
}

#[cfg(unix)]
#[tokio::test]
async fn configured_dir_may_itself_be_a_symlink() {
    use std::os::unix::fs::symlink;

    let real = tree();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("pictures");
    symlink(real.path(), &link).unwrap();

    let got = local(vec![link.clone()], false).fetch("").await.unwrap();
    assert_eq!(
        rel_paths(&got, &link),
        set(&[".hidden.jpg", "B.PNG", "a.jpg", "c.webp"])
    );
}

#[test]
fn name_and_enabled_come_from_config() {
    let off = Local::new(LocalConfig::default());
    assert_eq!(off.name(), "local");
    assert!(!off.enabled(), "local is disabled by default");
    assert!(local(vec![], true).enabled());
}
