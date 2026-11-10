//! `atomic_write`, `PathLock` and `update`.

use std::fs;
use std::path::Path;
use std::sync::Barrier;

use crate::within_deadline;
use kakemono_fs::{PathLock, UpdateError, atomic_write, update};

const ROUNDS: usize = 50;

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Replaces the `key=value` line for `key` in a tiny line-based format, or appends one.
fn set_line(old: Option<&[u8]>, key: &str, value: usize) -> Vec<u8> {
    let text = old
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .unwrap_or_default();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| !l.starts_with(&format!("{key}=")))
        .map(str::to_owned)
        .collect();
    lines.push(format!("{key}={value}"));
    lines.sort();
    (lines.join("\n") + "\n").into_bytes()
}

fn get_line(path: &Path, key: &str) -> Option<usize> {
    fs::read_to_string(path).unwrap().lines().find_map(|l| {
        l.strip_prefix(&format!("{key}="))
            .map(|v| v.parse().unwrap())
    })
}

#[test]
fn atomic_write_creates_and_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    atomic_write(&path, b"one").unwrap();
    atomic_write(&path, b"two").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"two");
    assert_eq!(names_in(dir.path()), ["a.txt"]);
}

#[test]
fn atomic_write_needs_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let err = atomic_write(&dir.path().join("missing/a.txt"), b"x").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn concurrent_writes_use_unique_temp_files() {
    within_deadline(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let barrier = Barrier::new(4);

        std::thread::scope(|s| {
            for writer in 0..4 {
                let (path, barrier) = (&path, &barrier);
                s.spawn(move || {
                    barrier.wait();
                    for round in 0..ROUNDS {
                        let body = format!("writer {writer} round {round}\n").repeat(64);
                        atomic_write(path, body.as_bytes()).unwrap();
                    }
                });
            }
        });

        let text = fs::read_to_string(&path).unwrap();
        let first = text.lines().next().unwrap();
        assert_eq!(text, format!("{first}\n").repeat(64), "torn write");
        assert_eq!(
            names_in(dir.path()),
            ["state.json"],
            "temp files left behind"
        );
    });
}

#[cfg(unix)]
#[test]
fn new_files_are_private_and_old_permissions_are_kept() {
    use std::os::unix::fs::PermissionsExt;

    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    atomic_write(&path, b"one").unwrap();
    assert_eq!(mode(&path), 0o600);

    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    atomic_write(&path, b"two").unwrap();
    assert_eq!(mode(&path), 0o644);
}

#[cfg(windows)]
#[test]
fn read_only_flag_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    atomic_write(&path, b"one").unwrap();
    assert!(!fs::metadata(&path).unwrap().permissions().readonly());
}

/// Windows refuses to replace a file that a handle without `FILE_SHARE_DELETE` has open (a virus
/// scanner, an indexer, another writer's rename). The write waits for that handle to close
/// instead of failing with "access denied".
#[cfg(windows)]
#[test]
fn a_briefly_locked_target_is_replaced_once_released() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::Duration;

    const FILE_SHARE_READ: u32 = 1;

    within_deadline(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "old").unwrap();
        let holder = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();

        std::thread::scope(|s| {
            s.spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                drop(holder);
            });
            atomic_write(&path, b"new").unwrap();
        });

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(names_in(dir.path()), ["config.toml"]);
    });
}

#[cfg(unix)]
#[test]
fn a_symlink_stays_a_symlink() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let real_dir = dir.path().join("dotfiles");
    fs::create_dir(&real_dir).unwrap();
    let target = real_dir.join("config.toml");
    fs::write(&target, "old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    atomic_write(&link, b"new").unwrap();

    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(fs::read(&target).unwrap(), b"new");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(names_in(&real_dir), ["config.toml"]);
    assert_eq!(names_in(dir.path()), ["config.toml", "dotfiles"]);
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_creates_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("real.toml");
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink("real.toml", &link).unwrap();

    atomic_write(&link, b"new").unwrap();

    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(fs::read(&target).unwrap(), b"new");
}

#[test]
fn concurrent_updates_of_different_keys_both_survive() {
    within_deadline(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kv.txt");
        let barrier = Barrier::new(2);

        std::thread::scope(|s| {
            for key in ["left", "right"] {
                let (path, barrier) = (&path, &barrier);
                s.spawn(move || {
                    barrier.wait();
                    for round in 1..=ROUNDS {
                        update(path, |old| {
                            Ok::<_, std::convert::Infallible>(set_line(old, key, round))
                        })
                        .unwrap();
                    }
                });
            }
        });

        assert_eq!(get_line(&path, "left"), Some(ROUNDS));
        assert_eq!(get_line(&path, "right"), Some(ROUNDS));
    });
}

#[cfg(unix)]
#[test]
fn updates_through_a_link_and_its_target_share_one_lock() {
    within_deadline(|| {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("count.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "n=0\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let barrier = Barrier::new(2);

        std::thread::scope(|s| {
            for path in [&target, &link] {
                let barrier = &barrier;
                s.spawn(move || {
                    barrier.wait();
                    for _ in 0..ROUNDS {
                        update(path, |old| {
                            let n = get_count(old.unwrap());
                            Ok::<_, std::convert::Infallible>(format!("n={}\n", n + 1).into_bytes())
                        })
                        .unwrap();
                    }
                });
            }
        });

        assert_eq!(get_line(&target, "n"), Some(2 * ROUNDS));
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    });
}

#[cfg(unix)]
fn get_count(bytes: &[u8]) -> usize {
    std::str::from_utf8(bytes)
        .unwrap()
        .trim()
        .strip_prefix("n=")
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn update_passes_none_for_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.txt");
    update(&path, |old| {
        assert!(old.is_none());
        Ok::<_, std::convert::Infallible>(b"made".to_vec())
    })
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"made");
}

#[test]
fn a_rejected_update_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keep.txt");
    fs::write(&path, "original").unwrap();

    let err = update(&path, |_| Err("no")).unwrap_err();

    assert!(matches!(err, UpdateError::Rejected("no")), "{err:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    assert_eq!(names_in(dir.path()), ["keep.txt"]);
}

#[test]
fn a_held_lock_blocks_update_until_dropped() {
    within_deadline(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.txt");
        fs::write(&path, "before").unwrap();

        let lock = PathLock::acquire(&path).unwrap();
        std::thread::scope(|s| {
            let writer = s.spawn(|| {
                update(&path, |_| {
                    Ok::<_, std::convert::Infallible>(b"after".to_vec())
                })
                .unwrap();
            });
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(!writer.is_finished(), "update ran while the lock was held");
            assert_eq!(fs::read_to_string(lock.path()).unwrap(), "before");
            drop(lock);
            writer.join().unwrap();
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), "after");
    });
}
