//! Browse sessions (`docs/plans/picker-browse-impl.md` Task 12): `Engine::browse` interleaves
//! the enabled online sources, slices their batches into pages of 10, dedupes, reports each
//! source's state, and keeps pages idempotent. Scripted providers stand in for the real ones;
//! their preview URLs are refused by the guard (no media hosts), so no test touches the network.
//!
//! The fake provider and engine builder are shared with `browse_use.rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kakemono_config::Config;
use kakemono_engine::browse::{BrowseDeps, BrowseError, BrowseLimits};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine};
use kakemono_platform::api::{Fit, Platform, Theme};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_protocol::types::{BrowseData, BrowseSourceState};
use kakemono_sources::browse::{BrowseBatch, BrowseCursor, BrowseSupport, ImageRef, RemoteItem};
use kakemono_sources::{GuardConfig, Provider, ProviderError, SourceSet, WallpaperResult};
use tempfile::TempDir;

/// What a [`Scripted`] provider's `browse` does.
pub enum Mode {
    /// Batch `n` (cursor `None` is batch 0, `Some("n")` batch n) holds these ids; the last
    /// batch has no `next`.
    Pages(Vec<Vec<String>>),
    /// Every call is rate limited with this `Retry-After` hint.
    RateLimited(Option<u64>),
    /// Every call never returns.
    Hang,
    /// Every call needs setup.
    NotConfigured,
}

/// A scripted browsing provider. Its items' URLs are `<base>/img/<id>.jpg`.
pub struct Scripted {
    name: &'static str,
    mode: Mode,
    base: String,
    hosts: &'static [&'static str],
    page_image: bool,
    calls: AtomicUsize,
    resolves: AtomicUsize,
    cursors: Mutex<Vec<Option<String>>>,
}

#[expect(dead_code, reason = "used by browse_use.rs (Task 13)")]
impl Scripted {
    pub fn new(name: &'static str, mode: Mode) -> Self {
        Self {
            name,
            mode,
            base: "https://example.invalid".to_owned(),
            hosts: &[],
            page_image: false,
            calls: AtomicUsize::new(0),
            resolves: AtomicUsize::new(0),
            cursors: Mutex::new(Vec::new()),
        }
    }

    /// One batch of `ids`.
    pub fn one_batch(name: &'static str, ids: &[&str]) -> Self {
        Self::new(name, Mode::Pages(vec![strings(ids)]))
    }

    /// Items' URLs under `base`, allowed by the guard when their host is in `hosts`.
    pub fn serving(mut self, base: &str, hosts: &'static [&'static str]) -> Self {
        self.base = base.to_owned();
        self.hosts = hosts;
        self
    }

    /// Items as [`ImageRef::Page`] links that `resolve_image` turns into the image URL.
    pub fn page_images(mut self) -> Self {
        self.page_image = true;
        self
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn resolves(&self) -> usize {
        self.resolves.load(Ordering::SeqCst)
    }

    pub fn cursors(&self) -> Vec<Option<String>> {
        self.cursors.lock().unwrap().clone()
    }

    fn item(&self, id: &str) -> RemoteItem {
        let url = format!("{}/img/{id}.jpg", self.base);
        RemoteItem {
            remote_id: id.to_owned(),
            preview_url: format!("{}/preview/{id}.jpg", self.base),
            image: if self.page_image {
                ImageRef::Page(format!("{}/w/{id}", self.base))
            } else {
                ImageRef::Direct(url)
            },
            source_url: format!("{}/w/{id}", self.base),
            title: format!("title {id}"),
            author: format!("author {id}"),
            width: Some(1920),
            height: Some(1080),
            nsfw: id.starts_with("nsfw"),
        }
    }
}

#[async_trait]
impl Provider for Scripted {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        Ok(Vec::new())
    }

    fn browse_support(&self) -> BrowseSupport {
        BrowseSupport::Paged
    }

    fn media_hosts(&self) -> &'static [&'static str] {
        self.hosts
    }

    async fn browse(
        &self,
        _query: &str,
        cursor: Option<&BrowseCursor>,
    ) -> Result<BrowseBatch, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.cursors
            .lock()
            .unwrap()
            .push(cursor.map(|c| c.0.clone()));
        match &self.mode {
            Mode::Pages(pages) => {
                let n: usize = cursor.map_or(0, |c| c.0.parse().unwrap());
                let ids = pages.get(n).cloned().unwrap_or_default();
                Ok(BrowseBatch {
                    items: ids.iter().map(|id| self.item(id)).collect(),
                    next: (n + 1 < pages.len()).then(|| BrowseCursor((n + 1).to_string())),
                })
            }
            Mode::RateLimited(retry_after_secs) => Err(ProviderError::RateLimitedFor {
                message: format!("{} api: rate limited (status 429)", self.name),
                retry_after_secs: *retry_after_secs,
            }),
            Mode::Hang => std::future::pending().await,
            Mode::NotConfigured => Err(ProviderError::NotConfigured(format!(
                "{} requires access_key",
                self.name
            ))),
        }
    }

    async fn resolve_image(&self, item: &RemoteItem) -> Result<String, ProviderError> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        match &item.image {
            ImageRef::Direct(url) => Ok(url.clone()),
            ImageRef::Page(page) => Ok(page.replace("/w/", "/img/") + ".jpg"),
        }
    }
}

