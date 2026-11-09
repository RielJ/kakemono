package config

import (
	"os"
	"path/filepath"
	"testing"
)

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

func TestDefaults_Interval(t *testing.T) {
	cfg := Defaults()
	if cfg.Daemon.Interval != "30m" {
		t.Errorf("expected daemon interval 30m, got %q", cfg.Daemon.Interval)
	}
}

func TestDefaults_CacheSize(t *testing.T) {
	cfg := Defaults()
	if cfg.Daemon.CacheSize != 50 {
		t.Errorf("expected cache size 50, got %d", cfg.Daemon.CacheSize)
	}
}

func TestDefaults_SocketPath(t *testing.T) {
	cfg := Defaults()
	if cfg.Daemon.SocketPath == "" {
		t.Error("expected non-empty default SocketPath")
	}
}

func TestDefaults_WallhavenPurity(t *testing.T) {
	cfg := Defaults()
	if cfg.Wallhaven.Purity != "100" {
		t.Errorf("expected wallhaven purity 100, got %q", cfg.Wallhaven.Purity)
	}
}

func TestDefaults_WallhavenCategories(t *testing.T) {
	cfg := Defaults()
	if cfg.Wallhaven.Categories != "111" {
		t.Errorf("expected wallhaven categories 111, got %q", cfg.Wallhaven.Categories)
	}
}

func TestDefaults_APODKey(t *testing.T) {
	cfg := Defaults()
	if cfg.APOD.APIKey != "DEMO_KEY" {
		t.Errorf("expected APOD api_key DEMO_KEY, got %q", cfg.APOD.APIKey)
	}
}

func TestDefaults_Reddit(t *testing.T) {
	cfg := Defaults()
	if len(cfg.Reddit.Subreddits) == 0 {
		t.Error("expected non-empty default subreddits")
	}
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

func TestLoad_ValidFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.toml")

	content := `
[daemon]
interval = "1h"
cache_size = 100

[wallhaven]
enabled = true
api_key = "testkey"

[reddit]
enabled = true
subreddits = ["EarthPorn", "spaceporn"]
`
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}

	cfg, err := Load(path)
	if err != nil {
		t.Fatalf("Load returned error: %v", err)
	}

	if cfg.Daemon.Interval != "1h" {
		t.Errorf("expected interval 1h, got %q", cfg.Daemon.Interval)
	}
	if cfg.Daemon.CacheSize != 100 {
		t.Errorf("expected cache_size 100, got %d", cfg.Daemon.CacheSize)
	}
	if cfg.Wallhaven.APIKey != "testkey" {
		t.Errorf("expected api_key testkey, got %q", cfg.Wallhaven.APIKey)
	}
	if len(cfg.Reddit.Subreddits) != 2 {
		t.Errorf("expected 2 subreddits, got %d", len(cfg.Reddit.Subreddits))
	}

	// Unspecified keys should keep defaults.
	if cfg.APOD.APIKey != "DEMO_KEY" {
		t.Errorf("default APOD api_key overwritten unexpectedly: %s", cfg.APOD.APIKey)
	}
}

func TestLoad_MissingFile_ReturnsError(t *testing.T) {
	_, err := Load("/nonexistent/path/config.toml")
	if err == nil {
		t.Error("expected error for missing file, got nil")
	}
}

func TestLoad_InvalidTOML(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.toml")

	if err := os.WriteFile(path, []byte("this is not valid toml ]["), 0o644); err != nil {
		t.Fatal(err)
	}

	_, err := Load(path)
	if err == nil {
		t.Error("expected error for invalid TOML, got nil")
	}
}

func TestLoad_ExpandsTilde(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.toml")

	content := `
[daemon]
download_dir = "~/Pictures/custom"
`
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}

	cfg, err := Load(path)
	if err != nil {
		t.Fatalf("Load returned error: %v", err)
	}

	home, _ := os.UserHomeDir()
	expected := filepath.Join(home, "Pictures", "custom")
	if cfg.Daemon.DownloadDir != expected {
		t.Errorf("expected expanded path %q, got %q", expected, cfg.Daemon.DownloadDir)
	}
}

// ---------------------------------------------------------------------------
// LoadOrDefault
// ---------------------------------------------------------------------------

func TestLoadOrDefault_MissingFile(t *testing.T) {
	cfg, err := LoadOrDefault("/nonexistent/path/config.toml")
	if err != nil {
		t.Fatalf("LoadOrDefault returned unexpected error: %v", err)
	}
	if cfg == nil {
		t.Fatal("LoadOrDefault returned nil config")
	}
	if cfg.Daemon.Interval != "30m" {
		t.Errorf("expected default interval 30m, got %q", cfg.Daemon.Interval)
	}
}

// ---------------------------------------------------------------------------
// DefaultConfigPath
// ---------------------------------------------------------------------------

func TestDefaultConfigPath_Default(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", "")

	path := DefaultConfigPath()
	home, _ := os.UserHomeDir()
	expected := filepath.Join(home, ".config", "wallpaperd", "config.toml")
	if path != expected {
		t.Errorf("expected %q, got %q", expected, path)
	}
}

func TestDefaultConfigPath_XDG(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", "/custom/xdg")

	path := DefaultConfigPath()
	expected := "/custom/xdg/wallpaperd/config.toml"
	if path != expected {
		t.Errorf("expected %q, got %q", expected, path)
	}
}

// ---------------------------------------------------------------------------
// InitConfig
// ---------------------------------------------------------------------------

func TestInitConfig_CreatesFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "subdir", "config.toml")

	if err := InitConfig(path); err != nil {
		t.Fatalf("InitConfig returned error: %v", err)
	}

	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("could not read created config: %v", err)
	}
	if len(data) == 0 {
		t.Error("created config file is empty")
	}
}

func TestInitConfig_CreatesParentDirs(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "a", "b", "c", "config.toml")

	if err := InitConfig(path); err != nil {
		t.Fatalf("InitConfig failed: %v", err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Errorf("config file not created: %v", err)
	}
}

func TestInitConfig_RefusesOverwrite(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.toml")

	if err := os.WriteFile(path, []byte("[daemon]\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	err := InitConfig(path)
	if err == nil {
		t.Error("expected error when overwriting existing file, got nil")
	}
}

// ---------------------------------------------------------------------------
// Struct completeness smoke test
// ---------------------------------------------------------------------------

func TestConfig_AllFieldsAccessible(t *testing.T) {
	var cfg Config
	_ = cfg.Daemon.Interval
	_ = cfg.Daemon.DownloadDir
	_ = cfg.Daemon.CacheSize
	_ = cfg.Daemon.SocketPath
	_ = cfg.Wallhaven.Enabled
	_ = cfg.Wallhaven.APIKey
	_ = cfg.Wallhaven.Purity
	_ = cfg.Wallhaven.Sorting
	_ = cfg.Wallhaven.Categories
	_ = cfg.Unsplash.Enabled
	_ = cfg.Unsplash.AccessKey
	_ = cfg.Reddit.Enabled
	_ = cfg.Reddit.Subreddits
	_ = cfg.Bing.Enabled
	_ = cfg.Bing.Market
	_ = cfg.APOD.Enabled
	_ = cfg.APOD.APIKey
	_ = cfg.Local.Enabled
	_ = cfg.Local.Dirs
	_ = cfg.Local.Recursive
	_ = cfg.Light.Sources
	_ = cfg.Light.Queries
	_ = cfg.Dark.Sources
	_ = cfg.Dark.Queries
	_ = cfg.Favorites.Dir

}
