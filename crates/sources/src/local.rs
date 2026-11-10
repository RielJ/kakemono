//! Local image listing for the `folder`, `album-name` and `album-date` sources: [`list_images`]
//! and [`album_next`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Most image paths [`list_images`] returns; the walk stops once it has this many.
const MAX_LISTED: usize = 50_000;

/// Image files (`jpg`, `jpeg`, `png`, `webp`, any case) under `dir`, or only its direct children
/// when not `recursive`. Used by the `folder` and `album-*` sources.
///
/// - Hidden entries (name starts with `.`) are skipped, files and directories alike. That covers
///   the downloader's `.kakemono-seen` ledger. `dir` itself may be hidden.
/// - The walk is iterative and stops at [`MAX_LISTED`] results.
/// - Symlinks are followed, to files and to directories, but each directory is entered at most
///   once (by canonical path), so a link loop ends. Dangling links are skipped.
/// - A missing, unreadable or non-directory `dir` gives an empty list; unreadable entries are
///   skipped.
///
/// Paths are `dir` joined with the entry names (not canonicalised). Order is a depth-first walk:
/// a directory's files by name, then its subdirectories by name.
pub fn list_images(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    list_images_capped(dir, recursive, MAX_LISTED)
}

fn list_images_capped(dir: &Path, recursive: bool, cap: usize) -> Vec<PathBuf> {
    if !std::fs::metadata(dir).is_ok_and(|m| m.is_dir()) {
        return Vec::new();
    }
    let mut visited: HashSet<PathBuf> = HashSet::new();
    if let Ok(canonical) = std::fs::canonicalize(dir) {
        visited.insert(canonical);
    }
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        let mut entries: Vec<std::fs::DirEntry> = read.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let mut subdirs = Vec::new();
        for entry in entries {
            if entry.file_name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // Resolve symlinks; a dangling one fails here and is skipped.
            let (is_dir, is_file) = if file_type.is_symlink() {
                match std::fs::metadata(&path) {
                    Ok(meta) => (meta.is_dir(), meta.is_file()),
                    Err(_) => continue,
                }
            } else {
                (file_type.is_dir(), file_type.is_file())
            };
            if is_dir {
                if recursive && std::fs::canonicalize(&path).is_ok_and(|c| visited.insert(c)) {
                    subdirs.push(path);
                }
            } else if is_file && is_listed_image(&path) {
                out.push(path);
                if out.len() >= cap {
                    return out;
                }
            }
        }
        // Reversed so the stack pops them in name order.
        stack.extend(subdirs.into_iter().rev());
    }
    out
}

/// The extensions sources pick from: `jpg`, `jpeg`, `png`, `webp`, any case. Narrower than
/// [`crate::is_image_file`], which accepts more formats for downloads.
fn is_listed_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["jpg", "jpeg", "png", "webp"]
            .iter()
            .any(|known| e.eq_ignore_ascii_case(known))
    })
}

/// Order of an `album-*` source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumOrder {
    /// By file name.
    Name,
    /// By modification time, oldest first; ties broken by file name (the same as by path,
    /// since an album is one folder).
    Date,
}

/// The album image that comes after `after` in `dir`, in `order`. Albums are the direct
/// children of `dir` ([`list_images`] with `recursive = false`).
///
/// - `None` gives the first image.
/// - After the last image it wraps to the first.
/// - When `after` is not in the folder, the first image that sorts after it is returned (by its
///   file name for [`AlbumOrder::Name`]; by its mtime and name for [`AlbumOrder::Date`]), or the
///   first image when none does. For `Date`, an `after` that cannot be stat'ed gives the first
///   image.
/// - No images (or a missing `dir`) gives `None`.
///
/// `after` is matched by file name, so a relative or absolute form of the same path both work.
pub fn album_next(dir: &Path, after: Option<&Path>, order: AlbumOrder) -> Option<PathBuf> {
    let files = list_images(dir, false);
    match order {
        AlbumOrder::Name => {
            let keyed = files
                .into_iter()
                .filter_map(|p| Some((p.file_name()?.to_owned(), p)))
                .collect();
            let after_key = after.and_then(Path::file_name).map(ToOwned::to_owned);
            next_by_key(keyed, after, after_key)
        }
        AlbumOrder::Date => {
            let keyed = files
                .into_iter()
                .filter_map(|p| {
                    let name = p.file_name()?.to_owned();
                    Some(((mtime(&p), name), p))
                })
                .collect();
            let after_key = after.and_then(|a| {
                let modified = std::fs::metadata(a).and_then(|m| m.modified()).ok()?;
                Some((modified, a.file_name()?.to_owned()))
            });
            next_by_key(keyed, after, after_key)
        }
    }
}

