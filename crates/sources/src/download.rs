//! Image download into the cache dir (port of Go `providers.DownloadFile`) plus the file-name
//! and image-type helpers shared with the local provider and the engine.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, StatusCode};
use tokio::io::AsyncWriteExt;

use crate::http;

/// A download smaller than this is treated as an error page, not an image (Go `minImageSize`).
pub const MIN_IMAGE_BYTES: u64 = 10 * 1024;

/// Highest numeric suffix tried when the target file name is taken (`name-1.jpg` ...).
const MAX_COLLISION_SUFFIX: u32 = 9999;

/// Why [`download`] failed. A failed download never leaves a file behind.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// `url` is not an absolute URL.
    #[error("download: invalid url {url}: {source}")]
    InvalidUrl {
        /// The URL as given.
        url: String,
        /// The parse error.
        #[source]
        source: url::ParseError,
    },
    /// The request or the body read failed.
    #[error("download: GET {url}: {source}")]
    Request {
        /// The URL as given.
        url: String,
        /// The transport error.
        #[source]
        source: reqwest::Error,
    },
    /// The server answered with a status other than 200.
    #[error("download: GET {url} returned status {status}")]
    Status {
        /// The URL as given.
        url: String,
        /// HTTP status code.
        status: u16,
    },
    /// The `Content-Type` is not an image type (see [`is_image_content_type`]).
    #[error("download: {url} returned non-image content-type {content_type:?}")]
    NotImage {
        /// The URL as given.
        url: String,
        /// The `Content-Type` header, empty when missing.
        content_type: String,
    },
    /// The body is smaller than [`MIN_IMAGE_BYTES`].
    #[error("download: {url} is too small ({bytes} bytes)")]
    TooSmall {
        /// The URL as given.
        url: String,
        /// Bytes received.
        bytes: u64,
    },
    /// Creating the dir, writing or renaming the file failed.
    #[error("download: {path}: {source}", path = path.display())]
    Io {
        /// The path being written.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Every name from `name.ext` to `name-9999.ext` already exists.
    #[error("download: no free file name for {name} in {dir}", dir = dir.display())]
    NoFreeName {
        /// The sanitized base name.
        name: String,
        /// The target dir.
        dir: PathBuf,
    },
}

/// Downloads the image at `url` into `dir` and returns the path of the new file.
///
/// - Uses [`http::send_with_retry`] (429/503 retries) through `client`.
/// - Requires status 200 and an image `Content-Type` before touching the disk.
/// - The file name is the last URL path segment (query dropped), passed through
///   [`sanitize_filename`]. An empty name becomes `wallpaper-<unix-nanos>`. A name without an
///   image extension gets one from the content type (`photo-123` → `photo-123.jpg`), so the
///   file is picked up by [`is_image_file`]. Go kept such names bare.
/// - The body streams into a hidden `.part` file in `dir`, then is renamed into place. On any
///   error the `.part` file is removed.
/// - Fewer than [`MIN_IMAGE_BYTES`] bytes is an error.
/// - If the name is taken, a suffix is added: `a.jpg`, `a-1.jpg`, `a-2.jpg`, ... Go overwrote.
pub async fn download(client: &Client, url: &str, dir: &Path) -> Result<PathBuf, DownloadError> {
    let parsed = url::Url::parse(url).map_err(|source| DownloadError::InvalidUrl {
        url: url.to_owned(),
        source,
    })?;

    let request_err = |source| DownloadError::Request {
        url: url.to_owned(),
        source,
    };
    let mut response = http::send_with_retry(client.get(parsed.clone()))
        .await
        .map_err(request_err)?;

    if response.status() != StatusCode::OK {
        return Err(DownloadError::Status {
            url: url.to_owned(),
            status: response.status().as_u16(),
        });
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let Some(ext) = image_extension_for(&content_type) else {
        return Err(DownloadError::NotImage {
            url: url.to_owned(),
            content_type,
        });
    };

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|source| DownloadError::Io {
            path: dir.to_owned(),
            source,
        })?;

    let name = file_name_for(&parsed, ext);
    let part = dir.join(format!(
        ".{name}.{}-{}.part",
        std::process::id(),
        unix_nanos()
    ));

    let written = match stream_to_file(&mut response, &part).await {
        Ok(n) => n,
        Err(e) => {
            remove_quietly(&part).await;
            return Err(match e {
                StreamError::Request(source) => request_err(source),
                StreamError::Io(source) => DownloadError::Io {
                    path: part.clone(),
                    source,
                },
            });
        }
    };

    if written < MIN_IMAGE_BYTES {
        remove_quietly(&part).await;
        return Err(DownloadError::TooSmall {
            url: url.to_owned(),
            bytes: written,
        });
    }

    match place(&part, dir, &name).await {
        Ok(path) => Ok(path),
        Err(e) => {
            remove_quietly(&part).await;
            Err(e)
        }
    }
}

