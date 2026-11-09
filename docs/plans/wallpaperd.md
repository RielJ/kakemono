# Implementation Plan: `wallpaperd` — macOS Wallpaper Manager in Go

## Summary

Build `wallpaperd`, a Go CLI daemon that manages macOS desktop wallpapers with multiple download sources (Wallhaven, Unsplash, Bing, Reddit, APOD, Local), auto-rotation with light/dark theme awareness, history/favorites/trash management, and first-class sketchybar integration via a Unix socket IPC daemon, shell plugin scripts, and custom sketchybar events.

---

## Architecture

### Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| **CLI framework** | `cobra` | Industry standard for Go CLIs, auto-generated help |
| **Config** | `BurntSushi/toml` | Simple, TOML-native |
| **IPC** | Unix domain socket, JSON protocol | No CGO, scriptable, testable with `nc` |
| **State** | Single `state.json` | Human-readable, debuggable |
| **HTML scraping** | `PuerkitoBio/goquery` | jQuery-like, well-maintained |
| **Wallpaper setter** | `osascript` via `exec.Command` | No CGO, works across macOS versions |
| **Scheduling** | `time.Ticker` in goroutine | Simple, cancellable |
| **Dark/Light** | `defaults read -g AppleInterfaceStyle` | Reliable, no CGO |

### File Layout

```
/Users/rielj/personal/projects/macos-wallpapers/
├── main.go
├── go.mod                              # module github.com/rielj/wallpaperd
├── go.sum
├── cmd/
│   ├── root.go
│   ├── daemon.go
│   ├── next.go
│   ├── prev.go
│   ├── favorite.go
│   ├── trash.go
│   ├── status.go
│   ├── pause.go
│   ├── resume.go
│   └── set.go
├── internal/
│   ├── config/
│   │   └── config.go
│   ├── ipc/
│   │   ├── protocol.go
│   │   └── client.go
│   ├── daemon/
│   │   ├── daemon.go
│   │   └── server.go
│   ├── wallpaper/
│   │   ├── setter.go
│   │   ├── manager.go
│   │   └── state.go
│   ├── theme/
│   │   └── theme.go
│   ├── providers/
│   │   ├── provider.go
│   │   ├── registry.go
│   │   ├── helpers.go
│   │   ├── wallhaven/
│   │   │   └── wallhaven.go
│   │   ├── unsplash/
│   │   │   └── unsplash.go
│   │   ├── bing/
│   │   │   └── bing.go
│   │   ├── reddit/
│   │   │   └── reddit.go
│   │   ├── apod/
│   │   │   └── apod.go
│   │   └── local/
│   │       └── local.go
│   └── sketchybar/
│       └── notifier.go
├── sketchybar/
│   ├── README.md
│   ├── plugins/
│   │   ├── wallpaper.sh
│   │   └── wallpaper_popup.sh
│   └── sketchybarrc.sh
└── README.md
```

### IPC Protocol

Socket: `~/.config/wallpaperd/daemon.sock`
PID: `~/.config/wallpaperd/daemon.pid`
State: `~/.config/wallpaperd/state.json`
Config: `~/.config/wallpaperd/config.toml`

**Request:** `{"cmd": "next", "args": {}}\n`
**Response:** `{"ok": true, "error": "", "data": {...}}\n`

---

## Section 1: Project Scaffolding

### Task 1 — Initialize Go module, main.go, and cobra root command

Create the Go module, install dependencies, wire up `main.go` and `cmd/root.go`.

Files: `go.mod`, `main.go`, `cmd/root.go`

Dependencies: `github.com/spf13/cobra`, `github.com/BurntSushi/toml`, `github.com/PuerkitoBio/goquery`

`main.go`:
```go
package main
import "github.com/rielj/wallpaperd/cmd"
func main() { cmd.Execute() }
```

`cmd/root.go`: cobra root command with persistent flags `--config` and `--socket`.

### Task 2 — Config struct and TOML loading

File: `internal/config/config.go`

