# Implementation plan: Variety-style sources, background downloading and filtering

Approved plan: Plan A from multi-plan run `20261110-015200-plan`, with five changes (see
"Changes from Plan A" below). Spec: `docs/plans/variety-sources.md`.

## Changes from Plan A

1. **Task 0 (new): stabilise the working tree.** The uncommitted 5-tab settings rewrite must pass
   `tsc` and `vitest` before Task 1 starts.
2. **Clean room.** No task reads Variety's source code. Lightness thresholds come from the spec:
   `DARK_MAX = 0.35`, `LIGHT_MIN = 0.60` (mean relative luminance, Rec. 709), tuned only by our
   own tests. Kakemono is MIT; Variety is GPL-3.0.
3. **One guarded delete helper.** `engine/src/files.rs` gains `safe_delete(path, &Protect)`. Every
   delete path (Remove with files, `trim_to_mb`, filter rejects in the downloader) goes through it.
   It refuses anything outside `download_dir`, anything under `manual_dir`, favorites or trash,
   the current wallpaper, history entries and local-source paths. Owned by Task 11.
4. **`DownloadNow` IPC.** Task 16 adds `Request::DownloadNow { id: Option<String> }`
   (`"download_now"`) routed to `Engine::download_one`. Task 17 adds the Tauri command
   `download_now(id)`. The UI calls it after Add and after Use, so a new source shows images at
   once instead of after the next download interval.
5. Everything else (migration design, `Unsupported` type, `apply_json` arrays of tables, IPC
   `check_source`/`fetch`, HTML5 drag and drop) stays as Plan A wrote it.

## Task 0: Stabilise the working tree

- **Files owned:** `crates/ui/frontend/src/settings/**` (only to fix errors)
- **Depends on:** none
- **Steps:** run `npx tsc -b --noEmit` and `npx vitest run` in `crates/ui/frontend`, plus
  `cargo test --workspace` and `cargo clippy --workspace --all-targets`. Fix whatever fails from the
  in-flight rewrite without changing behaviour.
- **Automated verification:** all four commands pass.


## Summary

This plan implements `docs/plans/variety-sources.md` in about 27 thin slices. The work runs in five phases:

1. **Config.** Add the new keys: `[[sources]]`, the new `[daemon]` keys and `[filters]`. Add source-id helpers, a pure legacy-to-sources migration, and array-of-tables support in `apply_json`. All of this is additive, so the engine and the UI keep working on the old fields.
2. **Building blocks.** Each one sits in its own file:
   - per-instance online sources in `sources`
   - local listing and album helpers
   - an image-facts cache, soft filters, the MB-based trim and a local image pool in `engine`
   - screen size in `platform`
3. **The switch.** A background `Downloader` fills `downloads/<source-id>/`. `Engine::next` picks from local images and stops downloading on demand, except as a cold-start fallback. The migration is wired into load with a `.bak` copy. `change_on_start` and `internet` start working.
4. **IPC and Tauri.** New `check_source` and `fetch` IPC requests, and new Tauri commands for validating, opening, deleting and dropping. Then a two-tab settings UI: General has the sources table, Add/Edit/Remove/Open Folder/Use, download settings, filters, the picker and the shortcut hint. Advanced has everything technical.
5. **Cleanup.** Remove the legacy schema fields, add drag and drop, and write the docs.

After every slice these must pass: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `npx tsc -b --noEmit` and `npx vitest run`.

## Architecture

### Config shape (crate `kakemono-config`)
- `schema.rs` additions:
  ```rust
  #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
  #[serde(default)]
  pub struct SourceEntry {
      pub id: String,            // [a-z0-9-]{1,32}; names downloads/<id>/
      pub enabled: bool,
      #[serde(rename = "type")]
      pub kind: SourceType,
      pub location: String,      // keywords | URL | subreddit | path | "" (bing/apod)
  }
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
  #[serde(rename_all = "kebab-case")]
  pub enum SourceType { Wallhaven, Unsplash, Reddit, #[default] Bing, Apod, Folder, Images, AlbumName, AlbumDate,
                        #[serde(other)] Unsupported }
  ```
  `Config.sources: Vec<SourceEntry>`, serialized as `[[sources]]`. A `type` that isn't recognised loads as `Unsupported`. The engine skips it and the UI shows it as "Unknown" instead of failing the whole load.
- New `DaemonConfig` keys:
  - `change_on_start: bool` (false)
  - `internet: bool` (true)
  - `download_enabled: bool` (true)
  - `download_interval: String` (Go duration, `"10m"`, parsed with `duration::parse_go`, as `interval` is)
  - `download_limit_mb_enabled: bool` (true)
  - `download_limit_mb: u32` (500)
  - `manual_dir: Option<PathBuf>` (`None` means `<download_dir>/manual`; resolved by `DaemonConfig::manual_dir_resolved()`; TS type `string | null`, serialized as `null`, never skipped)
- New `FiltersConfig` (`[filters]`):
  - `landscape`
  - `min_size_enabled` and `min_size_percent` (50, 80 or 100; default 80)
  - `brightness: BrightnessFilter { Off, Dark, Light, System }`
  - `color_enabled` and `color` (`"#rrggbb"`)
  - `rating_enabled` and `min_rating` (1–5, default 4)
  - `name_regex_enabled` and `name_regex`
- Defaults (`defaults.rs`), as the spec's acceptance requires:
  - `bing` (id `bing`, enabled)
  - `wallhaven` "nature" (id `wh-nature`, enabled)
  - `folder` = favorites dir (id `favorites`, enabled)
- `config/src/sources.rs` (new) holds:
  - `SourceType::{is_online, takes_input, prefix, label}`
  - `valid_id(&str)` and `deterministic_id(kind, location, taken: &HashSet<String>)`, an FNV-1a hash giving `"{prefix}-{hex4}"` with `-2`, `-3`, … added on collision
  - `normalize_sources(&mut Vec<SourceEntry>) -> Vec<String /*warnings*/>`, which replaces invalid or duplicate ids deterministically

  Ids are generated deterministically everywhere except the UI's Add (which uses random hex). That way two processes that migrate the same file at the same time produce the same ids.
- **Migration** lives in `config/src/migrate.rs` and works on a `toml_edit::DocumentMut`:
  - It parses the legacy fields with private copies of the current legacy structs and defaults (`legacy::LegacyConfig`, which includes `ThemeConfig` and the legacy light/dark defaults). Legacy semantics, such as "missing `[bing]` means enabled", are preserved even after task 24 deletes those fields from `schema.rs`.
  - It emits rows following the spec's rules.
  - It removes `[light]`, `[dark]`, `daemon.cache_size`, the provider `enabled` keys, `reddit.subreddits`, `local.dirs` and `local.recursive`. A table that ends up empty is removed too.
  - It appends a `[[sources]]` array of tables.

  "Legacy" means: no `sources` key, plus at least one legacy key present. An explicit `sources = []` is respected.
  - `parse_str` migrates in memory, so every reader sees `sources`.
  - `migrate_file(path) -> Result<Option<MigrationReport>, ConfigError>` rewrites the file once:
    1. Copy the file to `config.toml.bak`, but only if no `.bak` exists. If one exists, use `config.toml.bak.<unix-ts>`.
    2. Write the result with `edit::write_atomic`.

    The daemon calls it at startup and the UI calls it in `get_config`. A failure (for example a read-only file) logs a warning and keeps the in-memory result.
- `edit.rs::apply_json`: a JSON array whose elements are all objects, patched onto a key with no existing value or an existing array of tables, is written as an `ArrayOfTables` (`[[sources]]`), not an inline array. Existing comments above the first `[[sources]]` are kept.

