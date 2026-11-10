//! The source ops behind the `check_source`, `fetch` and `download_now` requests
//! (wire shapes in `kakemono_protocol::types`).
//!
//! - [`Engine::check_source`] counts the images a `[[sources]]` row would give. Online types run
//!   one provider query (no download, [`CHECK_TIMEOUT`]); local types count files with
//!   `list_images`. It never writes a file.
//! - [`Engine::fetch_url`] saves a dropped image (an http(s) URL, a `file://` URL or an absolute
//!   path) into `[daemon] manual_dir` and sets it. The body must be a JPEG, PNG or WebP by its
//!   magic bytes (the `Content-Type` and the extension are not trusted) and at most
//!   [`MAX_FETCH_BYTES`]. The history entry has `source` and `source_id` `"manual"`.
//!
//! Errors are typed ([`SourceOpError`], [`ManualError`]); their `Display` text is what the
//! daemon sends, unchanged from the Go-era strings.
//! - [`Engine::download_now`] downloads one image for a source now, ignoring the downloader's
//!   backoff, without setting it.

use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_protocol::types::WallpaperInfo;
use kakemono_sources::download::MAX_IMAGE_BYTES;
use kakemono_sources::local::list_images;
use kakemono_sources::{build_online, is_image_file};
use reqwest::Url;
use tokio::task::JoinError;

use crate::engine::{Engine, EngineError, SourceOpError, disk_info};
use crate::gotext::{go_stat_error, redact_in};
use crate::pool::MANUAL_ID;

/// How long [`Engine::check_source`] waits for the provider.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// The largest image [`Engine::fetch_url`] accepts: 64 MB, the limit every source shares.
pub const MAX_FETCH_BYTES: u64 = MAX_IMAGE_BYTES;

/// The error of an op that needs the network while `[daemon] internet` is off.
pub const INTERNET_OFF: &str = "Internet access is off";

/// The error of [`Engine::check_source`] when the source gives zero images.
pub const NO_IMAGES_FOUND: &str = "no images found";

/// The row id [`Engine::check_source`] gives its temporary row.
const CHECK_ID: &str = "check";

