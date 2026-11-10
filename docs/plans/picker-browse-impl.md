# Picker browse + tabs — implementation plan

Status: approved, not started. Merged from the four-planner run `20261110-154947-plan`: Plan A is the base, with additions from Plans B, C and D (listed under "Merge notes").
Evidence: `.pi/research/picker-browse.md` (local, git-ignored), cited as "R §n".

## Decisions (fixed by the user)

1. The picker has five tabs: **Sources | Downloaded | Local | Favorites | Cache**. It always opens on Sources.
2. **Sources** shows remote previews from the enabled online sources, 10 per page, and loads the next page on scroll. Sources are interleaved. When more than one online source exists, a source filter appears. Nothing full-size downloads until the user acts: Set as wallpaper, Add to Favorites or Download fetches the full image into `download_dir/<id>/`.
3. **Downloaded** lists enabled online source folders plus `manual_dir`. **Local** lists the local folder, album and images sources. **Favorites** lists the favorites dir. **Cache** lists loose files at the root of `download_dir` plus `download_dir/<id>/` folders whose id is not an enabled online source.
4. The current wallpaper is pinned first and highlighted in whichever on-disk tab contains it. History is no longer a separate block.
5. **NSFW:**
   - Wallhaven follows the configured `purity`. Items with purity other than `sfw` are blurred until hover or focus.
   - A new setting, `[picker] blur_nsfw` (default `true`), turns the blur off.
   - Reddit `over_18` posts are always dropped from browse.
   - Unsplash, Bing and APOD items are never flagged.
6. **Tab keys:**
   - The cross-axis arrows switch tabs and wrap around: Up/Down in the horizontal strip, Left/Right in the vertical one.
   - `1`–`5` jump straight to a tab.
   - `[` and `]` cycle the Sources source filter.
   - `Tab` keeps its native focus behaviour.
7. Browse is allowed when `[daemon] download_enabled = false`, because browsing is user-initiated (like `download_now`). Only `[daemon] internet = false` blocks it.

## Summary

The on-disk tabs (Downloaded, Local, Favorites, Cache) are per-tab versions of today's `thumbnails` listing, paged 40 at a time. The current wallpaper is pinned first in the tab that contains it. Cache also covers orphan `download_dir/<id>/` folders.

The Sources tab is served by a new daemon-side **browse session**:
- The webview asks for `page N` of a session through a new `browse` request.
- The daemon interleaves the enabled online sources in round-robin order. It pulls batches from a new `Provider::browse(query, cursor)` method, slices them into pages of 10, and removes duplicates.
- It downloads each preview into a capped, SSRF-guarded preview cache (`<cache_dir>/previews/`).
- It returns only metadata, an opaque `(source_id, remote_id)` pair and a local preview path.
- The UI admits those paths into `thumb://`, so the strict CSP is unchanged. An `IntersectionObserver` sentinel loads the next page.

Set, Favorite and Download send `browse_use { source_id, remote_id, action }`. The daemon looks the item up in its own index; the webview never supplies a URL. The daemon then:
1. Downloads the full image into `download_dir/<id>/` under `SourceLocks`, using the guarded client.
2. Records the seen ledger.
3. Sets the wallpaper with real provenance (`source` = kind, `source_id` = id), or copies the file to favorites.

## Architecture

### Data flow: scroll → daemon → provider → preview cache → thumb:// → webview

```text
BrowseStrip (React)
  sentinel visible / keyboard near end ──► api.browsePage({session, page, source})
                                              │ tauri invoke "browse_page"
crates/ui/src-tauri commands_sources/browse.rs┤ DaemonLink::call_alone(Request::Browse{..})
                                              ▼
daemon server.rs dispatch ──► Engine::browse(session, page, source)
   engine/src/browse/session.rs
     Session { sources: Vec<SourceCursor>, pages: Vec<Vec<IndexedItem>>, seen_ids }
     produce page N: refill empty buffers concurrently (Provider::browse, fail-fast HTTP,
       8 s per-source deadline), round-robin take ≤10, dedupe (source_id, remote_id)
   engine/src/browse/preview.rs
     for each item (concurrency 4, 6 s each, 4 MiB cap, ≤4096 px, magic-byte sniff):
       guarded client GET preview_url ─► <cache_dir>/previews/<source_id>/<rid>.<ext>
   ◄── BrowseData { session, page, restarted, done, preview_dir,
                    items[{source_id, kind, remote_id, title, author, width, height, nsfw,
                           preview: Option<abs path>}], sources[status] }
UI command: verify each preview path is under preview_dir, thumbs.admit(ListKind::Previews, ..)
   ──► BrowseListing { items[{.., imageUrl: "thumb://localhost/<id>" | null}], .. }
<img src="thumb://..."> ──► thumbs.rs resolve (Previews root) ─► pixel-cap check ─► 384 px JPEG
```

### Set, Favorite or Download on a remote item

```text
click / Enter / f / menu ──► api.browseUse({sourceId, remoteId, action})
   ──► call_alone_with_timeout(120 s, Request::BrowseUse{..})
   ──► Engine::browse_use:
        internet check → index lookup (ItemExpired if gone) → source still enabled?
        → cached download path still on disk? reuse : {
             SourceLocks::lock(id); url = provider.resolve_image(item) (Wallhaven scrape: page fetch)
             guard.check(url, provider.media_hosts()); sources::download(&guarded, url, dir)
             record_seen(dir, url); trim_to_mb(protect new file) }
        → Set: set_file_with(path, Provenance{source: kind, source_id, title, source_url})
          Favorite: copy_unique_into(favorites.dir)   Download: nothing more
   ◄── BrowseUseData { path, wallpaper: Option<WallpaperInfo> }
```

### Key decisions

1. **The daemon owns everything remote.** It holds the provider keys, HTTP clients, rate-limit state and SSRF guard. The UI crate gets no HTTP client. The webview sees only `thumb://` URLs and opaque ids.
2. **Server-side sessions with idempotent page numbers.**
   - `browse { session: None, page: 0 }` creates a session.
   - `page: k` returns cached page `k` if it was already produced, produces it when `k == produced`, and returns `PageOutOfRange` for anything larger.
   - A retry after a UI timeout is therefore safe, and already-shown items never reorder.
   - An unknown or expired session restarts: the response has `restarted: true, page: 0`.
   - Limits: at most 4 sessions (LRU) with a 30 min idle TTL. Sessions are cleared on `Engine::reload`.