Define complete config struct with TOML tags and sensible defaults:

```go
type Config struct {
    Daemon     DaemonConfig     `toml:"daemon"`
    Wallhaven  WallhavenConfig  `toml:"wallhaven"`
    Unsplash   UnsplashConfig   `toml:"unsplash"`
    Reddit     RedditConfig     `toml:"reddit"`
    Bing       BingConfig       `toml:"bing"`
    APOD       APODConfig       `toml:"apod"`
    Local      LocalConfig      `toml:"local"`
    Light      ThemeConfig      `toml:"light"`
    Dark       ThemeConfig      `toml:"dark"`
    Favorites  FavoritesConfig  `toml:"favorites"`
    Sketchybar SketchybarConfig `toml:"sketchybar"`
}

type DaemonConfig struct {
    Interval    string `toml:"interval"`      // "30m"
    DownloadDir string `toml:"download_dir"`  // "~/Pictures/Wallpapers/cache"
    CacheSize   int    `toml:"cache_size"`    // 50
}

type WallhavenConfig struct {
    Enabled bool   `toml:"enabled"`
    APIKey  string `toml:"api_key"` // optional, falls back to HTML scraping
    Purity  string `toml:"purity"`  // "100" SFW default
    Sorting string `toml:"sorting"` // "random"
}

type UnsplashConfig struct {
    Enabled   bool   `toml:"enabled"`
    AccessKey string `toml:"access_key"` // optional
}

type RedditConfig struct {
    Enabled    bool     `toml:"enabled"`
    Subreddits []string `toml:"subreddits"` // ["EarthPorn","wallpapers"]
}

type BingConfig struct {
    Enabled bool   `toml:"enabled"`
    Market  string `toml:"market"` // "en-US"
}

type APODConfig struct {
    Enabled bool   `toml:"enabled"`
    APIKey  string `toml:"api_key"` // "DEMO_KEY" default
}

type LocalConfig struct {
    Enabled   bool     `toml:"enabled"`
    Dirs      []string `toml:"dirs"`
    Recursive bool     `toml:"recursive"`
}

type ThemeConfig struct {
    Sources []string `toml:"sources"` // provider names to use
    Queries []string `toml:"queries"` // search terms
}

type FavoritesConfig struct {
    Dir string `toml:"dir"` // "~/Pictures/Wallpapers/favorites"
}

type SketchybarConfig struct {
    Enabled  bool   `toml:"enabled"`
    ItemName string `toml:"item_name"` // "wallpaper"
}
```

Functions: `Load(path string) (*Config, error)`, `DefaultConfigPath() string`, `InitConfig(path string) error` (writes documented default config).

---

## Section 2: IPC Layer

### Task 3 — IPC protocol types

File: `internal/ipc/protocol.go`

```go
type Command struct {
    Cmd  string         `json:"cmd"`
    Args map[string]any `json:"args,omitempty"`
}

type Response struct {
    OK    bool            `json:"ok"`
    Error string          `json:"error,omitempty"`
    Data  json.RawMessage `json:"data,omitempty"`
}

type StatusData struct {
    Current      *WallpaperInfo `json:"current"`
    Paused       bool           `json:"paused"`
    Interval     string         `json:"interval"`
    Theme        string         `json:"theme"`
    HistoryCount int            `json:"history_count"`
    HistoryIndex int            `json:"history_index"`
}

type WallpaperInfo struct {
    Path      string `json:"path"`
    Source    string `json:"source"`
    SourceURL string `json:"source_url"`
    ImageURL  string `json:"image_url"`
    Title     string `json:"title"`
    Author    string `json:"author"`
    Width     int    `json:"width"`
    Height    int    `json:"height"`
    SetAt     string `json:"set_at"`
}
```

### Task 4 — IPC client

File: `internal/ipc/client.go`

```go
type Client struct { SocketPath string }
func NewClient(socketPath string) *Client
func (c *Client) Send(cmd Command) (*Response, error)  // dial unix, encode JSON + \n, decode response
func (c *Client) IsRunning() bool                       // try connecting, return true if success
```

