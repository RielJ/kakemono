# wallpaperd

A macOS wallpaper manager daemon written in Go.

Features:
- **Multiple sources**: Bing Daily, Wallhaven, Unsplash, Reddit, NASA APOD, Local files
- **Auto-rotation** on a configurable interval
- **Light/dark mode awareness** — different providers or queries per theme
- **History navigation** — go back to previous wallpapers with `wallpaperd prev`
- **Favorites & Trash** — curate your collection
- **sketchybar integration** — live status bar item with popup controls
- **Unix socket IPC** — scriptable with `nc` or any JSON tool

---

## Installation

```bash
go install github.com/rielj/wallpaperd@latest
```

Or build from source:

```bash
git clone https://github.com/rielj/wallpaperd
cd wallpaperd
go build -o wallpaperd .
```

---

## Quick Start

```bash
# Start the daemon (background)
wallpaperd daemon

# Fetch and set the next wallpaper immediately
wallpaperd next

# See what's currently set
wallpaperd status

# Navigate history
wallpaperd prev
wallpaperd next

# Curate
wallpaperd favorite   # copy to ~/Pictures/Wallpapers/favorites
wallpaperd trash      # move to trash, load next

# Set a specific file
wallpaperd set ~/Downloads/myphoto.jpg

# Pause/resume rotation
wallpaperd pause
wallpaperd resume
```

---

## Configuration

The config file is auto-generated on first run at:

```
~/.config/wallpaperd/config.toml
```

### Full reference

```toml
[daemon]
# How often to change the wallpaper (Go duration string)
interval = "30m"
# Where to store downloaded wallpapers
download_dir = "~/Pictures/Wallpapers/cache"
# Maximum number of cached wallpapers (oldest deleted when exceeded)
cache_size = 50

[wallhaven]
enabled = true
# Optional API key — https://wallhaven.cc/settings/account
api_key = ""
# Purity: "100" = SFW, "110" = SFW+Sketchy, "111" = all
purity = "100"
# Sorting: "random", "hot", "toplist", "latest"
sorting = "random"

[unsplash]
enabled = true
# Optional API key — https://unsplash.com/developers
access_key = ""

[reddit]
enabled = true
subreddits = ["EarthPorn", "wallpapers", "spaceporn", "CityPorn", "SkyPorn"]

[bing]
enabled = true
market = "en-US"

[apod]
enabled = true
# NASA API key — https://api.nasa.gov (or use "DEMO_KEY")
api_key = "DEMO_KEY"

[local]
enabled = false
dirs = ["/path/to/your/wallpapers"]
recursive = true

# Providers and queries for light mode
[light]
sources = ["bing", "unsplash", "wallhaven"]
queries = ["nature", "landscape", "architecture"]

# Providers and queries for dark mode
[dark]
sources = ["bing", "unsplash", "wallhaven"]
queries = ["space", "night", "cityscape", "dark"]

[favorites]
dir = "~/Pictures/Wallpapers/favorites"

[sketchybar]
enabled = true
item_name = "wallpaper"
```

---

## CLI Commands

| Command | Description |
|---|---|
| `wallpaperd daemon` | Start the daemon (background) |
| `wallpaperd daemon --foreground` | Start the daemon in the current process |
| `wallpaperd next` | Switch to the next wallpaper |
| `wallpaperd prev` | Revert to the previous wallpaper |
| `wallpaperd set <path>` | Set a specific image file |
| `wallpaperd favorite` | Copy current wallpaper to favorites dir |
| `wallpaperd trash` | Move current to trash, load next |
| `wallpaperd pause` | Pause auto-rotation |
| `wallpaperd resume` | Resume auto-rotation |
| `wallpaperd status` | Pretty-print current status |
| `wallpaperd status --json` | Output status as JSON |

All commands accept `--config <path>` and `--socket <path>` flags.

---

## Sketchybar Integration

See [`sketchybar/README.md`](sketchybar/README.md) for full setup instructions.

Quick install:

```bash
PLUGIN_DIR="$HOME/.config/sketchybar/plugins"
cp sketchybar/plugins/wallpaper.sh "$PLUGIN_DIR/"
cp sketchybar/plugins/wallpaper_popup.sh "$PLUGIN_DIR/"
chmod +x "$PLUGIN_DIR/"*.sh
# Then add sketchybar/sketchybarrc.sh contents to your sketchybarrc
```

