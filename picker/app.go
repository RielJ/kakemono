package main

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/rielj/wallpaperd/internal/wallpaper"
	"github.com/wailsapp/wails/v2/pkg/runtime"
)

// Thumbnail represents a single wallpaper thumbnail entry sent to the frontend.
type Thumbnail struct {
	Path      string `json:"path"`
	Source    string `json:"source"`
	Title     string `json:"title"`
	IsCurrent bool   `json:"isCurrent"`
	ImageURL  string `json:"imageUrl"` // /local/<encoded-path>
}

// PickerApp is the Wails-bound struct for the picker window.
type PickerApp struct {
	ctx        context.Context
	configPath string
	socketPath string
}

// NewPickerApp creates a new PickerApp.
func NewPickerApp() *PickerApp {
	configPath := os.Getenv("WALLPAPERD_CONFIG")
	if configPath == "" {
		configPath = config.DefaultConfigPath()
	}
	socketPath := os.Getenv("WALLPAPERD_SOCKET")
	if socketPath == "" {
		socketPath = config.DefaultSocketPath()
	}
	return &PickerApp{
		configPath: configPath,
		socketPath: socketPath,
	}
}

// startup is called by Wails when the application starts.
// It reads the picker position from config and positions the window accordingly.
func (a *PickerApp) startup(ctx context.Context) {
	a.ctx = ctx

	// Read position from config
	position := "bottom" // default
	cfg, err := config.LoadOrDefault(a.configPath)
	if err == nil && cfg.Picker.Position != "" {
		position = cfg.Picker.Position
	}

	screens, err := runtime.ScreenGetAll(ctx)
	if err == nil && len(screens) > 0 {
		// Find primary screen (or just use first)
		screen := screens[0]
		for _, s := range screens {
			if s.IsPrimary {
				screen = s
				break
			}
		}

		sw := screen.Size.Width
		sh := screen.Size.Height

		switch position {
		case "top":
			runtime.WindowSetSize(ctx, sw, 160)
			runtime.WindowSetPosition(ctx, 0, 0)
		case "left":
			runtime.WindowSetSize(ctx, 200, sh)
			runtime.WindowSetPosition(ctx, 0, 0)
		case "right":
			runtime.WindowSetSize(ctx, 200, sh)
			runtime.WindowSetPosition(ctx, sw-200, 0)
		default: // "bottom"
			runtime.WindowSetSize(ctx, sw, 160)
			runtime.WindowSetPosition(ctx, 0, sh-160)
		}
	}

	runtime.WindowShow(ctx)
}

// GetPosition returns the current picker position from config.
func (a *PickerApp) GetPosition() string {
	cfg, err := config.LoadOrDefault(a.configPath)
	if err == nil && cfg.Picker.Position != "" {
		return cfg.Picker.Position
	}
	return "bottom"
}

// ─── ServeHTTP — custom asset handler for local file serving ──────────────────

// ServeHTTP handles /local/<url-encoded-path> requests to serve wallpaper images
// from the filesystem into the WKWebView.
func (a *PickerApp) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if !strings.HasPrefix(r.URL.Path, "/local/") {
		http.NotFound(w, r)
		return
	}

	rawPath := strings.TrimPrefix(r.URL.Path, "/local/")
	filePath, err := url.QueryUnescape(rawPath)
	if err != nil {
		http.Error(w, "bad path", http.StatusBadRequest)
		return
	}

	// Only serve recognised image file types.
	ext := strings.ToLower(filepath.Ext(filePath))
	switch ext {
	case ".jpg", ".jpeg", ".png", ".gif", ".webp":
		// allowed
	default:
		http.Error(w, "unsupported file type", http.StatusForbidden)
		return
	}

	http.ServeFile(w, r, filePath)
}

// ─── Bound methods for frontend ───────────────────────────────────────────────

