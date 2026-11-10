//! The `thumb://` protocol: picker thumbnails behind an id → path allowlist (plan §H, Task 44).
//!
//! The picker never sends a path. `thumbnails` admits the listed files with [`Thumbs::admit`],
//! which gives each one an opaque id (a per-process keyed hash of its real path). A request
//! carries only that id. The allowlist is the only way a file is served:
//!
//! - an id that isn't 16 lowercase hex chars, or isn't in the current list → 403;
//! - a path is admitted only if its real path (symlinks resolved) is inside one of the roots
//!   (`[daemon] download_dir`, `[favorites] dir`, `[local] dirs`) and has an image extension;
//! - at serve time the path is resolved again and must still be the admitted real path inside
//!   a root, so a file swapped for a symlink after listing is refused (403).
//!
//! This replaces Go's `/local/<any-path>` handler, which served any image file on disk.
//!
//! The same listing gates the path-taking picker commands: [`Thumbs::is_admitted`] is true only
//! for an entry of the last listing.
//!
//! Thumbnails are JPEGs whose short edge is [`THUMB_EDGE`] px, cached in
//! `<platform cache dir>/thumbs/<content hash>.jpg`, never next to the wallpapers. An id also
//! hashes the file's size and mtime, so a file replaced in place gets a new URL and the webview
//! never shows its old thumbnail. At most [`DECODE_LIMIT`] images are decoded at once, and
//! [`Thumbs::prune_cache`] deletes cache files older than [`CACHE_MAX_AGE`].

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime};

use image::{DynamicImage, ImageDecoder, ImageReader};
use tauri::http::{Response, StatusCode, header};

/// URI scheme name.
pub const SCHEME: &str = "thumb";
/// Short edge of a thumbnail, px. The strip is 160 logical px tall, so 384 stays sharp at 2x.
pub const THUMB_EDGE: u32 = 384;
/// JPEG quality of cached thumbnails.
pub const JPEG_QUALITY: u8 = 82;
/// Extensions that may be admitted (same set the listing uses).
pub const IMAGE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
/// Images decoded at once. Each decode holds a full wallpaper in memory.
pub const DECODE_LIMIT: usize = 4;
/// Cache files not written for this long are deleted by [`Thumbs::prune_cache`].
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// Bumped when the thumbnail recipe changes, so old cache files are not reused.
const RECIPE: &str = "v1-384-q82";

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// Malformed id, unknown id, or a path outside the roots: 403.
    Forbidden,
    /// Admitted, but the file is gone: 404.
    NotFound,
}

#[derive(Debug, Default)]
struct Allow {
    /// Real (canonical) paths of the roots.
    roots: Vec<PathBuf>,
    /// id → admitted real path.
    ids: HashMap<String, PathBuf>,
    /// Real paths of every entry in the last listing, served or not ([`Thumbs::is_admitted`]).
    listed: HashSet<PathBuf>,
    /// (real path, len, mtime) → content key, so a cache hit doesn't re-read the image. Holds
    /// only files of the last listing.
    keys: HashMap<Stamp, String>,
}

/// (real path, len, mtime): a file and the version of its content.
type Stamp = (PathBuf, u64, Option<SystemTime>);

fn stamp(real: &Path, meta: &std::fs::Metadata) -> Stamp {
    (real.to_owned(), meta.len(), meta.modified().ok())
}

/// A counting semaphore for blocking threads.
#[derive(Debug)]
struct Gate {
    free: Mutex<usize>,
    freed: Condvar,
}

/// Held while decoding; gives the slot back on drop.
struct Permit<'a>(&'a Gate);

impl Gate {
    fn new(slots: usize) -> Self {
        Self {
            free: Mutex::new(slots),
            freed: Condvar::new(),
        }
    }

    fn enter(&self) -> Permit<'_> {
        let mut free = self
            .free
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *free == 0 {
            free = self
                .freed
                .wait(free)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *free -= 1;
        Permit(self)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut free = self
            .0
            .free
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *free += 1;
        self.0.freed.notify_one();
    }
}