---

## Section 3: Wallpaper Core

### Task 5 — Provider interface and WallpaperInfo

File: `internal/providers/provider.go`

```go
type WallpaperResult struct {
    SourceURL string // origin page
    ImageURL  string // direct image URL
    Title     string
    Author    string
    Source    string // provider name
}

type Provider interface {
    Name() string
    Enabled() bool
    Fetch(ctx context.Context, query string) ([]WallpaperResult, error)
}
```

File: `internal/providers/helpers.go`

Shared utility functions:
- `DownloadFile(ctx, url, destDir, httpClient) (localPath string, error)` — downloads URL, validates content-type is image, file > 10KB
- `IsImageFile(path string) bool`
- `ExpandPath(path string) string` — expands `~`
- `EnsureDir(path string) error`
- Shared `http.Client` with `User-Agent: wallpaperd/1.0`

### Task 6 — Provider registry

File: `internal/providers/registry.go`

```go
type Registry struct { providers map[string]Provider }
func NewRegistry(cfg *config.Config) *Registry
func (r *Registry) Get(name string) (Provider, bool)
func (r *Registry) Enabled() []Provider
func (r *Registry) Random(names []string) Provider // pick random from named subset
```

### Task 7 — State management

File: `internal/wallpaper/state.go`

```go
type State struct {
    Current      *ipc.WallpaperInfo   `json:"current"`
    History      []ipc.WallpaperInfo  `json:"history"`
    HistoryIndex int                  `json:"history_index"`
    Paused       bool                 `json:"paused"`
    LastChanged  time.Time            `json:"last_changed"`
}

func LoadState(path string) (*State, error)
func (s *State) Save(path string) error
func (s *State) Push(w ipc.WallpaperInfo)      // add to history, trim to 100, set index to end
func (s *State) Prev() *ipc.WallpaperInfo       // move index back, return item or nil
func (s *State) Next() *ipc.WallpaperInfo       // move index forward, return item or nil (if in history)
func (s *State) CanGoPrev() bool
func (s *State) HasFutureHistory() bool          // true if index < len-1
```

### Task 8 — macOS wallpaper setter

File: `internal/wallpaper/setter.go`

```go
func SetWallpaper(imagePath string) error       // osascript tell System Events set picture of every desktop
func GetCurrentWallpaper() (string, error)      // osascript to read current picture
```

Use AppleScript:
```applescript
tell application "System Events"
    tell every desktop
        set picture to "/path/to/image.jpg"
    end tell
end tell
```

### Task 9 — Theme detection

File: `internal/theme/theme.go`

```go
type Theme string
const (ThemeDark Theme = "dark"; ThemeLight Theme = "light")

func Current() Theme          // defaults read -g AppleInterfaceStyle
```

### Task 10 — Wallpaper manager (orchestration)

File: `internal/wallpaper/manager.go`

```go
type Manager struct {
    config    *config.Config
    state     *State
    statePath string
    registry  *providers.Registry
    mu        sync.Mutex
}

func NewManager(cfg *config.Config, registry *providers.Registry) (*Manager, error)
func (m *Manager) Next(ctx context.Context) (*ipc.WallpaperInfo, error)
func (m *Manager) Prev() (*ipc.WallpaperInfo, error)
func (m *Manager) SetFile(path string) (*ipc.WallpaperInfo, error)
func (m *Manager) Favorite() error
func (m *Manager) Trash() error
func (m *Manager) Pause()
func (m *Manager) Resume()
func (m *Manager) Status() ipc.StatusData
```

`Next()` logic:
1. If `state.HasFutureHistory()` → move index forward, set from history
2. Otherwise → pick provider based on theme config, call `provider.Fetch()`, pick random result, download, add to history, set wallpaper
3. Save state, return info

`Prev()` logic:
1. If `state.CanGoPrev()` → move index back, set from history
2. Save state, return info

