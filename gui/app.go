package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/BurntSushi/toml"
	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/ipc"
)

// ─── DTO types with json tags ─────────────────────────────────────────────────
// These mirror config.Config but with lowercase json tags so the TypeScript
// frontend receives clean, predictable field names matching src/types/config.ts.

type ConfigDTO struct {
	Daemon     DaemonConfigDTO     `json:"daemon"`
	Wallhaven  WallhavenConfigDTO  `json:"wallhaven"`
	Unsplash   UnsplashConfigDTO   `json:"unsplash"`
	Reddit     RedditConfigDTO     `json:"reddit"`
	Bing       BingConfigDTO       `json:"bing"`
	APOD       APODConfigDTO       `json:"apod"`
	Local      LocalConfigDTO      `json:"local"`
	Light      ThemeConfigDTO      `json:"light"`
	Dark       ThemeConfigDTO      `json:"dark"`
	Favorites  FavoritesConfigDTO  `json:"favorites"`
	Picker     PickerConfigDTO     `json:"picker"`
}

type DaemonConfigDTO struct {
	RotationEnabled bool   `json:"rotation_enabled"`
	Interval        string `json:"interval"`
	DownloadDir     string `json:"download_dir"`
	CacheSize       int    `json:"cache_size"`
	TrashDir        string `json:"trash_dir"`
	SocketPath      string `json:"socket_path"`
}

type WallhavenConfigDTO struct {
	Enabled    bool   `json:"enabled"`
	APIKey     string `json:"api_key"`
	Purity     string `json:"purity"`
	Sorting    string `json:"sorting"`
	Categories string `json:"categories"`
}

type UnsplashConfigDTO struct {
	Enabled   bool   `json:"enabled"`
	AccessKey string `json:"access_key"`
}

type RedditConfigDTO struct {
	Enabled    bool     `json:"enabled"`
	Subreddits []string `json:"subreddits"`
}

type BingConfigDTO struct {
	Enabled bool   `json:"enabled"`
	Market  string `json:"market"`
}

type APODConfigDTO struct {
	Enabled bool   `json:"enabled"`
	APIKey  string `json:"api_key"`
}

type LocalConfigDTO struct {
	Enabled   bool     `json:"enabled"`
	Dirs      []string `json:"dirs"`
	Recursive bool     `json:"recursive"`
}

type ThemeConfigDTO struct {
	Sources []string `json:"sources"`
	Queries []string `json:"queries"`
}

type FavoritesConfigDTO struct {
	Dir string `json:"dir"`
}

type PickerConfigDTO struct {
	Position string `json:"position"`
}

// StatusDTO mirrors ipc.StatusData for the frontend (with an extra Running field).
type StatusDTO struct {
	Running      bool          `json:"running"`
	Current      *WallpaperDTO `json:"current"`
	Paused       bool          `json:"paused"`
	Interval     string        `json:"interval"`
	Theme        string        `json:"theme"`
	HistoryCount int           `json:"history_count"`
	HistoryIndex int           `json:"history_index"`
}