---

## Auto-start with launchctl

Save the following as `~/Library/LaunchAgents/com.rielj.wallpaperd.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.rielj.wallpaperd</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/wallpaperd</string>
        <string>daemon</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>/tmp/wallpaperd.log</string>
    <key>StandardErrorPath</key><string>/tmp/wallpaperd.err</string>
</dict>
</plist>
```

Then:

```bash
launchctl load ~/Library/LaunchAgents/com.rielj.wallpaperd.plist
```

---

## IPC Protocol

The daemon exposes a Unix domain socket at `~/.config/wallpaperd/daemon.sock`.
You can script it directly:

```bash
echo '{"cmd":"status"}' | nc -U ~/.config/wallpaperd/daemon.sock
echo '{"cmd":"next"}'   | nc -U ~/.config/wallpaperd/daemon.sock
```

**Request format:**
```json
{"cmd": "next", "args": {}}
```

**Response format:**
```json
{"ok": true, "data": {...}}
{"ok": false, "error": "message"}
```

Supported commands: `next`, `prev`, `favorite`, `trash`, `pause`, `resume`, `status`, `set` (requires `args.path`).

---

## State & Files

| File | Purpose |
|---|---|
| `~/.config/wallpaperd/config.toml` | Configuration |
| `~/.config/wallpaperd/state.json` | History, current wallpaper, paused state |
| `~/.config/wallpaperd/daemon.sock` | IPC socket |
| `~/.config/wallpaperd/daemon.pid` | Daemon PID |
| `~/Pictures/Wallpapers/cache/` | Downloaded wallpapers |
| `~/Pictures/Wallpapers/favorites/` | Favorited wallpapers |
| `~/Pictures/Wallpapers/trash/` | Trashed wallpapers |

---

## Settings GUI

wallpaperd ships with an optional native macOS settings window built with
[Wails](https://wails.io) (Go + React + TypeScript). The GUI lets you configure
every option visually — toggle providers, set API keys, manage subreddits,
adjust intervals, browse favorites, and trigger wallpaper actions — without
touching `config.toml` directly.

> **Note:** The GUI is entirely optional. `config.toml` is the source of truth
> and works perfectly without the GUI.

### Prerequisites

| Tool | Version | Install |
|---|---|---|
| Go | 1.21+ | <https://go.dev/dl> |
| Node.js | 18+ | <https://nodejs.org> |
| Wails CLI | v2 | `go install github.com/wailsapp/wails/v2/cmd/wails@latest` |

Verify everything is set up:

```bash
wails doctor
```

### Building the Settings GUI

```bash
# Development — hot-reload dev server (React HMR + Go auto-recompile)
cd gui && make dev

# Release build — produces gui/build/bin/wallpaperd-settings.app
cd gui && make build

# Universal binary (runs natively on Apple Silicon and Intel)
cd gui && make build-universal
```

Alternatively, using Wails directly:

```bash
cd gui
wails dev           # development
wails build         # release (.app bundle)
wails build -platform darwin/universal   # universal binary
```

### Launching the Settings Window

After building, open the app bundle directly:

```bash
open gui/build/bin/wallpaperd-settings.app
```

Or launch it from the command line via `wallpaperd`:

```bash
wallpaperd settings
```

The window reads and writes `~/.config/wallpaperd/config.toml` directly, and
communicates with a running daemon over the IPC socket for live actions (next,
prev, favorite, trash).

### Features

- **General** tab — interval, cache size, download directory
- **Providers** tab — enable/disable Bing, Wallhaven, Unsplash, Reddit, APOD,
  Local; configure API keys, subreddits, directories
- **Themes** tab — per-theme source selection and search queries for light/dark
  mode
- **Favorites** tab — browse favorited wallpapers with previews
- **Sketchybar** tab — enable integration and set the item name
- **Status bar** — shows daemon status, current wallpaper, and action buttons
  (Next / Prev / Favorite / Trash)
- Follows macOS system dark/light mode automatically

### Screenshots

<!-- TODO: add screenshots once the GUI is complete -->

---

## Development

```bash
go test ./...
go vet ./...
go build ./...
```
