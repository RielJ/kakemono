//! The background downloader (`docs/plans/variety-sources-impl.md` Task 13).
//!
//! It keeps `<download_dir>/<id>/` filled for every enabled online source, so a wallpaper change
//! can pick from disk ([`crate::pool`]) instead of downloading on demand.
//!
//! # Loop
//!
//! [`Downloader::run`] re-reads the config on every pass, so a reload takes effect without a
//! restart. While `[daemon] internet` or `download_enabled` is off it sends no request at all.
//! Otherwise:
//!
//! - On start and after every wake (a reload), [`Downloader::fill_empty`] downloads one image for
//!   each online source whose folder has no image yet.
//! - Every `download_interval`, [`Downloader::tick`] downloads one image for the source with the
//!   fewest images that isn't in backoff (ties go to config order). A wake doesn't move this
//!   timer, so frequent reloads can't starve it.
//!
//! # One download
//!
//! [`download_one`] (and [`try_download`], which keeps the error kind):
//!
//! 1. `fetch(query)`, then drops the results whose `image_url` is in the seen ledger
//!    `<download_dir>/<id>/.kakemono-seen` (newline list, newest last, capped at [`SEEN_CAP`]).
//! 2. Downloads a random remaining result with [`kakemono_sources::download`] and records its URL
//!    in the ledger, so it isn't downloaded again even if the filters reject it.
//! 3. Computes its facts (stored in the [`FactsCache`]). If the `[filters]` reject it, the file is
//!    deleted with [`files::safe_delete`].
//! 4. If `download_limit_mb_enabled`, trims the whole download folder with
//!    [`files::trim_to_mb`], protecting the new file.
//!
//! # Backoff
//!
//! Per source, after a failed download (`n` = failures in a row before this one):
//!
//! - [`ProviderError::RateLimited`]: `min(interval × 2^n, 6h)`;
//! - nothing new (every result already seen, or no result): 6h;
//! - any other error: `min(1m × 2^n, interval × 4)`.
//!
//! A success, or a download the filters rejected, clears it. Backoff survives a wake, so a reload
//! doesn't hammer a rate-limited API; a source removed from the config loses its entry.
//! [`download_one`] ignores backoff: it is what `DownloadNow` and the cold-start fallback call.
//!
//! # Seam for the engine (Task 14)
//!
//! The downloader doesn't know `Engine`. Everything it needs comes through [`DownloadEnv`], a
//! trait of cheap, synchronous getters the engine implements on itself:
//!
//! - [`DownloadEnv::config`]: the current config snapshot (re-read on every pass);
//! - [`DownloadEnv::sources`]: the current [`SourceSet`] (rebuilt by `reload`);
//! - [`DownloadEnv::facts`], [`DownloadEnv::http`]: the shared facts cache and HTTP client;
//! - [`DownloadEnv::protect`]: what deletes must spare (manual, favorites, trash, the current
//!   wallpaper and history), built from the live state;
//! - [`DownloadEnv::filter_ctx`]: the screen size and theme for the filters;
//! - [`DownloadEnv::downloaded`]: an optional hook called after every attempt the loop makes.
//!
//! Task 14 spawns `Downloader::new().run(&engine, shutdown, wake)` from `Engine::run`, keeps the
//! `Arc<Notify>` and calls `wake.notify_one()` from `reload` (`notify_one` keeps a permit, so a
//! reload during a download isn't lost), and calls [`download_one`] for `download_one(id)`.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kakemono_config::schema::Config;
use kakemono_config::valid_id;
use kakemono_sources::local::list_images;
use kakemono_sources::{OnlineSource, ProviderError, SourceSet, WallpaperResult};
use rand::seq::IndexedRandom;
use reqwest::Client;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::engine::redact_in;
use crate::facts::FactsCache;
use crate::files::{self, Protect, SEEN_LEDGER};
use crate::filters::{FilterCtx, Filters};

/// Used when `[daemon] download_interval` is invalid or zero.
pub const DEFAULT_DOWNLOAD_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// The seen ledger keeps this many URLs, dropping the oldest.
pub const SEEN_CAP: usize = 5000;
/// The longest backoff, and the backoff after "nothing new".
pub const MAX_BACKOFF: Duration = Duration::from_secs(6 * 60 * 60);
/// The first backoff after an error that isn't a rate limit.
pub const ERROR_BACKOFF: Duration = Duration::from_secs(60);