`Favorite()`: copy current wallpaper file to favorites dir
`Trash()`: move current wallpaper to trash dir, auto-call `Next()`

---

## Section 4: Providers

### Task 11 — Bing Daily provider

File: `internal/providers/bing/bing.go`

```go
const bingAPI = "https://www.bing.com/HPImageArchive.aspx?format=js&idx=%d&n=8&mkt=%s"
```

Fetch JSON, parse `images[].url`, prepend `https://www.bing.com`, replace resolution with `_UHD.jpg` for max quality. Return up to 8 results (last 8 days).

### Task 12 — Wallhaven provider (API + HTML scraping)

File: `internal/providers/wallhaven/wallhaven.go`

Two modes:

**With API key:**
```
GET https://wallhaven.cc/api/v1/search?q=...&sorting=random&purity=100&apikey=KEY
```
Parse `data[].path` (direct image URL) and `data[].url` (page URL).

**Without API key (legacy HTML scraping):**
```
GET https://wallhaven.cc/search?q=...&sorting=random&purity=100
```
Use goquery:
1. Find `figure.thumb` elements
2. Extract preview link from `a.preview[href]`
3. For each preview, fetch the wallpaper page
4. Find `img#wallpaper` → `src` attribute = direct image URL

Include rate limiting: 1-2s delay between requests. Handle 429 with backoff.

### Task 13 — Reddit provider

File: `internal/providers/reddit/reddit.go`

```go
const redditJSON = "https://www.reddit.com/r/%s/hot.json?limit=100"
```

Pick random subreddit from config, fetch JSON (no auth needed for public subs), filter posts where `data.url` ends in `.jpg`/`.png`/`.jpeg`, skip low-score posts. Set `User-Agent: wallpaperd/1.0` (Reddit requires non-browser UA). Handle `imgur.com/XYZ` → `i.imgur.com/XYZ.jpg` conversion.

### Task 14 — Unsplash provider

File: `internal/providers/unsplash/unsplash.go`

**With API key:**
```
GET https://api.unsplash.com/photos/random?count=10&orientation=landscape&client_id=KEY
```

**Without API key:** Use `https://source.unsplash.com/random/3840x2160/?QUERY` — follows redirect to actual image.

### Task 15 — APOD provider

File: `internal/providers/apod/apod.go`

```go
const apodAPI = "https://api.nasa.gov/planetary/apod?api_key=%s&count=5"
```

Use `DEMO_KEY` if no key configured. Filter `media_type == "image"`. Prefer `hdurl` over `url`.

### Task 16 — Local directory provider

File: `internal/providers/local/local.go`

Walk configured dirs, collect all `.jpg/.jpeg/.png/.webp` files. Optionally recursive. Shuffle results. Return as `WallpaperResult` with `Source: "local"`.

---

## Section 5: Daemon

### Task 17 — IPC server

File: `internal/daemon/server.go`

```go
type Server struct {
    socketPath string
    manager    *wallpaper.Manager
    listener   net.Listener
}

func NewServer(socketPath string, manager *wallpaper.Manager) *Server
func (s *Server) Start(ctx context.Context) error  // remove stale socket, listen, accept loop
func (s *Server) handleConn(conn net.Conn)          // decode command, dispatch, encode response
func (s *Server) dispatch(cmd ipc.Command) (any, error) // switch on cmd.Cmd → manager methods
```

### Task 18 — Daemon main loop

File: `internal/daemon/daemon.go`

```go
type Daemon struct {
    config  *config.Config
    manager *wallpaper.Manager
    server  *Server
    notifier *sketchybar.Notifier
}

func NewDaemon(cfg *config.Config) (*Daemon, error)
func (d *Daemon) Run(ctx context.Context) error
```

`Run()`:
1. Write PID file
2. Start IPC server in goroutine
3. Start rotation ticker goroutine — when tick fires and not paused, call `manager.Next()`, then `notifier.Notify()`
4. Block on `ctx.Done()` (signal handling via `signal.NotifyContext` for SIGTERM/SIGINT)
5. Cleanup: remove socket, PID file, save state

