//! What the daemon's engine needs to browse the online sources from the picker:
//! the strict SSRF-guarded client and the preview
//! folder `<cache_dir>/previews`.

use std::path::Path;

use anyhow::Context as _;

use kakemono_config::Config;
use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_engine::Deps;
use kakemono_engine::browse::BrowseDeps;
use kakemono_engine::browse::PreviewCache;
use kakemono_sources::{GuardConfig, build_online, guarded_client};

/// The preview folder's name inside the cache dir.
pub const PREVIEWS_DIR: &str = "previews";

/// A location every online type accepts: keywords for Wallhaven and Unsplash, a subreddit for
/// Reddit; Bing and APOD ignore it.
const SAMPLE_LOCATION: &str = "wallpapers";

/// The union of every online provider's
/// [`media_hosts`](kakemono_sources::Provider::media_hosts), sorted and deduplicated. One
/// guarded client serves every source, so its redirects may go to any of them; the first URL
/// of each request is still checked against its own provider's list.
pub fn media_hosts(deps: &Deps) -> Vec<&'static str> {
    let cfg = Config::default();
    let mut hosts: Vec<&'static str> = SourceType::ALL
        .into_iter()
        .filter_map(|kind| {
            let entry = SourceEntry {
                id: kind.as_str().to_owned(),
                kind,
                location: SAMPLE_LOCATION.to_owned(),
                ..SourceEntry::default()
            };
            build_online(&entry, &cfg, deps.http.clone()).ok()
        })
        .flat_map(|source| source.provider.media_hosts().iter().copied())
        .collect();
    hosts.sort_unstable();
    hosts.dedup();
    hosts
}

/// `deps` with browsing on: the strict guard, a guarded client that follows redirects only to
/// [`media_hosts`], and previews under `<cache_dir>/previews`.
///
/// # Errors
///
/// When reqwest can't build the guarded client (TLS backend setup).
pub fn with_browse(deps: Deps, cache_dir: &Path) -> anyhow::Result<Deps> {
    let guard = GuardConfig::strict();
    let http =
        guarded_client(guard, &media_hosts(&deps)).context("building the browse HTTP client")?;
    let preview_root = cache_dir.join(PREVIEWS_DIR);
    Ok(deps.with_browse(BrowseDeps::new(http, guard, preview_root)))
}

/// Prunes the preview cache of `deps` in the background (old and surplus previews from earlier
/// runs). Call from inside the tokio runtime.
pub fn prune_previews_soon(deps: &BrowseDeps) {
    let cache = PreviewCache::new(deps.preview_root.clone(), deps.http.clone(), deps.guard)
        .with_limits(deps.preview_limits);
    tokio::spawn(async move {
        match cache.prune().await {
            Ok(0) => {}
            Ok(n) => tracing::debug!(removed = n, "pruned the preview cache"),
            Err(err) => tracing::warn!("can't prune the preview cache: {err}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every online type contributes its hosts, so a sample location that one of them stops
    /// accepting shows up here instead of as refused redirects in production.
    #[test]
    fn media_hosts_cover_every_online_provider() {
        assert_eq!(
            media_hosts(&test_deps()),
            [
                "apod.nasa.gov",
                "bing.com",
                "i.imgur.com",
                "i.redd.it",
                "images.unsplash.com",
                "plus.unsplash.com",
                "preview.redd.it",
                "wallhaven.cc",
                "www.bing.com",
            ]
        );
    }

    #[test]
    fn previews_live_in_the_cache_dir() {
        let deps = with_browse(test_deps(), Path::new("/cache/kakemono")).unwrap();
        let browse = deps.browse.unwrap();
        assert_eq!(
            (
                browse.preview_root.as_path(),
                browse.guard == GuardConfig::strict()
            ),
            (Path::new("/cache/kakemono/previews"), true)
        );
    }

    fn test_deps() -> Deps {
        use kakemono_platform::Platform;
        use kakemono_platform::fake::{ManualSignals, RecordingSetter};
        use std::sync::Arc;
        let platform = Platform {
            setter: Arc::new(RecordingSetter::new()),
            signals: Arc::new(ManualSignals::default()),
        };
        Deps::new(platform, kakemono_sources::http::client().unwrap())
    }
}