/// What the downloader needs from its host (the engine). See the module docs.
pub trait DownloadEnv: Send + Sync {
    /// The current config.
    fn config(&self) -> Arc<Config>;
    /// The enabled online sources of the current config.
    fn sources(&self) -> Arc<SourceSet>;
    /// The shared image-facts cache.
    fn facts(&self) -> Arc<FactsCache>;
    /// The client downloads go through.
    fn http(&self) -> Client;
    /// The paths deletes must spare: manual, favorites and trash dirs, the current wallpaper and
    /// the history.
    fn protect(&self) -> Protect;
    /// The screen size and theme the filters judge against.
    fn filter_ctx(&self) -> FilterCtx;
    /// Called after every download attempt [`Downloader::run`] makes, with the source id and
    /// the result of [`download_one`]. Does nothing by default.
    fn downloaded(&self, _source_id: &str, _outcome: &Result<PathBuf, String>) {}
}

/// Why [`try_download`] produced no image. `Display` is the error text [`download_one`] returns.
#[derive(Debug, thiserror::Error)]
pub enum DownloadFailure {
    /// The source id can't name a folder.
    #[error("source id {0:?} is not a valid folder name")]
    InvalidId(String),
    /// `fetch` failed.
    #[error("provider {name} fetch: {error}")]
    Fetch {
        name: &'static str,
        error: ProviderError,
    },
    /// `fetch` returned nothing.
    #[error("provider {name} returned no results for query {query:?}")]
    NoResults { name: &'static str, query: String },
    /// Every result is in the seen ledger.
    #[error("provider {name} has nothing new for query {query:?}")]
    NothingNew { name: &'static str, query: String },
    /// The image download failed. The text is `download image: …`, secrets redacted.
    #[error("{0}")]
    Download(String),
    /// The image was downloaded, failed the filters and was deleted.
    #[error("downloaded {} failed the filters ({}); deleted", path.display(), failed.join(", "))]
    Filtered {
        path: PathBuf,
        failed: Vec<&'static str>,
    },
}

/// One source's backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// The source isn't tried by the loop before this.
    pub until: Instant,
    /// Failed downloads in a row.
    pub failures: u32,
}

/// The background download loop's state: the per-source backoff.
#[derive(Debug, Default)]
pub struct Downloader {
    backoff: HashMap<String, Backoff>,
    /// The invalid `download_interval` already warned about.
    warned: Option<String>,
}

impl Downloader {
    /// A downloader with no backoff.
    pub fn new() -> Self {
        Self::default()
    }

    /// The backoff of source `id`, if it ever failed and hasn't succeeded since.
    pub fn backoff(&self, id: &str) -> Option<Backoff> {
        self.backoff.get(id).copied()
    }