### Task 19 — Sketchybar notifier

File: `internal/sketchybar/notifier.go`

```go
type Notifier struct { Enabled bool; ItemName string }
func (n *Notifier) Notify(info *ipc.WallpaperInfo) error  // sketchybar --trigger wallpaper_changed INFO=...
```

Silently ignore errors if sketchybar is not installed (`exec.ErrNotFound`).

---

## Section 6: CLI Commands

### Task 20 — All CLI commands

Files: `cmd/daemon.go`, `cmd/next.go`, `cmd/prev.go`, `cmd/favorite.go`, `cmd/trash.go`, `cmd/status.go`, `cmd/pause.go`, `cmd/resume.go`, `cmd/set.go`

Each non-daemon command follows the pattern:
```go
var nextCmd = &cobra.Command{
    Use:   "next",
    Short: "Switch to next wallpaper",
    RunE: func(cmd *cobra.Command, args []string) error {
        client := ipc.NewClient(socketPath())
        resp, err := client.Send(ipc.Command{Cmd: "next"})
        // handle error, print result
    },
}
```

`cmd/daemon.go`: `--foreground` flag. Without it, re-exec self with `--foreground` detached from terminal (`syscall.SysProcAttr{Setsid: true}`).

`cmd/status.go`: `--json` flag. Without it, pretty-print status. With it, output raw JSON.

---

## Section 7: Sketchybar Integration

### Task 21 — Sketchybar shell scripts and config snippet

File: `sketchybar/plugins/wallpaper.sh`

```bash
#!/usr/bin/env bash
STATUS=$(wallpaperd status --json 2>/dev/null)
if [ $? -ne 0 ]; then
    sketchybar --set "$NAME" label="No daemon" icon="🖼"
    exit 0
fi
SOURCE=$(echo "$STATUS" | jq -r '.current.source // "?"')
PAUSED=$(echo "$STATUS" | jq -r 'if .paused then "⏸" else "" end')
THEME=$(echo "$STATUS" | jq -r 'if .theme == "dark" then "🌙" else "☀️" end')
sketchybar --set "$NAME" label="${THEME} ${SOURCE}${PAUSED}" icon="🖼"
```

File: `sketchybar/plugins/wallpaper_popup.sh`

```bash
#!/usr/bin/env bash
case "$NAME" in
    *next)     wallpaperd next ;;
    *prev)     wallpaperd prev ;;
    *favorite) wallpaperd favorite ;;
    *trash)    wallpaperd trash ;;
    *pause)
        STATUS=$(wallpaperd status --json 2>/dev/null)
        IS_PAUSED=$(echo "$STATUS" | jq -r '.paused')
        if [ "$IS_PAUSED" = "true" ]; then wallpaperd resume; else wallpaperd pause; fi
        ;;
esac
sketchybar --set wallpaper popup.drawing=off
```

File: `sketchybar/sketchybarrc.sh`

Drop-in snippet for user's sketchybarrc:
```bash
PLUGIN_DIR="$HOME/.config/sketchybar/plugins"
sketchybar --add event wallpaper_changed
sketchybar --add item wallpaper right \
    --set wallpaper update_freq=0 icon="🖼" label="..." \
        script="$PLUGIN_DIR/wallpaper.sh" \
        click_script="sketchybar --set \$NAME popup.drawing=toggle" \
    --subscribe wallpaper wallpaper_changed

sketchybar --add item wallpaper.next popup.wallpaper \
    --set wallpaper.next icon="⏭" label="Next" click_script="$PLUGIN_DIR/wallpaper_popup.sh"
sketchybar --add item wallpaper.prev popup.wallpaper \
    --set wallpaper.prev icon="⏮" label="Previous" click_script="$PLUGIN_DIR/wallpaper_popup.sh"
sketchybar --add item wallpaper.favorite popup.wallpaper \
    --set wallpaper.favorite icon="⭐" label="Favorite" click_script="$PLUGIN_DIR/wallpaper_popup.sh"
sketchybar --add item wallpaper.trash popup.wallpaper \
    --set wallpaper.trash icon="🗑" label="Trash" click_script="$PLUGIN_DIR/wallpaper_popup.sh"
sketchybar --add item wallpaper.pause popup.wallpaper \
    --set wallpaper.pause icon="⏸" label="Pause/Resume" click_script="$PLUGIN_DIR/wallpaper_popup.sh"
```

