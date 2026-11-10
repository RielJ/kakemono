//! Image facts for the soft filters (spec items 19–23): pixel size, mean luminance, dominant
//! colours and the EXIF/XMP rating. [`compute`] does the blocking work; [`FactsCache`] keeps the
//! results in `<state_dir>/facts.json`, keyed by path and checked against `(mtime_ns, size)`.
//!
//! - Luminance is the mean Rec. 709 relative luminance of a 64×64 downscale, computed on
//!   linearised sRGB (0 = black, 1 = white).
//! - The palette is a 4-bit-per-channel histogram of the same downscale. It keeps the top
//!   [`PALETTE_MAX`] buckets with at least [`PALETTE_MIN_SHARE`] of the pixels. Each colour is
//!   the mean of the pixels in its bucket, with its share of the image.
//! - The rating is the EXIF `Rating` tag (0x4746) in IFD0, else the first `xmp:Rating` in the
//!   first [`XMP_SCAN_BYTES`] of the file. Only 0–5 counts; anything else is `None`.
//!
//! A file that can't be read or decoded is an `io::Error` (`InvalidData` for bad images), never a
//! panic, so callers can skip it.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Cursor};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Side of the square downscale used for luminance and the palette.
pub const DOWNSCALE: u32 = 64;
/// Most colours kept in [`ImageFacts::palette`].
pub const PALETTE_MAX: usize = 5;
/// Smallest share of the pixels a palette colour needs.
pub const PALETTE_MIN_SHARE: f32 = 0.05;
/// How much of the file start is scanned for an XMP `xmp:Rating`.
pub const XMP_SCAN_BYTES: usize = 512 * 1024;
/// File name of the cache inside the state dir.
pub const CACHE_FILE: &str = "facts.json";
/// Delay between the first unsaved change and the debounced save.
pub const SAVE_DEBOUNCE: Duration = Duration::from_secs(5);

/// EXIF `Rating` (Microsoft/XMP mirror), IFD0.
const EXIF_RATING_TAG: u16 = 0x4746;
/// Highest valid rating.
const RATING_MAX: u8 = 5;
/// Version of the JSON layout of `facts.json`. A different version starts empty.
const CACHE_VERSION: u32 = 1;

/// What the filters need to know about one image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageFacts {
    pub width: u32,
    pub height: u32,
    /// Mean Rec. 709 relative luminance on linearised sRGB, 0..=1.
    pub luminance: f32,
    /// Dominant colours (sRGB) with their share of the pixels, largest share first.
    pub palette: Vec<([u8; 3], f32)>,
    /// EXIF or XMP rating 0–5, if the file has one.
    pub rating: Option<u8>,
}

/// Reads and decodes `path` and computes its facts. Blocking: call it from `spawn_blocking`.
///
/// Errors: the read error, or `InvalidData` when the image can't be decoded.
pub fn compute(path: &Path) -> io::Result<ImageFacts> {
    let bytes = fs::read(path)?;
    compute_bytes(&bytes)
}

/// [`compute`] on bytes already in memory.
pub fn compute_bytes(bytes: &[u8]) -> io::Result<ImageFacts> {
    // A decoder bug must not take the daemon down; treat a panic as a bad image.
    panic::catch_unwind(AssertUnwindSafe(|| compute_inner(bytes)))
        .unwrap_or_else(|_| Err(invalid("image decoder panicked")))
}