/// Why [`Engine::fetch_url`] failed before setting the image. Crosses IPC as its `Display`
/// string, shown as is (no `manager: ` prefix).
#[derive(Debug, thiserror::Error)]
pub enum ManualError {
    /// The input is neither an http(s) URL, a `file://` URL nor an absolute path.
    #[error("not an http(s) URL or absolute file path: {0:?}")]
    BadInput(String),
    /// A URL with `[daemon] internet` off.
    #[error("Internet access is off")]
    InternetOff,
    /// The GET failed. `url` has its secrets redacted.
    #[error("download {url}: {reason}")]
    Download { url: String, reason: FetchFailure },
    /// The body isn't a JPEG, PNG or WebP by its magic bytes.
    #[error("not a JPEG, PNG or WebP image")]
    NotAnImage,
    /// The body or file is over [`MAX_FETCH_BYTES`].
    #[error("image is larger than {} MB", MAX_FETCH_BYTES / 1024 / 1024)]
    TooLarge,
    /// The local file can't be stat'ed. Go's `os.Stat` text.
    #[error("{}", go_stat_error(path, source))]
    Stat {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The local path is a folder or another non-file.
    #[error("{} is not a file", .0.display())]
    NotAFile(PathBuf),
    /// Reading the local file failed.
    #[error("read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Creating `manual_dir` or writing the new file failed.
    #[error("save {}: {source}", path.display())]
    Save {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Every `-<n>` name in `manual_dir` is taken.
    #[error("save {}: no free file name", .0.display())]
    NoFreeName(PathBuf),
    /// The save task panicked or was cancelled.
    #[error("save failed: {0}")]
    Task(#[source] JoinError),
}

/// Why the GET of [`ManualError::Download`] failed.
#[derive(Debug, thiserror::Error)]
pub enum FetchFailure {
    /// The request or the body read failed (the error carries no URL).
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// A non-2xx answer.
    #[error("HTTP {0}")]
    Status(u16),
}

impl Engine {
    /// Counts the images a `[[sources]]` row of type `kind` at `location` would give, without
    /// saving anything. Online types: one provider query with the current provider settings
    /// (API keys, `[wallhaven]` options), at most [`CHECK_TIMEOUT`]; the count is the number of
    /// results. Local types: `folder` counts recursively, `album-name`/`album-date` count the
    /// folder's own images, `images` is 1 for an image file. A leading `~` is expanded.
    ///
    /// # Errors
    ///
    /// A [`SourceOpError`], shown as is: [`NO_IMAGES_FOUND`] for zero, [`INTERNET_OFF`] for an
    /// online type with `[daemon] internet` off, `check timed out after 15s`, a location that
    /// doesn't parse (e.g. `unsplash: ...`), the provider's error text, `unsupported source
    /// type`.
    pub async fn check_source(
        &self,
        kind: SourceType,
        location: &str,
    ) -> Result<u32, SourceOpError> {
        let location = location.trim();
        let count = if kind.is_local() {
            let path = expand_home(location);
            tokio::task::spawn_blocking(move || count_local(kind, &path))
                .await
                .map_err(SourceOpError::Check)?
        } else if kind.is_online() {
            self.check_online(kind, location).await?
        } else {
            return Err(SourceOpError::Unsupported);
        };
        if count == 0 {
            return Err(SourceOpError::NoImagesFound);
        }
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    async fn check_online(&self, kind: SourceType, location: &str) -> Result<usize, SourceOpError> {
        let cfg = self.config();
        if !cfg.daemon.internet {
            return Err(SourceOpError::InternetOff);
        }
        let entry = SourceEntry {
            id: CHECK_ID.to_owned(),
            enabled: true,
            kind,
            location: location.to_owned(),
        };
        let http = self.inner.deps.http.clone();
        // Parse the location first: the sources factory skips a bad row with only a log line.
        build_online(&entry, &cfg, http.clone())?;
        let mut check_cfg = (*cfg).clone();
        check_cfg.sources = vec![entry];
        let set = (self.inner.deps.sources)(&check_cfg, http);
        let source = set
            .online
            .into_iter()
            .next()
            .ok_or(SourceOpError::CantBuild(kind))?;
        let fetch = source.provider.fetch(&source.query);
        match tokio::time::timeout(CHECK_TIMEOUT, fetch).await {
            Err(_) => Err(SourceOpError::CheckTimeout),
            Ok(Err(e)) => Err(SourceOpError::Provider(e)),
            Ok(Ok(results)) => Ok(results.len()),
        }
    }

    /// Saves the image at `input` into `[daemon] manual_dir` and sets it, as a drop on the UI
    /// does. `input` is an `http`/`https` URL (downloaded with the shared client), a `file://`
    /// URL or an absolute path (copied; a file already directly in `manual_dir` is set in
    /// place). The saved name is the URL's or file's name with the extension of the detected
    /// type, made unique with `-2`, `-3`, ...
    ///
    /// The history entry: `source` and `source_id` `"manual"`, `image_url` the URL (empty for a
    /// local file), the file name as title.
    ///
    /// A file that was saved but couldn't be set stays in `manual_dir`.
    ///
    /// # Errors
    ///
    /// [`EngineError::Manual`] (text shown as is): `not an http(s) URL or absolute file path:
    /// ...`, [`INTERNET_OFF`] for a URL with `[daemon] internet` off, `download ...: ...`
    /// (secrets in the URL redacted), `not a JPEG, PNG or WebP image`, `image is larger than 64
    /// MB`, `stat <path>: ...`, `save <path>: ...`; or [`EngineError::Set`].
    pub async fn fetch_url(&self, input: &str) -> Result<WallpaperInfo, EngineError> {
        let input = input.trim();
        let manual_dir = self.config().daemon.manual_dir_resolved();
        let (path, image_url) = match parse_input(input)? {
            Input::Remote(url) => {
                if !self.config().daemon.internet {
                    return Err(ManualError::InternetOff.into());
                }
                let (bytes, format) = self.download_image(&url).await?;
                let name = url_file_name(&url);
                let path = save_unique(&manual_dir, &name, format, bytes).await?;
                (path, input.to_owned())
            }
            Input::Local(path) => (import_local(&manual_dir, &path).await?, String::new()),
        };

        let _op = self.lock_op().await;
        self.set_image(&path).await?;
        let now = (self.inner.deps.now)();
        let info = WallpaperInfo {
            image_url,
            ..disk_info(&path, MANUAL_ID, MANUAL_ID, &now)
        };
        tracing::info!(path = %path.display(), "fetched into the manual folder");
        self.snap().state.push(info.clone(), now);
        self.persist().await;
        self.changed(&info);
        Ok(info)
    }

    /// Downloads one image now, ignoring the downloader's backoff, without setting it. With
    /// `id`: for that enabled online source ([`Engine::download_one`]). Without: for every
    /// enabled online source whose download folder has no image, in config order.
    ///
    /// Returns the downloaded file (the first one without `id`), or `None` without `id` when no
    /// source needed an image.
    ///
    /// # Errors
    ///
    /// A [`SourceOpError`]: [`INTERNET_OFF`], `no enabled online source "<id>"`, or the
    /// download failures joined with `"; "` when none succeeded.
    pub async fn download_now(&self, id: Option<&str>) -> Result<Option<PathBuf>, SourceOpError> {
        if let Some(id) = id {
            return self.download_one(id).await.map(Some);
        }
        let (cfg, sources) = {
            let snap = self.snap();
            (Arc::clone(&snap.cfg), Arc::clone(&snap.sources))
        };
        if !cfg.daemon.internet {
            return Err(SourceOpError::InternetOff);
        }
        let download_dir = cfg.daemon.download_dir.clone();
        let empty: Vec<String> = tokio::task::spawn_blocking(move || {
            sources
                .online
                .iter()
                .filter(|s| list_images(&download_dir.join(&s.id), false).is_empty())
                .map(|s| s.id.clone())
                .collect()
        })
        .await
        .map_err(SourceOpError::List)?;

        let mut first = None;
        let mut failures = Vec::new();
        for id in empty {
            match self.download_one(&id).await {
                Ok(path) => {
                    tracing::info!(source_id = %id, path = %path.display(), "downloaded now");
                    first.get_or_insert(path);
                }
                Err(SourceOpError::Download(failure)) => {
                    tracing::warn!(source_id = %id, error = %failure, "download now failed");
                    failures.push(failure);
                }
                // The source left the config, or the internet was turned off, mid-loop.
                Err(other) => {
                    tracing::warn!(source_id = %id, error = %other, "download now skipped");
                }
            }
        }
        match first {
            Some(path) => Ok(Some(path)),
            None if failures.is_empty() => Ok(None),
            None => Err(SourceOpError::Downloads(failures)),
        }
    }

    /// GETs `url` with the shared client, at most [`MAX_FETCH_BYTES`], and checks the type.
    async fn download_image(&self, url: &Url) -> Result<(Vec<u8>, ImageFormat), ManualError> {
        let raw = url.as_str();
        let fail = |reason: FetchFailure| ManualError::Download {
            url: redact_in(raw, raw),
            reason,
        };
        let mut response = self
            .inner
            .deps
            .http
            .get(url.clone())
            .send()
            .await
            .map_err(|e| fail(e.without_url().into()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(fail(FetchFailure::Status(status.as_u16())));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_FETCH_BYTES)
        {
            return Err(ManualError::TooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| fail(e.without_url().into()))?
        {
            if (bytes.len() + chunk.len()) as u64 > MAX_FETCH_BYTES {
                return Err(ManualError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let format = ImageFormat::sniff(&bytes).ok_or(ManualError::NotAnImage)?;
        Ok((bytes, format))
    }
}

/// What `fetch_url` was given.
enum Input {
    Remote(Url),
    Local(PathBuf),
}

fn parse_input(input: &str) -> Result<Input, ManualError> {
    let bad = || ManualError::BadInput(input.to_owned());
    if Path::new(input).is_absolute() {
        return Ok(Input::Local(PathBuf::from(input)));
    }
    let url = Url::parse(input).map_err(|_| bad())?;
    match url.scheme() {
        "http" | "https" => Ok(Input::Remote(url)),
        "file" => url.to_file_path().map(Input::Local).map_err(|()| bad()),
        _ => Err(bad()),
    }
}

/// The type of an accepted image, by its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageFormat {
    Jpeg,
    Png,
    WebP,
}

impl ImageFormat {
    fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::WebP)
        } else {
            None
        }
    }

    /// Whether `ext` (lower case) is a usual extension for this type.
    fn fits(self, ext: &str) -> bool {
        match self {
            Self::Jpeg => matches!(ext, "jpg" | "jpeg"),
            Self::Png => ext == "png",
            Self::WebP => ext == "webp",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
        }
    }
}

/// The URL's last path segment, percent-decoded (`image` when there is none).
fn url_file_name(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut s| s.next_back())
        .map(percent_decode)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "image".to_owned())
}

fn percent_decode(s: &str) -> String {
    // `Url::to_file_path` decodes; reuse it on a throwaway file URL.
    Url::parse(&format!("file:///{s}"))
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| s.to_owned())
}

/// `name` made safe, with an extension that fits `format`: `photo` → `photo.png`,
/// `a.jpeg` stays, `a.gif` holding a PNG → `a.gif.png`.
fn file_name_for(name: &str, format: ImageFormat) -> (String, String) {
    let name = kakemono_sources::sanitize_filename(name.trim());
    let name = name.trim_start_matches('.');
    let path = Path::new(name);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext {
        Some(ext) if format.fits(&ext) => {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ext = path.extension().unwrap_or_default().to_string_lossy();
            (or_image(stem), ext.into_owned())
        }
        _ => (or_image(name.to_owned()), format.ext().to_owned()),
    }
}

fn or_image(stem: String) -> String {
    if stem.is_empty() {
        "image".to_owned()
    } else {
        stem
    }
}

/// Writes `bytes` to a new file in `dir` named after `name` (see [`file_name_for`]), adding
/// `-2`, `-3`, ... until the name is free. Creates `dir`.
async fn save_unique(
    dir: &Path,
    name: &str,
    format: ImageFormat,
    bytes: Vec<u8>,
) -> Result<PathBuf, ManualError> {
    let (stem, ext) = file_name_for(name, format);
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let save_err = |p: &Path, source: io::Error| ManualError::Save {
            path: p.to_path_buf(),
            source,
        };
        std::fs::create_dir_all(&dir).map_err(|e| save_err(&dir, e))?;
        for n in 1u32..=u32::MAX {
            let file = if n == 1 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem}-{n}.{ext}")
            };
            let path = dir.join(file);
            let mut out = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(save_err(&path, e)),
            };
            if let Err(e) = out.write_all(&bytes).and_then(|()| out.sync_all()) {
                let _ = std::fs::remove_file(&path);
                return Err(save_err(&path, e));
            }
            return Ok(path);
        }
        Err(ManualError::NoFreeName(dir))
    })
    .await
    .map_err(ManualError::Task)?
}

