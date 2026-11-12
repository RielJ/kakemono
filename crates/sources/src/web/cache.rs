//! What each page last gave, shared by every `WebPage` built for the same link.
//!
//! Sources are rebuilt on every config reload, so a per-instance cache would be empty after each
//! one; this map lives for the whole process instead. Keyed by the page URL. The std mutex is
//! only held to copy an entry in or out, never across an `.await`.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use crate::guard::GuardedHttp;

/// Most pages kept; the oldest entry goes first.
pub(crate) const MAX_ENTRIES: usize = 32;

/// One wallpaper-sized image found on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Image {
    /// The full-size image.
    pub url: String,
    /// A smaller version for previews, when the page names one the client may fetch.
    pub preview: Option<String>,
    /// Display name.
    pub title: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// The last read of one page.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// When the page was read.
    pub at: Instant,
    /// The usable images, or the user-facing reason there are none.
    pub outcome: Result<Arc<[Image]>, String>,
    /// The client allowed the page host and the hosts of the page's images.
    pub http: GuardedHttp,
}

static CACHE: LazyLock<Mutex<HashMap<String, Entry>>> = LazyLock::new(Mutex::default);

fn lock() -> MutexGuard<'static, HashMap<String, Entry>> {
    // A panic while holding the lock leaves a map that is still consistent: every write is a
    // single insert or remove.
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A copy of the entry for `key`, however old.
pub(crate) fn get(key: &str) -> Option<Entry> {
    lock().get(key).cloned()
}

/// Stores `entry` for `key`, evicting the oldest entries beyond [`MAX_ENTRIES`].
pub(crate) fn put(key: &str, entry: Entry) {
    let mut map = lock();
    map.insert(key.to_owned(), entry);
    while map.len() > MAX_ENTRIES {
        let Some(oldest) = map.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone()) else {
            break;
        };
        map.remove(&oldest);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::guard::GuardConfig;

    fn entry(at: Instant) -> Entry {
        Entry {
            at,
            outcome: Err("x".into()),
            http: GuardedHttp::new(GuardConfig::strict(), &["example.com"]).unwrap(),
        }
    }

    #[test]
    fn put_evicts_the_oldest_entry_beyond_the_cap() {
        // Keys unique to this test: the map is shared by every test in the binary.
        let base = Instant::now() - Duration::from_secs(3600);
        let oldest = "cache-test://evict/0";
        put(oldest, entry(base));
        for i in 1..=MAX_ENTRIES {
            let at = base + Duration::from_secs(u64::try_from(i).unwrap());
            put(&format!("cache-test://evict/{i}"), entry(at));
        }
        assert!(get(oldest).is_none());
        assert!(lock().len() <= MAX_ENTRIES);
    }
}