fn compute_inner(bytes: &[u8]) -> io::Result<ImageFacts> {
    let img = image::load_from_memory(bytes).map_err(|e| invalid(&e.to_string()))?;
    let (width, height) = (img.width(), img.height());
    if width == 0 || height == 0 {
        return Err(invalid("image has no pixels"));
    }
    let small = if width <= DOWNSCALE && height <= DOWNSCALE {
        img.to_rgb8()
    } else {
        img.thumbnail_exact(DOWNSCALE.min(width), DOWNSCALE.min(height))
            .to_rgb8()
    };
    let pixels: Vec<[u8; 3]> = small.pixels().map(|p| p.0).collect();
    Ok(ImageFacts {
        width,
        height,
        luminance: mean_luminance(&pixels),
        palette: palette(&pixels),
        rating: rating(bytes),
    })
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// sRGB channel value (0–255) to linear light (0–1), IEC 61966-2-1.
fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Mean Rec. 709 relative luminance of `pixels` (0 for no pixels).
fn mean_luminance(pixels: &[[u8; 3]]) -> f32 {
    if pixels.is_empty() {
        return 0.0;
    }
    let lut: Vec<f32> = (0..=255u8).map(srgb_to_linear).collect();
    let sum: f64 = pixels
        .iter()
        .map(|[r, g, b]| {
            f64::from(
                0.2126 * lut[usize::from(*r)]
                    + 0.7152 * lut[usize::from(*g)]
                    + 0.0722 * lut[usize::from(*b)],
            )
        })
        .sum();
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let mean = (sum / pixels.len() as f64) as f32;
    mean.clamp(0.0, 1.0)
}

/// Top buckets of a 4-bit-per-channel histogram, each as the mean colour of its pixels.
fn palette(pixels: &[[u8; 3]]) -> Vec<([u8; 3], f32)> {
    if pixels.is_empty() {
        return Vec::new();
    }
    // bucket -> (count, sum r, sum g, sum b)
    let mut buckets: HashMap<u16, (u64, u64, u64, u64)> = HashMap::new();
    for &[r, g, b] in pixels {
        let key = (u16::from(r >> 4) << 8) | (u16::from(g >> 4) << 4) | u16::from(b >> 4);
        let e = buckets.entry(key).or_default();
        e.0 += 1;
        e.1 += u64::from(r);
        e.2 += u64::from(g);
        e.3 += u64::from(b);
    }
    let mut ranked: Vec<(u16, (u64, u64, u64, u64))> = buckets.into_iter().collect();
    ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    #[allow(clippy::cast_precision_loss)]
    let total = pixels.len() as f32;
    ranked
        .into_iter()
        .take(PALETTE_MAX)
        .filter_map(|(_, (n, r, g, b))| {
            #[allow(clippy::cast_precision_loss)]
            let share = n as f32 / total;
            let mean = |s: u64| u8::try_from(s / n).unwrap_or(u8::MAX);
            (share >= PALETTE_MIN_SHARE).then(|| ([mean(r), mean(g), mean(b)], share))
        })
        .collect()
}

/// EXIF `Rating`, else the XMP `xmp:Rating` in the first [`XMP_SCAN_BYTES`].
fn rating(bytes: &[u8]) -> Option<u8> {
    exif_rating(bytes).or_else(|| xmp_rating(&bytes[..bytes.len().min(XMP_SCAN_BYTES)]))
}

fn exif_rating(bytes: &[u8]) -> Option<u8> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .ok()?;
    let tag = exif::Tag(exif::Context::Tiff, EXIF_RATING_TAG);
    let value = exif.get_field(tag, exif::In::PRIMARY)?.value.get_uint(0)?;
    u8::try_from(value).ok().filter(|r| *r <= RATING_MAX)
}

/// First valid `xmp:Rating="N"` (or `'N'`) attribute or `<xmp:Rating>N</xmp:Rating>` element.
fn xmp_rating(bytes: &[u8]) -> Option<u8> {
    const NEEDLE: &[u8] = b"xmp:Rating";
    let mut from = 0;
    while let Some(pos) = find(&bytes[from..], NEEDLE) {
        let after = from + pos + NEEDLE.len();
        if let Some(r) = parse_rating_after(&bytes[after..]) {
            return Some(r);
        }
        from = after;
    }
    None
}

/// Parses `="N"`, `='N'` or `>N<` (whitespace allowed around the value) at the start of `rest`.
fn parse_rating_after(rest: &[u8]) -> Option<u8> {
    let rest = trim_start(rest);
    let (&first, rest) = rest.split_first()?;
    let (value, close) = match first {
        b'=' => {
            let rest = trim_start(rest);
            let (&quote, rest) = rest.split_first()?;
            if quote != b'"' && quote != b'\'' {
                return None;
            }
            (rest, quote)
        }
        b'>' => (rest, b'<'),
        _ => return None,
    };
    let value = trim_start(value);
    let digits = value.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    if trim_start(&value[digits..]).first() != Some(&close) {
        return None;
    }
    let n: u16 = std::str::from_utf8(&value[..digits]).ok()?.parse().ok()?;
    u8::try_from(n).ok().filter(|r| *r <= RATING_MAX)
}

