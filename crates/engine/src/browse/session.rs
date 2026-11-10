//! Browse sessions and [`Engine::browse`] (`docs/plans/picker-browse-impl.md` Task 12).
//!
//! A session interleaves its online sources round-robin into pages of [`BROWSE_PAGE`] items.
//! Each source keeps its provider's cursor, a buffer of items not yet shown and the remote ids
//! it has seen, so an item a provider repeats (Bing's overlapping archive windows, Wallhaven's
//! random scrape) is shown once. Provider cursors never leave the daemon.
//!
//! - **Idempotent pages.** Page `k` is returned from the session when it was already produced,
//!   produced when `k` is the next page, and [`BrowseError::PageOutOfRange`] past that. A UI
//!   retry after a timeout therefore never skips or reorders items.
//! - **Refill.** Producing a page tops up every active source's buffer to a full page, with at
//!   most [`BrowseLimits::max_calls`] provider calls per source, all sources at once, under one
//!   [`BrowseLimits::source_deadline`]. A source that misses it is `Failed` for this page; a
//!   rate-limited one is skipped until its `Retry-After` (or [`RATE_LIMIT_FALLBACK`]) passed.
//! - **Done.** A page is `done` when no source has buffered items or can fetch more.
//! - **Limits.** At most [`BrowseLimits::sessions`] sessions (least recently used evicted), idle
//!   ones expire after [`BrowseLimits::idle_ttl`]. Sessions are dropped when the engine's
//!   sources are rebuilt ([`Engine::reload`]), so a changed source never reuses a cursor of its
//!   old config; the next request restarts at page 0.
//! - **Item index.** Every item shown is remembered by `(source_id, remote_id)`, at most
//!   [`BrowseLimits::index_cap`] (least recently used evicted), for `browse_use`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::{StreamExt, future, stream};
use kakemono_config::schema::SourceType;
use kakemono_protocol::types::{BrowseData, BrowseItem, BrowseSourceState, BrowseSourceStatus};
use kakemono_sources::browse::{BROWSE_PAGE, BrowseCursor, BrowseSupport, RemoteItem};
use kakemono_sources::{OnlineSource, ProviderError, SourceSet};
use tokio::time::Instant;

use super::preview::{PreviewCache, PreviewJob};
use crate::engine::{BrowseDeps, BrowseError, Engine};

/// How long a rate-limited source is skipped when the server gave no `Retry-After`.
pub const RATE_LIMIT_FALLBACK: Duration = Duration::from_secs(60);

/// Bounds of browsing. [`Default`] is the plan's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowseLimits {
    /// How long one page's refill waits for each source (8 s).
    pub source_deadline: Duration,
    /// How long one `browse` call may take in all, previews included (20 s, under the IPC
    /// client's 30 s timeout).
    pub total: Duration,
    /// Provider calls per source per page (3).
    pub max_calls: usize,
    /// Sessions kept (4).
    pub sessions: usize,
    /// Idle time after which a session expires (30 min).
    pub idle_ttl: Duration,
    /// Items kept for `browse_use` (2000).
    pub index_cap: usize,
}

impl Default for BrowseLimits {
    fn default() -> Self {
        Self {
            source_deadline: Duration::from_secs(8),
            total: Duration::from_secs(20),
            max_calls: 3,
            sessions: 4,
            idle_ttl: Duration::from_secs(30 * 60),
            index_cap: 2000,
        }
    }
}

/// One browsed image, as `browse_use` finds it again.
#[derive(Debug)]
pub(crate) struct IndexedItem {
    /// The `[[sources]]` row id.
    pub(crate) source_id: String,
    /// The row's type when the item was browsed.
    pub(crate) kind: SourceType,
    /// The provider's media hosts, for the preview's guard check.
    pub(crate) hosts: &'static [&'static str],
    pub(crate) item: RemoteItem,
}

type ItemKey = (String, String);