/// Copies the local image `src` into `manual_dir` (or uses it when it is already directly in
/// `manual_dir`), after the size and type checks.
async fn import_local(manual_dir: &Path, src: &Path) -> Result<PathBuf, ManualError> {
    let meta = tokio::fs::metadata(src)
        .await
        .map_err(|source| ManualError::Stat {
            path: src.to_path_buf(),
            source,
        })?;
    if !meta.is_file() {
        return Err(ManualError::NotAFile(src.to_path_buf()));
    }
    if meta.len() > MAX_FETCH_BYTES {
        return Err(ManualError::TooLarge);
    }
    let bytes = read_capped(src).await?;
    let format = ImageFormat::sniff(&bytes).ok_or(ManualError::NotAnImage)?;
    let in_manual = match (
        src.parent().and_then(|p| p.canonicalize().ok()),
        manual_dir.canonicalize().ok(),
    ) {
        (Some(parent), Some(dir)) => parent == dir,
        _ => false,
    };
    if in_manual {
        return Ok(src.to_path_buf());
    }
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    save_unique(manual_dir, &name, format, bytes).await
}

/// The images a local row of type `kind` at `location` gives (see [`Engine::check_source`]).
fn count_local(kind: SourceType, location: &Path) -> usize {
    match kind {
        SourceType::Folder => list_images(location, true).len(),
        SourceType::AlbumName | SourceType::AlbumDate => list_images(location, false).len(),
        SourceType::Images => usize::from(location.is_file() && is_image_file(location)),
        _ => 0,
    }
}