fn trim_start(b: &[u8]) -> &[u8] {
    let n = b.iter().take_while(|c| c.is_ascii_whitespace()).count();
    &b[n..]
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// One cached result, valid while the file keeps the same `(mtime_ns, size)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    mtime_ns: u64,
    size: u64,
    facts: ImageFacts,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    path: PathBuf,
    #[serde(flatten)]
    entry: Entry,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: Vec<Record>,
}

#[derive(Debug, Default)]
struct Inner {
    map: HashMap<PathBuf, Entry>,
    dirty: bool,
}

/// Facts per path, persisted as JSON. Thread-safe; share it as `Arc<FactsCache>`.
#[derive(Debug)]
pub struct FactsCache {
    path: PathBuf,
    debounce: Duration,
    inner: Mutex<Inner>,
    save_pending: AtomicBool,
}

impl FactsCache {
    /// Loads `<state_dir>/facts.json` (see [`FactsCache::load`]).
    pub fn in_state_dir(state_dir: &Path) -> Self {
        Self::load(state_dir.join(CACHE_FILE))
    }

    /// Loads the cache stored at `path`. A missing file starts empty. An unreadable or corrupt
    /// file starts empty with a warning and is replaced on the next save.
    pub fn load(path: PathBuf) -> Self {
        let map = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<CacheFile>(&bytes) {
                Ok(file) if file.version == CACHE_VERSION => file
                    .entries
                    .into_iter()
                    .map(|r| (r.path, r.entry))
                    .collect(),
                Ok(file) => {
                    tracing::info!(
                        path = %path.display(),
                        version = file.version,
                        "facts cache has another version; starting empty"
                    );
                    HashMap::new()
                }
                Err(err) => {
                    tracing::warn!(path = %path.display(), "facts cache is corrupt, starting empty: {err}");
                    HashMap::new()
                }
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(err) => {
                tracing::warn!(path = %path.display(), "can't read the facts cache, starting empty: {err}");
                HashMap::new()
            }
        };
        Self {
            path,
            debounce: SAVE_DEBOUNCE,
            inner: Mutex::new(Inner { map, dirty: false }),
            save_pending: AtomicBool::new(false),
        }
    }

    /// Replaces the delay used by the debounced save (default [`SAVE_DEBOUNCE`]).
    #[must_use]
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Where the cache is stored.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    /// True when nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The cached facts for `path` if its `(mtime_ns, size)` still match, else computes and
    /// caches them. Blocking. A failed compute drops any stale entry and returns the error.
    pub fn get_or_compute(&self, path: &Path) -> io::Result<ImageFacts> {
        let (mtime_ns, size) = stamp(path)?;
        if let Some(e) = self.lock().map.get(path)
            && e.mtime_ns == mtime_ns
            && e.size == size
        {
            return Ok(e.facts.clone());
        }
        match compute(path) {
            Ok(facts) => {
                let mut inner = self.lock();
                inner.map.insert(
                    path.to_path_buf(),
                    Entry {
                        mtime_ns,
                        size,
                        facts: facts.clone(),
                    },
                );
                inner.dirty = true;
                Ok(facts)
            }
            Err(err) => {
                let mut inner = self.lock();
                if inner.map.remove(path).is_some() {
                    inner.dirty = true;
                }
                Err(err)
            }
        }
    }

