//! The browse preview cache: each remote item's small preview, downloaded through the SSRF
//! guard into `<root>/<source_id>/<remote_id>.<ext>`.
//!
//! A preview is kept only when it passes every check: the URL passes
//! [`GuardConfig::check_url`] against the provider's host list, the response is a success
//! within [`PreviewLimits::timeout`] and [`PreviewLimits::max_bytes`], the first bytes are
//! JPEG, PNG or WebP, and the header's dimensions are at most [`PreviewLimits::max_side`] on
//! each side. The body is written to a `.part` file and renamed into place, so a reader never
//! sees half a file. A file already in the cache is reused with no network call.
//!
//! Nothing here decodes pixels: the header check runs before the UI's thumbnailer does.

use std::cmp::Reverse;
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use futures::{StreamExt, stream};
use image::{ImageFormat, ImageReader};
use kakemono_sources::browse::{RemoteItem, sanitize_remote_id};
use kakemono_sources::{GuardConfig, GuardError};
use reqwest::{Client, Url};

/// The bounds on preview downloads and on the cache's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewLimits {
    /// The largest body kept, in bytes.
    pub max_bytes: u64,
    /// The largest width or height a preview's header may declare.
    pub max_side: u32,
    /// The deadline for one preview's request and body.
    pub timeout: Duration,
    /// How many previews [`PreviewCache::fetch_all`] downloads at once.
    pub concurrency: usize,
    /// Files older than this are removed by [`PreviewCache::prune`].
    pub max_age: Duration,
    /// The most files [`PreviewCache::prune`] keeps (the newest ones).
    pub max_files: usize,
}

impl Default for PreviewLimits {
    /// The plan's values: 4 MiB, 4096 px, 6 s each, 4 at once, 24 h and 2000 files.
    fn default() -> Self {
        Self {
            max_bytes: 4 * 1024 * 1024,
            max_side: 4096,
            timeout: Duration::from_secs(6),
            concurrency: 4,
            max_age: Duration::from_secs(24 * 60 * 60),
            max_files: 2000,
        }
    }
}

/// One preview to fetch: the item, the source it came from, that source's
/// [`kakemono_sources::Provider::allowed_hosts`], which the preview URL must match, and its
/// [`kakemono_sources::Provider::media_client`] when it has one.
#[derive(Debug, Clone)]
pub struct PreviewJob<'a> {
    /// The config id of the item's source; the cache's subfolder.
    pub source_id: &'a str,
    /// The item whose `preview_url` is fetched and whose `remote_id` names the file.
    pub item: &'a RemoteItem,
    /// Host suffixes the preview URL must match (see [`GuardConfig::check_url`]).
    pub hosts: Arc<[String]>,
    /// The source's own guarded client; `None` downloads with the cache's client.
    pub client: Option<Client>,
}