/// Replaces characters that are unsafe in file names on any OS with `_`.
///
/// Port of Go `sanitizeFilename` (`/ \ : * ? " < > |`), plus ASCII control characters.
pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

/// Whether `path` has a recognised image extension (case-insensitive). Port of Go `IsImageFile`:
/// jpg, jpeg, png, webp, gif, bmp, tiff, tif.
pub fn is_image_file(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tiff" | "tif"
            )
        })
}

/// Whether a `Content-Type` value is an accepted image type (parameters ignored,
/// case-insensitive). Port of Go `isImageContentType`.
pub fn is_image_content_type(content_type: &str) -> bool {
    image_extension_for(content_type).is_some()
}

/// The extension to use for an accepted image content type.
fn image_extension_for(content_type: &str) -> Option<&'static str> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/bmp" => Some("bmp"),
        "image/tiff" => Some("tiff"),
        _ => None,
    }
}

/// Sanitized file name for `url`, with `ext` appended when it has no image extension.
fn file_name_for(url: &url::Url, ext: &str) -> String {
    let last = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or_default();
    // Some hosts (Bing's `/th?id=OHR.x_UHD.jpg`) serve every image from one
    // path and name it in the query; prefer that so files don't collide.
    let from_query = (!is_image_file(last))
        .then(|| {
            url.query_pairs()
                .map(|(_, v)| sanitize_filename(&v))
                .find(|v| is_image_file(v))
        })
        .flatten();
    let mut name = from_query.unwrap_or_else(|| sanitize_filename(last));
    if name.is_empty() || name == "." || name == ".." {
        name = format!("wallpaper-{}", unix_nanos());
    }
    if !is_image_file(&name) {
        name = format!("{name}.{ext}");
    }
    name
}

enum StreamError {
    Request(reqwest::Error),
    Io(std::io::Error),
}

/// Streams the body into a new file at `path` and returns the byte count.
async fn stream_to_file(response: &mut reqwest::Response, path: &Path) -> Result<u64, StreamError> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(StreamError::Io)?;
    let mut written: u64 = 0;
    while let Some(chunk) = response.chunk().await.map_err(StreamError::Request)? {
        file.write_all(&chunk).await.map_err(StreamError::Io)?;
        written += chunk.len() as u64;
    }
    file.flush().await.map_err(StreamError::Io)?;
    file.sync_all().await.map_err(StreamError::Io)?;
    Ok(written)
}