#[derive(Debug)]
struct IndexEntry {
    used: u64,
    item: Arc<IndexedItem>,
    /// Where `browse_use` saved the full image.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by browse_use (Task 13)"))]
    downloaded: Option<PathBuf>,
}

/// Browsed items by `(source_id, remote_id)`, least recently used evicted past the cap.
#[derive(Debug, Default)]
pub(crate) struct ItemIndex {
    entries: HashMap<ItemKey, IndexEntry>,
    tick: u64,
}

impl ItemIndex {
    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// Remembers `item` (a newer copy replaces the item but keeps its download), then evicts
    /// the least recently used entries past `cap`.
    pub(crate) fn insert(&mut self, item: &Arc<IndexedItem>, cap: usize) {
        let used = self.next_tick();
        let key = (item.source_id.clone(), item.item.remote_id.clone());
        let entry = self.entries.entry(key).or_insert_with(|| IndexEntry {
            used,
            item: Arc::clone(item),
            downloaded: None,
        });
        entry.used = used;
        entry.item = Arc::clone(item);
        while self.entries.len() > cap.max(1) {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// The item and where it was downloaded, marking it used.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by browse_use (Task 13)"))]
    pub(crate) fn get(
        &mut self,
        source_id: &str,
        remote_id: &str,
    ) -> Option<(Arc<IndexedItem>, Option<PathBuf>)> {
        let used = self.next_tick();
        let entry = self
            .entries
            .get_mut(&(source_id.to_owned(), remote_id.to_owned()))?;
        entry.used = used;
        Some((Arc::clone(&entry.item), entry.downloaded.clone()))
    }

    /// Records where the item was downloaded (no-op once it was evicted).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by browse_use (Task 13)"))]
    pub(crate) fn set_downloaded(&mut self, source_id: &str, remote_id: &str, path: PathBuf) {
        if let Some(entry) = self
            .entries
            .get_mut(&(source_id.to_owned(), remote_id.to_owned()))
        {
            entry.downloaded = Some(path);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// What one source is doing.
#[derive(Debug, Clone)]
enum CursorState {
    Active,
    Unsupported,
    NotConfigured(String),
    RateLimited {
        message: String,
        retry_after_secs: Option<u64>,
        until: Instant,
    },
    Failed(String),
}

/// One source of a session.
struct SourceCursor {
    source: OnlineSource,
    /// The provider's cursor for the next batch; `None` before the first.
    cursor: Option<BrowseCursor>,
    /// The provider said there is no next batch.
    ended: bool,
    /// Items fetched but not shown yet.
    buffer: VecDeque<RemoteItem>,
    /// Remote ids already shown or buffered.
    seen: HashSet<String>,
    state: CursorState,
}

impl SourceCursor {
    fn new(source: OnlineSource) -> Self {
        let state = if source.provider.browse_support() == BrowseSupport::Unsupported {
            CursorState::Unsupported
        } else {
            CursorState::Active
        };
        Self {
            source,
            cursor: None,
            ended: false,
            buffer: VecDeque::new(),
            seen: HashSet::new(),
            state,
        }
    }

    /// Before a page: a failed source tries again, a rate limit that ran out is lifted.
    fn wake(&mut self, now: Instant) {
        match &self.state {
            CursorState::Failed(_) => self.state = CursorState::Active,
            CursorState::RateLimited { until, .. } if *until <= now => {
                self.state = CursorState::Active;
            }
            _ => {}
        }
    }

    fn can_fetch(&self) -> bool {
        matches!(self.state, CursorState::Active) && !self.ended
    }

    /// Whether this source may still give items.
    fn has_more(&self) -> bool {
        !self.buffer.is_empty() || self.can_fetch()
    }

    /// Fetches batches until `want` items are buffered, the provider ends, `max_calls` calls
    /// were made, or `deadline` passed.
    async fn refill(&mut self, want: usize, max_calls: usize, deadline: Instant) {
        let mut calls = 0;
        while self.can_fetch() && self.buffer.len() < want && calls < max_calls {
            calls += 1;
            let provider = Arc::clone(&self.source.provider);
            let call = provider.browse(&self.source.query, self.cursor.as_ref());
            match tokio::time::timeout_at(deadline, call).await {
                Err(_) => {
                    self.state =
                        CursorState::Failed(format!("{}: no answer in time", provider.name()));
                }
                Ok(Err(err)) => {
                    tracing::debug!(source_id = %self.source.id, error = %err, "browse failed");
                    self.state = failed_state(err, Instant::now());
                }
                Ok(Ok(batch)) => {
                    for item in batch.items {
                        if self.seen.insert(item.remote_id.clone()) {
                            self.buffer.push_back(item);
                        }
                    }
                    match batch.next {
                        Some(next) => self.cursor = Some(next),
                        None => self.ended = true,
                    }
                }
            }
        }
    }

    fn status(&self) -> BrowseSourceStatus {
        let state = match &self.state {
            CursorState::Active if self.ended && self.buffer.is_empty() => {
                BrowseSourceState::Exhausted
            }
            CursorState::Active => BrowseSourceState::Active,
            CursorState::Unsupported => BrowseSourceState::Unsupported,
            CursorState::NotConfigured(message) => BrowseSourceState::NotConfigured {
                message: message.clone(),
            },
            CursorState::RateLimited {
                message,
                retry_after_secs,
                ..
            } => BrowseSourceState::RateLimited {
                message: message.clone(),
                retry_after_secs: *retry_after_secs,
            },
            CursorState::Failed(message) => BrowseSourceState::Failed {
                message: message.clone(),
            },
        };
        BrowseSourceStatus {
            source_id: self.source.id.clone(),
            kind: self.source.kind.as_str().to_owned(),
            state,
        }
    }
}

/// The state a failed `browse` call leaves its source in.
fn failed_state(err: ProviderError, now: Instant) -> CursorState {
    if err.is_rate_limited() {
        let retry_after_secs = err.retry_after_secs();
        let wait = retry_after_secs.map_or(RATE_LIMIT_FALLBACK, Duration::from_secs);
        return CursorState::RateLimited {
            message: err.to_string(),
            retry_after_secs,
            until: now + wait,
        };
    }
    match err {
        ProviderError::Unsupported(_) => CursorState::Unsupported,
        ProviderError::NotConfigured(message) => CursorState::NotConfigured(message),
        other => CursorState::Failed(other.to_string()),
    }
}

/// One produced page.
struct Page {
    items: Vec<Arc<IndexedItem>>,
    done: bool,
}

/// One browse session: its sources and the pages produced so far.
struct Session {
    sources: Vec<SourceCursor>,
    pages: Vec<Page>,
}

impl Session {
    fn new(sources: &SourceSet, filter: Option<&str>) -> Self {
        Self {
            sources: sources
                .online
                .iter()
                .filter(|s| filter.is_none_or(|id| s.id == id))
                .cloned()
                .map(SourceCursor::new)
                .collect(),
            pages: Vec::new(),
        }
    }

    /// Page `page`: the produced one, or the next one produced now.
    async fn page(
        &mut self,
        page: u32,
        limits: &BrowseLimits,
    ) -> Result<(Vec<Arc<IndexedItem>>, bool), BrowseError> {
        let produced = self.pages.len();
        let wanted = usize::try_from(page).unwrap_or(usize::MAX);
        if wanted > produced {
            return Err(BrowseError::PageOutOfRange {
                page,
                produced: u32::try_from(produced).unwrap_or(u32::MAX),
            });
        }
        if wanted == produced {
            self.produce(limits).await;
        }
        let page = &self.pages[wanted];
        Ok((page.items.clone(), page.done))
    }

    /// Refills the sources and takes the next page round-robin.
    async fn produce(&mut self, limits: &BrowseLimits) {
        let now = Instant::now();
        for source in &mut self.sources {
            source.wake(now);
        }
        let deadline = now + limits.source_deadline;
        future::join_all(
            self.sources
                .iter_mut()
                .map(|s| s.refill(BROWSE_PAGE, limits.max_calls, deadline)),
        )
        .await;

        let mut items = Vec::with_capacity(BROWSE_PAGE);
        'rounds: loop {
            let mut took = false;
            for source in &mut self.sources {
                if items.len() == BROWSE_PAGE {
                    break 'rounds;
                }
                if let Some(item) = source.buffer.pop_front() {
                    took = true;
                    items.push(Arc::new(IndexedItem {
                        source_id: source.source.id.clone(),
                        kind: source.source.kind,
                        hosts: source.source.provider.media_hosts(),
                        item,
                    }));
                }
            }
            if !took {
                break;
            }
        }
        let done = !self.sources.iter().any(SourceCursor::has_more);
        self.pages.push(Page { items, done });
    }

    fn statuses(&self) -> Vec<BrowseSourceStatus> {
        self.sources.iter().map(SourceCursor::status).collect()
    }
}

/// A session in the browser's list.
struct Slot {
    id: String,
    filter: Option<String>,
    last_used: Instant,
    session: Arc<tokio::sync::Mutex<Session>>,
}

/// What [`Browser::open`] found or created.
struct Opened {
    id: String,
    session: Arc<tokio::sync::Mutex<Session>>,
    /// The page to serve: the asked one, or 0 for a new session.
    page: u32,
    restarted: bool,
    created: bool,
}

#[derive(Default)]
struct BrowserState {
    /// The source set the sessions were built from; a different one drops them all.
    sources: Option<Arc<SourceSet>>,
    /// Least recently used first.
    sessions: Vec<Slot>,
    index: ItemIndex,
}

/// Browsing set up from [`BrowseDeps`].
pub(crate) struct Ready {
    pub(crate) deps: BrowseDeps,
    pub(crate) previews: PreviewCache,
}

/// The engine's browse sessions and item index. Held by the engine.
pub(crate) struct Browser {
    ready: Option<Ready>,
    state: Mutex<BrowserState>,
}

impl Browser {
    /// A browser over `deps`; `None` makes every browse call [`BrowseError::NotAvailable`].
    pub(crate) fn new(deps: Option<BrowseDeps>) -> Self {
        let ready = deps.map(|mut deps| {
            deps.preview_root = std::path::absolute(&deps.preview_root)
                .unwrap_or_else(|_| deps.preview_root.clone());
            if let Err(err) = std::fs::create_dir_all(&deps.preview_root) {
                tracing::warn!(dir = %deps.preview_root.display(), "can't create the preview folder: {err}");
            }
            let previews = PreviewCache::new(deps.preview_root.clone(), deps.http.clone(), deps.guard)
                .with_limits(deps.preview_limits);
            Ready { deps, previews }
        });
        Self {
            ready,
            state: Mutex::new(BrowserState::default()),
        }
    }

    /// The browse setup.
    pub(crate) fn ready(&self) -> Result<&Ready, BrowseError> {
        self.ready.as_ref().ok_or(BrowseError::NotAvailable)
    }

    fn state(&self) -> MutexGuard<'_, BrowserState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The item index. Never hold it across an `.await`.
    pub(crate) fn index(&self) -> IndexGuard<'_> {
        IndexGuard(self.state())
    }

    /// The session `requested` for `filter` over `sources`, or a new one.
    fn open(
        &self,
        requested: Option<&str>,
        page: u32,
        filter: Option<&str>,
        sources: &Arc<SourceSet>,
        limits: &BrowseLimits,
    ) -> Opened {
        let now = Instant::now();
        let mut state = self.state();
        if !state
            .sources
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, sources))
        {
            state.sessions.clear();
            state.sources = Some(Arc::clone(sources));
        }
        state
            .sessions
            .retain(|s| now.saturating_duration_since(s.last_used) < limits.idle_ttl);
        if let Some(id) = requested
            && let Some(at) = state
                .sessions
                .iter()
                .position(|s| s.id == id && s.filter.as_deref() == filter)
        {
            let mut slot = state.sessions.remove(at);
            slot.last_used = now;
            let opened = Opened {
                id: slot.id.clone(),
                session: Arc::clone(&slot.session),
                page,
                restarted: false,
                created: false,
            };
            state.sessions.push(slot);
            return opened;
        }
        let excess = (state.sessions.len() + 1).saturating_sub(limits.sessions.max(1));
        state.sessions.drain(..excess);
        let slot = Slot {
            id: format!("{:032x}", rand::random::<u128>()),
            filter: filter.map(str::to_owned),
            last_used: now,
            session: Arc::new(tokio::sync::Mutex::new(Session::new(sources, filter))),
        };
        let opened = Opened {
            id: slot.id.clone(),
            session: Arc::clone(&slot.session),
            page: 0,
            restarted: requested.is_some() || page != 0,
            created: true,
        };
        state.sessions.push(slot);
        opened
    }
}

/// The locked item index ([`Browser::index`]).
pub(crate) struct IndexGuard<'a>(MutexGuard<'a, BrowserState>);