/// Reads `src`, at most [`MAX_FETCH_BYTES`]: a file that grew past the cap after the size check
/// is [`ManualError::TooLarge`], not read whole.
async fn read_capped(src: &Path) -> Result<Vec<u8>, ManualError> {
    let path = src.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let read_err = |source| ManualError::Read {
            path: path.clone(),
            source,
        };
        let file = std::fs::File::open(&path).map_err(read_err)?;
        let mut bytes = Vec::new();
        file.take(MAX_FETCH_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(read_err)?;
        if bytes.len() as u64 > MAX_FETCH_BYTES {
            return Err(ManualError::TooLarge);
        }
        Ok(bytes)
    })
    .await
    .map_err(ManualError::Task)?
}

/// `location` with a leading `~` expanded to the home directory.
fn expand_home(location: &str) -> PathBuf {
    let path = PathBuf::from(location);
    kakemono_config::expand_tilde(&path, &kakemono_config::defaults::home_dir()).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_accepts_only_jpeg_png_and_webp() {
        assert_eq!(
            ImageFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            ImageFormat::sniff(b"\x89PNG\r\n\x1a\n...."),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            ImageFormat::sniff(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageFormat::WebP)
        );
        assert_eq!(ImageFormat::sniff(b"GIF89a"), None);
        assert_eq!(ImageFormat::sniff(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(ImageFormat::sniff(b""), None);
    }

    #[test]
    fn file_names_get_an_extension_that_fits() {
        let png = ImageFormat::Png;
        let jpg = ImageFormat::Jpeg;
        assert_eq!(file_name_for("photo", png), ("photo".into(), "png".into()));
        assert_eq!(file_name_for("a.JPEG", jpg), ("a".into(), "JPEG".into()));
        assert_eq!(file_name_for("a.gif", png), ("a.gif".into(), "png".into()));
        assert_eq!(file_name_for("..", png), ("image".into(), "png".into()));
        assert_eq!(
            file_name_for("a/b:c.png", png),
            ("a_b_c".into(), "png".into())
        );
        assert_eq!(file_name_for(".png", png), ("png".into(), "png".into()));
    }

    #[test]
    fn url_file_names_are_decoded() {
        let url = |s: &str| Url::parse(s).unwrap();
        assert_eq!(url_file_name(&url("https://x/a%20b.jpg?k=1")), "a b.jpg");
        assert_eq!(url_file_name(&url("https://x/")), "image");
        assert_eq!(url_file_name(&url("https://x")), "image");
    }

    #[test]
    fn inputs_are_urls_or_absolute_paths() {
        assert!(matches!(
            parse_input("https://x/a.jpg"),
            Ok(Input::Remote(_))
        ));
        assert!(matches!(
            parse_input("HTTP://x/a.jpg"),
            Ok(Input::Remote(_))
        ));
        assert!(matches!(parse_input("/tmp/a.jpg"), Ok(Input::Local(_))));
        assert!(
            matches!(parse_input("file:///tmp/a%20b.jpg"), Ok(Input::Local(p)) if p == Path::new("/tmp/a b.jpg"))
        );
        for bad in ["a.jpg", "ftp://x/a.jpg", "", "data:image/png;base64,AA"] {
            assert!(parse_input(bad).is_err(), "{bad}");
        }
    }
}