/// The first item whose key sorts after `after_key`, wrapping to the first item. A missing key
/// for a given `after` also gives the first item.
fn next_by_key<K: Ord>(
    mut items: Vec<(K, PathBuf)>,
    after: Option<&Path>,
    after_key: Option<K>,
) -> Option<PathBuf> {
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let start = match (after, after_key) {
        (Some(_), Some(key)) => items.partition_point(|(k, _)| *k <= key),
        _ => 0,
    };
    let index = if start < items.len() { start } else { 0 };
    items.into_iter().nth(index).map(|(_, p)| p)
}

/// A file's mtime, or the epoch when it cannot be read (so it sorts first, deterministically).
fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, SystemTime};

    use super::*;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"x").unwrap();
    }

    fn set_mtime(path: &Path, secs: u64) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    /// Paths relative to `root`, with `/` separators, sorted.
    fn rel(root: &Path, paths: &[PathBuf]) -> Vec<String> {
        let mut out: Vec<String> = paths
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        out.sort();
        out
    }

    fn tree() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for f in [
            "a.jpg",
            "B.PNG",
            "c.WebP",
            "d.JPEG",
            "anim.gif",
            "notes.txt",
            "noext",
            ".hidden.jpg",
            ".kakemono-seen",
            "sub/e.jpeg",
            "sub/deeper/f.png",
            "sub/readme.md",
            ".git/g.jpg",
            ".cache/deep/h.png",
        ] {
            touch(&tmp.path().join(f));
        }
        tmp
    }

    #[test]
    fn local_list_images_recursive_finds_nested_and_skips_hidden() {
        let tmp = tree();
        let got = list_images(tmp.path(), true);
        assert_eq!(
            rel(tmp.path(), &got),
            [
                "B.PNG",
                "a.jpg",
                "c.WebP",
                "d.JPEG",
                "sub/deeper/f.png",
                "sub/e.jpeg"
            ]
        );
    }

    #[test]
    fn local_list_images_flat_only_direct_children() {
        let tmp = tree();
        let got = list_images(tmp.path(), false);
        assert_eq!(
            rel(tmp.path(), &got),
            ["B.PNG", "a.jpg", "c.WebP", "d.JPEG"]
        );
    }

    #[test]
    fn local_list_images_missing_dir_or_file_is_empty() {
        let tmp = tree();
        assert!(list_images(&tmp.path().join("nope"), true).is_empty());
        assert!(list_images(&tmp.path().join("a.jpg"), true).is_empty());
    }

    #[test]
    fn local_list_images_hidden_root_is_still_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".walls");
        touch(&root.join("x.jpg"));
        assert_eq!(rel(&root, &list_images(&root, true)), ["x.jpg"]);
    }

    #[test]
    fn local_list_images_cap_stops_the_walk() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..10 {
            touch(&tmp.path().join(format!("sub{i}/img{i}.jpg")));
        }
        assert_eq!(list_images_capped(tmp.path(), true, 3).len(), 3);
        assert_eq!(list_images_capped(tmp.path(), true, 100).len(), 10);
        assert_eq!(MAX_LISTED, 50_000);
    }

    #[cfg(unix)]
    #[test]
    fn local_list_images_symlink_loop_terminates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("a/x.jpg"));
        // a/loop -> root: following it naively would recurse forever.
        std::os::unix::fs::symlink(root, root.join("a/loop")).unwrap();
        let got = list_images(root, true);
        assert_eq!(
            got.iter().filter(|p| p.ends_with("x.jpg")).count(),
            1,
            "{got:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_list_images_keeps_symlinked_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        touch(&tmp.path().join("outside/real.png"));
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("outside/real.png"), root.join("link.png"))
            .unwrap();
        std::os::unix::fs::symlink(
            tmp.path().join("outside/missing.png"),
            root.join("dead.png"),
        )
        .unwrap();
        assert_eq!(rel(&root, &list_images(&root, true)), ["link.png"]);
    }

    fn album() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        // Name order: a, b, c. Date order: c (10), a (20), b (20; tie broken by path, after a).
        for (name, secs) in [("b.jpg", 20), ("a.jpg", 20), ("c.png", 10)] {
            let p = tmp.path().join(name);
            touch(&p);
            set_mtime(&p, secs);
        }
        touch(&tmp.path().join("notes.txt"));
        touch(&tmp.path().join(".hidden.jpg"));
        tmp
    }

    fn name(p: Option<PathBuf>) -> Option<String> {
        p.map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
    }

    #[test]
    fn local_album_next_name_first_then_sequence_and_wrap() {
        let tmp = album();
        let d = tmp.path();
        let o = AlbumOrder::Name;
        assert_eq!(name(album_next(d, None, o)).as_deref(), Some("a.jpg"));
        assert_eq!(
            name(album_next(d, Some(&d.join("a.jpg")), o)).as_deref(),
            Some("b.jpg")
        );
        assert_eq!(
            name(album_next(d, Some(&d.join("b.jpg")), o)).as_deref(),
            Some("c.png")
        );
        assert_eq!(
            name(album_next(d, Some(&d.join("c.png")), o)).as_deref(),
            Some("a.jpg")
        );
    }

    #[test]
    fn local_album_next_name_missing_after_takes_next_in_order() {
        let tmp = album();
        let d = tmp.path();
        let o = AlbumOrder::Name;
        assert_eq!(
            name(album_next(d, Some(&d.join("abc.jpg")), o)).as_deref(),
            Some("b.jpg")
        );
        assert_eq!(
            name(album_next(d, Some(&d.join("zzz.jpg")), o)).as_deref(),
            Some("a.jpg")
        );
    }

    #[test]
    fn local_album_next_date_orders_by_mtime_ties_by_path() {
        let tmp = album();
        let d = tmp.path();
        let o = AlbumOrder::Date;
        assert_eq!(name(album_next(d, None, o)).as_deref(), Some("c.png"));
        assert_eq!(
            name(album_next(d, Some(&d.join("c.png")), o)).as_deref(),
            Some("a.jpg")
        );
        assert_eq!(
            name(album_next(d, Some(&d.join("a.jpg")), o)).as_deref(),
            Some("b.jpg")
        );
        assert_eq!(
            name(album_next(d, Some(&d.join("b.jpg")), o)).as_deref(),
            Some("c.png")
        );
    }

    #[test]
    fn local_album_next_date_missing_after_gives_first() {
        let tmp = album();
        let d = tmp.path();
        let got = album_next(d, Some(&d.join("gone.jpg")), AlbumOrder::Date);
        assert_eq!(name(got).as_deref(), Some("c.png"));
    }

    #[test]
    fn local_album_next_empty_or_missing_dir_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("notes.txt"));
        assert_eq!(album_next(tmp.path(), None, AlbumOrder::Name), None);
        assert_eq!(
            album_next(&tmp.path().join("nope"), None, AlbumOrder::Date),
            None
        );
    }

    #[test]
    fn local_album_next_single_file_wraps_to_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let only = tmp.path().join("only.jpg");
        touch(&only);
        assert_eq!(
            album_next(tmp.path(), Some(&only), AlbumOrder::Name),
            Some(only)
        );
    }
}