impl std::ops::Deref for IndexGuard<'_> {
    type Target = ItemIndex;

    fn deref(&self) -> &ItemIndex {
        &self.0.index
    }
}

impl std::ops::DerefMut for IndexGuard<'_> {
    fn deref_mut(&mut self) -> &mut ItemIndex {
        &mut self.0.index
    }
}

impl Ready {
    /// Each item's preview path (`None` when it failed or `deadline` passed), in order.
    async fn previews(
        &self,
        items: &[Arc<IndexedItem>],
        deadline: Instant,
    ) -> Vec<Option<PathBuf>> {
        stream::iter(items)
            .map(|indexed| async move {
                let job = PreviewJob {
                    source_id: &indexed.source_id,
                    item: &indexed.item,
                    hosts: indexed.hosts,
                };
                tokio::time::timeout_at(deadline, self.previews.fetch(job))
                    .await
                    .ok()
                    .flatten()
            })
            .buffered(self.deps.preview_limits.concurrency.max(1))
            .collect()
            .await
    }

    /// Prunes the preview cache in the background.
    fn prune_soon(&self) {
        let previews = self.previews.clone();
        tokio::spawn(async move {
            match previews.prune().await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(removed = n, "pruned the preview cache"),
                Err(err) => tracing::warn!("can't prune the preview cache: {err}"),
            }
        });
    }
}

