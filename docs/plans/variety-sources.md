# Spec: Variety-style sources, downloading and filtering

Status: approved by the user, 2026-07. Implementation plan to follow.

## Goal

Make kakemono's settings as simple and familiar as Variety's preferences. The user should be able
to find the sources list on the first screen, tick sources on and off, add a source by choosing a
type and typing keywords or pasting a URL, and change only the few settings they care about
(interval, start at login, picker orientation, how to bind a shortcut). Everything technical
moves to an Advanced tab.

The engine changes from "download one image on each change" to Variety's model: a background
downloader fills one folder per source, and changes pick from images already on disk.

Reference: Variety at <https://github.com/varietywalls/variety>, used for its UI layout and
behaviour only.

**License boundary:** Variety is GPL-3.0 and kakemono is MIT. Don't copy, translate or port
Variety code or long text. Reimplement from the behaviour described in this spec. Write our own
UI wording. Credit Variety as an inspiration in the README and the About section.

## Decisions

| Topic | Decision |
|---|---|
| Fetch model | Variety's: background downloader into `downloads/<source-id>/`; changes pick from enabled sources' local images. |
| Light/dark | Drop the `[light]`/`[dark]` source and query lists. One `[[sources]]` list. Item 21 becomes a filter: Off, Dark, Light, Match system appearance. Existing light and dark lists merge into the sources list on upgrade. |
| Drag and drop (17) | In scope, last phase. Dropping an image file or image URL on the picker or settings window fetches it into the manual-downloads folder and sets it. |
| Clipboard monitoring (18) | Deferred. Add to `docs/TODO.md` with notes (macOS `NSPasteboard.changeCount` poll, Wayland `wl-paste --watch`, X11 XFixes). |
| Shortcuts | Hint only: copyable `kakemono pick` / `kakemono settings` commands and short per-OS how-tos (macOS: Shortcuts.app, Raycast, skhd; Linux: GNOME/KDE custom shortcuts, Hyprland/Sway/niri bind lines). No global-shortcut plugin. |
| Settings layout | Two tabs: **General** and **Advanced**. Plus the existing picker window. |
| Out of scope | Variety items 15, 25–31: EXIF auto-rotate, slideshow, menu-bar icon options, CLI hint for a hidden icon, favorites operations menu, lock screen, copy-to-static-folder. Also quotes, clock, effects. |

## Items in scope

Numbers refer to the inventory the user picked from.

### General tab

1. **Start at login** checkbox (existing launchd/systemd service install).
2. **Change wallpaper every [N] [minutes|hours|days]** checkbox + value + unit (existing
   `rotation_enabled`, `interval`).
3. **Change wallpaper on start** checkbox. New `[daemon] change_on_start` (default false). When
   true, the daemon changes the wallpaper once at startup.
4. **Use Internet access** switch. New `[daemon] internet` (default true). When off, the
   downloader stops, online sources are greyed in the list, and Add hides online types (Variety
   disables them).
5. **Images** table: columns Enabled (checkbox), Type, Location. One row per source instance.
   Several rows of the same type are allowed (two Wallhaven searches).