    /// Runs until `shutdown` (see the module docs). `wake` (a reload) fills empty folders at
    /// once and re-reads the config.
    pub async fn run<E: DownloadEnv + ?Sized>(
        &mut self,
        env: &E,
        shutdown: CancellationToken,
        wake: Arc<Notify>,
    ) {
        // Start counts as a wake: fill the empty folders first.
        let mut fill = true;
        let mut due: Option<Instant> = None;
        loop {
            let cfg = env.config();
            let interval = self.interval(&cfg);
            let now = Instant::now();
            // A reload may shorten the interval: never wait longer than the new one.
            let mut next = due.map_or(now + interval, |d| d.min(now + interval));
            if active(&cfg) {
                let work = async {
                    if fill {
                        self.fill_empty(env).await;
                        fill = false;
                    } else if now >= next {
                        self.tick(env).await;
                        next = Instant::now() + interval;
                    }
                };
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = work => {}
                }
            } else if now >= next {
                next = now + interval;
            }
            due = Some(next);
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = wake.notified() => fill = true,
                () = tokio::time::sleep_until(next) => {}
            }
        }
    }

    /// Downloads one image for every online source whose folder has no image and that isn't in
    /// backoff. Returns the ids tried, in order.
    pub async fn fill_empty<E: DownloadEnv + ?Sized>(&mut self, env: &E) -> Vec<String> {
        let (cfg, sources) = (env.config(), env.sources());
        self.prune(&sources);
        let mut tried = Vec::new();
        for source in &sources.online {
            if self.in_backoff(&source.id) {
                continue;
            }
            if image_count(&cfg, &source.id).await != Some(0) {
                continue;
            }
            self.attempt(env, &cfg, source).await;
            tried.push(source.id.clone());
        }
        tried
    }

    /// Downloads one image for the source with the fewest images that isn't in backoff. Returns
    /// its id, or `None` when every source is in backoff (or there is none).
    pub async fn tick<E: DownloadEnv + ?Sized>(&mut self, env: &E) -> Option<String> {
        let (cfg, sources) = (env.config(), env.sources());
        self.prune(&sources);
        let mut fewest: Option<(usize, &OnlineSource)> = None;
        for source in &sources.online {
            if self.in_backoff(&source.id) {
                continue;
            }
            let Some(count) = image_count(&cfg, &source.id).await else {
                continue;
            };
            if fewest.is_none_or(|(min, _)| count < min) {
                fewest = Some((count, source));
            }
        }
        let (_, source) = fewest?;
        self.attempt(env, &cfg, source).await;
        Some(source.id.clone())
    }

    /// One download for the loop: updates the backoff and calls [`DownloadEnv::downloaded`].
    async fn attempt<E: DownloadEnv + ?Sized>(
        &mut self,
        env: &E,
        cfg: &Config,
        source: &OnlineSource,
    ) {
        let interval = self.interval(cfg);
        let outcome = match try_download(env, source).await {
            Ok(path) => {
                tracing::info!(source_id = %source.id, path = %path.display(), "downloaded");
                self.backoff.remove(&source.id);
                Ok(path)
            }
            Err(failure) => {
                let failures = self.backoff.get(&source.id).map_or(0, |b| b.failures);
                match backoff_delay(&failure, failures, interval) {
                    Some(delay) => {
                        tracing::warn!(
                            source_id = %source.id,
                            error = %failure,
                            ?delay,
                            "download failed; backing off"
                        );
                        self.backoff.insert(
                            source.id.clone(),
                            Backoff {
                                until: Instant::now() + delay,
                                failures: failures.saturating_add(1),
                            },
                        );
                    }
                    None => {
                        tracing::info!(source_id = %source.id, "{failure}");
                        self.backoff.remove(&source.id);
                    }
                }
                Err(failure.to_string())
            }
        };
        env.downloaded(&source.id, &outcome);
    }

    /// Whether source `id` must not be tried yet.
    fn in_backoff(&self, id: &str) -> bool {
        self.backoff
            .get(id)
            .is_some_and(|b| Instant::now() < b.until)
    }

    /// Forgets the backoff of sources no longer configured.
    fn prune(&mut self, sources: &SourceSet) {
        self.backoff.retain(|id, _| sources.get(id).is_some());
    }

    /// `[daemon] download_interval`, or [`DEFAULT_DOWNLOAD_INTERVAL`] (warned once per value)
    /// when it is invalid or zero.
    fn interval(&mut self, cfg: &Config) -> Duration {
        let raw = &cfg.daemon.download_interval;
        let problem = match cfg.daemon.download_interval_duration() {
            Ok(d) if !d.is_zero() => {
                self.warned = None;
                return d;
            }
            Ok(_) => "the interval is zero".to_owned(),
            Err(e) => e.to_string(),
        };
        if self.warned.as_deref() != Some(raw.as_str()) {
            tracing::warn!(
                interval = %raw,
                %problem,
                "invalid [daemon] download_interval, defaulting to 10m"
            );
            self.warned = Some(raw.clone());
        }
        DEFAULT_DOWNLOAD_INTERVAL
    }
}

/// Whether the config allows background downloads.
fn active(cfg: &Config) -> bool {
    cfg.daemon.internet && cfg.daemon.download_enabled
}

/// The number of images in source `id`'s download folder, or `None` for an id that can't name
/// a folder.
async fn image_count(cfg: &Config, id: &str) -> Option<usize> {
    if !valid_id(id) {
        return None;
    }
    let dir = cfg.daemon.download_dir.join(id);
    let count = tokio::task::spawn_blocking(move || list_images(&dir, false).len()).await;
    Some(count.unwrap_or(0))
}

/// Downloads one image for `source` (see the module docs), ignoring backoff. The error text
/// keeps the engine's `provider {name} fetch: {e}` format.
pub async fn download_one<E: DownloadEnv + ?Sized>(
    env: &E,
    source: &OnlineSource,
) -> Result<PathBuf, String> {
    try_download(env, source).await.map_err(|e| e.to_string())
}