3. **Provider cursors never leave the daemon.** `BrowseCursor(String)` is private to each provider: a page number, a seed, a Reddit `after` or an APOD date.
4. **Paging happens in two layers.** Providers return their natural batch (Wallhaven 24, Reddit 25, Bing 8, APOD about 14, Unsplash 10). The session slices batches into pages of 10 and refills as needed, with at most 3 provider calls per source per page.
5. **Fail-fast HTTP for browse.**
   - `http::send_fail_fast` never sleeps on 429/503. It maps both to `ProviderError::RateLimited`, with a hint parsed from `Retry-After`.
   - The session marks that source `RateLimited { until }` and skips it until then.
   - The whole `browse` call is capped at 20 s, under the client's 30 s `REQUEST_TIMEOUT`. Sources still loading at the deadline give a partial page.
6. **SSRF guard** (`crates/sources/src/guard.rs`):
   - Only `https` is allowed. URLs with credentials (`user:pass@`) or an explicit port other than 443 are rejected.
   - IP-literal hosts and `localhost` are rejected.
   - Each provider has a host-suffix allowlist (`Provider::media_hosts()`). A suffix only matches at a label boundary.
   - A custom `reqwest::dns::Resolve` drops loopback, private, link-local, CGNAT, multicast, unspecified and ULA addresses.
   - Redirects are capped at 3 hops, and every hop must pass the same checks.
   - Tests use `GuardConfig::permissive_loopback()` (doc-hidden) so wiremock on `127.0.0.1` works. The daemon always builds the strict guard.
7. **Previews are written to disk instead of sent over IPC** (the NDJSON line cap is 1 MiB).
   - Location: `<cache_dir>/previews/<source_id>/<sanitized remote_id>.<ext>`. An existing file is reused with no network call.
   - Files older than 24 h are pruned at daemon start and on session creation, with a hard cap of 2000 files.
   - The daemon reports `preview_dir`, and the UI admits only paths that canonicalize under it. This still works if the daemon's cache dir differs from the UI's.
8. **Admission is tracked per tab.** `Thumbs::admit(kind: ListKind, roots, paths)` replaces only that kind's set, and `is_admitted` checks the union of all sets. `ListKind = Downloaded | Local | Favorites | Cache | Previews`.
9. **Provenance.**
   - New `Engine::set_file_with(path, Provenance)`.
   - `set_file(path)` now infers provenance: a path under `download_dir/<id>/` of an enabled online source records `source = kind`, `source_id = id`. Picking a file in the Downloaded tab keeps its source too.
10. **Explicit picks skip `[filters]`.** The seen ledger is still recorded, so the background loop won't fetch the same image again.
11. **NSFW.** The daemon sets `nsfw` per item (see Decisions §5), and the picker reads `[picker] blur_nsfw` from config. Reddit `over_18` posts are dropped inside the provider.
12. **`[daemon] internet = false`** makes `browse` and `browse_use` fail with `InternetOff`. The Sources tab then shows "Browsing is off — internet access is disabled in Settings."

### Contracts

**`crates/sources/src/browse.rs` (Task 1)**
```rust
pub const BROWSE_PAGE: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseCursor(pub String);              // provider-private; never serialized to the wire

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageRef { Direct(String), Page(String) } // Page = must resolve (Wallhaven scrape)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteItem {
    pub remote_id: String,          // sanitized: [A-Za-z0-9_-]{1,64}, else "h" + 16-hex hash
    pub preview_url: String,
    pub image: ImageRef,
    pub source_url: String,         // daemon-side only, for history provenance
    pub title: String,
    pub author: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub nsfw: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowseBatch { pub items: Vec<RemoteItem>, pub next: Option<BrowseCursor> } // None = end

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseSupport { Paged, Endless, Finite, Unsupported }
```

**`Provider` additions (`provider.rs`).** These are default methods, so existing implementations still compile:
```rust
fn browse_support(&self) -> BrowseSupport { BrowseSupport::Unsupported }
fn media_hosts(&self) -> &'static [&'static str] { &[] }
async fn browse(&self, query: &str, cursor: Option<&BrowseCursor>)
    -> Result<BrowseBatch, ProviderError> { Err(ProviderError::Unsupported(self.name().into())) }
async fn resolve_image(&self, item: &RemoteItem) -> Result<String, ProviderError> {
    match &item.image { ImageRef::Direct(u) => Ok(u.clone()),
                        ImageRef::Page(_) => Err(ProviderError::Unsupported(self.name().into())) }
}
// ProviderError gains: #[error("{0} cannot browse")] Unsupported(String)
```

**Protocol (`crates/protocol/src/types.rs`, Task 2).** The wire commands are `browse` and `browse_use`. Existing requests are unchanged.
```rust
Request::Browse { session: Option<String>, page: u32, source: Option<String> }
Request::BrowseUse { source_id: String, remote_id: String, action: BrowseAction }

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowseAction { Set, Favorite, Download }

pub struct BrowseData {
    pub session: String, pub page: u32, pub restarted: bool, pub done: bool,
    pub preview_dir: String,
    pub items: Vec<BrowseItem>, pub sources: Vec<BrowseSourceStatus>,
}
pub struct BrowseItem {
    pub source_id: String, pub kind: String, pub remote_id: String,
    pub title: String, pub author: String,
    pub width: Option<u32>, pub height: Option<u32>, pub nsfw: bool,
    pub preview: Option<String>,                 // abs path in preview_dir; None = preview failed
}
pub struct BrowseSourceStatus { pub source_id: String, pub kind: String, pub state: BrowseSourceState }
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BrowseSourceState {
    Active, Exhausted, Unsupported,
    RateLimited { message: String, retry_after_secs: Option<u64> },
    NotConfigured { message: String }, Failed { message: String },
}
pub struct BrowseUseData { pub path: String, pub wallpaper: Option<WallpaperInfo> }
```
Wire examples (hand-written golden fixtures):
- `{"cmd":"browse","args":{"page":0}}`
- `{"cmd":"browse","args":{"session":"9f2c…","page":3,"source":"wh-59f2"}}`
- `{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":"x8e1k2","action":"favorite"}}`