/// The allowlist and the cache dir. Managed as `Arc<Thumbs>`.
pub struct Thumbs {
    salt: RandomState,
    cache_dir: PathBuf,
    allow: Mutex<Allow>,
    decode: Gate,
}

impl Thumbs {
    /// `cache_dir` is where thumbnails are written (created on first write).
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            salt: RandomState::new(),
            cache_dir,
            allow: Mutex::new(Allow::default()),
            decode: Gate::new(DECODE_LIMIT),
        }
    }

    /// `<platform cache dir>/thumbs` (`kakemono_config::paths::cache_dir`). Falls back to a
    /// temp dir (logged) when the platform dirs can't be resolved.
    pub fn detect() -> Self {
        let dir = match kakemono_config::paths::cache_dir() {
            Ok(dir) => dir.join("thumbs"),
            Err(err) => {
                let dir = std::env::temp_dir().join("kakemono-thumbs");
                tracing::warn!(error = %err, fallback = %dir.display(), "no cache dir for thumbnails");
                dir
            }
        };
        Self::new(dir)
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    fn with<T>(&self, f: impl FnOnce(&mut Allow) -> T) -> T {
        let mut guard = self
            .allow
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }

    fn id_for(&self, (real, len, mtime): &Stamp) -> String {
        let mut hasher = self.salt.build_hasher();
        hasher.write(real.as_os_str().as_encoded_bytes());
        hasher.write_u64(*len);
        if let Some(since) = mtime.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()) {
            hasher.write_u128(since.as_nanos());
        }
        format!("{:016x}", hasher.finish())
    }

    /// Replaces the allowlist with `paths` that resolve inside `roots`. Returns one image URL
    /// per input path, `""` for a path that was not admitted. Every path that exists becomes a
    /// picker entry for [`is_admitted`](Self::is_admitted), served or not.
    pub fn admit<P: AsRef<Path>>(&self, roots: &[P], paths: &[&str]) -> Vec<String> {
        let roots: Vec<PathBuf> = roots
            .iter()
            .filter_map(|r| std::fs::canonicalize(r.as_ref()).ok())
            .collect();
        let listed: HashSet<PathBuf> = paths
            .iter()
            .filter_map(|path| std::fs::canonicalize(path).ok())
            .collect();
        let mut ids = HashMap::new();
        let urls = paths
            .iter()
            .map(|path| match admissible(&roots, Path::new(path)) {
                Some((real, meta)) => {
                    let id = self.id_for(&stamp(&real, &meta));
                    ids.insert(id.clone(), real);
                    url_for(&id)
                }
                None => String::new(),
            })
            .collect();
        self.with(|allow| {
            allow.roots = roots;
            allow.ids = ids;
            allow.keys.retain(|(real, ..), _| listed.contains(real));
            allow.listed = listed;
        });
        urls
    }

    /// True when `path` (symlinks resolved) is an entry of the last listing. The picker only
    /// ever sends such paths, so `favorite`, `set_file` and `reveal` refuse anything else.
    pub fn is_admitted(&self, path: &Path) -> bool {
        let Ok(real) = std::fs::canonicalize(path) else {
            return false;
        };
        self.with(|allow| allow.listed.contains(&real))
    }

    /// Maps a request path (`/<id>`) to the admitted file, re-checking it on disk.
    pub fn resolve(&self, uri_path: &str) -> Result<PathBuf, Denied> {
        let id = uri_path.strip_prefix('/').unwrap_or(uri_path);
        if !is_id(id) {
            return Err(Denied::Forbidden);
        }
        let (roots, admitted) = self.with(|allow| {
            (
                allow.roots.clone(),
                allow.ids.get(id).cloned().ok_or(Denied::Forbidden),
            )
        });
        let admitted = admitted?;
        let real = match std::fs::canonicalize(&admitted) {
            Ok(real) => real,
            Err(_) if !admitted.exists() => return Err(Denied::NotFound),
            Err(_) => return Err(Denied::Forbidden),
        };
        // A file replaced by a symlink after listing resolves somewhere else.
        if real != admitted || !inside(&roots, &real) {
            return Err(Denied::Forbidden);
        }
        Ok(real)
    }

    /// Serves `uri_path`: 200 with a JPEG, or 403 / 404 / 415 / 500 with an empty body.
    pub fn respond(&self, uri_path: &str) -> Response<Vec<u8>> {
        let path = match self.resolve(uri_path) {
            Ok(path) => path,
            Err(Denied::Forbidden) => return status(StatusCode::FORBIDDEN),
            Err(Denied::NotFound) => return status(StatusCode::NOT_FOUND),
        };
        match self.thumbnail(&path) {
            Ok(bytes) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/jpeg")
                .header(header::CACHE_CONTROL, "private, max-age=3600")
                .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                .body(bytes)
                .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR)),
            Err(ThumbError::Decode(err)) => {
                tracing::debug!(path = %path.display(), error = %err, "cannot decode image");
                status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
            }
            Err(ThumbError::Io(err)) => {
                tracing::warn!(path = %path.display(), error = %err, "cannot make thumbnail");
                status(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }

    /// The cached thumbnail of `path`, generated on a miss.
    /// `path` is opened once, checked to still be the file at `path`, and read from that handle.
    pub fn thumbnail(&self, path: &Path) -> Result<Vec<u8>, ThumbError> {
        let mut file = open_checked(path)?;
        let memo = stamp(path, &file.metadata()?);
        if let Some(key) = self.with(|allow| allow.keys.get(&memo).cloned())
            && let Ok(bytes) = std::fs::read(self.cache_dir.join(format!("{key}.jpg")))
        {
            return Ok(bytes);
        }

        let mut source = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut source)?;
        let key = content_key(&source);
        self.with(|allow| {
            if allow.listed.contains(path) {
                allow.keys.insert(memo, key.clone());
            }
        });
        let cached = self.cache_dir.join(format!("{key}.jpg"));
        if let Ok(bytes) = std::fs::read(&cached) {
            return Ok(bytes);
        }

        let jpeg = {
            let _permit = self.decode.enter();
            make_thumbnail(&source)?
        };
        std::fs::create_dir_all(&self.cache_dir)?;
        // Write then rename, so a concurrent reader never sees half a file.
        let tmp = self
            .cache_dir
            .join(format!("{key}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, &jpeg)?;
        if let Err(err) = std::fs::rename(&tmp, &cached) {
            let _ = std::fs::remove_file(&tmp);
            tracing::debug!(error = %err, "cannot cache thumbnail");
        }
        Ok(jpeg)
    }
}

impl Thumbs {
    /// Deletes cached thumbnails (and leftover `.tmp` files) last written more than `max_age`
    /// ago. Returns how many were deleted. A missing cache dir deletes nothing.
    pub fn prune_cache(&self, max_age: Duration) -> std::io::Result<usize> {
        let entries = match std::fs::read_dir(&self.cache_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(err),
        };
        let now = SystemTime::now();
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let ours = matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("jpg" | "tmp")
            );
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|written| now.duration_since(written).unwrap_or_default() > max_age);
            if ours && old && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Opens `path` and checks that the handle is the file at `path` itself (Unix: same device
/// and inode as `symlink_metadata`). [`Thumbs::resolve`] checked the path; this stops a
/// component swapped for a symlink after that check from being read through the handle.
fn open_checked(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::File::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        let at_path = std::fs::symlink_metadata(path)?;
        if (opened.dev(), opened.ino()) != (at_path.dev(), at_path.ino()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the file changed after it was checked",
            ));
        }
    }
    Ok(file)
}