/// [`download_one`] with the failure kind kept, for callers that react to it.
pub async fn try_download<E: DownloadEnv + ?Sized>(
    env: &E,
    source: &OnlineSource,
) -> Result<PathBuf, DownloadFailure> {
    if !valid_id(&source.id) {
        return Err(DownloadFailure::InvalidId(source.id.clone()));
    }
    let cfg = env.config();
    let download_dir = &cfg.daemon.download_dir;
    let dir = download_dir.join(&source.id);
    let name = source.provider.name();
    let query = &source.query;

    let results = source
        .provider
        .fetch(query)
        .await
        .map_err(|error| DownloadFailure::Fetch { name, error })?;
    if results.is_empty() {
        return Err(DownloadFailure::NoResults {
            name,
            query: query.clone(),
        });
    }
    let ledger = dir.join(SEEN_LEDGER);
    let result = {
        let seen = read_seen(&ledger).await;
        let seen: HashSet<&str> = seen.iter().map(String::as_str).collect();
        let fresh: Vec<&WallpaperResult> = results
            .iter()
            .filter(|r| !seen.contains(r.image_url.as_str()))
            .collect();
        fresh.choose(&mut rand::rng()).map(|r| (*r).clone())
    };
    let Some(result) = result else {
        return Err(DownloadFailure::NothingNew {
            name,
            query: query.clone(),
        });
    };

    let url = result.image_url.as_str();
    let path = kakemono_sources::download(&env.http(), url, &dir)
        .await
        .map_err(|e| {
            DownloadFailure::Download(format!(
                "download image: {}",
                redact_in(&e.to_string(), url)
            ))
        })?;
    if let Err(err) = record_seen(&ledger, url).await {
        tracing::warn!(ledger = %ledger.display(), "can't record a downloaded url: {err}");
    }

    let facts = env.facts();
    let image = facts.facts(&path).await;
    facts.schedule_save();
    let filters = Filters::compile(&cfg.filters).0;
    if let Some(image) = image.filter(|_| !filters.is_empty())
        && let Err(failed) = filters.check(&path, &image, &env.filter_ctx())
    {
        if let Err(err) = files::safe_delete(&path, download_dir, &env.protect()) {
            tracing::warn!(path = %path.display(), "can't delete a filtered download: {err}");
        }
        return Err(DownloadFailure::Filtered { path, failed });
    }

    if cfg.daemon.download_limit_mb_enabled {
        let limit = u64::from(cfg.daemon.download_limit_mb) * 1024 * 1024;
        files::trim_to_mb(download_dir, limit, &env.protect().file(path.clone())).await;
    }
    Ok(path)
}

/// How long source backoff lasts after `failure`, given `failures` failures in a row before it.
/// `None` clears the backoff.
fn backoff_delay(failure: &DownloadFailure, failures: u32, interval: Duration) -> Option<Duration> {
    let doubled =
        |base: Duration| base.saturating_mul(1u32.checked_shl(failures).unwrap_or(u32::MAX));
    match failure {
        DownloadFailure::Fetch {
            error: ProviderError::RateLimited(_),
            ..
        } => Some(doubled(interval).min(MAX_BACKOFF)),
        DownloadFailure::NothingNew { .. } | DownloadFailure::NoResults { .. } => Some(MAX_BACKOFF),
        DownloadFailure::Filtered { .. } => None,
        _ => Some(doubled(ERROR_BACKOFF).min(interval.saturating_mul(4))),
    }
}

/// Appends `url` to the seen ledger at `ledger`, keeping the newest [`SEEN_CAP`] lines. Creates
/// the file (and its folder) if needed.
pub async fn record_seen(ledger: &Path, url: &str) -> io::Result<()> {
    let mut seen = read_seen(ledger).await;
    seen.push(url.to_owned());
    let skip = seen.len().saturating_sub(SEEN_CAP);
    let mut text = String::new();
    for line in &seen[skip..] {
        text.push_str(line);
        text.push('\n');
    }
    if let Some(dir) = ledger.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    // Write a hidden temp file and rename it, so a crash never leaves a torn ledger.
    let tmp = ledger.with_extension("tmp");
    tokio::fs::write(&tmp, text).await?;
    tokio::fs::rename(&tmp, ledger).await
}

