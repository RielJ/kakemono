# UI design checklist

The design standards from `docs/plans/kakemono-rust.md` §H, as a review checklist. Run it on
every change to the picker or Settings.

- `[x]`: met. The evidence is a file or a test.
- `[ ]`: not met yet. The note says what's missing.
- `[~]`: needs a person to check it on a real machine. The plan's manual verification covers it.

Last reviewed: Task 44 (picker). Settings has its own review in Task 46.

## Restraint

- [x] System font stack, 13 px on macOS and 14 px elsewhere: `--font-sans` and `--font-size`
  in `src/shared/tokens.css`. The picker sets `data-os` on `<html>` from `app_info`.
- [x] Neutral palette and one accent. The accent is `AccentColor` where the webview supports
  it, with a neutral blue fallback (`tokens.css`).
- [x] No gradients, glassmorphism or `backdrop-filter`: the picker CSS has none. The old
  Catppuccin gradient caption and blurred background are gone.
- [x] No emoji or Nerd Font glyphs as icons. Icons are inline lucide stroke paths in
  `picker/components/Icon.tsx` (grip, check, image placeholder) and appear only where they
  carry meaning.
- [x] No decorative illustrations or dashboard-style cards.

## Dark and light

- [x] `color-scheme: light dark`: set in `tokens.css` and in the `picker.html` `<meta>` tag.
- [x] Tokens switch on `prefers-color-scheme`: `tokens.css`.
- [x] The window follows the system theme: the picker builder doesn't set a theme.
- [x] No flash of the wrong theme. Three layers set the same background colours:
  - the inline `<head>` style in `picker.html`, painted before any CSS loads;
  - `set_background_color(background_for(theme))` in `picker.rs`, applied before each show;
  - `--bg-light` and `--bg-dark` in `tokens.css`.

  The test `picker::tests::window_background_matches_the_page_tokens` fails if the colours drift
  apart.
- [x] High contrast: borders and muted text use `CanvasText` under
  `prefers-contrast: more`.
- [~] Light and dark open with no flash: check on macOS, Windows and Linux.

## Motion

- [x] 120–160 ms ease-out (`--duration-fast` and `--duration`, with `--ease-out`). It covers
  hover rings, the caption fade, the menu reveal and the toast reveal.
- [x] Only `opacity`, `transform` and `box-shadow` animate. Nothing changes layout.
- [x] `prefers-reduced-motion: reduce` sets both durations to 0. A blanket rule in
  `picker/App.css` also switches off animations, transitions and smooth scrolling.
- [~] With Reduce Motion on, nothing animates: check by hand.

## Picker keyboard behaviour

- [x] The strip is `role="listbox"` with `aria-activedescendant` and `aria-orientation`.
  Each option is `role="option"`, labelled "title — source", and the current wallpaper has
  `aria-selected="true"` (`App.test.tsx`: "names options…").
- [x] ←/→ move, or ↑/↓ for left and right edges, and Home/End jump (`keys.test.ts`, and
  `App.test.tsx` "moves with…").
- [x] Enter (or Space) calls `set_file`, and so does a click (`App.test.tsx`).
- [x] F adds the wallpaper to favourites and confirms in a toast (`App.test.tsx`).
- [x] Delete or Backspace moves the current wallpaper to Trash, with a 4 s undo toast. Undo
  works from the toast button or with ⌘Z/Ctrl+Z ("trash with undo" in `App.test.tsx`).
  - Only the current wallpaper can be trashed. IPC `trash` takes no path, so for any other
    wallpaper the menu item is disabled and the key explains why.
  - The real `trash` waits until the undo time runs out, or until the picker hides or is
    closed with Esc. Just before sending it, the picker reads `status` again. If the current
    wallpaper changed (rotation or tray Next), nothing is trashed and a toast explains why
    (`trash.test.ts`, plus "does not trash the new wallpaper…" in `App.test.tsx`).
- [x] Esc hides the picker through `hide_picker`. The OS returns focus to the previous app
  (`App.test.tsx`).
- [x] Shift+F10 and the Menu key open the context menu, which works from the keyboard: focus
  starts on the first item, ↑/↓ wrap, Home/End jump, Enter runs an item, and Esc or Tab
  closes the menu and returns focus to the strip (the "context menu" tests in
  `App.test.tsx`).
- [x] Focus is visible: the active option gets a 2 px accent ring with a background-coloured
  gap, shown only under `:focus-visible`.
- [x] The picker closes on blur when `[picker] close_on_blur` is on (`picker.rs`, Task 43).
- [~] VoiceOver (macOS) and Narrator (Windows) read "title — source, selected".

## Picker placement

- [x] A thin drag handle (`data-tauri-drag-region`, cursor `grab`, labelled "Move picker")
  sits at the leading edge of the strip. Thumbnails are not drag regions, so a click still
  sets the wallpaper (`App.test.tsx`: "has a drag handle…").
- [x] The context menu has Reset Position, which calls `reset_picker_position`
  (`App.test.tsx`).

## Thumbnails

- [x] Images load from `thumb://localhost/<id>`, or `http://thumb.localhost/<id>` on
  Windows. The id is opaque and different on every run, and the URL never contains a path.
- [x] Only files listed by `tab_thumbnails` (or `current_thumbnail`) are admitted. Both the file and the roots
  (download, favourites and `[local]` dirs) are canonicalised, so a symlink out of a root is
  refused (`thumbs::tests::thumbs_symlink_from_download_dir_to_ssh_is_not_admitted`).
  Anything not admitted returns 403.
- [x] A missing or broken thumbnail shows a neutral placeholder, never a broken-image icon.
- [x] Empty, loading and error states are plain sentences that say what to do next.

## Not done yet

- [ ] Settings still uses its own macOS-style tokens and Tailwind. Moving it onto
  `tokens.css`, with a source list only under `data-os="macos"`, belongs to the Settings
  design pass.
- [ ] Live updates: the picker refreshes when it gets focus. Task 45 (`subscribe`) replaces
  that with `wallpaper://changed`.
- [ ] Protocol follow-up: `trash` should take an optional expected path so the daemon makes
  the "still current" check itself, atomically. The picker's `status` re-read narrows the
  race but can't close it.