/// A thumbnail could not be made.
#[derive(Debug, thiserror::Error)]
pub enum ThumbError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Decode(#[from] image::ImageError),
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = code;
    response
}

/// `thumb://localhost/<id>` (macOS, Linux) or `http://thumb.localhost/<id>` (Windows, where
/// WebView2 serves custom schemes over http).
pub fn url_for(id: &str) -> String {
    if cfg!(windows) {
        format!("http://{SCHEME}.localhost/{id}")
    } else {
        format!("{SCHEME}://localhost/{id}")
    }
}

/// 16 lowercase hex chars, nothing else (no `/`, `.`, `%`).
pub fn is_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn inside(roots: &[PathBuf], real: &Path) -> bool {
    roots.iter().any(|root| real.starts_with(root))
}

/// The real path and metadata of `path` if it is an image file inside one of `roots`
/// (already canonical).
fn admissible(roots: &[PathBuf], path: &Path) -> Option<(PathBuf, std::fs::Metadata)> {
    let real = std::fs::canonicalize(path).ok()?;
    let ext = real.extension()?.to_str()?.to_ascii_lowercase();
    let meta = std::fs::metadata(&real).ok()?;
    (IMAGE_EXTS.contains(&ext.as_str()) && meta.is_file() && inside(roots, &real))
        .then_some((real, meta))
}

