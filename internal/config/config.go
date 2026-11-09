// Package config loads and provides the wallpaperd configuration from a TOML file.
package config

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/BurntSushi/toml"
)

// Config is the top-level configuration structure parsed from config.toml.
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
	Picker     PickerConfig     `toml:"picker"`
}

// DaemonConfig holds daemon-level settings.
type DaemonConfig struct {
	RotationEnabled bool   `toml:"rotation_enabled"` // whether auto-rotation is active
	Interval        string `toml:"interval"`          // duration string e.g. "30m"
	DownloadDir     string `toml:"download_dir"`      // e.g. "~/Pictures/Wallpapers/cache"
	CacheSize       int    `toml:"cache_size"`        // max cached images
	TrashDir        string `toml:"trash_dir"`          // e.g. "~/Pictures/Wallpapers/trash"
	SocketPath      string `toml:"socket_path"`        // Unix socket; defaults to ~/.config/wallpaperd/daemon.sock
}

// WallhavenConfig holds Wallhaven provider settings.
type WallhavenConfig struct {
	Enabled    bool   `toml:"enabled"`
	APIKey     string `toml:"api_key"`    // optional; falls back to HTML scraping when empty
	Purity     string `toml:"purity"`     // "100" = SFW
	Sorting    string `toml:"sorting"`    // "random"
	Categories string `toml:"categories"` // "111" = general+anime+people
}

// UnsplashConfig holds Unsplash provider settings.
type UnsplashConfig struct {
	Enabled   bool   `toml:"enabled"`
	AccessKey string `toml:"access_key"` // optional
}

// RedditConfig holds Reddit provider settings.
type RedditConfig struct {
	Enabled    bool     `toml:"enabled"`
	Subreddits []string `toml:"subreddits"` // e.g. ["EarthPorn","wallpapers"]
}

// BingConfig holds Bing Daily provider settings.
type BingConfig struct {
	Enabled bool   `toml:"enabled"`
	Market  string `toml:"market"` // e.g. "en-US"
}

// APODConfig holds NASA APOD provider settings.
type APODConfig struct {
	Enabled bool   `toml:"enabled"`
	APIKey  string `toml:"api_key"` // "DEMO_KEY" is fine for light use
}

// LocalConfig holds local-directory provider settings.
type LocalConfig struct {
	Enabled   bool     `toml:"enabled"`
	Dirs      []string `toml:"dirs"`      // list of directory paths
	Recursive bool     `toml:"recursive"` // scan subdirectories
}

// ThemeConfig selects which providers and queries to use per macOS appearance.
type ThemeConfig struct {
	Sources []string `toml:"sources"` // provider names e.g. ["bing","wallhaven"]
	Queries []string `toml:"queries"` // search terms e.g. ["nature","landscape"]
}

// FavoritesConfig holds the favorites directory path.
type FavoritesConfig struct {
	Dir string `toml:"dir"` // e.g. "~/Pictures/Wallpapers/favorites"
}

// PickerConfig holds settings for the floating wallpaper picker HUD.
type PickerConfig struct {
	Position string `toml:"position"` // "bottom", "top", "left", "right"
}

// ─── Default path helpers ─────────────────────────────────────────────────────

// DefaultConfigPath returns the default config file location, respecting
// XDG_CONFIG_HOME if set.
func DefaultConfigPath() string {
	if xdg := os.Getenv("XDG_CONFIG_HOME"); xdg != "" {
		return filepath.Join(xdg, "wallpaperd", "config.toml")
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "wallpaperd", "config.toml")
}

// DefaultSocketPath returns the default Unix socket path.
// It follows the same convention as the other runtime files and lives under
// ~/.config/wallpaperd/ so that all wallpaperd files are co-located.
func DefaultSocketPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "wallpaperd", "daemon.sock")
}

// DefaultStatePath returns the default state file path.
func DefaultStatePath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "wallpaperd", "state.json")
}

// DefaultPIDPath returns the default PID file path.
func DefaultPIDPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "wallpaperd", "daemon.pid")
}

// ─── Default values ───────────────────────────────────────────────────────────

// Defaults returns a new Config populated with sensible defaults.
func Defaults() *Config {
	home, _ := os.UserHomeDir()
	return &Config{
		Daemon: DaemonConfig{
			RotationEnabled: true,
			Interval:        "30m",
			DownloadDir: filepath.Join(home, "Pictures", "Wallpapers", "cache"),
			CacheSize:   50,
			TrashDir:    filepath.Join(home, "Pictures", "Wallpapers", "trash"),
			SocketPath:  filepath.Join(home, ".config", "wallpaperd", "daemon.sock"),
		},
		Wallhaven: WallhavenConfig{
			Enabled:    false,
			Purity:     "100",
			Sorting:    "random",
			Categories: "111",
		},
		Unsplash: UnsplashConfig{
			Enabled: false,
		},
		Reddit: RedditConfig{
			Enabled:    false,
			Subreddits: []string{"EarthPorn", "wallpapers"},
		},
		Bing: BingConfig{
			Enabled: true,
			Market:  "en-US",
		},
		APOD: APODConfig{
			Enabled: false,
			APIKey:  "DEMO_KEY",
		},
		Local: LocalConfig{
			Enabled:   false,
			Dirs:      []string{},
			Recursive: true,
		},
		Light: ThemeConfig{
			Sources: []string{"bing"},
			Queries: []string{"landscape", "nature"},
		},
		Dark: ThemeConfig{
			Sources: []string{"bing"},
			Queries: []string{"space", "night", "dark"},
		},
		Favorites: FavoritesConfig{
			Dir: filepath.Join(home, "Pictures", "Wallpapers", "favorites"),
		},
		Picker: PickerConfig{
			Position: "bottom",
		},
	}
}