/// The URLs in the seen ledger at `ledger`, oldest first. A missing file is an empty list.
pub async fn read_seen(ledger: &Path) -> Vec<String> {
    match tokio::fs::read_to_string(ledger).await {
        Ok(text) => text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!(ledger = %ledger.display(), "can't read the seen ledger: {err}");
            }
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use image::{ImageFormat, Rgb, RgbImage};
    use kakemono_config::schema::{BrightnessFilter, FiltersConfig};
    use kakemono_platform::api::Theme;
    use kakemono_sources::fake::StaticProvider;
    use kakemono_sources::local::list_images;
    use kakemono_sources::{Provider, WallpaperResult};
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use wiremock::matchers::{method, path as url_path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const INTERVAL: Duration = Duration::from_secs(10 * 60);

    type Event = (String, Result<PathBuf, String>);

    struct TestEnv {
        cfg: Mutex<Arc<Config>>,
        sources: Arc<SourceSet>,
        facts: Arc<FactsCache>,
        http: Client,
        events: mpsc::UnboundedSender<Event>,
    }

    impl TestEnv {
        fn set_config(&self, edit: impl FnOnce(&mut Config)) {
            let mut guard = self.cfg.lock().unwrap();
            let mut cfg = Config::clone(&guard);
            edit(&mut cfg);
            *guard = Arc::new(cfg);
        }
    }

    impl DownloadEnv for TestEnv {
        fn config(&self) -> Arc<Config> {
            Arc::clone(&self.cfg.lock().unwrap())
        }
        fn sources(&self) -> Arc<SourceSet> {
            Arc::clone(&self.sources)
        }
        fn facts(&self) -> Arc<FactsCache> {
            Arc::clone(&self.facts)
        }
        fn http(&self) -> Client {
            self.http.clone()
        }
        fn protect(&self) -> Protect {
            Protect::new().dir(self.config().daemon.manual_dir_resolved())
        }
        fn filter_ctx(&self) -> FilterCtx {
            FilterCtx {
                screen: (0, 0),
                theme: Theme::Light,
            }
        }
        fn downloaded(&self, source_id: &str, outcome: &Result<PathBuf, String>) {
            let _ = self.events.send((source_id.to_owned(), outcome.clone()));
        }
    }

    /// A provider that is always rate limited.
    struct RateLimitedProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for RateLimitedProvider {
        fn name(&self) -> &'static str {
            "wallhaven"
        }
        async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::RateLimited("slow down".into()))
        }
    }

    struct Fixture {
        home: TempDir,
        env: Arc<TestEnv>,
        events: mpsc::UnboundedReceiver<Event>,
    }

    impl Fixture {
        fn new(sources: SourceSet) -> Self {
            let home = TempDir::new().unwrap();
            let mut cfg = kakemono_config::defaults_for_home(home.path());
            cfg.daemon.download_dir = home.path().join("downloads");
            cfg.daemon.manual_dir = Some(home.path().join("downloads/manual"));
            cfg.daemon.internet = true;
            cfg.daemon.download_enabled = true;
            cfg.daemon.download_interval = "10m".to_owned();
            cfg.daemon.download_limit_mb_enabled = false;
            cfg.filters = FiltersConfig::default();
            // Under a paused clock, a pending timer would auto-advance the clock while a request
            // is in flight: no pooled connections (idle timer), no debounce delay.
            let http = kakemono_sources::http::client_builder()
                .pool_max_idle_per_host(0)
                .build()
                .unwrap();
            let facts = Arc::new(
                FactsCache::load(home.path().join("facts.json")).with_debounce(Duration::ZERO),
            );
            let (tx, events) = mpsc::unbounded_channel();
            let env = Arc::new(TestEnv {
                cfg: Mutex::new(Arc::new(cfg)),
                sources: Arc::new(sources),
                facts,
                http,
                events: tx,
            });
            Self { home, env, events }
        }

        fn dir(&self, id: &str) -> PathBuf {
            self.home.path().join("downloads").join(id)
        }

        fn images(&self, id: &str) -> Vec<PathBuf> {
            list_images(&self.dir(id), false)
        }

        fn spawn_run(&self) -> (CancellationToken, Arc<Notify>, tokio::task::JoinHandle<()>) {
            let shutdown = CancellationToken::new();
            let wake = Arc::new(Notify::new());
            let env = Arc::clone(&self.env);
            let (s, w) = (shutdown.clone(), Arc::clone(&wake));
            let handle = tokio::spawn(async move { Downloader::new().run(&*env, s, w).await });
            (shutdown, wake, handle)
        }
    }

    /// The next `downloaded` event, failing (instead of hanging) if the loop ends first.
    async fn next_event(
        events: &mut mpsc::UnboundedReceiver<Event>,
        handle: &mut tokio::task::JoinHandle<()>,
    ) -> Event {
        tokio::select! {
            event = events.recv() => event.unwrap(),
            ended = handle => panic!("the download loop ended: {ended:?}"),
        }
    }

    /// A PNG of noise (so it is over `MIN_IMAGE_BYTES`), light or dark.
    fn noise_png(seed: u32, light: bool) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2_654_435_761).max(1);
        let base = if light { 225 } else { 0 };
        let img = RgbImage::from_fn(160, 120, |_, _| {
            let mut px = [0u8; 3];
            for c in &mut px {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                *c = base + (x % 30) as u8;
            }
            Rgb(px)
        });
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        let bytes = out.into_inner();
        assert!(bytes.len() as u64 > kakemono_sources::download::MIN_IMAGE_BYTES);
        bytes
    }

    fn png_response(seed: u32, light: bool) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "image/png")
            .set_body_bytes(noise_png(seed, light))
    }

    async fn serve(server: &MockServer, path: &str, light: bool, expect: u64) {
        Mock::given(method("GET"))
            .and(url_path(path))
            .respond_with(png_response(path.len() as u32, light))
            .expect(expect)
            .mount(server)
            .await;
    }

    fn provider(name: &'static str, server: &MockServer, paths: &[&str]) -> Arc<StaticProvider> {
        let results = paths
            .iter()
            .map(|p| StaticProvider::result(name, &format!("{}{p}", server.uri())))
            .collect();
        Arc::new(StaticProvider::new(name, results))
    }

    fn write_file(path: &Path, len: usize, age: Duration) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; len]).unwrap();
        let mtime = std::time::SystemTime::now() - age;
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn downloader_fills_an_empty_folder_right_after_start() {
        let server = MockServer::start().await;
        serve(&server, "/a/1.png", false, 1).await;
        let a = provider("bing", &server, &["/a/1.png"]);
        let b = provider("apod", &server, &["/b/1.png"]);
        let mut fx = Fixture::new(SourceSet::fake(vec![
            ("b", Arc::clone(&b) as Arc<dyn Provider>, ""),
            ("a", Arc::clone(&a) as Arc<dyn Provider>, ""),
        ]));
        write_file(&fx.dir("b").join("old.png"), 100, Duration::ZERO);

        let start = Instant::now();
        let (shutdown, _wake, mut handle) = fx.spawn_run();
        let (id, outcome) = next_event(&mut fx.events, &mut handle).await;

        assert_eq!(id, "a");
        assert_eq!(outcome.unwrap(), fx.dir("a").join("1.png"));
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "filled before any interval"
        );
        assert_eq!(fx.images("a").len(), 1);
        assert_eq!(b.calls(), 0, "a folder with images isn't filled");
        shutdown.cancel();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn downloader_downloads_for_the_fewest_files_source_after_the_interval() {
        let server = MockServer::start().await;
        serve(&server, "/b/1.png", false, 1).await;
        let a = provider("bing", &server, &["/a/1.png"]);
        let b = provider("apod", &server, &["/b/1.png"]);
        let mut fx = Fixture::new(SourceSet::fake(vec![
            ("a", Arc::clone(&a) as Arc<dyn Provider>, ""),
            ("b", Arc::clone(&b) as Arc<dyn Provider>, ""),
        ]));
        write_file(&fx.dir("a").join("x.png"), 100, Duration::ZERO);
        write_file(&fx.dir("a").join("y.png"), 100, Duration::ZERO);
        write_file(&fx.dir("b").join("x.png"), 100, Duration::ZERO);

        let start = Instant::now();
        let (shutdown, _wake, mut handle) = fx.spawn_run();
        let (id, outcome) = next_event(&mut fx.events, &mut handle).await;

        assert_eq!(id, "b");
        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(start.elapsed(), INTERVAL);
        assert_eq!(fx.images("b").len(), 2);
        assert_eq!(a.calls(), 0);
        shutdown.cancel();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn download_one_skips_urls_in_the_seen_ledger() {
        let server = MockServer::start().await;
        serve(&server, "/a/1.png", false, 0).await;
        serve(&server, "/a/2.png", false, 1).await;
        let a = provider("bing", &server, &["/a/1.png", "/a/2.png"]);
        let fx = Fixture::new(SourceSet::fake(vec![(
            "a",
            Arc::clone(&a) as Arc<dyn Provider>,
            "",
        )]));
        let ledger = fx.dir("a").join(files::SEEN_LEDGER);
        record_seen(&ledger, &format!("{}/a/1.png", server.uri()))
            .await
            .unwrap();
        let source = fx.env.sources.get("a").unwrap().clone();

        let path = download_one(&*fx.env, &source).await.unwrap();
        assert_eq!(path, fx.dir("a").join("2.png"));
        assert_eq!(read_seen(&ledger).await.len(), 2);

        let err = download_one(&*fx.env, &source).await.unwrap_err();
        assert_eq!(err, "provider bing has nothing new for query \"\"");
        server.verify().await;
    }

    #[tokio::test(start_paused = true)]
    async fn download_failing_the_dark_filter_is_deleted_and_marked_seen() {
        let server = MockServer::start().await;
        serve(&server, "/a/light.png", true, 1).await;
        let a = provider("bing", &server, &["/a/light.png"]);
        let fx = Fixture::new(SourceSet::fake(vec![(
            "a",
            Arc::clone(&a) as Arc<dyn Provider>,
            "",
        )]));
        fx.env
            .set_config(|c| c.filters.brightness = BrightnessFilter::Dark);
        let source = fx.env.sources.get("a").unwrap().clone();

        let err = try_download(&*fx.env, &source).await.unwrap_err();

        assert!(
            matches!(&err, DownloadFailure::Filtered { failed, .. } if failed == &["brightness"]),
            "{err:?}"
        );
        assert!(fx.images("a").is_empty(), "the rejected file is deleted");
        let seen = read_seen(&fx.dir("a").join(files::SEEN_LEDGER)).await;
        assert_eq!(seen, vec![format!("{}/a/light.png", server.uri())]);
    }

    #[tokio::test(start_paused = true)]
    async fn download_passing_the_filters_is_kept_with_facts_cached() {
        let server = MockServer::start().await;
        serve(&server, "/a/dark.png", false, 1).await;
        let a = provider("bing", &server, &["/a/dark.png"]);
        let fx = Fixture::new(SourceSet::fake(vec![(
            "a",
            Arc::clone(&a) as Arc<dyn Provider>,
            "",
        )]));
        fx.env
            .set_config(|c| c.filters.brightness = BrightnessFilter::Dark);
        let source = fx.env.sources.get("a").unwrap().clone();

        let path = download_one(&*fx.env, &source).await.unwrap();

        assert!(path.is_file());
        assert_eq!(
            fx.env.facts.len(),
            1,
            "facts are cached as the download lands"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limited_source_backs_off_while_others_are_tried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex("^/b/"))
            .respond_with(png_response(7, false))
            .mount(&server)
            .await;
        let limited = Arc::new(RateLimitedProvider {
            calls: AtomicUsize::new(0),
        });
        let b = provider("apod", &server, &["/b/1.png", "/b/2.png", "/b/3.png"]);
        let fx = Fixture::new(SourceSet::fake(vec![
            ("a", Arc::clone(&limited) as Arc<dyn Provider>, ""),
            ("b", Arc::clone(&b) as Arc<dyn Provider>, ""),
        ]));
        let mut d = Downloader::new();

        let t0 = Instant::now();
        assert_eq!(d.tick(&*fx.env).await.as_deref(), Some("a"));
        let backoff = d.backoff("a").unwrap();
        assert_eq!(
            backoff.until - t0,
            INTERVAL,
            "first rate limit waits one interval"
        );
        assert_eq!(backoff.failures, 1);

        assert_eq!(d.tick(&*fx.env).await.as_deref(), Some("b"));
        tokio::time::advance(INTERVAL - Duration::from_secs(1)).await;
        assert_eq!(d.tick(&*fx.env).await.as_deref(), Some("b"));
        assert_eq!(
            limited.calls.load(Ordering::SeqCst),
            1,
            "a waits out its backoff"
        );
        assert_eq!(fx.images("b").len(), 2);

        tokio::time::advance(Duration::from_secs(1)).await;
        let t1 = Instant::now();
        assert_eq!(d.tick(&*fx.env).await.as_deref(), Some("a"));
        assert_eq!(limited.calls.load(Ordering::SeqCst), 2);
        assert_eq!(d.backoff("a").unwrap().until - t1, INTERVAL * 2, "doubles");
        assert!(d.backoff("b").is_none(), "successes clear backoff");
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_new_backs_off_for_six_hours() {
        let server = MockServer::start().await;
        let a = provider("bing", &server, &["/a/1.png"]);
        let fx = Fixture::new(SourceSet::fake(vec![(
            "a",
            Arc::clone(&a) as Arc<dyn Provider>,
            "",
        )]));
        record_seen(
            &fx.dir("a").join(files::SEEN_LEDGER),
            &format!("{}/a/1.png", server.uri()),
        )
        .await
        .unwrap();
        let mut d = Downloader::new();

        let t0 = Instant::now();
        assert_eq!(d.tick(&*fx.env).await.as_deref(), Some("a"));
        assert_eq!(d.backoff("a").unwrap().until - t0, MAX_BACKOFF);
        assert_eq!(
            d.tick(&*fx.env).await,
            None,
            "the only source is in backoff"
        );
    }

    #[test]
    fn backoff_durations_follow_the_spec() {
        let rate = || DownloadFailure::Fetch {
            name: "x",
            error: ProviderError::RateLimited("r".into()),
        };
        let other = || DownloadFailure::Download("download image: boom".into());
        let nothing = DownloadFailure::NothingNew {
            name: "x",
            query: String::new(),
        };
        let m = Duration::from_secs(60);
        assert_eq!(backoff_delay(&rate(), 0, 10 * m), Some(10 * m));
        assert_eq!(backoff_delay(&rate(), 2, 10 * m), Some(40 * m));
        assert_eq!(backoff_delay(&rate(), 10, 10 * m), Some(MAX_BACKOFF));
        assert_eq!(backoff_delay(&rate(), 64, 10 * m), Some(MAX_BACKOFF));
        assert_eq!(backoff_delay(&other(), 0, 10 * m), Some(m));
        assert_eq!(backoff_delay(&other(), 3, 10 * m), Some(8 * m));
        assert_eq!(backoff_delay(&other(), 9, 10 * m), Some(40 * m));
        assert_eq!(backoff_delay(&nothing, 0, 10 * m), Some(MAX_BACKOFF));
        let filtered = DownloadFailure::Filtered {
            path: PathBuf::new(),
            failed: vec!["brightness"],
        };
        assert_eq!(backoff_delay(&filtered, 3, 10 * m), None);
    }

    #[tokio::test(start_paused = true)]
    async fn no_requests_while_internet_or_downloading_is_off() {
        for edit in [
            (|c: &mut Config| c.daemon.internet = false) as fn(&mut Config),
            |c: &mut Config| c.daemon.download_enabled = false,
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(png_response(1, false))
                .expect(0)
                .mount(&server)
                .await;
            let a = provider("bing", &server, &["/a/1.png"]);
            let fx = Fixture::new(SourceSet::fake(vec![(
                "a",
                Arc::clone(&a) as Arc<dyn Provider>,
                "",
            )]));
            fx.env.set_config(edit);

            let (shutdown, wake, handle) = fx.spawn_run();
            wake.notify_one();
            tokio::time::sleep(INTERVAL * 3 + Duration::from_secs(1)).await;
            shutdown.cancel();
            handle.await.unwrap();

            assert_eq!(a.calls(), 0);
            assert!(fx.images("a").is_empty());
            server.verify().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn mb_limit_is_enforced_after_each_download() {
        let server = MockServer::start().await;
        serve(&server, "/a/new.png", false, 1).await;
        let a = provider("bing", &server, &["/a/new.png"]);
        let fx = Fixture::new(SourceSet::fake(vec![(
            "a",
            Arc::clone(&a) as Arc<dyn Provider>,
            "",
        )]));
        fx.env.set_config(|c| {
            c.daemon.download_limit_mb_enabled = true;
            c.daemon.download_limit_mb = 1;
        });
        let oldest = fx.dir("a").join("oldest.png");
        let older = fx.dir("old").join("older.png");
        let manual = fx.dir("manual").join("kept.png");
        write_file(&oldest, 700 * 1024, Duration::from_secs(7200));
        write_file(&older, 300 * 1024, Duration::from_secs(3600));
        write_file(&manual, 2 * 1024 * 1024, Duration::from_secs(9000));
        let source = fx.env.sources.get("a").unwrap().clone();

        let path = download_one(&*fx.env, &source).await.unwrap();

        assert!(path.is_file(), "the new download is kept");
        assert!(!oldest.exists(), "the oldest download is trimmed");
        assert!(older.exists(), "trimming stops once under the limit");
        assert!(manual.exists(), "the manual folder is never trimmed");
    }

    #[tokio::test(start_paused = true)]
    async fn wake_runs_the_loop_immediately() {
        let server = MockServer::start().await;
        serve(&server, "/a/1.png", false, 1).await;
        let a = provider("bing", &server, &["/a/1.png"]);
        let mut fx = Fixture::new(SourceSet::fake(vec![(
            "a",
            Arc::clone(&a) as Arc<dyn Provider>,
            "",
        )]));
        fx.env.set_config(|c| c.daemon.internet = false);
        let start = Instant::now();
        let (shutdown, wake, mut handle) = fx.spawn_run();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(a.calls(), 0);

        fx.env.set_config(|c| c.daemon.internet = true);
        wake.notify_one();
        let (id, outcome) = next_event(&mut fx.events, &mut handle).await;

        assert_eq!(id, "a");
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(start.elapsed() < INTERVAL, "woke at {:?}", start.elapsed());
        shutdown.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn download_one_keeps_the_provider_fetch_error_format() {
        let failing = Arc::new(StaticProvider::failing(
            "bing",
            "bing: unexpected status 500",
        ));
        let fx = Fixture::new(SourceSet::fake(vec![(
            "a",
            Arc::clone(&failing) as Arc<dyn Provider>,
            "",
        )]));
        let source = fx.env.sources.get("a").unwrap().clone();
        let err = download_one(&*fx.env, &source).await.unwrap_err();
        assert_eq!(err, "provider bing fetch: bing: unexpected status 500");
    }

    #[tokio::test]
    async fn seen_ledger_keeps_the_newest_entries() {
        let dir = TempDir::new().unwrap();
        let ledger = dir.path().join("a").join(files::SEEN_LEDGER);
        let lines: String = (0..SEEN_CAP).map(|i| format!("u{i}\n")).collect();
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        std::fs::write(&ledger, lines).unwrap();

        record_seen(&ledger, "new").await.unwrap();

        let seen = read_seen(&ledger).await;
        assert_eq!(seen.len(), SEEN_CAP);
        assert_eq!(seen.first().map(String::as_str), Some("u1"));
        assert_eq!(seen.last().map(String::as_str), Some("new"));
    }
}