/// Stable content key: 128-bit FNV-1a of the bytes, plus the recipe version. Stable across
/// runs and toolchains (unlike `DefaultHasher`).
pub fn content_key(bytes: &[u8]) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    for byte in RECIPE.as_bytes().iter().chain(bytes) {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:032x}")
}

/// Target size: short edge [`THUMB_EDGE`], aspect kept, never upscaled.
pub fn thumb_size(width: u32, height: u32) -> (u32, u32) {
    let short = width.min(height);
    if short <= THUMB_EDGE || short == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = |n: u32| -> u32 {
        let scaled = u64::from(n) * u64::from(THUMB_EDGE) / u64::from(short);
        u32::try_from(scaled).unwrap_or(u32::MAX).max(1)
    };
    (scale(width), scale(height))
}

/// Decodes `source` (EXIF orientation applied) and encodes the thumbnail as JPEG.
pub fn make_thumbnail(source: &[u8]) -> Result<Vec<u8>, ThumbError> {
    let mut decoder = ImageReader::new(Cursor::new(source))
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let (w, h) = thumb_size(img.width(), img.height());
    let thumb = if (w, h) == (img.width(), img.height()) {
        img
    } else {
        img.resize_exact(w, h, image::imageops::FilterType::Triangle)
    };
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    thumb.to_rgb8().write_with_encoder(encoder)?;
    Ok(out)
}

