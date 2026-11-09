package wallpaper

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/rielj/wallpaperd/internal/providers"
)

// mockProvider implements providers.Provider for testing.
type mockProvider struct {
	name    string
	enabled bool
	results []providers.WallpaperResult
	err     error
}

func (m *mockProvider) Name() string  { return m.name }
func (m *mockProvider) Enabled() bool { return m.enabled }
func (m *mockProvider) Fetch(_ context.Context, _ string) ([]providers.WallpaperResult, error) {
	return m.results, m.err
}

// newTestManager builds a Manager with a temp directory and injected state.
func newTestManager(t *testing.T) *Manager {
	t.Helper()
	dir := t.TempDir()
	statePath := filepath.Join(dir, "state.json")

	cfg := &config.Config{}
	cfg.Defaults()
	cfg.Daemon.DownloadDir = dir
	cfg.Favorites.Dir = filepath.Join(dir, "favorites")

	reg := providers.NewRegistry()
	mgr, err := NewManager(cfg, reg, statePath)
	if err != nil {
		t.Fatalf("NewManager failed: %v", err)
	}
	return mgr
}

// TestManagerFavorite_CopiesFile verifies that Favorite copies the current wallpaper.
func TestManagerFavorite_CopiesFile(t *testing.T) {
	mgr := newTestManager(t)

	// Create a fake wallpaper file in the download dir.
	srcPath := filepath.Join(mgr.config.ExpandedDownloadDir(), "test.jpg")
	if err := os.WriteFile(srcPath, []byte("fake image data"), 0644); err != nil {
		t.Fatalf("writing test file: %v", err)
	}

	// Manually inject state with the file path.
	mgr.state.Current = &ipc.WallpaperInfo{Path: srcPath, Source: "test"}

	if err := mgr.Favorite(); err != nil {
		t.Fatalf("Favorite() failed: %v", err)
	}

	dstPath := filepath.Join(mgr.config.ExpandedFavoritesDir(), "test.jpg")
	if _, err := os.Stat(dstPath); err != nil {
		t.Errorf("expected favorite file to exist at %s: %v", dstPath, err)
	}
}

// TestManagerPauseResume verifies pause and resume state transitions.
func TestManagerPauseResume(t *testing.T) {
	mgr := newTestManager(t)

	if mgr.IsPaused() {
		t.Error("expected not paused initially")
	}

	mgr.Pause()
	if !mgr.IsPaused() {
		t.Error("expected paused after Pause()")
	}

	mgr.Resume()
	if mgr.IsPaused() {
		t.Error("expected not paused after Resume()")
	}
}

// TestManagerStatus returns correct status.
func TestManagerStatus(t *testing.T) {
	mgr := newTestManager(t)

	// Inject some state.
	mgr.state.History = []ipc.WallpaperInfo{
		{Path: "/tmp/wp1.jpg", Source: "bing", Title: "Test"},
	}
	mgr.state.HistoryIndex = 0
	mgr.state.Current = &mgr.state.History[0]
	mgr.state.Paused = true

	status := mgr.Status()
	if !status.Paused {
		t.Error("expected paused=true")
	}
	if status.HistoryCount != 1 {
		t.Errorf("expected history_count=1, got %d", status.HistoryCount)
	}
	if status.Current == nil {
		t.Fatal("expected current != nil")
	}
	if status.Current.Source != "bing" {
		t.Errorf("expected source=bing, got %q", status.Current.Source)
	}
}

// TestManagerPrev_EmptyHistory returns error when history is empty.
func TestManagerPrev_EmptyHistory(t *testing.T) {
	mgr := newTestManager(t)
	_, err := mgr.Prev()
	if err == nil {
		t.Error("expected error when no history")
	}
}

// TestManagerStatePush_IncreasesHistory verifies that pushing a wallpaper
// updates history and current state correctly.
func TestManagerStatePush_IncreasesHistory(t *testing.T) {
	mgr := newTestManager(t)

	info := ipc.WallpaperInfo{Path: "/tmp/test.jpg", Source: "local", Title: "Test Image"}
	mgr.state.Push(info)

	if mgr.state.Current == nil {
		t.Fatal("expected current != nil after push")
	}
	if mgr.state.Current.Path != "/tmp/test.jpg" {
		t.Errorf("expected path /tmp/test.jpg, got %s", mgr.state.Current.Path)
	}
	if len(mgr.state.History) != 1 {
		t.Errorf("expected 1 history entry, got %d", len(mgr.state.History))
	}
}

// TestResolveLocalPath_AbsolutePath verifies that an absolute filesystem path
// is returned as-is without attempting an HTTP download.
func TestResolveLocalPath_AbsolutePath(t *testing.T) {
	dir := t.TempDir()
	imgPath := filepath.Join(dir, "test.jpg")
	if err := os.WriteFile(imgPath, []byte("fake image"), 0644); err != nil {
		t.Fatalf("creating test file: %v", err)
	}

	result := providers.WallpaperResult{
		ImageURL: imgPath, // bare absolute path (as local provider now returns)
		Source:   "local",
	}

	got, err := resolveLocalPath(context.Background(), result, dir)
	if err != nil {
		t.Fatalf("resolveLocalPath() error: %v", err)
	}
	if got != imgPath {
		t.Errorf("expected %q, got %q", imgPath, got)
	}
}

// TestResolveLocalPath_FileURL verifies that a file:// URL is resolved to its
// underlying filesystem path without an HTTP download.
func TestResolveLocalPath_FileURL(t *testing.T) {
	dir := t.TempDir()
	imgPath := filepath.Join(dir, "test.jpg")
	if err := os.WriteFile(imgPath, []byte("fake image"), 0644); err != nil {
		t.Fatalf("creating test file: %v", err)
	}

	result := providers.WallpaperResult{
		ImageURL: "file://" + imgPath,
		Source:   "local",
	}

	got, err := resolveLocalPath(context.Background(), result, dir)
	if err != nil {
		t.Fatalf("resolveLocalPath() error: %v", err)
	}
	if got != imgPath {
		t.Errorf("expected %q, got %q", imgPath, got)
	}
}

// TestResolveLocalPath_MissingFile verifies that a missing local file returns an error.
func TestResolveLocalPath_MissingFile(t *testing.T) {
	dir := t.TempDir()
	result := providers.WallpaperResult{
		ImageURL: filepath.Join(dir, "nonexistent.jpg"),
		Source:   "local",
	}

	_, err := resolveLocalPath(context.Background(), result, dir)
	if err == nil {
		t.Error("expected error for missing local file, got nil")
	}
}

// TestManagerSaveState verifies that SaveState persists state to disk.
func TestManagerSaveState(t *testing.T) {
	dir := t.TempDir()
	statePath := filepath.Join(dir, "state.json")

	cfg := &config.Config{}
	cfg.Defaults()
	cfg.Daemon.DownloadDir = dir
	cfg.Favorites.Dir = filepath.Join(dir, "favorites")

	reg := providers.NewRegistry()
	mgr, err := NewManager(cfg, reg, statePath)
	if err != nil {
		t.Fatalf("NewManager failed: %v", err)
	}

	mgr.state.Push(ipc.WallpaperInfo{Path: "/tmp/w.jpg", Source: "test"})
	if err := mgr.SaveState(); err != nil {
		t.Fatalf("SaveState() failed: %v", err)
	}

	if _, err := os.Stat(statePath); err != nil {
		t.Errorf("expected state file to exist: %v", err)
	}
}