// WallpaperDTO mirrors ipc.WallpaperInfo for the frontend.
type WallpaperDTO struct {
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

// ─── Conversion helpers ───────────────────────────────────────────────────────

func toDTO(cfg *config.Config) *ConfigDTO {
	return &ConfigDTO{
		Daemon: DaemonConfigDTO{
			RotationEnabled: cfg.Daemon.RotationEnabled,
			Interval:        cfg.Daemon.Interval,
			DownloadDir:     cfg.Daemon.DownloadDir,
			CacheSize:       cfg.Daemon.CacheSize,
			TrashDir:        cfg.Daemon.TrashDir,
			SocketPath:      cfg.Daemon.SocketPath,
		},
		Wallhaven: WallhavenConfigDTO{
			Enabled:    cfg.Wallhaven.Enabled,
			APIKey:     cfg.Wallhaven.APIKey,
			Purity:     cfg.Wallhaven.Purity,
			Sorting:    cfg.Wallhaven.Sorting,
			Categories: cfg.Wallhaven.Categories,
		},
		Unsplash: UnsplashConfigDTO{
			Enabled:   cfg.Unsplash.Enabled,
			AccessKey: cfg.Unsplash.AccessKey,
		},
		Reddit: RedditConfigDTO{
			Enabled:    cfg.Reddit.Enabled,
			Subreddits: cfg.Reddit.Subreddits,
		},
		Bing: BingConfigDTO{
			Enabled: cfg.Bing.Enabled,
			Market:  cfg.Bing.Market,
		},
		APOD: APODConfigDTO{
			Enabled: cfg.APOD.Enabled,
			APIKey:  cfg.APOD.APIKey,
		},
		Local: LocalConfigDTO{
			Enabled:   cfg.Local.Enabled,
			Dirs:      cfg.Local.Dirs,
			Recursive: cfg.Local.Recursive,
		},
		Light: ThemeConfigDTO{
			Sources: cfg.Light.Sources,
			Queries: cfg.Light.Queries,
		},
		Dark: ThemeConfigDTO{
			Sources: cfg.Dark.Sources,
			Queries: cfg.Dark.Queries,
		},
		Favorites: FavoritesConfigDTO{
			Dir: cfg.Favorites.Dir,
		},
		Picker: PickerConfigDTO{
			Position: cfg.Picker.Position,
		},
	}
}

func fromDTO(dto *ConfigDTO) *config.Config {
	return &config.Config{
		Daemon: config.DaemonConfig{
			RotationEnabled: dto.Daemon.RotationEnabled,
			Interval:        dto.Daemon.Interval,
			DownloadDir:     dto.Daemon.DownloadDir,
			CacheSize:       dto.Daemon.CacheSize,
			TrashDir:        dto.Daemon.TrashDir,
			SocketPath:      dto.Daemon.SocketPath,
		},
		Wallhaven: config.WallhavenConfig{
			Enabled:    dto.Wallhaven.Enabled,
			APIKey:     dto.Wallhaven.APIKey,
			Purity:     dto.Wallhaven.Purity,
			Sorting:    dto.Wallhaven.Sorting,
			Categories: dto.Wallhaven.Categories,
		},
		Unsplash: config.UnsplashConfig{
			Enabled:   dto.Unsplash.Enabled,
			AccessKey: dto.Unsplash.AccessKey,
		},
		Reddit: config.RedditConfig{
			Enabled:    dto.Reddit.Enabled,
			Subreddits: dto.Reddit.Subreddits,
		},
		Bing: config.BingConfig{
			Enabled: dto.Bing.Enabled,
			Market:  dto.Bing.Market,
		},
		APOD: config.APODConfig{
			Enabled: dto.APOD.Enabled,
			APIKey:  dto.APOD.APIKey,
		},
		Local: config.LocalConfig{
			Enabled:   dto.Local.Enabled,
			Dirs:      dto.Local.Dirs,
			Recursive: dto.Local.Recursive,
		},
		Light: config.ThemeConfig{
			Sources: dto.Light.Sources,
			Queries: dto.Light.Queries,
		},
		Dark: config.ThemeConfig{
			Sources: dto.Dark.Sources,
			Queries: dto.Dark.Queries,
		},
		Favorites: config.FavoritesConfig{
			Dir: dto.Favorites.Dir,
		},
		Picker: config.PickerConfig{
			Position: dto.Picker.Position,
		},
	}
}

// ─── SettingsApp ──────────────────────────────────────────────────────────────

// SettingsApp is the Wails-bound struct that exposes all settings operations
// to the React frontend.
type SettingsApp struct {
	ctx        context.Context
	configPath string
	socketPath string
}

// NewSettingsApp creates a new SettingsApp, reading paths from environment
// variables (WALLPAPERD_CONFIG, WALLPAPERD_SOCKET) with fallback to defaults.
func NewSettingsApp() *SettingsApp {
	configPath := os.Getenv("WALLPAPERD_CONFIG")
	if configPath == "" {
		configPath = config.DefaultConfigPath()
	}

	socketPath := os.Getenv("WALLPAPERD_SOCKET")
	if socketPath == "" {
		socketPath = config.DefaultSocketPath()
	}

	return &SettingsApp{
		configPath: configPath,
		socketPath: socketPath,
	}
}

// startup is called by Wails when the application starts.
func (a *SettingsApp) startup(ctx context.Context) {
	a.ctx = ctx
}

// ─── Config methods ───────────────────────────────────────────────────────────

// GetConfig loads and returns the current configuration.
func (a *SettingsApp) GetConfig() (*ConfigDTO, error) {
	cfg, err := config.LoadOrDefault(a.configPath)
	if err != nil {
		return nil, fmt.Errorf("loading config: %w", err)
	}
	return toDTO(cfg), nil
}

// GetDefaultConfig returns the built-in default configuration.
func (a *SettingsApp) GetDefaultConfig() (*ConfigDTO, error) {
	return toDTO(config.Defaults()), nil
}

// SaveConfig writes the given configuration to disk as TOML.
func (a *SettingsApp) SaveConfig(dto ConfigDTO) error {
	cfg := fromDTO(&dto)

	// Ensure the config directory exists.
	if err := os.MkdirAll(filepath.Dir(a.configPath), 0o755); err != nil {
		return fmt.Errorf("creating config dir: %w", err)
	}

	var buf bytes.Buffer
	enc := toml.NewEncoder(&buf)
	if err := enc.Encode(cfg); err != nil {
		return fmt.Errorf("encoding config: %w", err)
	}

	if err := os.WriteFile(a.configPath, buf.Bytes(), 0o644); err != nil {
		return fmt.Errorf("writing config: %w", err)
	}

	// Update socket path if user changed it.
	if dto.Daemon.SocketPath != "" {
		a.socketPath = dto.Daemon.SocketPath
	}

	return nil
}

// GetConfigPath returns the path to the config file.
func (a *SettingsApp) GetConfigPath() string {
	return a.configPath
}

// ─── Daemon status ────────────────────────────────────────────────────────────

// IsDaemonRunning reports whether the wallpaperd daemon is reachable.
func (a *SettingsApp) IsDaemonRunning() bool {
	return ipc.NewClient(a.socketPath).IsRunning()
}

// GetStatus fetches current daemon status.
// Returns nil error with nil data if the daemon is not running.
func (a *SettingsApp) GetStatus() (*StatusDTO, error) {
	client := ipc.NewClient(a.socketPath)
	if !client.IsRunning() {
		return nil, fmt.Errorf("daemon not running")
	}

	resp, err := client.Send(ipc.Command{Cmd: "status"})
	if err != nil {
		return nil, fmt.Errorf("daemon error: %w", err)
	}
	if !resp.OK {
		return nil, fmt.Errorf("daemon error: %s", resp.Error)
	}

	var sd ipc.StatusData
	if err := json.Unmarshal(resp.Data, &sd); err != nil {
		return nil, fmt.Errorf("parsing status: %w", err)
	}

	status := &StatusDTO{
		Running:      true,
		Paused:       sd.Paused,
		Interval:     sd.Interval,
		Theme:        sd.Theme,
		HistoryCount: sd.HistoryCount,
		HistoryIndex: sd.HistoryIndex,
	}

	if sd.Current != nil {
		status.Current = &WallpaperDTO{
			Path:      sd.Current.Path,
			Source:    sd.Current.Source,
			SourceURL: sd.Current.SourceURL,
			ImageURL:  sd.Current.ImageURL,
			Title:     sd.Current.Title,
			Author:    sd.Current.Author,
			Width:     sd.Current.Width,
			Height:    sd.Current.Height,
			SetAt:     sd.Current.SetAt,
		}
	}

	return status, nil
}

// ─── Wallpaper controls ───────────────────────────────────────────────────────

// NextWallpaper asks the daemon to advance to the next wallpaper.
func (a *SettingsApp) NextWallpaper() error { return a.sendCmd("next") }

// PrevWallpaper asks the daemon to go back to the previous wallpaper.
func (a *SettingsApp) PrevWallpaper() error { return a.sendCmd("prev") }

// PauseWallpaper asks the daemon to pause automatic rotation.
func (a *SettingsApp) PauseWallpaper() error { return a.sendCmd("pause") }

// ResumeWallpaper asks the daemon to resume automatic rotation.
func (a *SettingsApp) ResumeWallpaper() error { return a.sendCmd("resume") }

// FavoriteWallpaper asks the daemon to copy the current wallpaper to favorites.
func (a *SettingsApp) FavoriteWallpaper() error { return a.sendCmd("favorite") }

// TrashWallpaper asks the daemon to move the current wallpaper to the trash.
func (a *SettingsApp) TrashWallpaper() error { return a.sendCmd("trash") }

// sendCmd sends a simple IPC command (no args) and returns any error.
func (a *SettingsApp) sendCmd(cmd string) error {
	client := ipc.NewClient(a.socketPath)
	resp, err := client.Send(ipc.Command{Cmd: cmd})
	if err != nil {
		return fmt.Errorf("daemon not running: %w", err)
	}
	if !resp.OK {
		return fmt.Errorf("daemon error: %s", resp.Error)
	}
	return nil
}

// ─── File / Finder utilities ──────────────────────────────────────────────────

// OpenFolder opens a directory in the system file manager (Finder on macOS).
func (a *SettingsApp) OpenFolder(path string) error {
	switch runtime.GOOS {
	case "darwin":
		return exec.Command("open", path).Run()
	case "linux":
		return exec.Command("xdg-open", path).Run()
	default:
		return fmt.Errorf("OpenFolder not supported on %s", runtime.GOOS)
	}
}

// OpenFavoritesFolder opens the favorites directory in Finder.
func (a *SettingsApp) OpenFavoritesFolder() error {
	cfg, err := config.LoadOrDefault(a.configPath)
	if err != nil {
		return err
	}
	dir := cfg.ExpandedFavoritesDir()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("creating favorites dir: %w", err)
	}
	return a.OpenFolder(dir)
}