File: `sketchybar/README.md` — setup instructions.

---

## Section 8: Documentation & Polish

### Task 22 — Default config file generation

Enhance `internal/config/config.go` `InitConfig()` to write a fully documented default `config.toml` with all options commented/explained.

### Task 23 — README.md

Document:
- Installation via `go install`
- Config file location and all options
- All CLI commands with examples
- Sketchybar setup instructions
- launchctl plist for auto-start:

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

---

## Section 9: Tests

### Task 24 — Unit tests for state and manager

Files: `internal/wallpaper/state_test.go`, `internal/wallpaper/manager_test.go`

- `TestPush_MaxSize` — verify trimming at 100
- `TestHistoryNavigation` — prev/next indices
- `TestStatePersistence` — save + load round-trip
- `TestManagerFavorite_CopiesFile`

### Task 25 — Unit tests for providers

Files: `internal/providers/bing/bing_test.go`, `internal/providers/wallhaven/wallhaven_test.go`, `internal/providers/reddit/reddit_test.go`

Use `httptest.NewServer` to mock external APIs:
- `TestBingFetch_ParsesJSON`
- `TestWallhavenAPI_ParsesResponse`
- `TestWallhavenScrape_FindsImages` (serve mock HTML)
- `TestReddit_FiltersNonImages`

---

## Edge Cases

| Scenario | Handling |
|---|---|
| Daemon not running | CLI prints: `wallpaperd daemon is not running. Start with: wallpaperd daemon` |
| Stale socket after crash | Server removes stale socket on startup; check PID file validity |
| Wallhaven rate limiting (429) | Exponential backoff (1s, 2s, 4s), max 3 retries |
| No wallpapers match query | Try next provider; after all fail, keep current wallpaper |
| Downloaded file not an image | Validate content-type + size > 10KB, delete invalid, retry |
| Cache dir full | Delete oldest files when `cache_size` exceeded |
| `osascript` fails | Log error, continue (don't crash daemon) |
| Dark/light detection fails | Default to `light` |
| sketchybar not installed | `Notify()` silently ignores `exec.ErrNotFound` |
| Reddit API blocks | Some subs need login; fall back to other provider |
| APOD returns video | Check `media_type == "image"`, skip videos |
| Concurrent CLI commands | Manager uses `sync.Mutex`; one command at a time |
| Config file missing | Auto-generate on first `wallpaperd daemon` |
| No network | Fall back to local provider if available |

## Acceptance Criteria

- [ ] `wallpaperd daemon` starts background process, creates socket
- [ ] `wallpaperd next` changes wallpaper and prints new source/title
- [ ] `wallpaperd prev` reverts to previous wallpaper from history
- [ ] `wallpaperd status --json` outputs valid JSON
- [ ] `wallpaperd favorite` copies current wallpaper to favorites dir
- [ ] `wallpaperd trash` moves current to trash dir and triggers next
- [ ] `wallpaperd pause` / `wallpaperd resume` stops/starts auto-rotation
- [ ] `wallpaperd set ~/path/image.jpg` sets specific wallpaper
- [ ] All 6 providers can fetch and return results
- [ ] Wallhaven works without API key (HTML scraping)
- [ ] Auto-rotation changes wallpaper at configured interval
- [ ] sketchybar item updates when `wallpaper_changed` event fires
- [ ] `go build ./...` succeeds
- [ ] `go vet ./...` clean
- [ ] `go test ./...` passes
- [ ] Daemon handles SIGTERM gracefully (removes socket, saves state)
