//! Types for browsing a provider page by page (picker Sources tab).
//!
//! A provider returns its natural batch ([`BrowseBatch`]) and an opaque [`BrowseCursor`] for the
//! next one. The engine slices batches into pages of [`BROWSE_PAGE`] items. Cursors and URLs
//! stay inside the daemon; only [`RemoteItem::remote_id`] and metadata reach the webview.

/// Items per page the picker asks the daemon for.
pub const BROWSE_PAGE: usize = 10;

/// Longest [`RemoteItem::remote_id`] that [`sanitize_remote_id`] keeps as is.
const MAX_REMOTE_ID_LEN: usize = 64;

/// Where the next batch starts. The format is private to each provider (a page number, a seed,
/// a Reddit `after`, an APOD date) and is never serialized to the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseCursor(pub String);

/// How to get the full-size image of a [`RemoteItem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageRef {
    /// The URL of the image file itself.
    Direct(String),
    /// The URL of a page that names the image file; the provider resolves it in
    /// [`crate::Provider::resolve_image`] (Wallhaven scrape mode).
    Page(String),
}

/// One remote image offered for browsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteItem {
    /// Stable id within the provider, safe as a file name: see [`sanitize_remote_id`].
    pub remote_id: String,
    /// URL of a small preview image.
    pub preview_url: String,
    /// Where the full-size image is.
    pub image: ImageRef,
    /// Human-readable page of the image. Daemon-side only, for history provenance.
    pub source_url: String,
    /// Title; empty when the source has none.
    pub title: String,
    /// Creator; empty when the source has none.
    pub author: String,
    /// Full-size width in pixels, when the source reports it.
    pub width: Option<u32>,
    /// Full-size height in pixels, when the source reports it.
    pub height: Option<u32>,
    /// Whether the picker blurs the preview until hover or focus.
    pub nsfw: bool,
}

/// One provider response: its items and the cursor of the next batch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowseBatch {
    /// Items in provider order. May be empty without meaning the end.
    pub items: Vec<RemoteItem>,
    /// Cursor of the next batch; `None` means there are no more items.
    pub next: Option<BrowseCursor>,
}

/// How a provider pages through its images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseSupport {
    /// Numbered pages or an `after` cursor that end at some point.
    Paged,
    /// Never runs out (e.g. random images).
    Endless,
    /// A small fixed set (e.g. Bing's last 16 images).
    Finite,
    /// The provider cannot browse; [`crate::Provider::browse`] returns
    /// [`crate::ProviderError::Unsupported`].
    Unsupported,
}

/// `raw` as a file-name-safe id: `raw` itself when it matches `[A-Za-z0-9_-]{1,64}`, otherwise
/// `h` followed by 16 lower-case hex digits of a stable hash of `raw`.
///
/// The hash is 64-bit FNV-1a, so the same `raw` gives the same id across runs and builds (the
/// preview cache reuses files by this name).
///
/// ```
/// use kakemono_sources::browse::sanitize_remote_id;
/// assert_eq!(sanitize_remote_id("t3_abc"), "t3_abc");
/// assert_eq!(sanitize_remote_id("a/b").len(), 17);
/// ```
pub fn sanitize_remote_id(raw: &str) -> String {
    let is_safe = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if (1..=MAX_REMOTE_ID_LEN).contains(&raw.len()) && raw.bytes().all(is_safe) {
        raw.to_owned()
    } else {
        format!("h{:016x}", fnv1a64(raw.as_bytes()))
    }
}

/// 64-bit FNV-1a: tiny, dependency-free and stable, unlike `std`'s `DefaultHasher`.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_reddit_fullname() {
        assert_eq!(sanitize_remote_id("t3_abc"), "t3_abc");
    }

    #[test]
    fn sanitize_keeps_apod_date() {
        assert_eq!(sanitize_remote_id("2025-01-02"), "2025-01-02");
    }

    #[test]
    fn sanitize_keeps_64_safe_chars() {
        let id = "a".repeat(64);
        assert_eq!(sanitize_remote_id(&id), id);
    }

    #[test]
    fn sanitize_hashes_65_chars() {
        let got = sanitize_remote_id(&"a".repeat(65));
        assert!(is_hashed(&got), "{got}");
    }

    #[test]
    fn sanitize_hashes_empty() {
        assert!(is_hashed(&sanitize_remote_id("")));
    }

    #[test]
    fn sanitize_hashes_path_separators_and_dots() {
        for raw in ["../etc", "a/b", "a\\b", "a.b", "..", "OHR.Foo_EN-US123"] {
            let got = sanitize_remote_id(raw);
            assert!(is_hashed(&got), "{raw} -> {got}");
        }
    }

    #[test]
    fn sanitize_hashes_non_ascii() {
        assert!(is_hashed(&sanitize_remote_id("café")));
    }

    /// Published FNV-1a 64 test vectors. A change here renames every cached preview.
    #[test]
    fn fnv1a64_matches_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn sanitize_hash_is_zero_padded_hex_of_fnv1a64() {
        assert_eq!(sanitize_remote_id(""), "hcbf29ce484222325");
    }

    #[test]
    fn sanitize_distinguishes_inputs() {
        assert_ne!(sanitize_remote_id("a/b"), sanitize_remote_id("a/c"));
    }

    fn is_hashed(id: &str) -> bool {
        id.len() == 17
            && id.starts_with('h')
            && id[1..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}