// RevealInFinder reveals a file in Finder (macOS) by selecting it.
func (a *SettingsApp) RevealInFinder(path string) error {
	if runtime.GOOS != "darwin" {
		return fmt.Errorf("RevealInFinder not supported on %s", runtime.GOOS)
	}
	return exec.Command("open", "-R", path).Run()
}

// SelectDirectory opens a native directory-picker dialog via AppleScript/zenity.
// Returns the selected path, or empty string if cancelled.
func (a *SettingsApp) SelectDirectory() (string, error) {
	if runtime.GOOS == "darwin" {
		// Use AppleScript to show a folder chooser.
		script := `POSIX path of (choose folder with prompt "Select a directory:")`
		out, err := exec.Command("osascript", "-e", script).Output()
		if err != nil {
			// User cancelled — not a real error.
			return "", nil
		}
		// osascript returns path with trailing newline
		result := string(out)
		if len(result) > 0 && result[len(result)-1] == '\n' {
			result = result[:len(result)-1]
		}
		return result, nil
	}
	return "", fmt.Errorf("SelectDirectory not supported on %s", runtime.GOOS)
}

// SelectFile opens a native file-picker dialog.
// Returns the selected path, or empty string if cancelled.
func (a *SettingsApp) SelectFile() (string, error) {
	if runtime.GOOS == "darwin" {
		script := `POSIX path of (choose file with prompt "Select a file:")`
		out, err := exec.Command("osascript", "-e", script).Output()
		if err != nil {
			return "", nil
		}
		result := string(out)
		if len(result) > 0 && result[len(result)-1] == '\n' {
			result = result[:len(result)-1]
		}
		return result, nil
	}
	return "", fmt.Errorf("SelectFile not supported on %s", runtime.GOOS)
}

