//! The "Web page" source: full-size images from a link the user pastes (a direct image, a static
//! HTML page or an RSS/Atom feed).
//!
//! [`extract`] turns a fetched page into candidate image URLs, [`dimensions`] reads an image's
//! size from its first bytes, [`probe`] reads the candidates' sizes over the network, [`cache`]
//! keeps each page's result, and [`provider`] ties them into the [`WebPage`] provider.

pub(crate) mod cache;
pub(crate) mod dimensions;
pub(crate) mod extract;
pub(crate) mod probe;
pub(crate) mod provider;

pub use provider::{WebPage, WebTuning};

/// Shortest side, in pixels, an image needs to count as a wallpaper.
pub(crate) const MIN_SHORT_SIDE: u32 = 1080;

/// Most candidate image URLs taken from one page.
pub(crate) const MAX_CANDIDATES: usize = 40;

/// Most bytes read from one image to find its size.
pub(crate) const PROBE_MAX_BYTES: usize = 256 * 1024;