`Client::request_with_timeout(req, Duration)` is added. `request()` delegates to it with `REQUEST_TIMEOUT`.

**UI ↔ frontend (Tasks 17–18).** Field names are camelCase via serde/ts-rs.
- `tab_thumbnails(tab: PickerTab, offset: u32) -> TabListing { items: Thumbnail[], currentIndex: number|null, hasMore: boolean }`
  - `PickerTab = "downloaded"|"local"|"favorites"|"cache"`.
  - A page holds 40 items. The current wallpaper is pinned only on `offset = 0`.
- `browse_page({session?, page, source?}) -> BrowseListing { session, page, restarted, done, items: RemoteThumb[], sources: BrowseSourceStatus[] }`
  - `RemoteThumb = {sourceId, kind, remoteId, title, author, width?, height?, nsfw, imageUrl: string|null}`.
- `browse_use({sourceId, remoteId, action}) -> { path }`.

**Config (Task 25).** `PickerConfig` gains `blur_nsfw: bool` (default `true`). The field is in the generated `PickerConfig.ts` binding and is commented in `template.toml`.

### Per-provider browse decisions (what the user sees)

| Provider | Endpoint / cursor | Preview / full | Support | User sees |
|---|---|---|---|---|
| Wallhaven API (key set) | `/api/v1/search?…&page=N[&seed=S]`; cursor `"{page}:{seed}"`; `next` = None when `page >= meta.last_page` | `thumbs.large` / `path`; `resolution`→w/h; `purity != "sfw"` → nsfw | Paged (random order stays stable via `meta.seed`) | Endless scroll in the configured sort |
| Wallhaven scrape (no key) | `/search?…&page=N`; parse `figure.thumb[data-wallpaper-id]` + `img[data-src]` (fallback `src`); 24/page, no per-item page fetch | thumb / `ImageRef::Page(<a.preview href>)`, resolved via the existing `img#wallpaper` scrape at use time | Paged; random sort may overlap → session dedupe | Endless scroll; Set takes ~1–2 s longer |
| Unsplash, empty query | **Editorial** `/photos?page=N&per_page=10`; capped at page 20 | `urls.small` / `urls.raw`+`QUALITY_PARAMS` | Paged | Newest editorial photos, then "End of results". At 50 req/h demo: "Unsplash rate limit reached — try again in N min" |
| Unsplash, query | `/search/photos?query=&page=N&per_page=10&orientation=landscape`; ends at `total_pages` | same | Paged | Paged search results, then "End of results" |
| Reddit | `/r/<sub>/hot.json?limit=25&raw_json=1[&after=X]`; cursor = `data.after` | smallest `preview.images[0].resolutions[]` with width ≥ 320 (fallback `url` via `resolve_image_url`); `over_18` dropped; `MIN_SCORE` kept; non-allowlisted image hosts dropped | Paged until `after == null` | Long scroll; may end after a few hundred posts |
| Bing | `HPImageArchive.aspx?format=js&idx=0&n=8&mkt=`, then `idx=8`; ends after the second batch | `<urlbase>_400x240.jpg` / existing UHD URL | Finite | ~8–16 images, then "End of Bing archive"; other sources continue |
| APOD | `?api_key=&start_date=E-13d&end_date=E`, newest first; cursor = next `end_date`; stops at 1995-06-16; images only | `url` / `hdurl` (fallback `url`); `copyright` → author | Paged by date | Endless back through the archive; DEMO_KEY limit shows the rate-limited state |

`media_hosts()`:
- Wallhaven: `wallhaven.cc` (covers `w.` and `th.`).
- Unsplash: `images.unsplash.com`, `plus.unsplash.com`.
- Reddit: `i.redd.it`, `preview.redd.it`, `i.imgur.com`.
- Bing: `www.bing.com`, `bing.com`.
- APOD: `apod.nasa.gov`.

`fetch()` keeps its current behaviour for every provider, so the background loop is unchanged.

### Picker layout and keys

The window size is unchanged, so `picker_place.rs` is not touched.

- **Horizontal (1000×160).** The 20 px grip column becomes a **32 px tab column**. The column background is the drag region; a 12 px grip area sits at the top, then five 28×28 icon tabs (12 + 140 = 152 ≤ 160). `aria-orientation="vertical"`.
- **Vertical (200×640).** The 20 px grip row stays. A **32 px tab row** sits below it: five 36 px icon tabs = 180 px. `aria-orientation="horizontal"`.
- **Tabs.** Icon-only, with `aria-label` and a `title` tooltip: Sources (cloud), Downloaded (download), Local (folder), Favorites (star), Cache (archive).
- **ARIA.** `role="tablist"` holds `role="tab"` buttons with `aria-selected` and `aria-controls="picker-panel"`, and roving `tabIndex`. The strip sits inside `role="tabpanel" id="picker-panel"`.
- **Keys.** See Decisions §6. All tab keys are handled before the `count === 0` guard in `keyAction`, so an empty tab never traps the user.
- **Source filter.** With more than one online source, the first slot of the Sources strip is a filter card: a radio group with "All" plus one entry per source (`kind (id)`). Changing the filter starts a new session.
- **Always open on Sources.**
  - `picker.rs::show_with` emits `picker-shown`. The frontend then resets to Sources, page 0, scroll 0, and focuses the strip.
  - Window `focus` only refreshes the active on-disk tab and does not reset the tab. Otherwise the user is pulled back to Sources mid-use when `close_on_blur = false`.
  - A browse session younger than 10 min is reused across shows, so reopening is instant.

## Merge notes (what came from where)

- **Plan A:** the base. Sessions, guard, preview cache, per-tab admission, provenance inference, layout, `picker-shown` reset, task lanes.
- **Plan B:** the guard also rejects credentials in URLs and explicit non-443 ports.
- **Plans B, C, D:** Unsplash with an empty query uses editorial `/photos` paging instead of `/photos/random`. It pages in a fixed order with no duplicates, and uses less of the rate limit.
- **Plan C:** the on-disk tabs are paged (40 per page, `hasMore`) instead of a hard cap.
- **User note:** blurring can be turned off with `[picker] blur_nsfw` (Task 25).