impl Engine {
    /// Page `page` of browse session `session`, over every enabled online source or only
    /// `source`. A missing, unknown, expired or differently filtered session starts a new one
    /// and serves its page 0 (`restarted` unless this was a plain first request). Each item's
    /// preview is downloaded into the preview folder first. Takes at most
    /// [`BrowseLimits::total`]; sources still loading at their deadline give a partial page.
    /// See the [module docs](self).
    ///
    /// # Errors
    ///
    /// [`BrowseError::InternetOff`], [`BrowseError::NotAvailable`],
    /// [`BrowseError::UnknownSource`] for a `source` that isn't an enabled online row, and
    /// [`BrowseError::PageOutOfRange`] past the next page.
    pub async fn browse(
        &self,
        session: Option<&str>,
        page: u32,
        source: Option<&str>,
    ) -> Result<BrowseData, BrowseError> {
        let (internet, sources) = {
            let snap = self.snap();
            (snap.cfg.daemon.internet, Arc::clone(&snap.sources))
        };
        if !internet {
            return Err(BrowseError::InternetOff);
        }
        let browser = &self.inner.browser;
        let ready = browser.ready()?;
        let limits = ready.deps.limits;
        let deadline = Instant::now() + limits.total;
        if let Some(id) = source
            && sources.get(id).is_none()
        {
            return Err(BrowseError::UnknownSource(id.to_owned()));
        }
        let opened = browser.open(session, page, source, &sources, &limits);
        if opened.created {
            ready.prune_soon();
        }
        let (items, done, statuses) = {
            let mut session = opened.session.lock().await;
            let (items, done) = session.page(opened.page, &limits).await?;
            (items, done, session.statuses())
        };
        {
            let mut index = browser.index();
            for item in &items {
                index.insert(item, limits.index_cap);
            }
        }
        let previews = ready.previews(&items, deadline).await;
        Ok(BrowseData {
            session: opened.id,
            page: opened.page,
            restarted: opened.restarted,
            done,
            preview_dir: path_text(&ready.deps.preview_root),
            items: items
                .iter()
                .zip(previews)
                .map(|(indexed, preview)| browse_item(indexed, preview.as_deref()))
                .collect(),
            sources: statuses,
        })
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn browse_item(indexed: &IndexedItem, preview: Option<&Path>) -> BrowseItem {
    let item = &indexed.item;
    BrowseItem {
        source_id: indexed.source_id.clone(),
        kind: indexed.kind.as_str().to_owned(),
        remote_id: item.remote_id.clone(),
        title: item.title.clone(),
        author: item.author.clone(),
        width: item.width,
        height: item.height,
        nsfw: item.nsfw,
        preview: preview.map(path_text),
    }
}

#[cfg(test)]
mod tests {
    use kakemono_sources::browse::ImageRef;

    use super::*;

    fn indexed(source_id: &str, remote_id: &str) -> Arc<IndexedItem> {
        Arc::new(IndexedItem {
            source_id: source_id.to_owned(),
            kind: SourceType::Bing,
            hosts: &[],
            item: RemoteItem {
                remote_id: remote_id.to_owned(),
                preview_url: String::new(),
                image: ImageRef::Direct(String::new()),
                source_url: String::new(),
                title: String::new(),
                author: String::new(),
                width: None,
                height: None,
                nsfw: false,
            },
        })
    }

    #[test]
    fn index_holds_at_most_the_cap() {
        let mut index = ItemIndex::default();
        for n in 0..2005 {
            index.insert(&indexed("b", &n.to_string()), 2000);
        }
        assert_eq!(index.len(), 2000);
    }

    #[test]
    fn index_evicts_the_least_recently_used() {
        let mut index = ItemIndex::default();
        index.insert(&indexed("b", "1"), 2);
        index.insert(&indexed("b", "2"), 2);
        index.get("b", "1");
        index.insert(&indexed("b", "3"), 2);
        assert_eq!(
            (
                index.get("b", "1").is_some(),
                index.get("b", "2").is_some(),
                index.get("b", "3").is_some()
            ),
            (true, false, true)
        );
    }

    #[test]
    fn index_keys_include_the_source_id() {
        let mut index = ItemIndex::default();
        index.insert(&indexed("a", "1"), 10);
        assert!(index.get("b", "1").is_none());
    }

    #[test]
    fn reinserting_an_item_keeps_its_download() {
        let mut index = ItemIndex::default();
        index.insert(&indexed("b", "1"), 10);
        index.set_downloaded("b", "1", PathBuf::from("/d/b/1.jpg"));
        index.insert(&indexed("b", "1"), 10);
        assert_eq!(
            index.get("b", "1").and_then(|(_, path)| path),
            Some(PathBuf::from("/d/b/1.jpg"))
        );
    }

    #[test]
    fn a_rate_limit_without_hint_waits_the_fallback() {
        let now = Instant::now();
        let state = failed_state(ProviderError::RateLimited("slow down".into()), now);
        assert!(
            matches!(&state, CursorState::RateLimited { until, retry_after_secs: None, .. } if *until == now + RATE_LIMIT_FALLBACK),
            "{state:?}"
        );
    }
}
