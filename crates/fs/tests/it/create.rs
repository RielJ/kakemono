//! `create_new`.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Barrier};

use crate::within_deadline;
use kakemono_fs::create_new;

const WRITERS: usize = 8;

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn creates_a_missing_file_and_leaves_no_temp() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    assert!(create_new(&path, b"interval = \"30m\"\n").unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"interval = \"30m\"\n");
    assert_eq!(names_in(dir.path()), ["config.toml"]);
}

#[test]
fn never_replaces_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, b"mine").unwrap();
    assert!(!create_new(&path, b"theirs").unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"mine");
    assert_eq!(names_in(dir.path()), ["config.toml"]);
}

/// A symlink at the path counts as existing, even when it dangles: nothing is written through it.
#[cfg(unix)]
#[test]
fn a_symlink_at_the_path_counts_as_existing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::os::unix::fs::symlink(dir.path().join("missing"), &path).unwrap();
    assert!(!create_new(&path, b"theirs").unwrap());
    assert!(!dir.path().join("missing").exists());
    assert_eq!(names_in(dir.path()), ["config.toml"]);
}

#[cfg(unix)]
#[test]
fn a_new_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    assert!(create_new(&path, b"key = 1\n").unwrap());
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}

#[test]
fn a_missing_folder_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = create_new(&dir.path().join("no/such/config.toml"), b"x").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
    assert!(names_in(dir.path()).is_empty());
}

/// Racing writers on one path: exactly one wins, the file holds that writer's complete bytes,
/// and no temp file is left behind by winners or losers.
#[test]
fn concurrent_creates_have_one_winner() {
    within_deadline(|| {
        for round in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let path = Arc::new(dir.path().join("config.toml"));
            let start = Arc::new(Barrier::new(WRITERS));
            let writers: Vec<_> = (0..WRITERS)
                .map(|i| {
                    let path = Arc::clone(&path);
                    let start = Arc::clone(&start);
                    std::thread::spawn(move || {
                        let bytes = format!("writer {i}\n").repeat(4096).into_bytes();
                        start.wait();
                        (create_new(&path, &bytes).unwrap(), bytes)
                    })
                })
                .collect();
            let results: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
            let winners: Vec<_> = results.iter().filter(|(won, _)| *won).collect();
            assert_eq!(winners.len(), 1, "round {round}");
            assert_eq!(fs::read(&*path).unwrap(), winners[0].1, "round {round}");
            assert_eq!(names_in(dir.path()), ["config.toml"], "round {round}");
        }
    });
}