## Risks and unknowns

1. **[ext] response fields are unverified.** These include Wallhaven `thumbs`/`meta.seed`, Unsplash `urls.small`/`total_pages`, Reddit `preview.images`/`raw_json`, the Bing `_400x240` variant and the Wallhaven HTML `data-src`. Provider tasks build fixtures from the documented shapes and parse defensively (`#[serde(default)]`). An item missing a preview URL is dropped. Each provider gets one `#[ignore]` live smoke test (`cargo test -- --ignored browse_live`).
2. **Rate limits.** Mitigations: fail-fast responses, a per-source `RateLimited{until}` skip, prefetch at most one page ahead, one page request in flight at a time, and the sentinel fires only after the previous page has rendered.
3. **Partial pages and latency.** Refills run with `join_all` under 8 s per-source deadlines, and previews are bounded (6 s each, concurrency 4) within the 20 s cap. If a page has 0 items and no source is Active, the response sets `done = true`.
4. **Allowlist breakage.** If a CDN host changes, the blocked item's preview becomes `None` and the card shows a placeholder. The guard logs the host at `warn`.
5. **DNS resolver vs proxies.** With `HTTPS_PROXY`, the proxy resolves names, so the resolver check is bypassed. The host allowlist still applies. This is documented.
6. **Decoder surface.** The daemon checks magic bytes and header dimensions (≤4096×4096) before keeping a preview. The UI re-checks dimensions for `ListKind::Previews`. Decoding still goes through `Gate` (4).
7. **Index eviction.** The item index is capped at 2000 entries. Using an evicted item shows "That preview expired — scroll or reopen to refresh."
8. **Download duration.** `browse_use` uses a 120 s timeout on its own connection. If it times out, the UI says "Still downloading in the background…".
9. **Trim and Cache tab races.** Files can disappear between listing and click. `set_file` already returns an error for a missing path, and the UI reloads the tab on error.
10. **Exact-set capability tests.** These must be updated in the same task that adds a command, so Tasks 17, 18 and 24 run in sequence.
11. **Remote id sanitizing.** Ids become file names. Ids that don't match `[A-Za-z0-9_-]{1,64}` are hashed, so Reddit `t3_x` and APOD dates still work.

## Open questions (defaults are used until the user answers)

1. Should an explicit browse pick skip `[filters]`? Default: **skip**.
2. Should folders of disabled (but still configured) online sources show in Cache? Default: **yes**; otherwise they appear in no tab.

## What we're NOT doing

- No remote origins in CSP, no HTTP client in `crates/ui/src-tauri`, and no URLs from the webview.
- No change to the background downloader beyond making `record_seen` and `SourceLocks` reachable within the crate.
- No `[filters]` on remote previews. The only new config key is `[picker] blur_nsfw`.
- No window resizing and no `picker_place.rs` changes.
- No "open source page", no search box in the picker, no per-item info panel.
- No "already downloaded" badges on remote items.
- No virtualization. Remote items are capped at 300 per session in the UI, after which a "Reload" card appears.
- No Go-dispatch parity for the new commands (they are Rust-only, with hand-written fixtures).
- No Trash or Reveal on remote items.

## Overview

- [ ] Task 1: Sources browse contract (types + default `Provider` methods)
- [ ] Task 2: Protocol `browse`/`browse_use` requests, payloads, bindings, client timeout
- [ ] Task 3: SSRF guard + guarded client + fail-fast send
- [ ] Task 4: Wallhaven API browse
- [ ] Task 5: Wallhaven scrape browse + `resolve_image`
- [ ] Task 6: Unsplash browse (editorial + search)
- [ ] Task 7: Reddit browse
- [ ] Task 8: Bing browse
- [ ] Task 9: APOD browse
- [ ] Task 10: Engine provenance (`set_file_with` + inferred provenance)
- [ ] Task 11: Engine preview cache
- [ ] Task 12: Engine browse sessions + `Engine::browse`
- [ ] Task 13: Engine `browse_use`
- [ ] Task 14: Daemon dispatch + strict guarded client wiring
- [ ] Task 15: UI `thumbs` per-tab admission by `ListKind` + preview root
- [ ] Task 16: UI `picker-shown` event
- [ ] Task 17: UI per-tab paged listings (`tab_thumbnails`), current pinned, cache orphans
- [ ] Task 18: UI browse commands (`browse_page`, `browse_use`)
- [ ] Task 19: Frontend api + bindings wrappers
- [ ] Task 20: Frontend tab keys in `keys.ts`
- [ ] Task 21: Frontend `TabBar`, `useSentinel` hook, icons, layout CSS
- [ ] Task 22: Frontend tabbed App with paged on-disk tabs
- [ ] Task 23: Frontend Sources tab (`BrowseStrip`, paging, states, actions, filter, blur)
- [ ] Task 24: Remove legacy `thumbnails` command + docs
- [ ] Task 25: Config `[picker] blur_nsfw` + Settings toggle

### Parallel lanes (each lane owns its own files)

- **Lane A (sources):** T1 → T3 → T4 → T5. T6, T7, T8 and T9 run in parallel after T1 and T3; each owns one provider file and its fixtures.
- **Lane B (protocol):** T2. Runs in parallel with T1 and T3.
- **Lane C (engine/cli):** T10 runs in parallel from the start. Then T11 (after T1, T3), T12 (after T1, T2, T11), T13 (after T3, T10, T12), and T14 (after T2, T12, T13).
- **Lane D (UI Rust):** T15 and T16 run in parallel from the start. Then T17 (after T15), and T18 (after T2, T15, T17).
- **Lane E (frontend + config):** T20, T21 and T25 run in parallel from the start. Then T19 (after T16, T17, T18), T22 (after T19, T20, T21), T23 (after T22, T25), and T24 (after T14, T23).

Gate commands:
- `scripts/gate.sh quick <crate>` while working.
- `scripts/gate.sh full <crates>` before merging each lane.
- Frontend: `cd crates/ui/frontend && npm run typecheck && npm test`.