6. **Add…** menu. Each item shows a bold name and a one-line description:
   - Local: Images (individual files), Folders (random, recursive), Sequential album by file
     name, Sequential album by file date.
   - Separator, then online sources sorted A–Z: APOD (no input), Bing (no input), Reddit
     ("URL or name of a subreddit:"), Unsplash ("Search keyword or URL:"), Wallhaven ("Enter
     keywords or paste URL here:").
   - Types that take input open a small dialog with the instruction text, one field, OK and
     Cancel. On OK the entry is validated by a real fetch ("Just a moment to check for
     images"). An empty result or error is shown inline and the source isn't added.
   - Types without input are added directly. Adding a second Bing or APOD is refused, as in
     Variety.
7. **Edit…** reopens the dialog for the selected row with its current value. Disabled for types
   without input.
8. **Remove…** asks whether to also delete the source's downloaded files
   (`downloads/<source-id>/`). Local sources never delete user files.
9. **Open Folder** opens the selected row's folder: the user's folder for local sources, and
   `downloads/<source-id>/` for online ones.
10. **Use Only These** enables only the selected rows and disables the rest. The current wallpaper stays until the next change.
11. **Download a wallpaper every [N] [unit]** checkbox + interval (default 10 minutes, enabled).
    The download folder (`[daemon] download_dir`) is shown on Advanced.
12. **Limit download folder to [N] MB, deleting the oldest files when necessary** checkbox +
    value (default 500 MB, enabled). Never deletes favorites, the current wallpaper, or files in
    local sources. Replaces the count-based `cache_size`; migrate by dropping `cache_size`.
13. **Favorites: copy favorite wallpapers to [folder]** (existing `[favorites] dir`).
- **Picker**: edge and orientation (existing `[picker] position`, `orientation`).
- **Shortcuts** hint (see Decisions).

### Filtering ("When possible, use images that…")

Soft filters, as in Variety: the change picks the first local candidate that passes all enabled
filters; if none of a bounded number of candidates passes, it uses the best one anyway and logs
why. The downloader also checks filters after each download and deletes failures (online
sources only), so the folders fill with matching images over time.

19. **Have landscape orientation** (width > height).
20. **Are big at least [50|80|100] % of the screen resolution** (compare against the largest
    connected display; the platform crate reports display sizes, or assume the main display).
21. **Are dark or light [Dark|Light|Match system appearance]**: mean relative luminance
    (Rec. 709) of a downscaled copy; dark below 0.35, light above 0.6 (tune with tests).
22. **Contain this color [picker]**: dominant colours of a downscaled copy within a distance
    threshold of the chosen colour.
23. **Have EXIF rating at least [1–5]**: `Rating` (0x4746) or XMP `xmp:Rating`. Images without
    a rating fail this filter.
24. **File name matches regex [text]**.

Image facts (size, luminance, palette, rating) are computed once per file and cached
(state dir, keyed by path + mtime + size) so changes stay instant.

### Wallpaper (Advanced tab)

14. **Display mode**: existing `[desktop] fit`.

### Downloading (Advanced tab)

16. **Save manually downloaded wallpapers to [folder]**. New `[daemon] manual_dir` (default
    `<download_dir>/manual`). Used by drag and drop (17). It's an implicit, always-enabled local
    source.
17. **Drag and drop**: see Decisions.

### Advanced tab, also

- Download folder, favorites folder, trash folder.
- Desktop backend, custom set command, apply to all Spaces (macOS), reapply on Space change.
- Hooks (`on_change`), socket path.
- Appearance (Light, Dark, System; default Light/white).
- Provider credentials: Wallhaven API key, Unsplash access key, APOD API key, Bing market.

## Config shape

New list replacing the per-provider `enabled` flags, `[reddit] subreddits`, `[local] dirs`, and
the `[light]`/`[dark]` tables:

```toml
[[sources]]
id = "wh-1a2b"          # stable, generated; names the download subfolder
enabled = true
type = "wallhaven"      # wallhaven | unsplash | reddit | bing | apod | folder | images | album-name | album-date
location = "nature"     # keywords, URL, subreddit, path, or empty for bing/apod
```

Provider credentials and options stay in their existing tables (`[wallhaven] api_key`, `purity`,
`sorting`, `categories`; `[unsplash] access_key`; `[apod] api_key`; `[bing] market`).

New `[daemon]` keys: `change_on_start`, `internet`, `download_enabled`, `download_interval`,
`download_limit_mb_enabled`, `download_limit_mb`, `manual_dir`. New `[filters]` table for 19–24.

### Migration

On load, a config without `[[sources]]` is converted, and the file is rewritten once (with a
`.bak` copy) through `toml_edit` so comments survive:

- Each provider with `enabled = true` becomes one row. Reddit gets one row per subreddit; local
  gets one `folder` row per dir.
- Wallhaven and Unsplash get one row per distinct query from `[light].queries` and
  `[dark].queries` (or one row with an empty location if there are none).
- Providers listed in `[light].sources`/`[dark].sources` but disabled stay as disabled rows.
- `[light]`, `[dark]`, and `cache_size` are removed.

## Existing architecture (facts for planners)

- Workspace crates: `cli`, `config`, `engine`, `platform`, `protocol`, `sources`, `ui`
  (`ui/src-tauri` Tauri v2 app, `ui/frontend` React + TypeScript + Tailwind, Vite, Vitest).
- `crates/config/src/schema.rs`: `Config` with `daemon`, `wallhaven`, `unsplash`, `reddit`,
  `bing`, `apod`, `local`, `light`, `dark`, `favorites`, `picker`, `desktop`, `hooks`, `ui`.
  Types export to TypeScript with ts-rs (`cargo test -p kakemono-ui --features export-bindings`
  writes `crates/ui/frontend/src/shared/bindings/*.ts`). Optional fields must serialize as `null`,
  not be skipped (`crates/config/tests/json_wire.rs`).
- `crates/config/src/edit.rs`: `patch`, `apply_json` (JSON patch from the settings UI applied to
  a `toml_edit` document), `write_atomic`. The settings UI saves with `commands::save_config`.
- `crates/sources`: `Provider` trait (`name() -> &'static str`, `enabled()`,
  `fetch(query) -> Vec<WallpaperResult>`), `Registry::from_config` builds one instance per
  provider type from config, `Registry::candidates(names)` returns enabled ones in random order.
  `download(client, url, dir)` writes a unique file. Reddit and Bing ignore `query`; Reddit picks
  a random configured subreddit.
- `crates/engine/src/engine.rs`: `fetch_and_set` picks the theme's source list and a random
  query, tries providers in random order, downloads into `download_dir`, sets, pushes history,
  then `files::trim_cache` trims `download_dir` by count (`cache_size`), sparing favorites,
  trash and history.
- `crates/engine/src/rotation.rs`: the rotation loop. It starts with `anchor = last_changed`;
  there is no change on start. It reacts to theme changes (`rotate_on_theme_change`) and Space
  changes.
- `crates/engine/src/state.rs`: history with prev/next; persisted JSON.
- `crates/protocol/src/types.rs`: IPC `Request` enum (Next, Prev, Set, Favorite, Trash, Pause,
  Resume, Status, …) over a Unix socket with a line codec.
- `crates/ui/src-tauri/src/commands.rs`: Tauri commands; `thumbnails` lists history, then
  `download_dir`, then favorites and local dirs, for the picker.
- The `image` crate (jpeg, png, webp) is already a workspace dependency.
- The settings frontend is mid-redesign (uncommitted): auto-save store, shared controls
  (`components/Group.tsx`, `Toggle.tsx`, `TagInput.tsx`, `Icon.tsx`), a "Now showing" card,
  picker edge/orientation preview, Inter font, and white default with `[ui] appearance`. Reuse
  these; replace the 5-section sidebar with the two tabs.
- Many error strings and formats are kept byte-compatible with the old Go daemon and tested
  (`*_matches_go` tests). Keep them unless the feature replaces them.

## Constraints

- Work in thin vertical slices. After each slice: `cargo test --workspace`, `cargo clippy
  --workspace --all-targets`, `npx tsc -b --noEmit` and `npx vitest run` in
  `crates/ui/frontend` all pass.
- No `skip_serializing_if` on config fields sent to the UI.
- Never delete user files from local sources, favorites, or the current wallpaper.
- The downloader must respect provider rate limits (`ProviderError::RateLimited`) with backoff,
  and stop when `internet = false`.
- Don't commit; the user commits.

## Acceptance

- A fresh install shows General with the sources table pre-filled (Bing enabled, Wallhaven
  "nature" enabled, favorites folder) and works offline after the first downloads.
- The user's current config migrates without losing a source or query, and the old file is
  kept as `config.toml.bak`.
- Adding a Wallhaven source with keywords validates against the live site, then images appear in
  `downloads/<id>/` within one download interval.
- Removing a source with "delete files" removes only `downloads/<id>/`.
- With the dark filter on, a change picks a dark image when one exists locally.
- Dropping an image URL on the picker sets it and saves it in `manual_dir`.