// GetThumbnails loads wallpaper history + cached downloads and returns a list
// of thumbnails for the frontend to display.
func (a *PickerApp) GetThumbnails() []Thumbnail {
	cfg, err := config.LoadOrDefault(a.configPath)
	if err != nil {
		cfg = config.Defaults()
	}

	// State file lives next to config
	statePath := filepath.Join(filepath.Dir(a.configPath), "state.json")
	state, err := wallpaper.LoadState(statePath)
	if err != nil {
		state = &wallpaper.State{HistoryIndex: -1}
	}

	var thumbs []Thumbnail
	seen := map[string]bool{}

	// Determine current wallpaper path
	currentPath := ""
	if state.Current != nil {
		currentPath = state.Current.Path
	}

	// History first (most recent = leftmost)
	for i := len(state.History) - 1; i >= 0; i-- {
		w := state.History[i]
		if seen[w.Path] {
			continue
		}
		if _, err := os.Stat(w.Path); err != nil {
			continue
		}
		seen[w.Path] = true
		thumbs = append(thumbs, Thumbnail{
			Path:      w.Path,
			Source:    w.Source,
			Title:     w.Title,
			IsCurrent: w.Path == currentPath,
			ImageURL:  "/local/" + url.QueryEscape(w.Path),
		})
	}

	// Scan download dir for files not in history
	downloadDir := expandHome(cfg.Daemon.DownloadDir)
	entries, _ := os.ReadDir(downloadDir)
	imageExts := map[string]bool{".jpg": true, ".jpeg": true, ".png": true, ".webp": true}
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		ext := strings.ToLower(filepath.Ext(e.Name()))
		if !imageExts[ext] {
			continue
		}
		path := filepath.Join(downloadDir, e.Name())
		if seen[path] {
			continue
		}
		seen[path] = true
		name := strings.TrimSuffix(e.Name(), ext)
		thumbs = append(thumbs, Thumbnail{
			Path:     path,
			Source:   "cache",
			Title:    name,
			ImageURL: "/local/" + url.QueryEscape(path),
		})
	}

	// Scan favorites dir too
	favDir := expandHome(cfg.Favorites.Dir)
	favEntries, _ := os.ReadDir(favDir)
	for _, e := range favEntries {
		if e.IsDir() {
			continue
		}
		ext := strings.ToLower(filepath.Ext(e.Name()))
		if !imageExts[ext] {
			continue
		}
		path := filepath.Join(favDir, e.Name())
		if seen[path] {
			continue
		}
		seen[path] = true
		name := strings.TrimSuffix(e.Name(), ext)
		thumbs = append(thumbs, Thumbnail{
			Path:     path,
			Source:   "favorites",
			Title:    name,
			ImageURL: "/local/" + url.QueryEscape(path),
		})
	}

	// Cap at 100 thumbnails
	if len(thumbs) > 100 {
		thumbs = thumbs[:100]
	}

	return thumbs
}

// SetWallpaper sets the given wallpaper via IPC to the daemon, with osascript fallback.
func (a *PickerApp) SetWallpaper(path string) error {
	client := ipc.NewClient(a.socketPath)
	resp, err := client.Send(ipc.Command{
		Cmd:  "set",
		Args: map[string]any{"path": path},
	})
	if err == nil && resp.OK {
		return nil
	}

	// Fallback: set directly via osascript
	script := fmt.Sprintf(
		`tell application "System Events" to tell every desktop to set picture to %q`,
		path,
	)
	return exec.Command("osascript", "-e", script).Run()
}

// FavoriteWallpaper copies the wallpaper at path to the favorites directory.
func (a *PickerApp) FavoriteWallpaper(path string) error {
	cfg, err := config.LoadOrDefault(a.configPath)
	if err != nil {
		return fmt.Errorf("loading config: %w", err)
	}

	favDir := expandHome(cfg.Favorites.Dir)
	if err := os.MkdirAll(favDir, 0o755); err != nil {
		return fmt.Errorf("creating favorites dir: %w", err)
	}

	dest := filepath.Join(favDir, filepath.Base(path))
	return copyFile(path, dest)
}

// Close closes the picker window.
func (a *PickerApp) Close() {
	runtime.Quit(a.ctx)
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

func expandHome(path string) string {
	if strings.HasPrefix(path, "~/") {
		home, _ := os.UserHomeDir()
		return filepath.Join(home, path[2:])
	}
	return path
}

func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()

	out, err := os.Create(dst)
	if err != nil {
		return err
	}
	defer out.Close()

	_, err = io.Copy(out, in)
	return err
}