---

## Task 1: Sources browse contract

- **Files owned:** `crates/sources/src/browse.rs` (new), `crates/sources/src/provider.rs`, `crates/sources/src/lib.rs`, `crates/sources/tests/it/browse_contract.rs` (new), `crates/sources/tests/it/main.rs`
- **Depends on:** none
- **Steps:**
  1. Tests first:
     - A dummy provider that implements only `name`/`fetch` returns `ProviderError::Unsupported` ("dummy cannot browse") from `browse()`.
     - Its `browse_support()` returns `Unsupported` and its `media_hosts()` is empty.
     - `resolve_image` returns the URL for `Direct` and an error for `Page`.
  2. Add `browse.rs` with the contract types and `pub fn sanitize_remote_id(&str) -> String`. Unit-test it, including `t3_abc` and `2025-01-02`.
  3. Add the default methods to `Provider` and `ProviderError::Unsupported`, and re-export them.
- **Verify:** `scripts/gate.sh quick sources`

## Task 2: Protocol requests, payloads, bindings, client timeout

- **Files owned:** `crates/protocol/src/types.rs`, `crates/protocol/src/client.rs`, `crates/protocol/tests/it/golden.rs`, `crates/protocol/tests/fixtures/browse*.json` (new), generated `crates/ui/frontend/src/shared/bindings/Browse*.ts`, and the temporary arms in `crates/cli/src/daemon/server.rs`
- **Depends on:** none
- **Steps:**
  1. Failing goldens first:
     - Round-trip `Browse` (with and without session/source) and `BrowseUse` (each action).
     - `{"cmd":"browse","args":{"page":0}}` maps to `Browse{session:None,page:0,source:None}`.
     - A missing `source_id`, or `action:"nuke"`, gives a `RequestError`.
     - The `BrowseSourceState` tagging is `{"state":"rate_limited",…}`.
  2. Add the variants, `name()` arms, parsing, `RequestError` cases and the `TS` payload structs.
  3. Add `Client::request_with_timeout`. Unit test: a server that never answers gives `ClientError::Timeout` after 50 ms.
  4. Regenerate the bindings and make `golden_ts_bindings_match_wire_shape` pass.
  5. Add temporary `dispatch` arms that return the error "browse not available yet", so the cli crate keeps building. Task 14 replaces them.
- **Verify:** `scripts/gate.sh quick protocol cli`

## Task 3: SSRF guard + guarded client + fail-fast send

- **Files owned:** `crates/sources/src/guard.rs` (new), `crates/sources/src/http.rs`, `crates/sources/src/lib.rs` (re-export line), `crates/sources/tests/it/guard.rs` (new), `crates/sources/tests/it/http.rs`
- **Depends on:** Task 1
- **Steps:**
  1. Tests first:
     - `check_url` rejects `http://`, `https://127.0.0.1/`, `https://[::1]/`, `https://localhost/`, `https://user:pw@w.wallhaven.cc/`, `https://w.wallhaven.cc:8443/`, `https://evilwallhaven.cc/` and unlisted hosts.
     - `check_url` accepts `https://w.wallhaven.cc/full/x.jpg`.
     - `is_blocked_ip` covers 10/8, 172.16/12, 192.168/16, 127/8, 169.254/16, 100.64/10, 0/8, ::1, fc00::/7, fe80::/10 and v4-mapped v6.
     - With wiremock and `permissive_loopback()`: more than 3 redirect hops errors, and a redirect to an unlisted host errors.
     - `send_fail_fast` returns a 429 with `Retry-After: 60` in under 1 s, and `retry_after_secs` parses the header.
  2. Implement `GuardConfig`, `check_url`, `BlockPrivateResolver` (`reqwest::dns::Resolve`), `guarded_client` (custom redirect policy, ≤3 hops) and the `GuardError` thiserror enum. Errors show the URL redacted.
  3. Add `send_fail_fast` and `retry_after_secs` to `http.rs`.
- **Verify:** `scripts/gate.sh quick sources`

## Task 4: Wallhaven API browse

- **Files owned:** `crates/sources/src/wallhaven/mod.rs`, `crates/sources/src/wallhaven/api.rs`, `crates/sources/tests/fixtures/wallhaven/api_browse_p1.json` and `api_browse_last.json` (new), `crates/sources/tests/it/wallhaven_api.rs`
- **Depends on:** Task 1, Task 3
- **Steps:**
  1. Fixtures with `thumbs`, `resolution`, `purity` and `meta{…,seed}`. One item has no `thumbs` and must be dropped.
  2. Tests:
     - The first call sends `page=1` with no seed, and the cursor encodes `2:<seed>`.
     - The next call sends `page=2&seed=<seed>`.
     - The last page gives `next: None`.
     - Fields map correctly, and purity `sketchy` sets `nsfw`.
     - A 429 gives `RateLimited` without a retry (`expect(1)`).
     - The API key never appears in an error.
  3. Implement. Scrape mode returns `Unsupported` until Task 5.
- **Verify:** `cargo test -p kakemono-sources wallhaven_api`, then `scripts/gate.sh quick sources`

## Task 5: Wallhaven scrape browse + `resolve_image`

- **Files owned:** `crates/sources/src/wallhaven/scrape.rs`, `crates/sources/src/wallhaven/mod.rs`, `crates/sources/tests/fixtures/wallhaven/scrape_browse.html` (new), `crates/sources/tests/it/wallhaven_scrape.rs`
- **Depends on:** Task 4
- **Steps:**
  1. Fixture: 24 thumbs. Some use `data-src`, one uses only `src`, and one has no image.
  2. Tests:
     - `browse` makes exactly one request and returns `ImageRef::Page` items with a cursor of `N+1`.
     - A page with no thumbs gives `None`.
     - `resolve_image` fetches the page and returns `img#wallpaper[src]`.
  3. Implement. `scrape_delay` applies only inside `resolve_image`.
- **Verify:** `cargo test -p kakemono-sources wallhaven_scrape`
- **Manual:** run `--ignored browse_live_wallhaven_scrape` to check the real HTML uses `data-src`.

## Task 6: Unsplash browse (editorial + search)

