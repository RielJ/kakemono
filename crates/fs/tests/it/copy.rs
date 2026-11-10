//! `copy_unique` and `copy_unique_into`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Barrier;

use crate::within_deadline;
use kakemono_fs::{CopyOp, collision_name, copy_unique, copy_unique_into};

/// Whether `path` is Go's collision name for `<stem>.<ext>`: `<stem>_<unix nanos>.<ext>`.
fn is_collision_name(path: &Path, stem: &str, ext: &str) -> bool {
    let name = path.file_name().unwrap().to_str().unwrap();
    name.strip_prefix(&format!("{stem}_"))
        .and_then(|rest| rest.strip_suffix(&format!(".{ext}")))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    fs::write(path, bytes).unwrap();
    path.to_owned()
}

#[test]
fn collision_name_keeps_the_extension() {
    assert_eq!(
        collision_name(Path::new("/d/photo.jpg"), 42),
        Path::new("/d/photo_42.jpg")
    );
    assert_eq!(
        collision_name(Path::new("/d/a.tar.gz"), 7),
        Path::new("/d/a.tar_7.gz")
    );
    assert_eq!(
        collision_name(Path::new("/d/.hidden"), 3),
        Path::new("/d/_3.hidden")
    );
    assert_eq!(
        collision_name(Path::new("/d/README"), 1),
        Path::new("/d/README_1")
    );
}

#[test]
fn copies_to_the_free_destination() {
    let dir = tempfile::tempdir().unwrap();
    let src = write(&dir.path().join("a.png"), b"image");
    let dst = dir.path().join("b.png");
    assert_eq!(copy_unique(&src, &dst).unwrap(), dst);
    assert_eq!(fs::read(&dst).unwrap(), b"image");
}

#[test]
fn a_collision_gets_a_go_name_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let src = write(&dir.path().join("src.png"), b"new");
    let dst = write(&dir.path().join("a.png"), b"old");

    let first = copy_unique(&src, &dst).unwrap();
    let second = copy_unique(&src, &dst).unwrap();

    assert_eq!(fs::read(&dst).unwrap(), b"old");
    assert_ne!(first, second);
    for p in [&first, &second] {
        assert!(is_collision_name(p, "a", "png"), "{}", p.display());
        assert_eq!(fs::read(p).unwrap(), b"new");
    }
}

#[test]
fn copy_unique_into_takes_the_source_name() {
    let dir = tempfile::tempdir().unwrap();
    let src_dir = dir.path().join("src");
    let favorites = dir.path().join("favorites");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&favorites).unwrap();
    let src = write(&src_dir.join("1.jpg"), b"one");
    write(&favorites.join("1.jpg"), b"other folder's 1.jpg");

    let copied = copy_unique_into(&src, &favorites).unwrap();

    assert!(
        is_collision_name(&copied, "1", "jpg"),
        "{}",
        copied.display()
    );
    assert_eq!(fs::read(&copied).unwrap(), b"one");
    assert_eq!(
        fs::read(favorites.join("1.jpg")).unwrap(),
        b"other folder's 1.jpg"
    );
}

#[test]
fn concurrent_copies_of_one_name_never_overwrite() {
    within_deadline(|| {
        const THREADS: usize = 8;
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("out").join("wall.jpg");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        let sources: Vec<PathBuf> = (0..THREADS)
            .map(|i| {
                write(
                    &dir.path().join(format!("src{i}.jpg")),
                    format!("body {i}").as_bytes(),
                )
            })
            .collect();
        let barrier = Barrier::new(THREADS);

        let written: Vec<(usize, PathBuf)> = std::thread::scope(|s| {
            let handles: Vec<_> = sources
                .iter()
                .enumerate()
                .map(|(i, src)| {
                    let (dst, barrier) = (&dst, &barrier);
                    s.spawn(move || {
                        barrier.wait();
                        (i, copy_unique(src, dst).unwrap())
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let unique: HashSet<&PathBuf> = written.iter().map(|(_, p)| p).collect();
        assert_eq!(
            unique.len(),
            THREADS,
            "two copies wrote one path: {written:?}"
        );
        for (i, path) in &written {
            assert_eq!(fs::read_to_string(path).unwrap(), format!("body {i}"));
            assert!(
                path == &dst || is_collision_name(path, "wall", "jpg"),
                "{}",
                path.display()
            );
        }
    });
}

#[test]
fn a_missing_source_is_an_open_error_naming_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("gone.png");
    let err = copy_unique_into(&src, dir.path()).unwrap_err();
    assert_eq!(err.op(), CopyOp::Open);
    assert_eq!(err.path(), src);
    assert_eq!(err.io_error().kind(), std::io::ErrorKind::NotFound);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_missing_destination_folder_is_an_open_error_naming_the_destination() {
    let dir = tempfile::tempdir().unwrap();
    let src = write(&dir.path().join("a.png"), b"x");
    let dst = dir.path().join("nope").join("a.png");
    let err = copy_unique(&src, &dst).unwrap_err();
    assert_eq!(err.op(), CopyOp::Open);
    assert_eq!(err.path(), dst);
    let io: std::io::Error = err.into();
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
}