// Defaults fills c with default values.  It is provided as a method so that
// tests can call cfg.Defaults() on an existing *Config pointer.
func (c *Config) Defaults() {
	*c = *Defaults()
}

// ─── Expanded path helpers ────────────────────────────────────────────────────

// ExpandedDownloadDir returns the download dir with ~ expanded.
func (c *Config) ExpandedDownloadDir() string {
	return expandPath(c.Daemon.DownloadDir)
}

// ExpandedFavoritesDir returns the favorites dir with ~ expanded.
func (c *Config) ExpandedFavoritesDir() string {
	return expandPath(c.Favorites.Dir)
}

// TrashDir returns the trash directory with ~ expanded.
func (c *Config) TrashDir() string {
	return expandPath(c.Daemon.TrashDir)
}

// ─── Loading ──────────────────────────────────────────────────────────────────

// Load reads and parses a TOML config file from path, applying defaults for
// any fields not present in the file. Returns an error if path does not exist.
func Load(path string) (*Config, error) {
	cfg := Defaults()

	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("config: cannot read %s: %w", path, err)
	}

	if _, err := toml.Decode(string(data), cfg); err != nil {
		return nil, fmt.Errorf("config: cannot parse %s: %w", path, err)
	}

	// Expand ~ in path fields after loading.
	cfg.Daemon.DownloadDir = expandPath(cfg.Daemon.DownloadDir)
	cfg.Daemon.TrashDir = expandPath(cfg.Daemon.TrashDir)
	cfg.Favorites.Dir = expandPath(cfg.Favorites.Dir)
	for i, d := range cfg.Local.Dirs {
		cfg.Local.Dirs[i] = expandPath(d)
	}

	return cfg, nil
}

// LoadOrDefault loads config from path; if the file does not exist returns
// Defaults() without an error.
func LoadOrDefault(path string) (*Config, error) {
	cfg, err := Load(path)
	if err != nil {
		var pathErr *os.PathError
		if errors.As(err, &pathErr) && os.IsNotExist(pathErr.Err) {
			return Defaults(), nil
		}
		if os.IsNotExist(err) {
			return Defaults(), nil
		}
	}
	return cfg, err
}

// expandPath expands a leading ~ to the user home directory.
func expandPath(p string) string {
	if strings.HasPrefix(p, "~/") {
		home, _ := os.UserHomeDir()
		return filepath.Join(home, p[2:])
	}
	return p
}

// ─── Initialisation ───────────────────────────────────────────────────────────

// InitConfig writes a fully documented default config.toml to path.
// Returns an error if path already exists (will not overwrite).
// The parent directory is created if necessary.
func InitConfig(path string) error {
	if _, err := os.Stat(path); err == nil {
		return fmt.Errorf("config: %s already exists; remove it first to re-initialise", path)
	}

	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return fmt.Errorf("config: cannot create config dir: %w", err)
	}

	const tmpl = `# wallpaperd configuration file
# Location: ~/.config/wallpaperd/config.toml

[daemon]
# Enable or disable automatic wallpaper rotation
rotation_enabled = true
# How often to rotate the wallpaper (Go duration string: "30m", "1h", "2h30m")
interval = "30m"
# Directory where downloaded wallpapers are cached
download_dir = "~/Pictures/Wallpapers/cache"
# Maximum number of images to keep in the cache directory
cache_size = 50
# Directory where trashed wallpapers are moved
trash_dir = "~/Pictures/Wallpapers/trash"
# Unix socket path used for IPC between the daemon and CLI commands
socket_path = "~/.config/wallpaperd/daemon.sock"

# --- Providers ---

[bing]
enabled = true
# Bing market code controlling language/region of results
market = "en-US"

[wallhaven]
enabled = false
# Optional Wallhaven API key — without it the provider falls back to HTML scraping
api_key = ""
# Purity filter: "100" = SFW, "110" = SFW+Sketchy, "111" = all
purity = "100"
# Sort order: "random", "toplist", "date_added", "relevance", "views", "favorites", "hot"
sorting = "random"
# Category filter: 3 bits (general/anime/people), "111" = all
categories = "111"

[unsplash]
enabled = false
# Optional Unsplash access key — without it the provider uses the anonymous URL
access_key = ""

[reddit]
enabled = false
# List of subreddits to pick from (one is chosen at random per request)
subreddits = ["EarthPorn", "wallpapers", "spaceporn"]

[apod]
enabled = false
# NASA API key — "DEMO_KEY" works for low-volume use
api_key = "DEMO_KEY"

[local]
enabled = false
# Directories to scan for local wallpapers
dirs = []
# Whether to scan subdirectories recursively
recursive = true

# --- Theme-aware provider selection ---

[light]
# Providers to use in light mode
sources = ["bing"]
# Search queries passed to providers that support them
queries = ["landscape", "nature", "cityscape"]

[dark]
# Providers to use in dark mode
sources = ["bing"]
queries = ["space", "night", "dark minimal", "galaxy"]

# --- Favorites ---

[favorites]
# Directory where favorited wallpapers are copied
dir = "~/Pictures/Wallpapers/favorites"

# --- Picker ---

[picker]
# Position of the floating thumbnail picker: "bottom", "top", "left", "right"
position = "bottom"
`

	if err := os.WriteFile(path, []byte(tmpl), 0o644); err != nil {
		return fmt.Errorf("config: cannot write default config to %s: %w", path, err)
	}
	return nil
}