- **Files owned:** `crates/sources/src/unsplash.rs`, `crates/sources/tests/fixtures/unsplash/browse_photos.json` and `browse_search.json` (new), `crates/sources/tests/it/unsplash.rs`
- **Depends on:** Task 1, Task 3
- **Steps:**
  1. Tests:
     - An empty query hits `/photos?page=1&per_page=10`, and page 20 gives `next: None`.
     - A query hits `/search/photos?page=N&per_page=10…`, and `page == total_pages` gives `None`.
     - Preview = `urls.small`. The full URL uses the existing raw+quality logic. `nsfw = false`.
     - A missing key gives `NotConfigured`, and a 429 gives `RateLimited` without a retry.
     - Existing `fetch` tests stay green.
  2. Implement with `send_fail_fast`. `browse_support()` = `Paged`. `media_hosts()` = `["images.unsplash.com","plus.unsplash.com"]`.
- **Verify:** `cargo test -p kakemono-sources unsplash`

## Task 7: Reddit browse

- **Files owned:** `crates/sources/src/reddit.rs`, `crates/sources/tests/fixtures/reddit/browse_p1.json` and `browse_end.json` (new), `crates/sources/tests/it/reddit.rs`
- **Depends on:** Task 1, Task 3
- **Steps:**
  1. Tests:
     - The request has `limit=25&raw_json=1`, and the next call has `after=t3_x`.
     - `after == null` gives `None`.
     - The preview is the smallest resolution at least 320 px wide.
     - `over_18`, low-score posts and unlisted hosts are dropped.
     - `remote_id` = the post's `name`.
  2. Implement with `send_fail_fast`.
- **Verify:** `cargo test -p kakemono-sources reddit`

## Task 8: Bing browse

- **Files owned:** `crates/sources/src/bing.rs`, `crates/sources/tests/fixtures/bing/browse_idx0.json` and `browse_idx8.json` (new), `crates/sources/tests/it/bing.rs`
- **Depends on:** Task 1, Task 3
- **Steps:**
  1. Tests: `idx=0&n=8` gives the cursor `"8"`, and `idx=8` gives `None`. Preview = `_400x240`. The full URL is the existing UHD URL. `remote_id` = sanitized `urlbase`. Support = `Finite`.
  2. Implement.
- **Verify:** `cargo test -p kakemono-sources bing`

## Task 9: APOD browse

- **Files owned:** `crates/sources/src/apod.rs`, `crates/sources/tests/fixtures/apod/browse_range.json` (new), `crates/sources/tests/it/apod.rs`
- **Depends on:** Task 1, Task 3
- **Steps:**
  1. Tests:
     - Inject "today" with `ApodProvider::with_today(NaiveDate)`.
     - The window and cursor follow the date math, and 1995-06-16 ends the archive.
     - Videos are skipped, and items come newest first.
     - `remote_id` = the date, and the author comes from `copyright`.
     - A 429 gives `RateLimited`.
  2. Implement. `media_hosts()` = `["apod.nasa.gov"]`.
- **Verify:** `cargo test -p kakemono-sources apod`

## Task 10: Engine provenance