// ─── Thumbnail ────────────────────────────────────────────────────────────────

// GetThumbnail returns a relative URL (/thumb?path=...) for the current
// wallpaper image, served by FileHandler below.  Returns empty string if the
// daemon is not running or no wallpaper is set.
func (a *SettingsApp) GetThumbnail() string {
	status, err := a.GetStatus()
	if err != nil || status == nil || status.Current == nil {
		return ""
	}
	path := status.Current.Path
	if path == "" {
		return ""
	}
	return "/thumb?path=" + url.QueryEscape(path)
}

// ─── FileHandler — custom Wails asset-server handler ─────────────────────────

// FileHandler is an http.Handler registered with the Wails asset server.
// It intercepts GET /thumb?path=<encoded> requests and streams the image from
// disk.  Only files with known image extensions are served for safety.
type FileHandler struct{}

func (FileHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/thumb" {
		http.NotFound(w, r)
		return
	}

	filePath := r.URL.Query().Get("path")
	if filePath == "" {
		http.Error(w, "missing path", http.StatusBadRequest)
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

// ─── Daemon management ────────────────────────────────────────────────────────

// RestartDaemon sends pause+resume to effectively restart cycle, or returns
// an error if the daemon is not running.
// Note: a true restart would require re-exec; this is a best-effort resume.
func (a *SettingsApp) RestartDaemon() error {
	if err := a.sendCmd("pause"); err != nil {
		return err
	}
	return a.sendCmd("resume")
}