/// Why a preview was not stored.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PreviewError {
    /// The preview URL does not parse.
    #[error("preview URL is not a valid URL")]
    BadUrl,
    /// The guard refused the URL, a redirect or a DNS answer.
    #[error(transparent)]
    Guard(#[from] GuardError),
    /// The request or the body failed (the error carries no URL).
    #[error("preview request failed: {0}")]
    Request(#[source] reqwest::Error),
    /// The server answered with a non-success status.
    #[error("preview request failed with status {0}")]
    Status(u16),
    /// The body is over [`PreviewLimits::max_bytes`].
    #[error("preview is too large")]
    TooLarge,
    /// The body is not a JPEG, PNG or WebP image, or its header does not parse.
    #[error("preview is not a JPEG, PNG or WebP image")]
    NotAnImage,
    /// The header declares a side over [`PreviewLimits::max_side`].
    #[error("preview is {width}x{height}, larger than allowed")]
    Dimensions {
        /// The declared width.
        width: u32,
        /// The declared height.
        height: u32,
    },
    /// The request and body took longer than [`PreviewLimits::timeout`].
    #[error("preview download timed out")]
    Timeout,
    /// Writing the file failed.
    #[error("cannot store preview: {0}")]
    Io(#[from] io::Error),
}

/// The preview cache under one root folder (the daemon uses `<cache_dir>/previews`).
#[derive(Debug, Clone)]
pub struct PreviewCache {
    root: PathBuf,
    client: Client,
    guard: GuardConfig,
    limits: PreviewLimits,
}

/// Makes `.part` names unique within the process, so two fetches of the same item never write
/// the same temporary file.
static PART_SEQ: AtomicU64 = AtomicU64::new(0);

impl PreviewCache {
    /// A cache under `root` that downloads with `client`, which should come from
    /// [`kakemono_sources::guarded_client`] built with the same `guard`.
    #[must_use]
    pub fn new(root: PathBuf, client: Client, guard: GuardConfig) -> Self {
        Self {
            root,
            client,
            guard,
            limits: PreviewLimits::default(),
        }
    }

    /// This cache with `limits` instead of [`PreviewLimits::default`].
    #[must_use]
    pub fn with_limits(self, limits: PreviewLimits) -> Self {
        Self { limits, ..self }
    }

    /// The root folder every preview is stored under.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The path of `job`'s preview: the cached file when there is one, otherwise a freshly
    /// downloaded and checked one.
    ///
    /// # Errors
    ///
    /// Any failed check, as a [`PreviewError`]. No file is left behind on error.
    pub async fn try_fetch(&self, job: PreviewJob<'_>) -> Result<PathBuf, PreviewError> {
        // Both ids should already be file-name safe; sanitizing again keeps any other value
        // from reaching the path.
        let dir = self.root.join(sanitize_remote_id(job.source_id));
        let stem = sanitize_remote_id(&job.item.remote_id);
        if let Some(cached) = cached_file(&dir, &stem).await {
            touch(&cached).await;
            return Ok(cached);
        }
        let url = Url::parse(&job.item.preview_url).map_err(|_| PreviewError::BadUrl)?;
        self.guard.check_url(&url, &job.hosts)?;
        let client = job.client.as_ref().unwrap_or(&self.client);
        let bytes = tokio::time::timeout(self.limits.timeout, self.download(client, url))
            .await
            .map_err(|_| PreviewError::Timeout)??;
        let format = sniff(&bytes).ok_or(PreviewError::NotAnImage)?;
        let (width, height) = ImageReader::with_format(Cursor::new(&bytes), format)
            .into_dimensions()
            .map_err(|_| PreviewError::NotAnImage)?;
        if width > self.limits.max_side || height > self.limits.max_side {
            return Err(PreviewError::Dimensions { width, height });
        }
        let path = dir.join(format!("{stem}.{}", ext(format)));
        store(&dir, &path, &bytes).await?;
        Ok(path)
    }

    /// [`PreviewCache::try_fetch`], with a failure logged and given as `None` (the card shows
    /// a placeholder).
    pub async fn fetch(&self, job: PreviewJob<'_>) -> Option<PathBuf> {
        let (source_id, item) = (job.source_id, job.item);
        match self.try_fetch(job).await {
            Ok(path) => Some(path),
            Err(err @ PreviewError::Guard(_)) => {
                tracing::warn!(source_id, remote_id = %item.remote_id, error = %err, "preview refused");
                None
            }
            Err(err) => {
                tracing::debug!(source_id, remote_id = %item.remote_id, error = %err, "preview skipped");
                None
            }
        }
    }

    /// [`PreviewCache::fetch`] for every job, at most [`PreviewLimits::concurrency`] at once.
    /// The results are in the order of `jobs`.
    pub async fn fetch_all<'a>(
        &self,
        jobs: impl IntoIterator<Item = PreviewJob<'a>>,
    ) -> Vec<Option<PathBuf>> {
        let mut done: Vec<(usize, Option<PathBuf>)> = stream::iter(jobs.into_iter().enumerate())
            .map(|(i, job)| async move { (i, self.fetch(job).await) })
            .buffer_unordered(self.limits.concurrency.max(1))
            .collect()
            .await;
        done.sort_unstable_by_key(|(i, _)| *i);
        done.into_iter().map(|(_, path)| path).collect()
    }

    /// Removes files older than [`PreviewLimits::max_age`], then the oldest files past
    /// [`PreviewLimits::max_files`]. A missing root is an empty cache. Returns how many files
    /// were removed; a file that cannot be removed is logged and skipped.
    ///
    /// # Errors
    ///
    /// When the root exists but cannot be listed.
    pub async fn prune(&self) -> io::Result<usize> {
        let root = self.root.clone();
        let limits = self.limits;
        tokio::task::spawn_blocking(move || prune_dir(&root, SystemTime::now(), limits))
            .await
            .map_err(io::Error::other)?
    }

    /// The body of `url`, at most [`PreviewLimits::max_bytes`].
    async fn download(&self, client: &Client, url: Url) -> Result<Vec<u8>, PreviewError> {
        let mut response = client.get(url).send().await.map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(PreviewError::Status(status.as_u16()));
        }
        let max = self.limits.max_bytes;
        if response.content_length().is_some_and(|n| n > max) {
            return Err(PreviewError::TooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(request_error)? {
            if (bytes.len() + chunk.len()) as u64 > max {
                return Err(PreviewError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

/// A guard refusal inside a request error (a redirect hop or a DNS answer) as
/// [`PreviewError::Guard`]; any other request error without its URL.
fn request_error(err: reqwest::Error) -> PreviewError {
    match GuardError::find_in(&err) {
        Some(guard) => PreviewError::Guard(guard.clone()),
        None => PreviewError::Request(err.without_url()),
    }
}

/// The accepted preview types, by their first bytes. Narrower than `image::guess_format` on
/// purpose: only the types the thumbnailer decodes.
fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::WebP)
    } else {
        None
    }
}

/// The extension of a type [`sniff`] accepts (`ImageFormat` is non-exhaustive, so the rest
/// fall to `jpg`; [`sniff`] never returns them).
fn ext(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::WebP => "webp",
        _ => "jpg",
    }
}

/// The already stored preview for `stem` in `dir`, whatever its type.
async fn cached_file(dir: &Path, stem: &str) -> Option<PathBuf> {
    for ext in ["jpg", "png", "webp"] {
        let path = dir.join(format!("{stem}.{ext}"));
        if tokio::fs::metadata(&path)
            .await
            .is_ok_and(|meta| meta.is_file())
        {
            return Some(path);
        }
    }
    None
}

/// Sets `path`'s modification time to now, so [`PreviewCache::prune`] ages a preview from
/// its last use, not its download. A failure only costs a re-download later, so it is logged.
async fn touch(path: &Path) {
    let owned = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        // Write access, not truncation: Windows needs it to change file times.
        fs::OpenOptions::new()
            .write(true)
            .open(&owned)?
            .set_modified(SystemTime::now())
    })
    .await
    .unwrap_or_else(|join| Err(io::Error::other(join)));
    if let Err(err) = result {
        tracing::debug!(path = %path.display(), error = %err, "preview mtime not refreshed");
    }
}

/// Writes `bytes` to a `.part` file in `dir`, then renames it to `path`.
async fn store(dir: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    let seq = PART_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut part = path.as_os_str().to_owned();
    part.push(format!(".{}.{seq}.part", std::process::id()));
    let part = PathBuf::from(part);
    let written = match tokio::fs::write(&part, bytes).await {
        Ok(()) => tokio::fs::rename(&part, path).await,
        Err(err) => Err(err),
    };
    if written.is_err() {
        // Best effort: a leftover `.part` is pruned by age later.
        let _ = tokio::fs::remove_file(&part).await;
    }
    written
}

/// [`PreviewCache::prune`]'s work, on the files in `root`'s subfolders. Folders are kept even
/// when empty: a fetch may be about to write into one.
fn prune_dir(root: &Path, now: SystemTime, limits: PreviewLimits) -> io::Result<usize> {
    let sources = match fs::read_dir(root) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        other => other?,
    };
    let mut removed = 0;
    let mut kept = Vec::new();
    for source in sources.flatten() {
        if !source.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(source.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            // A file with no readable time, or one from the future, counts as new.
            let modified = meta.modified().unwrap_or(now);
            let expired = now
                .duration_since(modified)
                .is_ok_and(|age| age > limits.max_age);
            if expired {
                removed += remove(&entry.path());
            } else {
                kept.push((modified, entry.path()));
            }
        }
    }
    if kept.len() > limits.max_files {
        kept.sort_unstable_by_key(|(modified, _)| Reverse(*modified));
        removed += kept
            .drain(limits.max_files..)
            .map(|(_, path)| remove(&path))
            .sum::<usize>();
    }
    Ok(removed)
}

/// Removes `path`: 1 when it was removed, 0 when it was already gone or cannot be removed.
fn remove(path: &Path) -> usize {
    match fs::remove_file(path) {
        Ok(()) => 1,
        Err(err) if err.kind() == io::ErrorKind::NotFound => 0,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "cannot prune preview");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_accepts_jpeg_png_and_webp_only() {
        let got = [
            sniff(&[0xFF, 0xD8, 0xFF, 0xE0]),
            sniff(b"\x89PNG\r\n\x1a\n...."),
            sniff(b"RIFF\0\0\0\0WEBPVP8 "),
            sniff(b"GIF89a"),
            sniff(b"RIFF\0\0\0\0WAVE"),
            sniff(b"<html>"),
            sniff(b""),
        ];
        assert_eq!(
            got,
            [
                Some(ImageFormat::Jpeg),
                Some(ImageFormat::Png),
                Some(ImageFormat::WebP),
                None,
                None,
                None,
                None
            ]
        );
    }
}