/// Moves `part` to the first free name among `name`, `stem-1.ext`, `stem-2.ext`, ...
///
/// Uses `hard_link` (fails with `AlreadyExists` instead of overwriting), so two concurrent
/// downloads of the same name cannot clobber each other. The caller removes `part` on error.
async fn place(part: &Path, dir: &Path, name: &str) -> Result<PathBuf, DownloadError> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    for n in 0..=MAX_COLLISION_SUFFIX {
        let candidate = match (n, ext) {
            (0, _) => name.to_owned(),
            (n, Some(ext)) => format!("{stem}-{n}.{ext}"),
            (n, None) => format!("{stem}-{n}"),
        };
        let path = dir.join(candidate);
        match tokio::fs::hard_link(part, &path).await {
            Ok(()) => {
                remove_quietly(part).await;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(DownloadError::Io { path, source }),
        }
    }
    Err(DownloadError::NoFreeName {
        name: name.to_owned(),
        dir: dir.to_owned(),
    })
}

async fn remove_quietly(path: &Path) {
    if let Err(e) = tokio::fs::remove_file(path).await {
        tracing::warn!(path = %path.display(), error = %e, "could not remove partial download");
    }
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_go_set_and_controls() {
        assert_eq!(
            sanitize_filename(r#"a/b\c:d*e?f"g<h>i|j"#),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_filename("tab\there\n"), "tab_here_");
        assert_eq!(
            sanitize_filename("wallhaven-abc123.jpg"),
            "wallhaven-abc123.jpg"
        );
    }

    #[test]
    fn image_file_extensions() {
        for ok in [
            "a.jpg",
            "a.JPEG",
            "dir/a.png",
            "a.webp",
            "a.gif",
            "a.bmp",
            "a.tiff",
            "a.TIF",
        ] {
            assert!(is_image_file(ok), "{ok}");
        }
        for bad in ["a", "a.txt", "a.jpg.part", "jpg", ".png.tmp", "a.heic"] {
            assert!(!is_image_file(bad), "{bad}");
        }
    }

    #[test]
    fn image_content_types() {
        assert!(is_image_content_type("image/jpeg"));
        assert!(is_image_content_type("IMAGE/PNG; charset=binary"));
        assert!(is_image_content_type(" image/webp "));
        assert!(!is_image_content_type("text/html; charset=utf-8"));
        assert!(!is_image_content_type(""));
        assert!(!is_image_content_type("image/svg+xml"));
    }

    #[test]
    fn file_name_from_url() {
        let u = |s| url::Url::parse(s).unwrap();
        assert_eq!(
            file_name_for(&u("https://x/full/ab/w-1.jpg?x=1"), "jpg"),
            "w-1.jpg"
        );
        assert_eq!(
            file_name_for(&u("https://x/photo-123?ixid=a"), "jpg"),
            "photo-123.jpg"
        );
        assert_eq!(file_name_for(&u("https://x/a%3Fb.png"), "png"), "a%3Fb.png");
        assert!(file_name_for(&u("https://x/"), "png").starts_with("wallpaper-"));
        assert!(file_name_for(&u("https://x/"), "png").ends_with(".png"));
    }

    /// Bing serves every image from `/th?id=<name>.jpg`; naming files after the
    /// path segment saved them all as `th`, each download overwriting the last.
    #[test]
    fn file_name_from_image_query_value() {
        let u = |s| url::Url::parse(s).unwrap();
        let a = file_name_for(
            &u("https://www.bing.com/th?id=OHR.SpringIceland_EN-US6846318257_UHD.jpg"),
            "jpg",
        );
        let b = file_name_for(
            &u("https://www.bing.com/th?id=OHR.PeachBloom_EN-US6957157989_UHD.jpg&rf=x"),
            "jpg",
        );
        assert_eq!(a, "OHR.SpringIceland_EN-US6846318257_UHD.jpg");
        assert_eq!(b, "OHR.PeachBloom_EN-US6957157989_UHD.jpg");
        // A query value that isn't an image name doesn't win over the path.
        assert_eq!(
            file_name_for(&u("https://x/photo-123?id=abc"), "jpg"),
            "photo-123.jpg"
        );
        // Query values can't escape the directory.
        assert_eq!(
            file_name_for(&u("https://x/th?id=../../etc/x.jpg"), "jpg"),
            ".._.._etc_x.jpg"
        );
    }
}