### Sources (crate `kakemono-sources`)
- New `instance.rs`:
  ```rust
  pub struct OnlineSource { pub id: String, pub kind: SourceType, pub provider: Arc<dyn Provider>, pub query: String }
  pub fn build_online(entry: &SourceEntry, cfg: &Config, http: Client) -> Result<OnlineSource, ProviderError>;
  pub struct SourceSet { pub online: Vec<OnlineSource> /* enabled + online only */ }
  impl SourceSet { pub fn from_config(cfg: &Config, http: Client) -> Self; pub fn get(&self, id: &str) -> Option<&OnlineSource>; }
  ```
  Location parsing:
  - **Reddit:** `r/x`, `x` or `https://(www.|old.)reddit.com/r/x/...` give `x`. Builds `RedditProvider` with `vec![x]`.
  - **Wallhaven:** keywords pass through. `https://wallhaven.cc/search?q=..&categories=..&purity=..&sorting=..` sets the query and overrides those options for this instance. `/tag/<n>` becomes `q=id:<n>`.
  - **Unsplash:** keywords, or `unsplash.com/s/photos/<kw>` gives `<kw>`. Any other URL is `NotConfigured("unsplash: only search URLs are supported")`.
  - **Bing and APOD:** the location is ignored.

  Credentials still come from `[wallhaven]`, `[unsplash]`, `[apod]` and `[bing]`.
- `local.rs` gains:
  - `list_images(dir, recursive) -> Vec<PathBuf>` (jpg, jpeg, png, webp; hidden files skipped; capped at 50 000 entries)
  - `album_next(dir, after: Option<&Path>, order: AlbumOrder::{Name, Date}) -> Option<PathBuf>` (wraps around)

  The existing `LocalProvider` stays until task 24.

### Engine (crate `kakemono-engine`)
- `facts.rs`: `ImageFacts { width, height, luminance: f32 /*0..1*/, palette: Vec<([u8;3], f32)>, rating: Option<u8> }`.
  - Computed in `spawn_blocking` from a 64×64 downscale (`image` crate):
    - luminance: mean Rec. 709 relative luminance (linearised sRGB)
    - palette: 4-bit-per-channel histogram, top 5 buckets with at least 5 % share
    - rating: EXIF `Rating` (tag 0x4746) through `kamadak-exif`, else a byte scan of the first 512 KiB for `xmp:Rating="N"` or `<xmp:Rating>N</xmp:Rating>`
  - `FactsCache` is stored as JSON at `<state_dir>/facts.json`, keyed by path and checked against `(mtime_ns, size)`. Missing paths are pruned on save, and saves are debounced.