pub fn strings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

/// `prefix1..=prefixN`.
pub fn ids(prefix: &str, n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("{prefix}{i}")).collect()
}

#[expect(dead_code, reason = "used by browse_use.rs (Task 13)")]
pub struct Browsing {
    _dir: TempDir,
    pub root: PathBuf,
    pub setter: RecordingSetter,
    pub engine: Engine,
}

#[expect(dead_code, reason = "used by browse_use.rs (Task 13)")]
impl Browsing {
    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn favorites(&self) -> PathBuf {
        self.root.join("favorites")
    }

    pub fn previews(&self) -> PathBuf {
        self.root.join("previews")
    }
}

/// Changes the test config before the engine starts (gets the temp root).
type ConfigEdit = Box<dyn FnOnce(&mut Config, &Path)>;

/// Builds a browsing engine.
pub struct BrowseBuilder {
    sources: Vec<(&'static str, Arc<dyn Provider>)>,
    internet: bool,
    limits: BrowseLimits,
    guard: GuardConfig,
    hosts: Vec<&'static str>,
    browse: bool,
    edit: Option<ConfigEdit>,
}

#[expect(dead_code, reason = "used by browse_use.rs (Task 13)")]
impl BrowseBuilder {
    pub fn new(sources: Vec<(&'static str, Arc<dyn Provider>)>) -> Self {
        Self {
            sources,
            internet: true,
            limits: BrowseLimits::default(),
            guard: GuardConfig::strict(),
            hosts: Vec::new(),
            browse: true,
            edit: None,
        }
    }

    pub fn internet(mut self, on: bool) -> Self {
        self.internet = on;
        self
    }

    pub fn limits(mut self, limits: BrowseLimits) -> Self {
        self.limits = limits;
        self
    }

    /// A `permissive_loopback` guarded client allowed to follow redirects to `hosts`.
    pub fn loopback(mut self, hosts: &[&'static str]) -> Self {
        self.guard = GuardConfig::permissive_loopback();
        self.hosts = hosts.to_vec();
        self
    }

    /// No [`BrowseDeps`] at all.
    pub fn without_browse(mut self) -> Self {
        self.browse = false;
        self
    }

    pub fn edit(mut self, edit: impl FnOnce(&mut Config, &Path) + 'static) -> Self {
        self.edit = Some(Box::new(edit));
        self
    }

    pub fn build(self) -> Browsing {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut cfg = Config::default();
        cfg.daemon.download_dir = root.join("downloads");
        cfg.daemon.trash_dir = root.join("trash");
        cfg.favorites.dir = root.join("favorites");
        cfg.daemon.internet = self.internet;
        cfg.daemon.download_limit_mb_enabled = false;
        cfg.desktop.fit = Fit::Center;
        cfg.sources = Vec::new();
        if let Some(edit) = self.edit {
            edit(&mut cfg, &root);
        }
        let set = SourceSet::fake(
            self.sources
                .iter()
                .map(|(id, p)| (*id, Arc::clone(p), ""))
                .collect(),
        );
        let setter = RecordingSetter::new();
        let platform = Platform {
            setter: Arc::new(setter.clone()),
            signals: Arc::new(ManualSignals::new(Theme::Light)),
        };
        let mut deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
            .with_sources(set)
            .with_clock(crate::common::now);
        if self.browse {
            let http = kakemono_sources::guarded_client(self.guard, &self.hosts).unwrap();
            deps = deps.with_browse(
                BrowseDeps::new(http, self.guard, root.join("previews")).with_limits(self.limits),
            );
        }
        let engine = Engine::new(cfg, State::new(), root.join("state.json"), deps).unwrap();
        Browsing {
            _dir: dir,
            root,
            setter,
            engine,
        }
    }
}

fn scripted(p: &Arc<Scripted>) -> Arc<dyn Provider> {
    Arc::clone(p) as Arc<dyn Provider>
}

fn remote_ids(data: &BrowseData) -> Vec<String> {
    data.items.iter().map(|i| i.remote_id.clone()).collect()
}

fn state_of<'a>(data: &'a BrowseData, id: &str) -> &'a BrowseSourceState {
    &data
        .sources
        .iter()
        .find(|s| s.source_id == id)
        .unwrap()
        .state
}

/// `a` with 12 items and `b` with 3, in one batch each.
fn two_sources() -> (Arc<Scripted>, Arc<Scripted>, Browsing) {
    let a = Arc::new(Scripted::new("wallhaven", Mode::Pages(vec![ids("a", 12)])));
    let b = Arc::new(Scripted::new("bing", Mode::Pages(vec![ids("b", 3)])));
    let f = BrowseBuilder::new(vec![("wh-1", scripted(&a)), ("bing-1", scripted(&b))]).build();
    (a, b, f)
}

#[tokio::test]
async fn page_zero_interleaves_sources_round_robin() {
    let (_a, _b, f) = two_sources();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    assert_eq!(
        remote_ids(&page),
        strings(&["a1", "b1", "a2", "b2", "a3", "b3", "a4", "a5", "a6", "a7"])
    );
}

#[tokio::test]
async fn items_carry_source_metadata_and_no_preview_when_refused() {
    let (_a, _b, f) = two_sources();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    let b1 = &page.items[1];
    assert_eq!(
        (
            b1.source_id.as_str(),
            b1.kind.as_str(),
            b1.title.as_str(),
            b1.author.as_str(),
            b1.width,
            b1.preview.clone()
        ),
        ("bing-1", "bing", "title b1", "author b1", Some(1920), None)
    );
}

#[tokio::test]
async fn preview_dir_is_the_absolute_preview_root() {
    let (_a, _b, f) = two_sources();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    assert_eq!(Path::new(&page.preview_dir), f.previews());
}

#[tokio::test]
async fn second_page_continues_after_the_first() {
    let (_a, _b, f) = two_sources();
    let first = f.engine.browse(None, 0, None).await.unwrap();
    let second = f
        .engine
        .browse(Some(&first.session), 1, None)
        .await
        .unwrap();
    assert_eq!(
        (remote_ids(&second), second.page, second.restarted),
        (strings(&["a8", "a9", "a10", "a11", "a12"]), 1, false)
    );
}

#[tokio::test]
async fn asking_for_a_page_again_returns_it_without_provider_calls() {
    let (a, b, f) = two_sources();
    let first = f.engine.browse(None, 0, None).await.unwrap();
    let calls = (a.calls(), b.calls());
    let again = f
        .engine
        .browse(Some(&first.session), 0, None)
        .await
        .unwrap();
    assert_eq!(
        (
            remote_ids(&again),
            again.session.clone(),
            (a.calls(), b.calls())
        ),
        (remote_ids(&first), first.session.clone(), calls)
    );
}

#[tokio::test]
async fn a_page_past_the_next_one_is_out_of_range() {
    let (_a, _b, f) = two_sources();
    let first = f.engine.browse(None, 0, None).await.unwrap();
    let err = f
        .engine
        .browse(Some(&first.session), 5, None)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            BrowseError::PageOutOfRange {
                page: 5,
                produced: 1
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn repeated_items_across_batches_are_dropped() {
    let a = Arc::new(Scripted::new(
        "wallhaven",
        Mode::Pages(vec![
            strings(&["x1", "x2", "x3"]),
            strings(&["x2", "x3", "x4"]),
            strings(&["x1", "x5"]),
        ]),
    ));
    let f = BrowseBuilder::new(vec![("wh-1", scripted(&a))]).build();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    assert_eq!(remote_ids(&page), strings(&["x1", "x2", "x3", "x4", "x5"]));
}

#[tokio::test]
async fn a_source_refills_with_at_most_three_calls_per_page() {
    let a = Arc::new(Scripted::new(
        "wallhaven",
        Mode::Pages((0..10).map(|n| vec![format!("p{n}")]).collect()),
    ));
    let f = BrowseBuilder::new(vec![("wh-1", scripted(&a))]).build();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    assert_eq!(
        (remote_ids(&page), a.cursors()),
        (
            strings(&["p0", "p1", "p2"]),
            vec![None, Some("1".to_owned()), Some("2".to_owned())]
        )
    );
}

#[tokio::test]
async fn a_rate_limited_source_is_reported_and_skipped() {
    let limited = Arc::new(Scripted::new("unsplash", Mode::RateLimited(Some(120))));
    let b = Arc::new(Scripted::new("bing", Mode::Pages(vec![ids("b", 12)])));
    let f =
        BrowseBuilder::new(vec![("us-1", scripted(&limited)), ("bing-1", scripted(&b))]).build();
    let first = f.engine.browse(None, 0, None).await.unwrap();
    f.engine
        .browse(Some(&first.session), 1, None)
        .await
        .unwrap();
    assert_eq!(
        (
            first.items.len(),
            state_of(&first, "us-1").clone(),
            limited.calls()
        ),
        (
            10,
            BrowseSourceState::RateLimited {
                message: "unsplash api: rate limited (status 429)".to_owned(),
                retry_after_secs: Some(120),
            },
            1
        )
    );
}

#[tokio::test]
async fn a_not_configured_source_is_reported() {
    let a = Arc::new(Scripted::new("unsplash", Mode::NotConfigured));
    let f = BrowseBuilder::new(vec![("us-1", scripted(&a))]).build();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    assert_eq!(
        (state_of(&page, "us-1").clone(), page.done),
        (
            BrowseSourceState::NotConfigured {
                message: "unsplash requires access_key".to_owned()
            },
            true
        )
    );
}

#[tokio::test]
async fn a_source_that_cannot_browse_is_unsupported() {
    let fake: Arc<dyn Provider> = Arc::new(kakemono_sources::fake::StaticProvider::new(
        "apod",
        Vec::new(),
    ));
    let f = BrowseBuilder::new(vec![("apod-1", fake)]).build();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    assert_eq!(
        (state_of(&page, "apod-1").clone(), page.done),
        (BrowseSourceState::Unsupported, true)
    );
}

#[tokio::test(start_paused = true)]
async fn a_hanging_source_does_not_block_the_page() {
    let hang = Arc::new(Scripted::new("reddit", Mode::Hang));
    let b = Arc::new(Scripted::new("bing", Mode::Pages(vec![ids("b", 3)])));
    let f = BrowseBuilder::new(vec![("rd-1", scripted(&hang)), ("bing-1", scripted(&b))]).build();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    assert_eq!(
        (
            remote_ids(&page),
            matches!(state_of(&page, "rd-1"), BrowseSourceState::Failed { .. })
        ),
        (strings(&["b1", "b2", "b3"]), true)
    );
}

#[tokio::test]
async fn exhausting_every_source_is_done() {
    let (_a, _b, f) = two_sources();
    let first = f.engine.browse(None, 0, None).await.unwrap();
    let second = f
        .engine
        .browse(Some(&first.session), 1, None)
        .await
        .unwrap();
    assert_eq!(
        (
            first.done,
            second.done,
            state_of(&second, "wh-1").clone(),
            state_of(&first, "wh-1").clone()
        ),
        (
            false,
            true,
            BrowseSourceState::Exhausted,
            BrowseSourceState::Active
        )
    );
}

#[tokio::test]
async fn no_online_source_is_an_empty_done_page() {
    let f = BrowseBuilder::new(Vec::new()).build();
    let page = f.engine.browse(None, 0, None).await.unwrap();
    assert_eq!(
        (
            page.items.len(),
            page.sources.len(),
            page.done,
            page.restarted
        ),
        (0, 0, true, false)
    );
}

#[tokio::test]
async fn an_unknown_session_restarts_at_page_zero() {
    let (_a, _b, f) = two_sources();
    let page = f.engine.browse(Some("nope"), 3, None).await.unwrap();
    assert_eq!((page.restarted, page.page, page.items.len()), (true, 0, 10));
}

#[tokio::test]
async fn the_source_filter_browses_one_source() {
    let (a, _b, f) = two_sources();
    let page = f.engine.browse(None, 0, Some("bing-1")).await.unwrap();
    assert_eq!(
        (remote_ids(&page), page.sources.len(), a.calls()),
        (strings(&["b1", "b2", "b3"]), 1, 0)
    );
}

#[tokio::test]
async fn an_unknown_source_filter_is_an_error() {
    let (_a, _b, f) = two_sources();
    let err = f.engine.browse(None, 0, Some("gone")).await.unwrap_err();
    assert!(
        matches!(&err, BrowseError::UnknownSource(id) if id == "gone"),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_changed_filter_restarts_the_session() {
    let (_a, _b, f) = two_sources();
    let all = f.engine.browse(None, 0, None).await.unwrap();
    let filtered = f
        .engine
        .browse(Some(&all.session), 1, Some("bing-1"))
        .await
        .unwrap();
    assert_eq!(
        (filtered.restarted, filtered.page, remote_ids(&filtered)),
        (true, 0, strings(&["b1", "b2", "b3"]))
    );
}

#[tokio::test]
async fn internet_off_refuses_to_browse() {
    let (a, b) = (
        Arc::new(Scripted::one_batch("wallhaven", &["a1"])),
        Arc::new(Scripted::one_batch("bing", &["b1"])),
    );
    let f = BrowseBuilder::new(vec![("wh-1", scripted(&a)), ("bing-1", scripted(&b))])
        .internet(false)
        .build();
    let err = f.engine.browse(None, 0, None).await.unwrap_err();
    assert!(matches!(err, BrowseError::InternetOff), "{err:?}");
}

#[tokio::test]
async fn without_browse_deps_browsing_is_not_available() {
    let f = BrowseBuilder::new(Vec::new()).without_browse().build();
    let err = f.engine.browse(None, 0, None).await.unwrap_err();
    assert!(matches!(err, BrowseError::NotAvailable), "{err:?}");
}

#[tokio::test]
async fn reload_clears_sessions() {
    let (_a, _b, f) = two_sources();
    let first = f.engine.browse(None, 0, None).await.unwrap();
    f.engine.reload((*f.engine.config()).clone()).await.unwrap();
    let after = f
        .engine
        .browse(Some(&first.session), 1, None)
        .await
        .unwrap();
    assert_eq!((after.restarted, after.page), (true, 0));
}

#[tokio::test]
async fn reload_restarts_paging_from_the_first_batch() {
    let a = Arc::new(Scripted::new(
        "wallhaven",
        Mode::Pages(vec![ids("a", 10), ids("x", 10)]),
    ));
    let f = BrowseBuilder::new(vec![("wh-1", scripted(&a))]).build();
    let first = f.engine.browse(None, 0, None).await.unwrap();
    f.engine.reload((*f.engine.config()).clone()).await.unwrap();
    f.engine
        .browse(Some(&first.session), 1, None)
        .await
        .unwrap();
    assert_eq!(a.cursors(), vec![None, None]);
}

#[tokio::test]
async fn at_most_four_sessions_are_kept() {
    let (_a, _b, f) = two_sources();
    let oldest = f.engine.browse(None, 0, None).await.unwrap();
    for _ in 0..4 {
        f.engine.browse(None, 0, None).await.unwrap();
    }
    let again = f
        .engine
        .browse(Some(&oldest.session), 1, None)
        .await
        .unwrap();
    assert!(again.restarted);
}

#[tokio::test]
async fn the_most_recently_used_session_survives_eviction() {
    let (_a, _b, f) = two_sources();
    let kept = f.engine.browse(None, 0, None).await.unwrap();
    for _ in 0..3 {
        f.engine.browse(None, 0, None).await.unwrap();
    }
    f.engine.browse(Some(&kept.session), 0, None).await.unwrap();
    f.engine.browse(None, 0, None).await.unwrap();
    let again = f.engine.browse(Some(&kept.session), 1, None).await.unwrap();
    assert!(!again.restarted);
}

#[tokio::test(start_paused = true)]
async fn an_idle_session_expires() {
    let (_a, _b, f) = two_sources();
    let first = f.engine.browse(None, 0, None).await.unwrap();
    tokio::time::advance(BrowseLimits::default().idle_ttl + std::time::Duration::from_secs(1))
        .await;
    let again = f
        .engine
        .browse(Some(&first.session), 1, None)
        .await
        .unwrap();
    assert!(again.restarted);
}

#[test]
fn default_limits_match_the_plan() {
    let limits = BrowseLimits::default();
    assert_eq!(
        (
            limits.sessions,
            limits.index_cap,
            limits.idle_ttl.as_secs(),
            limits.source_deadline.as_secs(),
            limits.total.as_secs(),
            limits.max_calls
        ),
        (4, 2000, 30 * 60, 8, 20, 3)
    );
}

#[tokio::test]
async fn the_error_texts_are_pinned() {
    let texts = [
        BrowseError::InternetOff.to_string(),
        BrowseError::NotAvailable.to_string(),
        BrowseError::UnknownSource("wh-1".into()).to_string(),
        BrowseError::PageOutOfRange {
            page: 5,
            produced: 1,
        }
        .to_string(),
        BrowseError::ItemExpired.to_string(),
    ];
    assert_eq!(
        texts,
        [
            "Internet access is off",
            "browsing is not available in this daemon",
            "no enabled online source \"wh-1\"",
            "browse page 5 is out of range: the session has 1 page(s)",
            "That preview expired — scroll or reopen to refresh.",
        ]
    );
}
