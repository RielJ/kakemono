# wallpaperd × sketchybar

This directory contains everything you need to integrate `wallpaperd` with [sketchybar](https://github.com/FelixKratz/SketchyBar).

## What you get

- A sketchybar item showing the current wallpaper source, title, and theme mode
- A popup menu with **Next**, **Previous**, **Favorite**, **Trash**, and **Pause/Resume** buttons
- Live updates via the custom `wallpaper_changed` event

## Prerequisites

```bash
brew install sketchybar jq
go install github.com/rielj/wallpaperd@latest
```

## Installation

1. **Copy plugins** to your sketchybar plugin directory:

```bash
PLUGIN_DIR="$HOME/.config/sketchybar/plugins"
mkdir -p "$PLUGIN_DIR"
cp plugins/wallpaper.sh "$PLUGIN_DIR/"
cp plugins/wallpaper_popup.sh "$PLUGIN_DIR/"
chmod +x "$PLUGIN_DIR/wallpaper.sh" "$PLUGIN_DIR/wallpaper_popup.sh"
```

2. **Add to your sketchybarrc** — either source the snippet or paste the relevant lines:

```bash
# In your ~/.config/sketchybar/sketchybarrc
source "$HOME/.config/sketchybar/plugins/../wallpaperd/sketchybarrc.sh"
# Or simply copy the contents of sketchybarrc.sh into your sketchybarrc
```

3. **Start the daemon**:

```bash
wallpaperd daemon
```

4. **Reload sketchybar**:

```bash
sketchybar --reload
```

## How it works

- `wallpaperd` fires `sketchybar --trigger wallpaper_changed` every time the wallpaper changes.
- The `wallpaper.sh` plugin calls `wallpaperd status --json` to read current state and updates the bar item.
- Clicking the item opens a popup; each button calls `wallpaper_popup.sh` with `$NAME` set to the button name.

## Customisation

Edit `sketchybarrc.sh` to change:
- The icon (`icon="🖼"`)
- Which side of the bar (`right` / `left` / `center`)
- Label length (currently capped at 25 chars in `wallpaper.sh`)