- `filters.rs` (pure):
  - `Filters::compile(&FiltersConfig) -> (Filters, Vec<String> warnings)`. An invalid regex is a warning and that filter is off.
  - `Filters::check(&self, path, &ImageFacts, ctx: &FilterCtx { screen: (u32,u32), theme: Theme }) -> Result<(), Vec<&'static str /*failed filter names*/>>`
  - `score()` is the number of filters passed, used for the best-effort fallback.
  - Thresholds live in named consts:
    - `DARK_MAX = 0.35` and `LIGHT_MIN = 0.60` (from the spec; do not consult Variety's source).
    - `COLOR_MAX_DELTA_E = 30.0` (CIE76 in Lab)
- `files.rs`: `trim_to_mb(download_dir, limit_bytes, protect: &Protect)`. It walks `download_dir` recursively, including legacy flat files at its root, and deletes the oldest files by mtime until the total is at or under the limit. It never touches:
  - `manual_dir`
  - the favorites dir
  - the trash dir
  - any local-source path
  - the current wallpaper
  - history paths

  Seen-ledger files are skipped and don't count. `trim_cache` stays until task 24.
- `pool.rs`: `Pool::collect(cfg, sources)` returns `Vec<PoolSource { id, kind, files_or_album }>` for each enabled source:
  - online: `download_dir/<id>`
  - `folder`: recursive
  - `images`: the file itself
  - `album-*`: next in order after the cursor
  - plus an implicit `manual` source for `manual_dir`

  `pick(...)`:
  1. Choose a random non-empty source.
  2. Take up to 20 random files from it, excluding the current wallpaper and the last 5 history entries.
  3. Return the first file that passes the filters. If none passes, try another source, up to 20 candidates in total.
  4. If nothing passes, return the best score and `tracing::info!` the filters that failed.

  Albums ignore the filters, because sequence is the user's intent. Album cursors are stored in `State` (`album_cursors: BTreeMap<String, PathBuf>`, `#[serde(default)]`).
- `downloader.rs`: `Downloader::run(engine, shutdown)` is spawned from `Engine::run`. It does nothing while `!internet || !download_enabled`, re-reading the config on every iteration so a reload takes effect. Each tick:
  1. Choose the enabled online source with the fewest files that isn't in backoff.
  2. `fetch(query)`, then drop results whose `image_url` is in `downloads/<id>/.kakemono-seen` (newline list capped at 5000).
  3. Download one with `sources::download` into `downloads/<id>/` and record it in the seen ledger.
  4. Compute facts. If the filters fail, delete the file. Deleting is allowed because it's an online download.
  5. Run `trim_to_mb` if `download_limit_mb_enabled`.

  On start and after a reload, it fills each empty online folder immediately, one image per source.

  Backoff is per source:
  - `RateLimited`: `min(interval × 2^n, 6h)`
  - other errors: `min(1m × 2^n, interval × 4)`
  - "nothing new" (for example Bing's daily image already seen): 6h

  It also exposes `Engine::download_one(id) -> Result<PathBuf, String>`, used for the cold-start fallback and for validation.
- `engine.rs`:
  - `fetch_and_set` becomes `pick_and_set`: collect the pool, pick, set, push history.
  - If the pool is empty, `internet` is on and online sources are enabled, download one image on demand from the online sources in random order, using the current error format (`EngineError::Fetch`).
  - Otherwise return the new error `EngineError::NoImages`: "manager: no images available; enable a source or wait for downloads".
  - `EngineError::NoProvider` and its `*_matches_go` test are removed, because the feature replaces them.
  - `Deps::with_registry_factory` becomes `with_sources_factory(Fn(&Config, Client) -> SourceSet)`, so tests inject fakes.
  - `reload` rebuilds the `SourceSet` and wakes the downloader through `tokio::sync::Notify`.
- `rotation.rs`: if `change_on_start`, rotate once at the start of `run` (reason `"start"`) and set the anchor to now. Theme-change rotation stays. With `brightness = system`, it gives the matching image.
- New file `manual.rs` (`impl Engine`):
  - `check_source(entry) -> Result<usize, String>`: does a live fetch and returns the result count. For local types, it counts the files `list_images` returns.
  - `fetch_url(url_or_path) -> Result<WallpaperInfo, EngineError>`: downloads (or copies a `file://` or absolute path) into `manual_dir`, then sets it. Keeping these out of `engine.rs` lets the IPC task run without touching it.

### Platform
`Signals::screen_size(&self) -> Option<(u32, u32)>` returns the largest display in physical pixels. The trait default is `None`.
- macOS: `NSScreen.screens`, the largest `frame × backingScaleFactor`.
- Fake: settable.
- Linux and Windows: `None` for now.

Filters fall back to 1920×1080 and warn once.

### IPC protocol changes (`crates/protocol/src/types.rs`)
Two new `Request` variants, both added to `TryFrom<Command>`, `From<Request>` and `name()`:
- `CheckSource { kind: String, location: String }` (`"check_source"`, args `type` and `location`). Response data: `{"count": n}`. Errors come back as strings.
- `Fetch { url: String }` (`"fetch"`, arg `url`; an empty or missing `url` gives `RequestError::MissingUrl`). Response data: `WallpaperInfo`, as `set` returns.

There are no new events, and existing wire formats are unchanged. The daemon handler is in `crates/cli/src/daemon/server.rs`.

### Tauri commands (`crates/ui/src-tauri`)
New file `commands_sources.rs`, registered in `lib.rs`:
- `check_source(kind, location) -> CheckResult { count }`: runs `lifecycle::ensure_daemon` first, then the IPC `CheckSource` with a 20 s timeout.
- `delete_source_files(id)`: valid id only, deletes `download_dir/<id>` after a `canonicalize` check that it's a direct child of `download_dir` and isn't `manual_dir`, the favorites dir or the trash dir.
- `open_source_folder(id)`:
  - local types: open the location (the parent dir for `images`)
  - online: create, then open `download_dir/<id>`
- `fetch_dropped_url(url)`: IPC `Fetch`.
- `import_dropped_file(name, bytes: Vec<u8>)`: writes a unique file into `manual_dir`, then IPC `Set`.

`commands::thumbnails` lists, in order: history, then the folders of enabled sources (online downloads, then manual, then local), then favorites.

"Use" needs no new command: the frontend calls `save_config` with the sources patch, then `next`.

### UI
- `sections.ts` reduces `TabId` to `'general' | 'advanced'`, and a `Tabs.tsx` segmented header replaces `Sidebar.tsx`.
- **General:**
  - `SourcesTable.tsx` with Add…, Edit…, Remove…, Open Folder and Use
  - `FiltersGroup.tsx`
  - the start-at-login / interval / change-on-start / internet group
  - `DownloadingGroup.tsx` (download every N and the MB limit)
  - the favorites folder
  - picker edge and orientation (the existing `PickerSettings.tsx`)
  - `ShortcutHint.tsx`
- **Advanced:**
  - folders (download, manual, trash)
  - display mode
  - desktop (the existing `DesktopSettings.tsx`)
  - hooks and socket
  - appearance
  - `ProviderKeys.tsx`
- `sourceTypes.ts` is the single table of type metadata: label, description, prompt text, whether it takes input, online or local, and the id prefix.
- Saving goes through the existing auto-save store and `configPatch.ts`. The `sources` array is always patched as a whole.

### Drag and drop
Both windows disable Tauri's native drop handler (`.disable_drag_drop_handler()` on the `WebviewWindowBuilder`, or `"dragDropEnabled": false` in `tauri.conf.json`, whichever creates the window). HTML5 `drop` events then deliver URLs (`text/uri-list` / `text/plain`) and `File` bytes on every OS, without depending on native paths. The hook `shared/useImageDrop.ts` routes URLs to `fetch_dropped_url` and files to `import_dropped_file`.

## Risks and unknowns

- **Migration data loss.**
  - The migration runs on a deserialized copy that uses the *legacy* defaults kept in `migrate.rs`, so a missing `[bing]` still means "enabled".
  - Test it with: the real user config (ask the user for a copy, or reconstruct it from `docs/migration.md`), the template, an empty file, light-only, dark-only, and a disabled provider listed in `[dark].sources`.
  - Comments inside removed tables are lost. That's acceptable, and `.bak` keeps them.
  - `[local] recursive = false` becomes a recursive `folder` row (Variety folders are recursive). Log it in the `MigrationReport`.
- **Concurrent migration.** The daemon and the UI may migrate at the same time. Deterministic ids make the result identical. `.bak` is written only if absent, so the second writer can't replace the original backup with an already-migrated file.
- **ts-rs bindings.** The new types must be exported; check that the export test in `kakemono-ui --features export-bindings` reaches nested types. The frontend fixture `settings/testConfig.ts` must gain every new field, or `tsc` fails. Each schema task updates it.
- **`apply_json` and arrays of tables.** If `[[sources]]` were written inline, hand-editing would get ugly. Task 5 covers it. Until task 5 lands, the UI doesn't write `sources`.
- **Large local folders.** Scanning on every change can be slow. Only the chosen source is listed, inside `spawn_blocking`, capped at 50k files. A listing cache keyed on the dir mtime is a possible follow-up, not in scope.
- **Screen size on Linux and Windows.** It's unknown, and the filter uses 1920×1080. That's documented as a TODO.
- **Lightness thresholds.** Spec values 0.35/0.60; adjust only from our own test images.
- **EXIF crate.** `kamadak-exif` (lib name `exif`) is a new dependency. If the build environment is offline, fall back to a minimal TIFF IFD0 parser (about 80 lines) for tag 0x4746. Decide in task 8.
- **Regex dialects.** The UI checks the regex with JS `RegExp`, but the daemon uses Rust `regex`. If they disagree, the daemon logs a warning and skips the filter. The UI states that the daemon has the final say.
- **Validation needs the daemon.** `check_source` calls `ensure_daemon` first. If the daemon still can't be reached, the dialog shows the error and offers "Add anyway". Local types are also checked through the daemon.
- **Bing duplicates.** Bing returns the same daily image, so the seen ledger turns that into "nothing new" with a 6h backoff, not a busy loop.
- **History entries pointing at trimmed files.** `trim_to_mb` spares history paths. If an older history file is gone, `prev` uses the existing missing-file handling. Check it in task 11.
- **DnD URLs.** Disabling the native handler might change behaviour in the picker. Its window-drag (`data-tauri-drag-region`) isn't affected, but test it manually on macOS and one Wayland compositor.
- **Go-compat tests.** Only tests for strings this feature replaces are deleted: `NoProvider`, and the `cache_size` trim text if one exists. Everything else keeps passing unchanged.
- **The CLI may reference legacy fields.** `doctor`, `status`, `firstrun` and snapshot tests may read `light`/`dark`/`enabled`. Task 24 greps for and fixes them: `rg -n "\.light|\.dark|cache_size|\.enabled|subreddits|local\.dirs" crates`.

## What we're NOT doing

- Variety items 15 and 25–31: EXIF auto-rotate, slideshow, menu-bar icon options, a CLI hint for a hidden icon, the favorites operations menu, lock screen, and copy-to-static-folder.
- Quotes, clock and effects.
- Clipboard monitoring (18). It only gets a `docs/TODO.md` entry.
- A global-shortcut plugin. There's a hint only.
- Screen-size detection on Linux and Windows.
- Moving legacy flat files at the root of `download_dir` into per-source folders. They stay where they are and are trimmed by age.
- New CLI subcommands (`kakemono fetch` and the like) and new daemon events.
- Per-source download counts or progress in the UI.
- Committing. The user commits.

## Overview

- [x] Task 1: Config: `[[sources]]` schema, `SourceType`, defaults, bindings
- [x] Task 2: Config: new `[daemon]` keys and `[filters]` table
- [x] Task 3: Config: source-id helpers and `normalize_sources`
- [x] Task 4: Config: pure legacy → `[[sources]]` migration (`migrate.rs`)
- [x] Task 5: Config: `apply_json` writes arrays of objects as `[[tables]]`
- [x] Task 6: Sources: per-instance online sources (`instance.rs`, location parsing)
- [x] Task 7: Sources: local listing and album helpers
- [x] Task 8: Engine: image facts and the facts cache
- [x] Task 9: Engine: soft filters
- [x] Task 10: Platform: `screen_size()`
- [x] Task 11: Engine: MB-based `trim_to_mb`
- [x] Task 12: Engine: local pool and pick, plus album cursors in state
- [x] Task 13: Engine: background downloader
- [x] Task 14: Engine: switch changes to pick-from-disk; change_on_start; internet switch
- [x] Task 15: Wire migration into load, the daemon start and the UI `get_config`
- [x] Task 16: IPC: `check_source` and `fetch` requests; `Engine::check_source` / `fetch_url`
- [x] Task 17: Tauri: source commands, the dropped-file/URL commands, thumbnails from sources
- [x] Task 18: Frontend: two-tab shell (General, Advanced) and the `sourceTypes.ts` table
- [x] Task 19: Frontend: Sources table with enable, Remove, Open Folder, Use
- [x] Task 20: Frontend: Add… menu, source dialog with live validation, Edit…
- [x] Task 21: Frontend: General groups (interval, change on start, internet, downloading, favorites, picker, shortcut hint)
- [x] Task 22: Frontend: Filters group
- [x] Task 23: Frontend: Advanced tab (folders, display mode, desktop, hooks, socket, appearance, provider keys)
- [x] Task 24: Remove the legacy schema fields and code paths
- [x] Task 25: Drag and drop on the picker and settings windows
- [x] Task 26: Docs: TODO (clipboard), migration notes, template comments
- [ ] Task 27: End-to-end acceptance pass

Parallel lanes, once their dependencies are done:
- **Config:** tasks 1 → 2 → 3 → 4, with task 5 running alongside 3 and 4.
- **Sources:** tasks 6 and 7 can run together.
- **Engine:** 8 → 9 → 12 → 13 → 14, with 10 and 11 in parallel.
- **IPC/UI:** 16 → 17, and 18 → (19 ∥ 21 ∥ 23) → 20, 22.
- **Docs:** 26 can run at any time.

## Task 1: Config: `[[sources]]` schema, `SourceType`, defaults, bindings

- **Files owned:** `crates/config/src/schema.rs`, `crates/config/src/defaults.rs`, `crates/config/src/load.rs`, `crates/config/src/template.toml`, `crates/config/tests/json_wire.rs`, `crates/ui/frontend/src/shared/bindings/*.ts` (regenerated), `crates/ui/frontend/src/settings/testConfig.ts`
- **Depends on:** none
- **Mode:** contract-first (every later task reads `SourceEntry`/`SourceType`)
- **Steps:**
  1. Test first in `schema.rs` tests or a new `crates/config/tests/sources_schema.rs`:
     - `[[sources]]` with `type = "album-name"` round-trips.
     - An unknown `type = "flickr"` loads as `SourceType::Unsupported`.
     - A missing `sources` gives the default three rows.
     - `sources = []` stays empty.
  2. Add `SourceEntry` and `SourceType` (see Architecture) and `pub sources: Vec<SourceEntry>` on `Config`. Place it after `local` so the TS field order is stable.
  3. In `defaults.rs`, add rows `{id:"bing",type:bing}`, `{id:"wh-nature",type:wallhaven,location:"nature"}` and `{id:"favorites",type:folder,location:<favorites dir>}`, all enabled. Use `defaults_for_home` so the favorites path follows `home`.
  4. In `load.rs::expand_all`, expand `~` in `location` for `Folder | Images | AlbumName | AlbumDate`. In `rebase_defaults`, rebase the favorites row's location.
  5. Add a commented `[[sources]]` example block to `template.toml`. Leave the legacy sections alone for now.
  6. Extend `json_wire.rs` to assert that `sources[0].type` is serialized under the key `"type"`.
  7. Regenerate the bindings and add `sources` to `testConfig.ts`.
- **Automated verification:**
  - `cargo test -p kakemono-config`
  - `cargo test -p kakemono-ui --features export-bindings`
  - `git status crates/ui/frontend/src/shared/bindings` shows `SourceEntry.ts` and `SourceType.ts`
  - `cargo clippy --workspace --all-targets`
  - `cd crates/ui/frontend && npx tsc -b --noEmit && npx vitest run`
- **Manual verification:** none

## Task 2: Config: new `[daemon]` keys and `[filters]` table

- **Files owned:** `crates/config/src/schema.rs`, `crates/config/src/defaults.rs`, `crates/config/src/load.rs`, `crates/config/src/template.toml`, `crates/config/tests/json_wire.rs`, bindings, `crates/ui/frontend/src/settings/testConfig.ts`
- **Depends on:** Task 1
- **Mode:** sequential
- **Steps:**
  1. Tests:
     - defaults are `change_on_start=false`, `internet=true`, `download_enabled=true`, `download_interval="10m"`, `download_limit_mb_enabled=true`, `download_limit_mb=500`, `manual_dir=None`
     - `manual_dir` serializes as JSON `null`
     - `DaemonConfig::manual_dir_resolved()` returns `download_dir/manual` when `None`, and expands `~` when set
     - `download_interval_duration()` parses `"10m"`
     - `FiltersConfig` defaults are all off, with `min_size_percent=80`, `min_rating=4`, `brightness="off"` and `color="#3b82f6"`
     - `brightness = "system"` round-trips
  2. Add the fields, `FiltersConfig`, `BrightnessFilter` (`rename_all = "lowercase"`) and `pub filters: FiltersConfig` on `Config`.
  3. Expand `manual_dir` in `expand_all`.
  4. Add the template entries with comments.
  5. Regenerate the bindings and update `testConfig.ts`.
- **Automated verification:** same commands as Task 1.
- **Manual verification:** none

## Task 3: Config: source-id helpers and `normalize_sources`

- **Files owned:** `crates/config/src/sources.rs` (new), `crates/config/src/lib.rs`
- **Depends on:** Task 1
- **Mode:** sequential (shares `lib.rs` with Task 4)
- **Steps:**
  1. Tests in `sources.rs`:
     - `valid_id("wh-1a2b")` is true; `"../x"`, `""`, `"A"` and 33 characters are false.
     - `deterministic_id(Wallhaven,"nature",&{})` is stable across calls and starts with `wh-`.
     - A collision gives a `-2` suffix.
     - `normalize_sources` replaces an empty, invalid or duplicate id and returns one warning for each.
     - `is_online` and `takes_input` hold for every variant. `Bing` and `Apod` don't take input; `Folder`/`Images`/`Album*` take a path.
  2. Implement:
     - prefixes `wh`, `us`, `rd`, `bing`, `apod`, `dir`, `img`, `aln`, `ald`, `unk`
     - FNV-1a 64-bit over `"{type}\0{location}"`, first 4 hex characters
  3. Re-export from `lib.rs`.
- **Automated verification:** `cargo test -p kakemono-config sources`, `cargo clippy -p kakemono-config --all-targets`
- **Manual verification:** none

## Task 4: Config: pure legacy → `[[sources]]` migration

- **Files owned:** `crates/config/src/migrate.rs` (new), `crates/config/src/lib.rs`, `crates/config/tests/migrate.rs` (new), `crates/config/tests/fixtures/migrate/*.toml` (new)
- **Depends on:** Task 3
- **Mode:** sequential
- **Steps:**
  1. Copy the legacy structs and their defaults into `migrate::legacy`:
     - `ThemeConfig`, with the current light/dark default lists copied verbatim from `defaults.rs`
     - the provider `enabled` flags, `RedditConfig.subreddits` with its default list, `LocalConfig.dirs` and `recursive`, and `cache_size`

     Deserialize them with `toml::from_str` from `doc.to_string()`.
  2. `pub fn needs_migration(doc: &DocumentMut) -> bool`: no `sources` key, and any of `light`, `dark`, `daemon.cache_size`, `<provider>.enabled`, `reddit.subreddits` or `local.dirs` present.
  3. `pub fn migrate_document(doc: &mut DocumentMut, home: &Path) -> Option<MigrationReport { rows: Vec<SourceEntry>, notes: Vec<String> }>`. It applies the spec's rules in this order: bing, wallhaven, unsplash, reddit, apod, local.
     - **Wallhaven and Unsplash:** one row per distinct query (light queries first, then dark, de-duplicated, order kept), or one row with an empty query if there are none.
     - **Other providers:** `enabled` comes from the provider flag. A provider that's disabled but listed in either theme's `sources` becomes a disabled row. A provider that's disabled and unlisted gets no row.
     - **Reddit:** one row per subreddit.
     - **Local:** one `folder` row per dir, with `~` collapsed back to `~/` when the path is under `home`.
     - **Ids:** `deterministic_id`.
     - **Cleanup:** remove the legacy keys and emptied tables, then append `[[sources]]`. Add a note when `local.recursive == false`.
  4. Fixture tests, each asserting the full resulting TOML with `insta`:
     - `go_template.toml` (the old template)
     - `light_dark_split.toml`
     - `disabled_but_listed.toml`
     - `reddit_multi.toml`
     - `local_dirs.toml`
     - `no_legacy.toml` (returns `None`)
     - `explicit_empty_sources.toml` (returns `None`)
     - `comments.toml`: a comment in `[daemon]` survives

     Also assert that a second run is a no-op (idempotent) and that two runs give the same ids (deterministic).
- **Automated verification:** `cargo test -p kakemono-config --test migrate`, `cargo insta test -p kakemono-config` (review the snapshots), `cargo clippy -p kakemono-config --all-targets`
- **Manual verification:** run the migration against a copy of the user's real `~/.config/kakemono/config.toml` with a scratch test (`KAKEMONO_MIGRATE_FIXTURE=path cargo test -p kakemono-config --test migrate real_config -- --ignored`) and check that no source or query is missing.

## Task 5: Config: `apply_json` writes arrays of objects as `[[tables]]`

- **Files owned:** `crates/config/src/edit.rs`, `crates/config/tests/edit_sources.rs` (new)
- **Depends on:** Task 1
- **Mode:** parallel-safe (with Tasks 3, 4, 6–11)
- **Steps:**
  1. Tests:
     - patching `{"sources":[{id,enabled,type,location}, …]}` onto a doc with no `sources` produces `[[sources]]` blocks
     - patching onto an existing array of tables replaces it, keeps the comment above the first block, and doesn't touch other tables
     - patching `[]` removes every block and writes `sources = []`
     - patching `{"filters":{"brightness":"dark"}}` merges as a table (regression)
  2. In `apply_json`, when the JSON value is an array whose elements are all objects, and the target is missing or is an `ArrayOfTables`, build a `toml_edit::ArrayOfTables`. Otherwise keep the existing behaviour.
- **Automated verification:** `cargo test -p kakemono-config --test edit_sources`, `cargo test -p kakemono-config`
- **Manual verification:** none

## Task 6: Sources: per-instance online sources

- **Files owned:** `crates/sources/src/instance.rs` (new), `crates/sources/src/lib.rs`, `crates/sources/src/reddit.rs`, `crates/sources/src/unsplash.rs`, `crates/sources/src/wallhaven/*` (constructor and URL parsing only), `crates/sources/src/fake.rs`
- **Depends on:** Task 3
- **Mode:** parallel-safe (with Tasks 5, 7, 8, 10, 11)
- **Steps:**
  1. Unit tests for the location parsers:
     - `reddit_location("https://www.reddit.com/r/EarthPorn/top/?t=week") == "EarthPorn"`, and `"r/earthporn"` and `"earthporn"` work too
     - `wallhaven_location("https://wallhaven.cc/search?q=forest&categories=100&purity=100&sorting=toplist")` gives the query `forest` with overrides
     - `"/tag/37"` gives `id:37`
     - plain keywords pass through
     - `unsplash_location("https://unsplash.com/s/photos/mountain-lake")` gives `"mountain lake"`
     - other Unsplash URLs give a `NotConfigured` error
  2. Add the constructors:
     - `RedditProvider::for_subreddit(name, http)`
     - `WallhavenProvider::with_overrides(cfg, http, overrides)`
     - Unsplash unchanged, with the query passed through `fetch`

     `build_online` ignores the legacy `enabled` flags: the instance is enabled by its row.
  3. `SourceSet::from_config(cfg, http)` skips disabled, local and `Unsupported` rows and logs build errors. In `fake.rs`, add `SourceSet::fake(vec![(id, Arc<dyn Provider>, query)])` behind `feature = "fake"`.
  4. A wiremock test: a Wallhaven instance built from a URL sends the overridden `categories`/`purity` params.
- **Automated verification:** `cargo test -p kakemono-sources`, `cargo test -p kakemono-sources --features fake`, `cargo clippy -p kakemono-sources --all-targets`
- **Manual verification:** none

## Task 7: Sources: local listing and album helpers

- **Files owned:** `crates/sources/src/local.rs`
- **Depends on:** Task 1
- **Mode:** parallel-safe (does not touch `lib.rs`, because `local` is already a module)
- **Steps:**
  1. Tests using `tempfile` (dev-dependency; add it to `crates/sources/Cargo.toml` only if it's missing, and record that the Cargo file is owned here in that case):
     - `list_images(dir, true)` finds nested jpg/png/webp, skips `.hidden` and `.kakemono-seen`, and ignores case in extensions
     - `album_next(dir, None, Name)` returns the first file by name; `album_next(dir, Some(last), Name)` wraps around
     - `Date` orders by mtime (set with `filetime`, or by sleeping between writes)
  2. Implement them with `std::fs` walking (iteration, not recursion) and a cap of 50 000 entries.
- **Automated verification:** `cargo test -p kakemono-sources local`
- **Manual verification:** none

## Task 8: Engine: image facts and the facts cache

- **Files owned:** `crates/engine/src/facts.rs` (new), `crates/engine/src/lib.rs`, `crates/engine/Cargo.toml`, root `Cargo.toml` (workspace deps `kamadak-exif`, `regex` if missing; `image` already exists)
- **Depends on:** none
- **Mode:** parallel-safe
- **Steps:**
  1. Tests generate images with `image::RgbImage` into a tempdir:
     - a black 200×100 image gives `luminance < 0.05`, a 200×100 size, and a palette dominated by `[0,0,0]`
     - a white image gives `> 0.95`
     - a JPEG with an XMP packet `xmp:Rating="4"` gives `rating == Some(4)` (write a minimal JPEG with an APP1 XMP segment by hand)
     - no rating gives `None`
     - `FactsCache::get_or_compute` computes once, then returns the cached value; changing the mtime recomputes
     - `save` drops missing paths, and `load` survives a corrupt JSON file by starting empty and warning
  2. Implement `ImageFacts`, `compute(path) -> io::Result<ImageFacts>` (blocking), and `FactsCache { path, map: Mutex<HashMap<PathBuf, Entry>> }` with `async fn facts(&self, path) -> Option<ImageFacts>` (`spawn_blocking`) and a debounced `save()`.
- **Automated verification:** `cargo test -p kakemono-engine facts`, `cargo clippy --workspace --all-targets`
- **Manual verification:** none

## Task 9: Engine: soft filters

- **Files owned:** `crates/engine/src/filters.rs` (new), `crates/engine/src/lib.rs`
- **Depends on:** Task 2, Task 8
- **Mode:** sequential (`lib.rs`)
- **Steps:**
  1. Table-driven tests over a hand-built `ImageFacts`:
     - landscape passes 1920×1080 and fails 1080×1920
     - `min_size_percent=80` with screen 2560×1440 passes 2048×1152 and fails 1920×1080
     - `brightness=Dark` passes luminance 0.2 and fails 0.7; `System` follows `ctx.theme`
     - color `#ff0000` passes a palette containing `[250,10,10]` and fails a palette of `[0,0,255]`
     - rating 4 passes `Some(5)` and fails `None` and `Some(3)`
     - `name_regex="^wall"` matches on the file name only
     - an invalid regex gives a compile warning and the filter is skipped
     - `score` counts passes
  2. Implement `Filters::compile`, `check` and `score`, with sRGB→Lab conversion and CIE76. Luminance is Rec. 709 relative luminance; thresholds per the spec.
- **Automated verification:** `cargo test -p kakemono-engine filters`
- **Manual verification:** none

## Task 10: Platform: `screen_size()`

- **Files owned:** `crates/platform/src/api.rs`, `crates/platform/src/fake.rs`, `crates/platform/src/macos/*` (the signals implementation file only)
- **Depends on:** none
- **Mode:** parallel-safe
- **Steps:**
  1. Add `fn screen_size(&self) -> Option<(u32, u32)> { None }` to the signals trait in `api.rs`.
  2. Make the fake settable (`set_screen_size`).
  3. On macOS, use the largest `NSScreen.screens` frame × `backingScaleFactor` (on the main thread if the existing code requires it; follow the patterns in the existing macOS signals).
  4. Add a test that the fake returns its set value, and a test that the default returns `None`.
- **Automated verification:** `cargo test -p kakemono-platform --features fake`, `cargo clippy --workspace --all-targets`
- **Manual verification:** on macOS, a scratch `tracing::info!` or a `kakemono doctor` line shows the right physical resolution (optional).

## Task 11: Engine: MB-based `trim_to_mb`

- **Files owned:** `crates/engine/src/files.rs`
- **Depends on:** Task 2
- **Mode:** parallel-safe (with 8, 9, 10)
- **Steps:**
  1. Tests in a tempdir:
     - `downloads/a/` (3 × 400 KB, increasing mtimes), `downloads/manual/` (1 MB) and a legacy root file. `trim_to_mb(limit=1 MB)` deletes the oldest files in `a/` and the root file, and never deletes anything in `manual/`, the current file or history files.
     - A favorites dir nested inside `download_dir` is spared.
     - `.kakemono-seen` files don't count and aren't deleted.
     - Nothing happens when already under the limit.
  2. Implement `pub struct Protect { dirs: Vec<PathBuf>, files: HashSet<PathBuf> }`, `pub fn safe_delete(path, download_dir, &Protect) -> io::Result<()>` (refuses anything outside `download_dir` or under a protected dir, protected files, symlink escapes; canonicalise both sides) and `pub async fn trim_to_mb(download_dir, limit_bytes, &Protect)`, which deletes only through `safe_delete`. Add `safe_delete` tests: `../` escape, a symlink pointing outside, a local-source dir nested inside `download_dir`, the current wallpaper. Keep `trim_cache` for now.
- **Automated verification:** `cargo test -p kakemono-engine files`
- **Manual verification:** none

## Task 12: Engine: local pool and pick, plus album cursors in state

- **Files owned:** `crates/engine/src/pool.rs` (new), `crates/engine/src/lib.rs`, `crates/engine/src/state.rs`
- **Depends on:** Task 6, Task 7, Task 9
- **Mode:** sequential
- **Steps:**
  1. Tests:
     - `Pool::collect` includes, for enabled rows only: online `download_dir/<id>`, `folder`, `images` and the implicit `manual`. Disabled rows and `Unsupported` are excluded.
     - `pick` with a dark filter and two candidates (one dark, one light) returns the dark one.
     - When all candidates fail, `pick` returns the best score and `PickOutcome::Fallback { failed }`.
     - The current wallpaper is excluded unless it's the only image.
     - An album source returns the next file and the cursor advances in `State`.
     - An empty pool returns `None`.
     - `State` with `album_cursors` round-trips, and an old state JSON without the field still loads.
  2. Implement `Pool`, `PoolSource`, and `pick(pool, cfg, filters, ctx, facts, state, rng) -> Option<Picked { path, source_id, kind }>`, using a seeded rng in tests.
- **Automated verification:** `cargo test -p kakemono-engine pool state`, `cargo clippy --workspace --all-targets`
- **Manual verification:** none

## Task 13: Engine: background downloader

- **Files owned:** `crates/engine/src/downloader.rs` (new), `crates/engine/src/lib.rs`
- **Depends on:** Task 11, Task 12
- **Mode:** sequential
- **Steps:**
  1. Tests use `tokio::time::pause`, a fake `SourceSet` (Task 6) and wiremock to serve image bytes:
     - an empty source folder gets one image right after the downloader starts
     - after `download_interval`, one more image is downloaded for the source with the fewest files
     - a URL already in `.kakemono-seen` isn't downloaded again
     - a downloaded image that fails the dark filter is deleted, and its URL is recorded as seen
     - `RateLimited` sets a backoff; the source isn't tried again before the backoff ends, while other sources are
     - `internet=false` or `download_enabled=false` means no requests at all (wiremock `expect(0)`)
     - the MB limit is enforced after each download
     - `notify` (a reload) wakes the loop immediately
  2. Implement `Downloader { backoff: HashMap<String, Backoff> }`, `run(engine, shutdown, wake: Arc<Notify>)` and `download_one(engine, &OnlineSource) -> Result<PathBuf, String>`, with error text in the existing `provider {name} fetch: {e}` format. Facts are stored in the cache as each download lands.
  3. Don't wire it into `Engine::run` yet. That's Task 14.
- **Automated verification:** `cargo test -p kakemono-engine downloader`
- **Manual verification:** none

## Task 14: Engine: switch changes to pick-from-disk; change_on_start; internet switch

- **Files owned:** `crates/engine/src/engine.rs`, `crates/engine/src/rotation.rs`, `crates/engine/tests/*` (existing engine integration tests that exercise `next`)
- **Depends on:** Task 10, Task 13
- **Mode:** sequential
- **Steps:**
  1. Update or add tests:
     - `next` with images in `downloads/<id>/` sets a local file without any HTTP request
     - with an empty pool and `internet=true`, `next` downloads on demand
     - with `internet=false` and an empty pool, `next` returns `NoImages`
     - `change_on_start=true` gives exactly one rotation at `run` start (paused clock); `false` gives none
     - `reload` with a new source wakes the downloader
     - the screen size from `signals.screen_size()` feeds the filter context, falling back to 1920×1080
  2. Replace `fetch_and_set` with `pick_and_set`. Swap `registry` for `sources: Arc<SourceSet>` in the snapshot, and add `facts: Arc<FactsCache>` (path `state_file.with_file_name("facts.json")`) and `wake: Arc<Notify>`. `Deps::with_sources_factory` replaces `with_registry_factory`; keep `with_registry` only if callers outside the engine use it, or remove it and fix callers in `crates/cli`.
  3. Remove `NoProvider` and its Go-compat test, and add `NoImages`.
  4. `Engine::run` spawns the downloader next to the rotation loop under the same `CancellationToken`.
  5. Stop calling `trim_cache` from the change path.
  6. Add the `change_on_start` branch in `rotation.rs::run`.
- **Automated verification:** `cargo test --workspace`, `cargo clippy --workspace --all-targets`
- **Manual verification:** none (the end-to-end check is in Task 27)

## Task 15: Wire migration into load, the daemon start and the UI `get_config`

- **Files owned:** `crates/config/src/load.rs`, `crates/config/src/lib.rs`, `crates/config/tests/migrate_file.rs` (new), `crates/cli/src/daemon/mod.rs`, `crates/ui/src-tauri/src/commands.rs` (`get_config` only)
- **Depends on:** Task 4, Task 14
- **Mode:** sequential
- **Steps:**
  1. Tests:
     - `parse_str(legacy_text)` gives `cfg.sources` equal to the migration rows
     - `migrate_file` writes `config.toml.bak` with the original bytes and rewrites `config.toml`; a second call returns `None` and doesn't touch the `.bak`
     - with an existing `.bak`, it writes `.bak.<ts>`
     - a read-only file returns `Err`, and the loader still returns the migrated config in memory
     - `normalize_sources` runs in `parse_str` (a bad id is fixed in memory)
  2. In `parse_str`:
     1. parse to `DocumentMut`
     2. if `needs_migration`, run `migrate_document`
     3. `toml::from_str(&doc.to_string())`
     4. `normalize_sources`
  3. Add `pub fn migrate_file(path, home)` using `edit::write_atomic`.
  4. Call it in the daemon's `run` before the config loads, and in the UI's `get_config` before reading. Log the `MigrationReport.notes`.
- **Automated verification:** `cargo test -p kakemono-config`, `cargo test --workspace`, `cargo clippy --workspace --all-targets`
- **Manual verification:** with a copy of the user's config in a temporary `XDG_CONFIG_HOME` (or the macOS equivalent the `paths.rs` override supports), start `kakemono daemon`. Check that `config.toml.bak` equals the original and that the new file has the expected `[[sources]]`.

## Task 16: IPC: `check_source` and `fetch`; `Engine::check_source` / `fetch_url`

- **Files owned:** `crates/protocol/src/types.rs`, `crates/engine/src/manual.rs` (new), `crates/engine/src/lib.rs`, `crates/cli/src/daemon/server.rs`
- **Depends on:** Task 14
- **Mode:** contract-first (Task 17 and the UI depend on it)
- **Steps:**
  1. Protocol tests:
     - `{"cmd":"check_source","args":{"type":"wallhaven","location":"forest"}}` parses to `CheckSource`
     - a missing `type` gives the new `RequestError::MissingType`
     - `{"cmd":"fetch","args":{"url":"https://x/y.jpg"}}` parses to `Fetch`, and an empty url gives `MissingUrl`
     - `From<Request> for Command` round-trips
     - existing parse tests are unchanged
  2. Engine tests:
     - `check_source` with a fake provider returning 3 results gives `Ok(3)`; with 0 it gives `Err("no images found")`; a provider error comes back as its display text
     - `check_source` for a `folder` counts its images
     - `internet=false` with an online type gives `Err("Internet access is off")`
     - `fetch_url` with a wiremock URL saves into `manual_dir` and sets it, and history gains the entry
     - `fetch_url` with an absolute file path copies the file into `manual_dir`
  3. Also add `DownloadNow { id: Option<String> }` (`"download_now"`, optional arg `id`) that calls `Engine::download_one` for that source, or for every enabled online source with no files when `id` is absent. Response data: `{"path": string | null}`. Protocol round-trip test plus an engine test with a fake source.
  4. Server: route all three requests. The responses are `{"count":n}`, `WallpaperInfo` and `{"path":…}`; errors use the existing `Response` error shape. The downloader's filter-reject delete uses `files::safe_delete`.
- **Automated verification:** `cargo test -p kakemono-protocol`, `cargo test -p kakemono-engine manual`, `cargo test --workspace`
- **Manual verification:** with the daemon running, `printf '{"cmd":"check_source","args":{"type":"bing","location":""}}\n' | nc -U <socket>` returns a count.

## Task 17: Tauri: source commands, the dropped-file/URL commands, thumbnails from sources

- **Files owned:** `crates/ui/src-tauri/src/commands_sources.rs` (new), `crates/ui/src-tauri/src/lib.rs`, `crates/ui/src-tauri/src/commands.rs` (`thumbnails` only), `crates/ui/src-tauri/src/daemon_link.rs` (only if a generic request helper is missing), `crates/ui/frontend/src/shared/api.ts`, `crates/ui/frontend/src/shared/api.test.ts`
- **Depends on:** Task 15, Task 16
- **Mode:** sequential
- **Steps:**
  1. Rust unit tests for the pure helpers:
     - `resolve_source_dir(cfg, id)` refuses an invalid id, `manual`, favorites and trash, and accepts `download_dir/<id>`
     - `open_target(entry, cfg)`: a folder gives its location, `images` gives the parent, online gives `download_dir/<id>`
     - `thumbnail_dirs(cfg)` gives enabled online dirs, then manual, then local, then favorites, de-duplicated
  2. Implement:
     - `check_source`, `download_now(id)`, `delete_source_files` (via `files::safe_delete` semantics: direct child of `download_dir` only), `open_source_folder` (through the existing open helper in `kakemono-platform::open` or the `reveal` code path)
     - `fetch_dropped_url`, `import_dropped_file` (a unique name in `manual_dir`, extension checked against jpg/jpeg/png/webp, 64 MB cap)

     Register them all in `generate_handler!`.
  3. Add typed wrappers in `api.ts` (`checkSource`, `deleteSourceFiles`, `openSourceFolder`, `fetchDroppedUrl`, `importDroppedFile`) with vitest tests that mock `invoke`.
- **Automated verification:** `cargo test -p kakemono-ui`, `cargo clippy --workspace --all-targets`, `cd crates/ui/frontend && npx tsc -b --noEmit && npx vitest run`
- **Manual verification:** none

## Task 18: Frontend: two-tab shell and `sourceTypes.ts`

- **Files owned:** `crates/ui/frontend/src/settings/sections.ts`, `settings/App.tsx`, `settings/App.test.tsx`, `settings/components/Sidebar.tsx` (deleted), `settings/components/Tabs.tsx` (new), `settings/sourceTypes.ts` (new), `settings/sourceTypes.test.ts` (new), `settings/components/SourcesTable.tsx` (new stub)
- **Depends on:** Task 2 (bindings). It can start before the engine work is done.
- **Mode:** sequential
- **Steps:**
  1. Tests:
     - App renders two tabs, "General" and "Advanced"; clicking Advanced shows the Advanced content; the default is General
     - `sourceTypes` has an entry for each `SourceType`
     - `ONLINE_TYPES` is sorted A–Z: APOD, Bing, Reddit, Unsplash, Wallhaven
     - the prompts match the spec ("URL or name of a subreddit:", "Search keyword or URL:", "Enter keywords or paste URL here:")
     - `newSourceId(kind)` matches `/^[a-z]+-[0-9a-f]{4}$/`
  2. Change `sections.ts` to `TabId = 'general' | 'advanced'`. Keep `fileManagerName`. `SOURCES`/`SOURCE_LABELS` stay until Task 24.
  3. Add `Tabs.tsx` built on `Segmented.tsx` styling.
  4. `App.tsx` renders `<GeneralSettings/>` or `<AdvancedSettings/>`. Mount the current Sources, Picker and Desktop sections inside them for now, so nothing disappears. `SourcesTable.tsx` is a stub that renders `null`.
- **Automated verification:** `cd crates/ui/frontend && npx tsc -b --noEmit && npx vitest run`
- **Manual verification:** `npm run tauri dev` (or the project's dev command): the settings window shows two tabs.

## Task 19: Frontend: Sources table with enable, Remove, Open Folder, Use

- **Files owned:** `settings/components/SourcesTable.tsx`, `settings/components/SourcesTable.test.tsx` (new), `settings/components/RemoveSourceDialog.tsx` (new)
- **Depends on:** Task 17, Task 18
- **Mode:** parallel-safe (with Tasks 21 and 23)
- **Steps:**
  1. Tests, with the store and the api mocked:
     - one row per `config.sources`, with the Enabled checkbox, a Type label and a Location ("—" for bing/apod)
     - toggling a checkbox patches the `sources` array with only that row's `enabled` flipped
     - multi-select with shift- or cmd-click
     - "Use" patches `enabled = selected` for every row and then calls `next`
     - Remove opens a dialog
       - for online rows it shows "Also delete downloaded files", which calls `deleteSourceFiles(id)` when checked
       - for local rows the checkbox is absent
       - confirming removes the row
     - "Open Folder" calls `openSourceFolder(id)`
     - with `daemon.internet=false`, online rows are greyed (`aria-disabled`)
     - Edit is disabled for bing and apod
  2. Implement the table with keyboard selection and the existing `Group` and `Toggle` components. Add/Edit buttons render but are wired in Task 20; they're disabled with a "coming" tooltip until then.
- **Automated verification:** `npx tsc -b --noEmit && npx vitest run SourcesTable`
- **Manual verification:** none

## Task 20: Frontend: Add… menu, source dialog with live validation, Edit…

- **Files owned:** `settings/components/AddSourceMenu.tsx` (new), `settings/components/SourceDialog.tsx` (new), their `.test.tsx` files, `settings/components/SourcesTable.tsx`
- **Depends on:** Task 19
- **Mode:** sequential
- **Steps:**
  1. Tests:
     - the menu lists the local group (Images, Folders, Sequential album by file name, Sequential album by file date), then a separator, then the online types A–Z, each with a bold name and a description
     - with `internet=false`, the online items are hidden
     - choosing Bing adds `{type:'bing', location:'', enabled:true, id:'bing-xxxx'}` directly; choosing Bing when a Bing row exists shows "Bing is already in the list" and adds nothing
     - choosing Wallhaven opens the dialog with "Enter keywords or paste URL here:"
       - OK calls `checkSource` and shows "Just a moment to check for images"
       - `{count:5}` adds the row
       - an error or 0 shows the error inline, adds nothing, and keeps the dialog open
       - Cancel adds nothing
     - Edit opens the dialog prefilled and validates again; OK updates `location` and keeps the `id`
     - local types open the folder or file picker (reuse `PathField`'s picker call). Images adds one `images` row per selected file. Validation uses `checkSource` as well.
     - "Add anyway" appears only when `checkSource` fails with a daemon-unreachable error
  2. Implement and wire into `SourcesTable`.
- **Automated verification:** `npx tsc -b --noEmit && npx vitest run`
- **Manual verification:** add a Wallhaven "forest" source in the running app. It validates against the live site, and after one download interval `downloads/<id>/` contains images.

## Task 21: Frontend: General groups

- **Files owned:** `settings/components/GeneralSettings.tsx`, `settings/components/DownloadingGroup.tsx` (new), `settings/components/ShortcutHint.tsx` (new), `settings/components/GeneralSettings.test.tsx` (new), `settings/components/ShortcutHint.test.tsx` (new), `settings/components/PickerSettings.tsx` (only if it needs adapting to sit inside General)
- **Depends on:** Task 18
- **Mode:** parallel-safe (with Tasks 19 and 23)
- **Steps:**
  1. Tests:
     - General renders, in this order: "Start at login" (the existing `StartAtLogin`), "Change wallpaper every [N][unit]", "Change wallpaper on start", "Use Internet access", the sources table, filters (placeholder until Task 22), Downloading ("Download a wallpaper every [N][unit]" and "Limit download folder to [N] MB, deleting the oldest files when necessary"), "Favorites: copy favorite wallpapers to [folder]", the Picker, then Shortcuts
     - each control patches the right key (`daemon.change_on_start`, `daemon.internet`, `daemon.download_enabled`, `daemon.download_interval` via `duration.ts`, `daemon.download_limit_mb_enabled`, `daemon.download_limit_mb` as an integer ≥ 10, `favorites.dir`)
     - ShortcutHint on macOS shows the copyable `kakemono pick` / `kakemono settings` and the Shortcuts.app, Raycast and skhd how-tos; on Linux it shows GNOME/KDE steps and the Hyprland, Sway and niri bind lines; the copy button writes to the clipboard (mocked)
  2. Implement them using `app_info().os` for the OS. Move the "Now showing" card to the top of General if it's already there.
- **Automated verification:** `npx tsc -b --noEmit && npx vitest run`
- **Manual verification:** visually check the General tab in light and dark appearance.

## Task 22: Frontend: Filters group

- **Files owned:** `settings/components/FiltersGroup.tsx` (new), `settings/components/FiltersGroup.test.tsx` (new), `settings/components/GeneralSettings.tsx` (mount line only)
- **Depends on:** Task 21
- **Mode:** sequential
- **Steps:**
  1. Tests:
     - the heading reads "When possible, use images that…"
     - "Have landscape orientation" patches `filters.landscape`
     - "Are big, at least [50|80|100] % of the screen resolution" patches `min_size_enabled` and `min_size_percent`
     - "Are [Dark|Light|Match system appearance]" with a checkbox patches `brightness` (unchecked means `off`)
     - "Contain this color [picker]" uses `<input type="color">` and patches `color_enabled` and `color`
     - "Have EXIF rating at least [1–5]" patches `rating_enabled` and `min_rating`
     - "File name matches regex [text]" patches the regex fields; an invalid JS regex shows an inline warning but still saves
  2. Implement and mount it in General under the sources table.
- **Automated verification:** `npx tsc -b --noEmit && npx vitest run FiltersGroup`
- **Manual verification:** with the dark filter on, press Next a few times in the running app. Dark images are picked when they exist locally (acceptance).

## Task 23: Frontend: Advanced tab

- **Files owned:** `settings/components/AdvancedSettings.tsx`, `settings/components/ProviderKeys.tsx` (new), `settings/components/AdvancedSettings.test.tsx` (new), `settings/DesktopSettings.tsx` (only to drop its own page chrome when embedded)
- **Depends on:** Task 18
- **Mode:** parallel-safe (with Tasks 19 and 21)
- **Steps:**
  1. Tests:
     - Advanced shows, in order:
       - Folders: download folder, "Save manually downloaded wallpapers to [folder]" (empty means "Default: <download_dir>/manual" and patches `daemon.manual_dir` to `null` or a path), trash folder
       - Wallpaper: "Display mode" (`desktop.fit`)
       - Desktop: backend, custom set command, apply to all Spaces (macOS only), reapply on Space change
       - Hooks (`on_change`), socket path
       - Appearance (Light, Dark, System)
       - Provider credentials: Wallhaven API key plus purity, sorting and categories; Unsplash access key; APOD API key; Bing market
     - each control patches the right key
     - API keys render as password fields with show/hide
  2. Move the existing controls from the old Sources and Advanced sections. Don't reimplement them.
- **Automated verification:** `npx tsc -b --noEmit && npx vitest run`
- **Manual verification:** none

## Task 24: Remove the legacy schema fields and code paths

- **Files owned:**
  - config: `crates/config/src/schema.rs`, `defaults.rs`, `load.rs`, `template.toml`, `tests/json_wire.rs`
  - sources: `crates/sources/src/registry.rs`, `crates/sources/src/local.rs` (the `LocalProvider` config constructor)
  - engine: `crates/engine/src/files.rs` (remove `trim_cache`)
  - CLI: `crates/cli/src/**` (only files that reference legacy fields, found by grep), `crates/cli/src/commands/snapshots/*`
  - frontend: bindings, `settings/testConfig.ts`, `settings/components/SourcesSettings.tsx` (deleted), `settings/sections.ts` (`SOURCES`/`SOURCE_LABELS` removed), `settings/validate.ts`, `settings/configPatch.ts` (legacy cases)
- **Depends on:** Tasks 14, 15, 19, 20, 21, 22, 23
- **Mode:** sequential
- **Steps:**
  1. Run `rg -n "\.light\b|\.dark\b|ThemeConfig|cache_size|subreddits|local\.dirs|\.enabled\b" crates` and list every hit.
  2. Remove from `Config`: `light` and `dark` (and the `light_theme_serde`/`dark_theme_serde` modules), `DaemonConfig.cache_size`, the provider `enabled` fields, `RedditConfig.subreddits`, `LocalConfig.dirs`/`recursive` (drop `LocalConfig` if it ends up empty). `migrate::legacy` keeps its own copies.
  3. Remove `Registry::from_config` and `candidates`, or keep `Registry` only if tests need it.
  4. Update `doctor`, `status` and `firstrun` to report on `[[sources]]`.
  5. Regenerate the bindings; fix the TypeScript and snapshots.
  6. Remove the legacy sections from `template.toml`.
  7. Add a test that loading a legacy file still migrates correctly after the cleanup (this guards `migrate::legacy`).
- **Automated verification:** `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo insta test --workspace` (review), `cargo test -p kakemono-ui --features export-bindings`, `cd crates/ui/frontend && npx tsc -b --noEmit && npx vitest run`
- **Manual verification:** none

## Task 25: Drag and drop on the picker and settings windows

- **Files owned:**
  - `crates/ui/frontend/src/shared/useImageDrop.ts` (new), `crates/ui/frontend/src/shared/useImageDrop.test.tsx` (new)
  - `crates/ui/frontend/src/picker/App.tsx`, `crates/ui/frontend/src/settings/App.tsx`
  - `crates/ui/src-tauri/src/windows.rs` and `crates/ui/src-tauri/src/picker.rs` (`.disable_drag_drop_handler()` on the builders), or `crates/ui/src-tauri/tauri.conf.json` if the windows are declared there
- **Depends on:** Task 17, Task 24
- **Mode:** sequential
- **Steps:**
  1. Tests for the hook, with synthetic `DragEvent`s and a mocked `DataTransfer`:
     - `text/uri-list` with `https://…/a.jpg` calls `fetchDroppedUrl`
     - a `File` of `image/png` calls `importDroppedFile(name, bytes)`
     - non-image files are ignored with a toast
     - only the first item of a multi-drop is used
     - `dragover` shows the overlay state
     - an error is surfaced
  2. Disable the native handler on both windows.
  3. Add a drop overlay ("Drop an image to set it") in both Apps.
- **Automated verification:** `npx tsc -b --noEmit && npx vitest run useImageDrop`, `cargo clippy --workspace --all-targets`
- **Manual verification:**
  - On macOS, and on one Linux Wayland compositor if available:
    - dragging an image from Finder or a file manager onto the picker sets it and saves a copy in `manual_dir`
    - dragging an image from a browser (URL) onto the picker sets it and saves it in `manual_dir` (acceptance)
  - Same on the settings window.
  - Picker window-dragging still works.

## Task 26: Docs: TODO (clipboard), migration notes, template comments

- **Files owned:** `docs/TODO.md`, `docs/migration.md`
- **Depends on:** none
- **Mode:** parallel-safe
- **Steps:**
  1. In `docs/TODO.md`, add "Clipboard monitoring (Variety item 18)" with notes:
     - macOS: poll `NSPasteboard.changeCount` every 1 s
     - Wayland: `wl-paste --watch`
     - X11: XFixes `SelectionNotify`
     - feed image URLs into `fetch`
  2. Also in `docs/TODO.md`, add "Screen size on Linux/Windows for the size filter" and "Listing cache for large local folders".
  3. In `docs/migration.md`, add a section "Sources list (2026-07)":
     - what the migration does and where the `.bak` is
     - that `[local] recursive=false` becomes recursive
     - how to restore from the backup
- **Automated verification:** `test -f docs/TODO.md && grep -q "Clipboard monitoring" docs/TODO.md`
- **Manual verification:** none

## Task 27: End-to-end acceptance pass

- **Files owned:** none (verification only; fixes go back to the task that owns the file)
- **Depends on:** Tasks 1–26
- **Mode:** sequential
- **Steps:**
  1. Run the full gate.
  2. Fresh install, with an empty config dir: the settings window opens on General with Bing, Wallhaven "nature" and Favorites enabled. Within a minute, `downloads/bing/` and `downloads/wh-nature/` hold images. Turn off the network: Next still works.
  3. Migration: put the user's real config in place and start the daemon. `config.toml.bak` equals the original, and every old source and query appears as a row.
  4. Add Wallhaven "forest": it validates live, and images appear in `downloads/<id>/` within one download interval.
  5. Remove that row with "delete files" checked: only `downloads/<id>/` is gone.
  6. Dark filter on: Next picks a dark image when one exists.
  7. Drop an image URL on the picker: it's set and saved in `manual_dir`.
  8. With the MB limit set to 20, after a few downloads the folder stays at or under 20 MB, and `manual/`, favorites and the current wallpaper remain.
- **Automated verification:** `cargo test --workspace && cargo clippy --workspace --all-targets && (cd crates/ui/frontend && npx tsc -b --noEmit && npx vitest run)`
- **Manual verification:** steps 2–8 above.