/// Registers the `thumb` scheme. Work runs on the blocking pool.
pub fn register<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    use tauri::Manager;
    builder.register_asynchronous_uri_scheme_protocol(SCHEME, |ctx, request, responder| {
        let app = ctx.app_handle().clone();
        let path = request.uri().path().to_owned();
        tauri::async_runtime::spawn_blocking(move || {
            let thumbs = app.state::<std::sync::Arc<Thumbs>>();
            responder.respond(thumbs.respond(&path));
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let path = dir.join(name);
        image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 30]))
            .save(&path)
            .unwrap();
        path
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        outside: PathBuf,
        thumbs: Thumbs,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("favorites");
        let outside = tmp.path().join("secret");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let thumbs = Thumbs::new(tmp.path().join("cache/thumbs"));
        Fixture {
            _tmp: tmp,
            root,
            outside,
            thumbs,
        }
    }

    fn id_of(url: &str) -> &str {
        url.rsplit('/').next().unwrap()
    }

    #[test]
    fn thumbs_admitted_file_is_served_as_a_small_jpeg() {
        let f = fixture();
        let pic = png(&f.root, "a.png", 1600, 900);
        let urls = f.thumbs.admit(&[&f.root], &[pic.to_str().unwrap()]);
        assert!(urls[0].contains(SCHEME), "{urls:?}");
        let res = f.thumbs.respond(&format!("/{}", id_of(&urls[0])));
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "image/jpeg");
        let img = image::load_from_memory(res.body()).unwrap();
        assert_eq!((img.width(), img.height()), (682, 384));
        // Cached under the cache dir, not next to the wallpaper.
        let cached: Vec<_> = std::fs::read_dir(f.thumbs.cache_dir()).unwrap().collect();
        assert_eq!(cached.len(), 1);
        assert_eq!(std::fs::read_dir(&f.root).unwrap().count(), 1);
    }

    #[test]
    fn thumbs_path_outside_the_roots_is_403() {
        let f = fixture();
        let secret = png(&f.outside, "secret.png", 10, 10);
        let urls = f.thumbs.admit(&[&f.root], &[secret.to_str().unwrap()]);
        assert_eq!(urls, vec![String::new()], "not admitted");
        // Even its would-be id is unknown.
        let real = std::fs::canonicalize(&secret).unwrap();
        let id = f
            .thumbs
            .id_for(&stamp(&real, &std::fs::metadata(&real).unwrap()));
        assert_eq!(f.thumbs.respond(&format!("/{id}")).status(), 403);
        // A raw path in the request is never a valid id.
        let raw = format!("/{}", secret.display());
        assert_eq!(f.thumbs.respond(&raw).status(), 403);
    }

    #[test]
    fn thumbs_dot_dot_is_403() {
        let f = fixture();
        let secret = png(&f.outside, "secret.png", 10, 10);
        // `..` in a listed path resolves outside the root: not admitted.
        let sneaky = f.root.join("../secret/secret.png");
        assert!(sneaky.exists());
        let urls = f.thumbs.admit(&[&f.root], &[sneaky.to_str().unwrap()]);
        assert_eq!(urls, vec![String::new()]);
        // `..` in the request itself, raw and percent-encoded.
        for uri in [
            "/../secret/secret.png",
            "/..%2F..%2Fsecret%2Fsecret.png",
            "/%2e%2e/secret.png",
            "/0123456789abcdef/../x",
        ] {
            assert_eq!(f.thumbs.respond(uri).status(), 403, "{uri}");
        }
        assert!(secret.exists());
    }

    #[cfg(unix)]
    #[test]
    fn thumbs_symlink_escaping_the_root_is_403() {
        let f = fixture();
        let secret = png(&f.outside, "secret.png", 10, 10);
        let link = f.root.join("link.png");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let urls = f.thumbs.admit(&[&f.root], &[link.to_str().unwrap()]);
        assert_eq!(urls, vec![String::new()], "symlink out of the root");

        // Admitted while a real file, then swapped for a symlink before the request.
        let pic = png(&f.root, "swap.png", 10, 10);
        let urls = f.thumbs.admit(&[&f.root], &[pic.to_str().unwrap()]);
        let uri = format!("/{}", id_of(&urls[0]));
        std::fs::remove_file(&pic).unwrap();
        std::os::unix::fs::symlink(&secret, &pic).unwrap();
        assert_eq!(f.thumbs.respond(&uri).status(), 403);
    }

    #[cfg(unix)]
    #[test]
    fn thumbs_symlink_inside_the_root_is_allowed() {
        let f = fixture();
        let pic = png(&f.root, "real.png", 10, 10);
        let link = f.root.join("alias.png");
        std::os::unix::fs::symlink(&pic, &link).unwrap();
        let urls = f.thumbs.admit(&[&f.root], &[link.to_str().unwrap()]);
        assert_eq!(
            f.thumbs.respond(&format!("/{}", id_of(&urls[0]))).status(),
            200
        );
    }

    /// A symlink in the download dir pointing at `~/.ssh` is not admitted: admission
    /// canonicalises both the path and the roots.
    #[cfg(unix)]
    #[test]
    fn thumbs_symlink_from_download_dir_to_ssh_is_not_admitted() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let downloads = home.join("Pictures/wallpapers/cache");
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&ssh).unwrap();
        let key = png(&ssh, "id_ed25519.png", 4, 4);
        let link = downloads.join("wallpaper.png");
        std::os::unix::fs::symlink(&key, &link).unwrap();
        // Also via a symlinked root: the root is canonicalised too.
        let alias = tmp.path().join("dl-alias");
        std::os::unix::fs::symlink(&downloads, &alias).unwrap();

        let thumbs = Thumbs::new(tmp.path().join("cache/thumbs"));
        let via_alias = alias.join("wallpaper.png");
        let urls = thumbs.admit(
            &[&downloads, &alias],
            &[link.to_str().unwrap(), via_alias.to_str().unwrap()],
        );
        assert_eq!(urls, vec![String::new(), String::new()]);
        let real = std::fs::canonicalize(&key).unwrap();
        let id = thumbs.id_for(&stamp(&real, &std::fs::metadata(&real).unwrap()));
        assert_eq!(thumbs.respond(&format!("/{id}")).status(), 403);
    }

    #[test]
    fn thumbs_relisting_replaces_the_allowlist() {
        let f = fixture();
        let a = png(&f.root, "a.png", 10, 10);
        let b = png(&f.root, "b.png", 10, 10);
        let first = f.thumbs.admit(&[&f.root], &[a.to_str().unwrap()]);
        f.thumbs.admit(&[&f.root], &[b.to_str().unwrap()]);
        assert_eq!(
            f.thumbs.respond(&format!("/{}", id_of(&first[0]))).status(),
            403
        );
    }

    #[test]
    fn thumbs_a_file_replaced_in_place_gets_a_new_url() {
        let f = fixture();
        let pic = png(&f.root, "a.png", 10, 10);
        let before = f.thumbs.admit(&[&f.root], &[pic.to_str().unwrap()]);
        let res = f.thumbs.respond(&format!("/{}", id_of(&before[0])));
        assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

        // Same path, new content: the webview must not reuse its cached response.
        png(&f.root, "a.png", 20, 20);
        let after = f.thumbs.admit(&[&f.root], &[pic.to_str().unwrap()]);
        assert_ne!(before, after);
        let res = f.thumbs.respond(&format!("/{}", id_of(&after[0])));
        let img = image::load_from_memory(res.body()).unwrap();
        assert_eq!((img.width(), img.height()), (20, 20));
    }

    #[test]
    fn thumbs_memo_keeps_only_listed_files() {
        let f = fixture();
        let a = png(&f.root, "a.png", 10, 10);
        let b = png(&f.root, "b.png", 10, 10);
        let urls = f.thumbs.admit(&[&f.root], &[a.to_str().unwrap()]);
        f.thumbs.respond(&format!("/{}", id_of(&urls[0])));
        assert_eq!(f.thumbs.with(|allow| allow.keys.len()), 1);
        f.thumbs.admit(&[&f.root], &[b.to_str().unwrap()]);
        assert_eq!(f.thumbs.with(|allow| allow.keys.len()), 0);
    }

    #[test]
    fn thumbs_prune_removes_old_cache_files_only() {
        let f = fixture();
        let dir = f.thumbs.cache_dir();
        std::fs::create_dir_all(dir).unwrap();
        let old = SystemTime::now() - Duration::from_secs(40 * 24 * 3600);
        for name in ["old.jpg", "old.1.tmp", "new.jpg", "keep.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        for name in ["old.jpg", "old.1.tmp", "keep.txt"] {
            let file = std::fs::File::options()
                .write(true)
                .open(dir.join(name))
                .unwrap();
            file.set_modified(old).unwrap();
        }
        assert_eq!(f.thumbs.prune_cache(CACHE_MAX_AGE).unwrap(), 2);
        let mut left: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["keep.txt", "new.jpg"]);
        // No cache dir yet is not an error.
        let empty = Thumbs::new(f.root.join("no-cache"));
        assert_eq!(empty.prune_cache(CACHE_MAX_AGE).unwrap(), 0);
    }

    #[test]
    fn thumbs_decode_gate_bounds_concurrency() {
        let gate = std::sync::Arc::new(Gate::new(2));
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (gate, active, peak) = (gate.clone(), active.clone(), peak.clone());
                std::thread::spawn(move || {
                    let _permit = gate.enter();
                    let now = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(5));
                    active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(peak.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[cfg(unix)]
    #[test]
    fn thumbs_read_refuses_a_handle_that_is_not_the_checked_file() {
        let f = fixture();
        let pic = png(&f.root, "a.png", 10, 10);
        let other = png(&f.root, "b.png", 10, 10);
        assert!(open_checked(&pic).is_ok());
        // A symlink at the checked path is not the file it was checked as.
        let link = f.root.join("link.png");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert_eq!(
            open_checked(&link).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn thumbs_is_admitted_only_for_the_last_listing() {
        let f = fixture();
        let pic = png(&f.root, "a.png", 10, 10);
        let other = png(&f.root, "b.png", 10, 10);
        let secret = png(&f.outside, "id_ed25519.png", 10, 10);
        assert!(!f.thumbs.is_admitted(&pic), "nothing listed yet");

        // Listed outside the roots: no thumbnail, but still a picker entry.
        f.thumbs.admit(
            &[&f.root],
            &[pic.to_str().unwrap(), secret.to_str().unwrap()],
        );
        assert!(f.thumbs.is_admitted(&pic));
        assert!(f.thumbs.is_admitted(&f.root.join("../favorites/a.png")));
        assert!(f.thumbs.is_admitted(&secret));
        assert!(!f.thumbs.is_admitted(&other), "never listed");
        assert!(!f.thumbs.is_admitted(&f.outside.join("missing.png")));

        f.thumbs.admit(&[&f.root], &[other.to_str().unwrap()]);
        assert!(!f.thumbs.is_admitted(&pic), "replaced by the new listing");
        assert!(f.thumbs.is_admitted(&other));
    }

    #[test]
    fn thumbs_missing_file_is_404_and_non_images_are_not_admitted() {
        let f = fixture();
        let pic = png(&f.root, "gone.png", 10, 10);
        let txt = f.root.join("notes.txt");
        std::fs::write(&txt, "hi").unwrap();
        let urls = f
            .thumbs
            .admit(&[&f.root], &[pic.to_str().unwrap(), txt.to_str().unwrap()]);
        assert_eq!(urls[1], "");
        std::fs::remove_file(&pic).unwrap();
        assert_eq!(
            f.thumbs.respond(&format!("/{}", id_of(&urls[0]))).status(),
            404
        );
    }

    #[test]
    fn thumbs_undecodable_image_is_415() {
        let f = fixture();
        let bad = f.root.join("bad.jpg");
        std::fs::write(&bad, b"not a jpeg").unwrap();
        let urls = f.thumbs.admit(&[&f.root], &[bad.to_str().unwrap()]);
        assert_eq!(
            f.thumbs.respond(&format!("/{}", id_of(&urls[0]))).status(),
            415
        );
    }

    #[test]
    fn thumbs_ids_are_strict_hex() {
        assert!(is_id("0123456789abcdef"));
        for bad in [
            "",
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdef0",
            "../../etc/pass",
        ] {
            assert!(!is_id(bad), "{bad}");
        }
    }

    #[test]
    fn thumbs_size_keeps_aspect_and_never_upscales() {
        assert_eq!(thumb_size(3840, 2160), (682, 384));
        assert_eq!(thumb_size(1080, 1920), (384, 682));
        assert_eq!(thumb_size(300, 200), (300, 200));
        assert_eq!(thumb_size(0, 0), (1, 1));
    }

    #[test]
    fn thumbs_content_key_is_stable() {
        assert_eq!(content_key(b"abc"), content_key(b"abc"));
        assert_ne!(content_key(b"abc"), content_key(b"abd"));
        assert_eq!(content_key(b"").len(), 32);
    }

    #[test]
    fn thumbs_default_cache_is_under_the_platform_cache_dir() {
        if let Ok(cache) = kakemono_config::paths::cache_dir() {
            assert_eq!(Thumbs::detect().cache_dir(), cache.join("thumbs"));
        }
    }
}
