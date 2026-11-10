# TODO

Deferred work. Each entry says why it was deferred and where to start.

## Clipboard monitoring (Variety item 18)

Variety item 18: watch the clipboard and fetch image URLs copied by the user, optionally only
from a list of hosts. Deferred from the Variety-style settings work (`docs/plans/variety-sources.md`)
because each platform needs its own watcher.

- **Where:** in the daemon, since it always runs. Fetched images go to `[daemon] manual_dir`, the
  same folder that drag and drop uses.
- **macOS:** poll `NSPasteboard.generalPasteboard.changeCount` once a second (cheap) and read
  the string only when the count changes.
- **Wayland:** `wl-paste --watch <cmd>`, or the `wlr-data-control` protocol directly.
- **X11:** XFixes `SelectionNotify` on `CLIPBOARD`.
- **Settings:** a checkbox "Monitor clipboard for image URLs", plus an optional host allowlist
  (Variety's default list includes wallhaven.cc, unsplash.com and reddit image hosts).
- **Fetching:** send each image URL to the daemon's `fetch` request, which already checks the
  type, caps the size at 64 MB, saves into `manual_dir`, and sets the wallpaper.

## Listing cache for large local folders

Each wallpaper change lists the files of every enabled local source and reads image headers
for the filters. With folders of tens of thousands of images this is slow.

- **Idea:** cache each folder's listing with its mtime, and each file's size, dimensions and
  brightness, keyed by path and mtime. Rebuild a folder's entry only when its mtime changes.
  Deep subfolders don't change the top folder's mtime, so also store each subfolder's mtime.
- **Where:** the engine's candidate listing (`crates/engine/src/pool.rs`).