- **Files owned:** `crates/engine/src/engine/ops.rs`, `crates/engine/tests/it/provenance.rs` (new, registered in the test crate's `main.rs`)
- **Depends on:** none
- **Steps:**
  1. Tests:
     - `set_file(download_dir/wh-1a2b/x.jpg)` records `source="wallhaven"`, `source_id="wh-1a2b"`.
     - A file outside any source folder still records `"local"`.
     - `set_file_with` records exactly the values it is given.
  2. Add `Provenance`, `set_file_with` and `infer_provenance`. It compares canonical parent dirs against `download_dir/<id>` for enabled online sources.
- **Verify:** `scripts/gate.sh quick engine`

## Task 11: Engine preview cache

- **Files owned:** `crates/engine/src/browse/mod.rs` (new), `crates/engine/src/browse/preview.rs` (new), `crates/engine/src/lib.rs` (`mod browse;`), `crates/engine/tests/it/browse_preview.rs` (new)
- **Depends on:** Task 1, Task 3
- **Steps:**
  1. Tests:
     - A PNG is stored at `<root>/<source_id>/<rid>.png`, and a second call makes no request.
     - A 5 MiB body, a `text/html` body or a 5000×10 header gives `None` and leaves no file.
     - A slow server with a 200 ms test timeout gives `None`.
     - At most 4 fetches are in flight at once.
     - `prune` removes old files and enforces the file cap.
  2. Implement `PreviewCache` with `fetch`, `fetch_all` (`buffer_unordered(4)`) and `prune`. Write to `.part`, then rename.
- **Verify:** `scripts/gate.sh quick engine`

## Task 12: Engine browse sessions + `Engine::browse`

- **Files owned:** `crates/engine/src/browse/session.rs` (new), `crates/engine/src/browse/mod.rs`, `crates/engine/src/engine/deps.rs`, `crates/engine/src/engine/mod.rs`, `crates/engine/src/engine/error.rs`, `crates/engine/tests/it/browse_session.rs` (new)
- **Depends on:** Task 1, Task 2, Task 11
- **Steps:**
  1. Tests with fake providers:
     - **Interleaving:** sources with `[a1..a12]` and `[b1..b3]` give page 0 = `a1 b1 a2 b2 a3 b3 a4 a5 a6 a7`.
     - **Idempotent pages:** asking for page 0 again returns the same page with no extra provider calls. Asking for page 5 when only one page exists gives `PageOutOfRange`.
     - **Duplicates:** repeated items across batches are removed.
     - **Failures:** a rate-limited source is reported and skipped, and a source that hangs doesn't block the page (paused tokio time).
     - **End and restart:** all sources exhausted gives `done`. An unknown session gives `restarted`.
     - **Filter:** the `source` filter works, and an unknown filter id gives `UnknownSource`.
     - **Limits:** `internet=false` gives `InternetOff`. `reload()` clears sessions. The LRU caps sessions at 4 and the index at 2000.
  2. Implement `Browser`, `SourceCursor` and `Engine::browse`. `EngineDeps` gains `browse_http` and `preview_root`. `BrowseError` is a thiserror enum.
- **Verify:** `scripts/gate.sh quick engine`

## Task 13: Engine `browse_use`

- **Files owned:** `crates/engine/src/browse/pick.rs` (new), `crates/engine/src/browse/mod.rs`, `crates/engine/src/downloader.rs` (visibility changes only), `crates/engine/tests/it/browse_use.rs` (new)
- **Depends on:** Task 3, Task 10, Task 12
- **Steps:**
  1. Tests:
     - `Set` downloads into `download_dir/<id>/`, records the seen ledger, sets the wallpaper and records the real provenance.
     - `Set` then `Favorite` downloads once and copies the file to favorites.
     - `Download` neither sets nor favorites.
     - A filtered-out image is kept.
     - An unlisted host gives `Guard` and no file.
     - An unknown id gives `ItemExpired`, a disabled source gives `UnknownSource`, and `internet=false` gives `InternetOff`.
     - An `ImageRef::Page` item goes through `resolve_image`.
  2. Implement. Hold `SourceLocks` for the whole download and trim, and `trim_to_mb` protects the new file.
- **Verify:** `scripts/gate.sh quick engine`

## Task 14: Daemon dispatch + strict client wiring

- **Files owned:** `crates/cli/src/daemon/server.rs`, the daemon file that builds `EngineDeps`, a cli daemon test file
- **Depends on:** Task 2, Task 12, Task 13
- **Steps:**
  1. Tests:
     - `{"cmd":"browse","args":{"page":0}}` with no online sources returns `ok` with empty items and `done`.
     - `browse_use` with an unknown id returns an error response.
     - The response contains no `apikey`, `client_id` or `api_key`, and stays under `MAX_LINE_BYTES`.
  2. Replace the temporary arms. Build the strict `guarded_client` and `preview_root = cache_dir()/previews`, and prune the preview cache at startup.
- **Verify:** `scripts/gate.sh quick cli engine`
- **Manual:** with a Bing source enabled, a raw `browse` request returns 8 items, and their preview files exist.

## Task 15: UI per-tab admission

- **Files owned:** `crates/ui/src-tauri/src/thumbs.rs`, the shim at the `admit` call site in `crates/ui/src-tauri/src/commands/listing.rs`, and the thumbs tests
- **Depends on:** none
- **Steps:**
  1. Tests:
     - Paths admitted by Downloaded and by Favorites both stay admitted.
     - A second Downloaded admit drops only the old Downloaded paths.
     - A Previews path outside the preview root is refused.
     - A 5000×5000 preview gives 415 without being decoded.
     - The same file gets the same id under every kind.
  2. Implement `ListKind` (including `Legacy` for the old command) and a `HashMap<ListKind, Allowlist>`.
- **Verify:** `scripts/gate.sh quick ui`

## Task 16: UI `picker-shown` event

- **Files owned:** `crates/ui/src-tauri/src/picker.rs`
- **Depends on:** none
- **Steps:** Add `SHOWN_EVENT = "picker-shown"` and emit it at the end of `show_with`. Log a `warn!` if the emit fails. The capabilities test must still pass.
- **Verify:** `scripts/gate.sh quick ui`

## Task 17: UI per-tab paged listings

- **Files owned:** `crates/ui/src-tauri/src/commands/listing.rs`, `crates/ui/src-tauri/src/ipc_commands.rs`, `crates/ui/src-tauri/src/lib.rs`, `crates/ui/src-tauri/capabilities/picker.json`, `crates/ui/src-tauri/tests/it/capabilities.rs`, listing tests, generated `PickerTab.ts` and `TabListing.ts`
- **Depends on:** Task 15
- **Steps:**
  1. Tests:
     - `tab_dirs` returns the right dirs per tab. Cache includes root files and `download_dir/*/` dirs that are valid ids but not enabled online ids. It excludes `manual`, nested favorites/trash dirs, dot-dirs and `.kakemono-seen`.
     - Paging: 40 items per page, `hasMore` set correctly, and offsets don't overlap.
     - The current wallpaper is first only on offset 0 of the tab that contains it. The rest are newest first by mtime.
  2. Add `tab_thumbnails(tab, offset)`, which admits with the matching `ListKind`. Register it in every place commands are listed, and update the exact-set capability test.
- **Verify:** `scripts/gate.sh quick ui`

## Task 18: UI browse commands

- **Files owned:** `crates/ui/src-tauri/src/commands_sources/browse.rs` (new), `crates/ui/src-tauri/src/commands_sources/mod.rs`, `crates/ui/src-tauri/src/daemon_link.rs` (`call_alone_with_timeout`), `crates/ui/src-tauri/src/ipc_commands.rs`, `crates/ui/src-tauri/src/lib.rs`, `crates/ui/src-tauri/capabilities/picker.json`, `crates/ui/src-tauri/tests/it/capabilities.rs`, generated `BrowseListing.ts` and `RemoteThumb.ts`
- **Depends on:** Task 2, Task 15, Task 17
- **Steps:**
  1. Tests for `to_listing`:
     - Paths under `preview_dir` become `thumb://` URLs.
     - Paths outside it, or `..` tricks, become `null` and are not admitted.
     - The DTO has no http(s) URLs.
  2. Implement `browse_page` (via `call_alone`) and `browse_use` (via `call_alone_with_timeout(120 s)`; a timeout shows "Still downloading in the background…"). Register both.
- **Verify:** `scripts/gate.sh quick ui`

## Task 19: Frontend api wrappers

- **Files owned:** `crates/ui/frontend/src/shared/api.ts`, `crates/ui/frontend/src/shared/api.test.ts`
- **Depends on:** Task 16, Task 17, Task 18
- **Steps:**
  1. Tests: `listTab`, `browsePage` and `browseUse` invoke the right command with the right arguments, and `onPickerShown` listens for the event.
  2. Implement with the `@/shared/bindings` types.
- **Verify:** `npm run typecheck && npm test -- api`

## Task 20: Frontend tab keys

- **Files owned:** `crates/ui/frontend/src/picker/keys.ts`, `crates/ui/frontend/src/picker/keys.test.ts`
- **Depends on:** none
- **Steps:**
  1. Tests:
     - The cross-axis arrows give `{type:"tab",delta}`, `1`–`5` give `tabIndex`, and `[`/`]` give `filter`.
     - All of these work when `count === 0`, and none fire with modifiers held.
     - The existing key actions are unchanged.
  2. Handle the new keys before the `count === 0` guard.
- **Verify:** `npm test -- keys`

## Task 21: Frontend `TabBar`, `useSentinel`, icons, CSS

- **Files owned:** `crates/ui/frontend/src/picker/components/TabBar.tsx` and `TabBar.test.tsx` (new), `crates/ui/frontend/src/picker/tabs.ts` (new), `crates/ui/frontend/src/picker/useSentinel.ts` and `useSentinel.test.ts` (new), `crates/ui/frontend/src/components/Icon/*`, `crates/ui/frontend/src/picker/App.css`
- **Depends on:** none
- **Steps:**
  1. Tests:
     - The tablist renders 5 labelled tabs, with `aria-selected`, roving `tabIndex` and the right `aria-orientation`.
     - Clicking a tab calls `onSelect`.
     - `useSentinel` calls `onVisible` once per intersection while not `busy`.
  2. Add the CSS for both orientations and keep the drag region on the bar.
- **Verify:** `npm run typecheck && npm test -- TabBar useSentinel`

## Task 22: Frontend tabbed App with paged on-disk tabs

- **Files owned:** `crates/ui/frontend/src/picker/App.tsx`, `crates/ui/frontend/src/picker/App.test.tsx`, `crates/ui/frontend/src/picker/components/ThumbnailStrip.tsx`
- **Depends on:** Task 19, Task 20, Task 21
- **Steps:**
  1. Tests:
     - The App starts on Sources (a placeholder until Task 23).
     - Downloaded calls `listTab("downloaded", 0)`, the current item is first, and the sentinel loads offset 40.
     - `picker-shown` resets to Sources. `focus` reloads the active tab without switching it.
     - Tab keys work on an empty tab.
     - Each tab has its own empty-state text.
     - The existing tests still pass.
  2. Implement. The Trash menu item stays enabled only for the current wallpaper.
- **Verify:** `npm run typecheck && npm test`
- **Manual:** check that both orientations fit and that tab switching works.

## Task 23: Frontend Sources tab

- **Files owned:** `crates/ui/frontend/src/picker/components/BrowseStrip.tsx` and `BrowseStrip.test.tsx` (new), `crates/ui/frontend/src/picker/useBrowse.ts` and `useBrowse.test.ts` (new), plus the placeholder swap in `App.tsx` and the remote card styles in `App.css`
- **Depends on:** Task 22, Task 25
- **Steps:**
  1. `useBrowse` tests:
     - Page 0, then `loadMore`, then page 1, with concurrent calls collapsed into one.
     - `done` stops loading, and `restarted` replaces the list.
     - A filter change starts a new session.
     - Errors, internet-off and per-source statuses are surfaced.
     - The list is capped at 300 items, then a Reload card shows.
  2. `BrowseStrip` tests:
     - **Paging:** the sentinel calls `loadMore`, and moving the keyboard within 3 items of the end also loads more.
     - **Actions:** Enter calls `browseUse(set)` and shows `aria-busy` until success. `f` triggers favorite. The menu has Set, Favorite, Download and Reset Position, and no Trash.
     - **Blur:** NSFW cards are blurred until hover or focus when `blurNsfw` is true, and not blurred when it is false.
     - **Display:** a null `imageUrl` shows a placeholder. The label reads `"<title> by <author> — <kind>"`.
     - **Filter:** the filter card appears only with more than one source, and `[`/`]` cycle it.
  3. Implement.
- **Verify:** `npm run typecheck && npm test`
- **Manual:** run end to end with Wallhaven, Bing and Reddit:
  - Pages load on scroll, and Bing reaches its end.
  - Set: the file lands in `download_dir/<id>/` and in the Downloaded tab, and history shows the real source.
  - Favorite: the image appears in the Favorites tab.
  - `internet=false` shows the disabled message.
  - `blur_nsfw=false` removes the blur.

## Task 24: Remove legacy `thumbnails` + docs

- **Files owned:** `crates/ui/src-tauri/src/commands/listing.rs`, `crates/ui/src-tauri/src/thumbs.rs` (drop `Legacy`), `crates/ui/src-tauri/src/ipc_commands.rs`, `crates/ui/src-tauri/src/lib.rs`, `crates/ui/src-tauri/capabilities/picker.json`, `crates/ui/src-tauri/tests/it/capabilities.rs`, `crates/ui/frontend/src/shared/api.ts`, this plan's status, the picker docs
- **Depends on:** Task 14, Task 23
- **Steps:**
  1. Delete `thumbnails`, `list_thumbnails`, `MAX_THUMBNAILS` and `listThumbnails`. `capabilities_cover_every_command_the_frontend_invokes` proves nothing still calls them.
  2. Document the tabs, keys, per-provider behaviour, rate-limit messages, `blur_nsfw`, and privacy: previews are fetched by the daemon and cached for 24 h in `<cache>/previews`.
- **Verify:** `scripts/gate.sh full protocol sources engine cli ui config` and `npm run typecheck && npm test`
- **Manual:** the Task 23 end-to-end run on macOS, plus one Linux run (X11, bottom edge, horizontal).

## Task 25: Config `[picker] blur_nsfw` + Settings toggle

- **Files owned:** `crates/config/src/schema.rs`, `crates/config/src/defaults.rs`, `crates/config/src/template.toml`, config tests, generated `crates/ui/frontend/src/shared/bindings/PickerConfig.ts`, `crates/ui/frontend/src/settings/components/PickerSettings.tsx` and its test, `crates/ui/frontend/src/settings/testConfig.ts`
- **Depends on:** none
- **Steps:**
  1. Tests first:
     - A config without the key loads `blur_nsfw = true`, and `blur_nsfw = false` round-trips.
     - The template parses and has the commented key.
     - The Settings Picker section shows a "Blur NSFW previews" switch that patches `picker.blur_nsfw`.
  2. Add the field with `#[serde(default)]` semantics, regenerate the binding, and add the switch.
- **Verify:** `scripts/gate.sh quick config` and `npm run typecheck && npm test -- PickerSettings configPatch`