    /// [`FactsCache::get_or_compute`] on the blocking pool. Returns `None` (logged at debug)
    /// when the file can't be read or decoded. Schedules a debounced save when something changed.
    pub async fn facts(self: &Arc<Self>, path: &Path) -> Option<ImageFacts> {
        let me = Arc::clone(self);
        let owned = path.to_path_buf();
        let result = tokio::task::spawn_blocking(move || me.get_or_compute(&owned)).await;
        if self.lock().dirty {
            self.schedule_save();
        }
        match result {
            Ok(Ok(facts)) => Some(facts),
            Ok(Err(err)) => {
                tracing::debug!(path = %path.display(), "no image facts: {err}");
                None
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), "image facts task failed: {err}");
                None
            }
        }
    }

    /// Saves after the debounce delay unless a save is already scheduled. Needs a tokio
    /// runtime; without one it does nothing (call [`FactsCache::save`] instead).
    pub fn schedule_save(self: &Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.save_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        handle.spawn(async move {
            tokio::time::sleep(me.debounce).await;
            me.save_pending.store(false, Ordering::SeqCst);
            let saver = Arc::clone(&me);
            match tokio::task::spawn_blocking(move || saver.save()).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    tracing::warn!(path = %me.path.display(), "can't save the facts cache: {err}");
                }
                Err(err) => tracing::warn!("facts cache save task failed: {err}"),
            }
        });
    }

    /// Drops entries whose file is gone and writes the cache if anything changed. Blocking.
    /// Writes through a temp file and a rename. Paths that aren't valid UTF-8 aren't written.
    pub fn save(&self) -> io::Result<()> {
        let paths: Vec<PathBuf> = self.lock().map.keys().cloned().collect();
        let missing: Vec<PathBuf> = paths.into_iter().filter(|p| !p.exists()).collect();
        let json = {
            let mut inner = self.lock();
            for p in &missing {
                if inner.map.remove(p).is_some() {
                    inner.dirty = true;
                }
            }
            if !inner.dirty {
                return Ok(());
            }
            let mut entries: Vec<Record> = inner
                .map
                .iter()
                .filter(|(p, _)| p.to_str().is_some())
                .map(|(p, e)| Record {
                    path: p.clone(),
                    entry: e.clone(),
                })
                .collect();
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            let json = serde_json::to_vec(&CacheFile {
                version: CACHE_VERSION,
                entries,
            })
            .map_err(io::Error::other)?;
            inner.dirty = false;
            json
        };
        if let Err(err) = write_atomic(&self.path, &json) {
            self.lock().dirty = true;
            return Err(err);
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// `(mtime in ns since the epoch, size)` of `path`. Pre-epoch mtimes count as 0.
fn stamp(path: &Path) -> io::Result<(u64, u64)> {
    let meta = fs::metadata(path)?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    Ok((mtime_ns, meta.len()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::fs::File;
    use std::time::SystemTime;

    fn solid(w: u32, h: u32, c: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb(c))
    }

    fn write_png(dir: &Path, name: &str, img: &RgbImage) -> PathBuf {
        let p = dir.join(name);
        img.save_with_format(&p, ImageFormat::Png).unwrap();
        p
    }

    fn jpeg_bytes(img: &RgbImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    /// Inserts an APP1 segment (`marker payload`) right after the JPEG SOI.
    fn with_app1(jpeg: &[u8], payload: &[u8]) -> Vec<u8> {
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        let len = u16::try_from(payload.len() + 2).unwrap();
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn xmp_app1(packet: &str) -> Vec<u8> {
        let mut p = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        p.extend_from_slice(packet.as_bytes());
        p
    }

    /// `Exif\0\0` + little-endian TIFF with one IFD0 entry: Rating (0x4746, SHORT) = `rating`.
    fn exif_app1(rating: u16) -> Vec<u8> {
        let mut p = b"Exif\0\0".to_vec();
        p.extend_from_slice(b"II*\0");
        p.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        p.extend_from_slice(&1u16.to_le_bytes()); // entry count
        p.extend_from_slice(&EXIF_RATING_TAG.to_le_bytes());
        p.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        p.extend_from_slice(&1u32.to_le_bytes()); // count
        p.extend_from_slice(&rating.to_le_bytes());
        p.extend_from_slice(&[0, 0]); // value padding
        p.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        p
    }

    #[test]
    fn facts_black_image_is_dark_with_black_palette() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_png(dir.path(), "black.png", &solid(200, 100, [0, 0, 0]));
        let f = compute(&p).unwrap();
        assert_eq!((f.width, f.height), (200, 100));
        assert!(f.luminance < 0.05, "luminance {}", f.luminance);
        assert_eq!(f.palette.first().map(|c| c.0), Some([0, 0, 0]));
        assert!(f.palette[0].1 > 0.99);
        assert_eq!(f.rating, None);
    }

    #[test]
    fn facts_white_image_is_light() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_png(dir.path(), "white.png", &solid(200, 100, [255, 255, 255]));
        let f = compute(&p).unwrap();
        assert!(f.luminance > 0.95, "luminance {}", f.luminance);
        assert_eq!(f.palette.first().map(|c| c.0), Some([255, 255, 255]));
    }

    #[test]
    fn facts_luminance_is_linearised_rec709() {
        // sRGB 128 grey is ~0.216 in linear light, not 0.5.
        let grey = compute_bytes(&jpeg_bytes(&solid(64, 64, [128, 128, 128]))).unwrap();
        assert!(
            (grey.luminance - 0.216).abs() < 0.01,
            "grey {}",
            grey.luminance
        );
        // Pure green weighs 0.7152, pure blue 0.0722.
        let png = |c| {
            let mut out = Cursor::new(Vec::new());
            solid(8, 8, c).write_to(&mut out, ImageFormat::Png).unwrap();
            compute_bytes(&out.into_inner()).unwrap().luminance
        };
        assert!((png([0, 255, 0]) - 0.7152).abs() < 0.001);
        assert!((png([0, 0, 255]) - 0.0722).abs() < 0.001);
    }

    #[test]
    fn facts_palette_keeps_top_buckets_over_min_share() {
        // 70 % red, 27 % blue, 3 % green: green is under 5 % and dropped.
        let mut img = solid(100, 100, [250, 10, 10]);
        for (x, y, px) in img.enumerate_pixels_mut() {
            let i = y * 100 + x;
            if i >= 7000 {
                *px = Rgb(if i >= 9700 { [0, 255, 0] } else { [0, 0, 255] });
            }
        }
        let pixels: Vec<[u8; 3]> = img.pixels().map(|p| p.0).collect();
        let pal = palette(&pixels);
        assert_eq!(pal.len(), 2, "{pal:?}");
        assert_eq!(pal[0].0, [250, 10, 10]);
        assert!((pal[0].1 - 0.70).abs() < 1e-4);
        assert_eq!(pal[1].0, [0, 0, 255]);
    }

    #[test]
    fn facts_xmp_attribute_rating_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let packet = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="4"/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
        let bytes = with_app1(&jpeg_bytes(&solid(32, 16, [90, 90, 90])), &xmp_app1(packet));
        let p = dir.path().join("rated.jpg");
        fs::write(&p, bytes).unwrap();
        let f = compute(&p).unwrap();
        assert_eq!(f.rating, Some(4));
        assert_eq!((f.width, f.height), (32, 16));
    }

    #[test]
    fn facts_xmp_element_rating_is_read() {
        let packet = "<rdf:Description><xmp:Rating> 3 </xmp:Rating></rdf:Description>";
        let bytes = with_app1(&jpeg_bytes(&solid(8, 8, [0, 0, 0])), &xmp_app1(packet));
        assert_eq!(compute_bytes(&bytes).unwrap().rating, Some(3));
    }

    #[test]
    fn facts_xmp_scan_skips_invalid_values() {
        assert_eq!(xmp_rating(br#"xmp:Rating="-1" xmp:Rating='2'"#), Some(2));
        assert_eq!(xmp_rating(br#"xmp:Rating="9""#), None);
        assert_eq!(xmp_rating(br#"xmp:RatingPercent="50""#), None);
        assert_eq!(xmp_rating(b"xmp:Rating"), None);
    }

    #[test]
    fn facts_xmp_rating_past_scan_window_is_ignored() {
        let mut bytes = vec![b' '; XMP_SCAN_BYTES];
        bytes.extend_from_slice(br#"xmp:Rating="5""#);
        assert_eq!(rating(&bytes), None);
        bytes.drain(..8); // the 14-byte attribute now ends 6 bytes past the window
        assert_eq!(rating(&bytes), None, "straddling the window");
        bytes.drain(..6);
        assert_eq!(rating(&bytes), Some(5));
    }

    #[test]
    fn facts_exif_rating_is_read_and_wins_over_xmp() {
        let jpeg = jpeg_bytes(&solid(8, 8, [0, 0, 0]));
        let exif_only = with_app1(&jpeg, &exif_app1(2));
        assert_eq!(compute_bytes(&exif_only).unwrap().rating, Some(2));
        let both = with_app1(&exif_only, &xmp_app1(r#"xmp:Rating="5""#));
        assert_eq!(compute_bytes(&both).unwrap().rating, Some(2));
        let out_of_range = with_app1(&jpeg, &exif_app1(99));
        assert_eq!(compute_bytes(&out_of_range).unwrap().rating, None);
    }

    #[test]
    fn facts_no_rating_is_none() {
        let f = compute_bytes(&jpeg_bytes(&solid(8, 8, [10, 20, 30]))).unwrap();
        assert_eq!(f.rating, None);
    }

    #[test]
    fn facts_corrupt_image_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.jpg");
        fs::write(&p, b"\xFF\xD8\xFF\xE0 this is not a jpeg").unwrap();
        assert_eq!(compute(&p).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let mut truncated = jpeg_bytes(&solid(64, 64, [1, 2, 3]));
        truncated.truncate(truncated.len() / 2);
        assert!(compute_bytes(&truncated).is_err());
        assert!(compute_bytes(&[]).is_err());
        assert_eq!(
            compute(&dir.path().join("missing.png")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    fn set_mtime(p: &Path, t: SystemTime) {
        File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn facts_cache_hits_until_mtime_changes() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_png(dir.path(), "a.png", &solid(40, 30, [0, 0, 0]));
        let cache = FactsCache::load(dir.path().join(CACHE_FILE));
        let first = cache.get_or_compute(&p).unwrap();
        let mtime = fs::metadata(&p).unwrap().modified().unwrap();

        // Same size and mtime, garbage content: a cache hit never re-reads the file.
        let len = fs::metadata(&p).unwrap().len();
        fs::write(&p, vec![0u8; usize::try_from(len).unwrap()]).unwrap();
        set_mtime(&p, mtime);
        assert_eq!(cache.get_or_compute(&p).unwrap(), first);

        // A new mtime recomputes, which now fails on the garbage and drops the entry.
        set_mtime(&p, mtime + Duration::from_secs(10));
        assert!(cache.get_or_compute(&p).is_err());
        assert!(cache.is_empty());
    }

    #[test]
    fn facts_cache_save_and_load_round_trip_dropping_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let keep = write_png(dir.path(), "keep.png", &solid(10, 10, [255, 255, 255]));
        let gone = write_png(dir.path(), "gone.png", &solid(10, 10, [0, 0, 0]));
        let file = dir.path().join("state").join(CACHE_FILE);
        let cache = FactsCache::load(file.clone());
        let kept = cache.get_or_compute(&keep).unwrap();
        cache.get_or_compute(&gone).unwrap();
        assert_eq!(cache.len(), 2);
        fs::remove_file(&gone).unwrap();
        cache.save().unwrap();
        assert_eq!(cache.len(), 1);

        let reloaded = FactsCache::load(file.clone());
        assert_eq!(reloaded.len(), 1);
        // Loaded entry is a hit: break the file content without changing size or mtime.
        let mtime = fs::metadata(&keep).unwrap().modified().unwrap();
        let len = fs::metadata(&keep).unwrap().len();
        fs::write(&keep, vec![0u8; usize::try_from(len).unwrap()]).unwrap();
        set_mtime(&keep, mtime);
        assert_eq!(reloaded.get_or_compute(&keep).unwrap(), kept);
        assert!(!file.with_extension("json.tmp").exists());
    }

    #[test]
    fn facts_cache_load_survives_corrupt_json() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(CACHE_FILE);
        fs::write(&file, b"{\"version\":1,\"entries\":[{\"path\":").unwrap();
        let cache = FactsCache::load(file.clone());
        assert!(cache.is_empty());

        // The next save replaces the corrupt file with a valid one.
        let p = write_png(dir.path(), "a.png", &solid(4, 4, [9, 9, 9]));
        cache.get_or_compute(&p).unwrap();
        cache.save().unwrap();
        assert_eq!(FactsCache::load(file).len(), 1);
    }

    #[test]
    fn facts_cache_load_starts_empty_on_wrong_shape_or_version() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(CACHE_FILE);
        fs::write(&file, b"[1,2,3]").unwrap();
        assert!(FactsCache::load(file.clone()).is_empty());
        fs::write(&file, b"{\"version\":999,\"entries\":[]}").unwrap();
        assert!(FactsCache::load(file).is_empty());
        assert!(FactsCache::in_state_dir(&dir.path().join("nope")).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn facts_async_computes_and_debounces_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_png(dir.path(), "a.png", &solid(20, 10, [0, 0, 0]));
        let b = write_png(dir.path(), "b.png", &solid(20, 10, [255, 255, 255]));
        let bad = dir.path().join("bad.png");
        fs::write(&bad, b"nope").unwrap();
        let cache =
            Arc::new(FactsCache::in_state_dir(dir.path()).with_debounce(Duration::from_millis(50)));
        assert_eq!(cache.facts(&a).await.map(|f| f.width), Some(20));
        assert!(cache.facts(&b).await.is_some());
        assert_eq!(cache.facts(&bad).await, None);
        assert!(!cache.path().exists(), "save is debounced, not immediate");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while FactsCache::load(cache.path().to_path_buf()).len() < 2 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "debounced save never ran"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
