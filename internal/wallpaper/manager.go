package wallpaper

import (
	"context"
	"fmt"
	"io"
	"math/rand"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/rielj/wallpaperd/internal/providers"
	"github.com/rielj/wallpaperd/internal/theme"
)

// resolveLocalPath returns the filesystem path for a wallpaper result.
// If the result's ImageURL is an absolute path or a file:// URL (as returned by
// the "local" provider), the file is already on disk and no download is needed.
// Otherwise the image is downloaded to downloadDir and the resulting local path
// is returned.
func resolveLocalPath(ctx context.Context, result providers.WallpaperResult, downloadDir string) (string, error) {
	imgURL := result.ImageURL

	// Handle explicit file:// scheme.
	if strings.HasPrefix(imgURL, "file://") {
		p := strings.TrimPrefix(imgURL, "file://")
		if _, err := os.Stat(p); err != nil {
			return "", fmt.Errorf("manager: local file not found %q: %w", p, err)
		}
		return p, nil
	}

	// Handle bare absolute filesystem paths (local provider after the bug-fix).
	if filepath.IsAbs(imgURL) {
		if _, err := os.Stat(imgURL); err != nil {
			return "", fmt.Errorf("manager: local file not found %q: %w", imgURL, err)
		}
		return imgURL, nil
	}

	// Regular HTTP/HTTPS URL — download it.
	localPath, err := providers.DownloadFile(ctx, imgURL, downloadDir, nil)
	if err != nil {
		return "", fmt.Errorf("manager: download image: %w", err)
	}
	return localPath, nil
}

// Manager orchestrates wallpaper rotation, history, favorites, and trash.
// All public methods are safe for concurrent use.
type Manager struct {
	config    *config.Config
	state     *State
	statePath string
	registry  *providers.Registry
	mu        sync.Mutex
}

// NewManager creates a Manager, loading persisted state from statePath.
// If statePath is empty, config.DefaultStatePath() is used.
func NewManager(cfg *config.Config, registry *providers.Registry, statePath ...string) (*Manager, error) {
	sp := config.DefaultStatePath()
	if len(statePath) > 0 && statePath[0] != "" {
		sp = statePath[0]
	}

	state, err := LoadState(sp)
	if err != nil {
		return nil, fmt.Errorf("manager: load state: %w", err)
	}

	// Ensure required directories exist.
	for _, dir := range []string{cfg.Daemon.DownloadDir, cfg.TrashDir(), cfg.Favorites.Dir} {
		if dir == "" {
			continue
		}
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return nil, fmt.Errorf("manager: create dir %s: %w", dir, err)
		}
	}

	return &Manager{
		config:    cfg,
		state:     state,
		statePath: sp,
		registry:  registry,
	}, nil
}

// Next advances to the next wallpaper.
//
//  1. If there is future history (user went back with Prev), the next history
//     entry is used directly without a network fetch.
//  2. Otherwise a new wallpaper is fetched from a provider chosen based on the
//     current macOS theme, downloaded, added to history, and set.
func (m *Manager) Next(ctx context.Context) (*ipc.WallpaperInfo, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.state.HasFutureHistory() {
		info := m.state.Next()
		if info == nil {
			return nil, fmt.Errorf("manager: unexpected nil from history")
		}
		if err := SetWallpaper(info.Path); err != nil {
			return nil, fmt.Errorf("manager: set wallpaper: %w", err)
		}
		_ = m.state.Save(m.statePath)
		return info, nil
	}

	// Fetch a fresh wallpaper from a provider.
	info, err := m.fetchAndSet(ctx)
	if err != nil {
		return nil, err
	}
	_ = m.state.Save(m.statePath)
	return info, nil
}

// Prev steps back to the previous wallpaper in history.
// Returns an error if already at the beginning of history.
func (m *Manager) Prev() (*ipc.WallpaperInfo, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	info := m.state.Prev()
	if info == nil {
		return nil, fmt.Errorf("manager: already at the beginning of history")
	}

	if err := SetWallpaper(info.Path); err != nil {
		return nil, fmt.Errorf("manager: set wallpaper: %w", err)
	}

	_ = m.state.Save(m.statePath)
	return info, nil
}

// SetFile sets a specific local image as the wallpaper, adding it to history.
func (m *Manager) SetFile(path string) (*ipc.WallpaperInfo, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if _, err := os.Stat(path); err != nil {
		return nil, fmt.Errorf("manager: file not found: %w", err)
	}

	if err := SetWallpaper(path); err != nil {
		return nil, fmt.Errorf("manager: set wallpaper: %w", err)
	}

	info := ipc.WallpaperInfo{
		Path:   path,
		Source: "local",
		Title:  filepath.Base(path),
		SetAt:  time.Now().Format(time.RFC3339),
	}
	m.state.Push(info)
	_ = m.state.Save(m.statePath)
	return &info, nil
}

// Favorite copies the current wallpaper file to the favorites directory.
func (m *Manager) Favorite() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.state.Current == nil {
		return fmt.Errorf("manager: no current wallpaper")
	}

	favDir := m.config.ExpandedFavoritesDir()
	if err := os.MkdirAll(favDir, 0o755); err != nil {
		return fmt.Errorf("manager: create favorites dir: %w", err)
	}

	src := m.state.Current.Path
	dst := filepath.Join(favDir, filepath.Base(src))

	if err := copyFile(src, dst); err != nil {
		return fmt.Errorf("manager: copy to favorites: %w", err)
	}
	return nil
}

// Trash moves the current wallpaper to the trash directory and immediately
// calls Next() to display a fresh wallpaper.
func (m *Manager) Trash(ctx context.Context) (*ipc.WallpaperInfo, error) {
	m.mu.Lock()

	if m.state.Current == nil {
		m.mu.Unlock()
		return nil, fmt.Errorf("manager: no current wallpaper")
	}

	trashDir := m.config.TrashDir()
	if err := os.MkdirAll(trashDir, 0o755); err != nil {
		m.mu.Unlock()
		return nil, fmt.Errorf("manager: create trash dir: %w", err)
	}

	src := m.state.Current.Path
	dst := filepath.Join(trashDir, filepath.Base(src))

	if err := os.Rename(src, dst); err != nil {
		if err2 := copyFile(src, dst); err2 != nil {
			m.mu.Unlock()
			return nil, fmt.Errorf("manager: move to trash: %w (copy fallback: %v)", err, err2)
		}
		_ = os.Remove(src)
	}

	m.mu.Unlock()

	return m.Next(ctx)
}

// Pause suspends auto-rotation.
func (m *Manager) Pause() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.state.Paused = true
	_ = m.state.Save(m.statePath)
}

// Resume re-enables auto-rotation.
func (m *Manager) Resume() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.state.Paused = false
	_ = m.state.Save(m.statePath)
}

// IsPaused reports whether auto-rotation is currently paused.
func (m *Manager) IsPaused() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.state.Paused
}

// Status returns a snapshot of the current daemon status.
func (m *Manager) Status() ipc.StatusData {
	m.mu.Lock()
	defer m.mu.Unlock()

	return ipc.StatusData{
		Current:      m.state.Current,
		Paused:       m.state.Paused,
		Interval:     m.config.Daemon.Interval,
		Theme:        string(theme.Current()),
		HistoryCount: len(m.state.History),
		HistoryIndex: m.state.HistoryIndex,
	}
}

// SaveState persists the current state to disk.
func (m *Manager) SaveState() error {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.state.Save(m.statePath)
}

// ─── internal helpers ────────────────────────────────────────────────────────

func (m *Manager) fetchAndSet(ctx context.Context) (*ipc.WallpaperInfo, error) {
	th := theme.Current()

	var sources []string
	var queries []string
	if th == theme.ThemeDark {
		sources = m.config.Dark.Sources
		queries = m.config.Dark.Queries
	} else {
		sources = m.config.Light.Sources
		queries = m.config.Light.Queries
	}

	provider := m.registry.Random(sources)
	if provider == nil {
		return nil, fmt.Errorf("manager: no enabled provider available (theme: %s, sources: %v)", th, sources)
	}

	query := ""
	if len(queries) > 0 {
		query = queries[rand.Intn(len(queries))]
	}

	results, err := provider.Fetch(ctx, query)
	if err != nil {
		return nil, fmt.Errorf("manager: provider %s fetch: %w", provider.Name(), err)
	}
	if len(results) == 0 {
		return nil, fmt.Errorf("manager: provider %s returned no results for query %q", provider.Name(), query)
	}

	result := results[rand.Intn(len(results))]

	// If the provider returned a local file path (e.g. from the "local"
	// provider), use it directly — there is nothing to download.
	// We detect this by checking for an absolute path or a file:// URL.
	localPath, err := resolveLocalPath(ctx, result, m.config.Daemon.DownloadDir)
	if err != nil {
		return nil, err
	}

	if err := SetWallpaper(localPath); err != nil {
		return nil, fmt.Errorf("manager: set wallpaper: %w", err)
	}

	m.trimCache()

	info := ipc.WallpaperInfo{
		Path:      localPath,
		Source:    result.Source,
		SourceURL: result.SourceURL,
		ImageURL:  result.ImageURL,
		Title:     result.Title,
		Author:    result.Author,
		SetAt:     time.Now().Format(time.RFC3339),
	}
	m.state.Push(info)
	return &info, nil
}

func (m *Manager) trimCache() {
	maxSize := m.config.Daemon.CacheSize
	if maxSize <= 0 {
		return
	}

	dir := m.config.Daemon.DownloadDir
	entries, err := os.ReadDir(dir)
	if err != nil {
		return
	}

	type fileEntry struct {
		path    string
		modTime time.Time
	}
	var files []fileEntry
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		p := filepath.Join(dir, e.Name())
		if !providers.IsImageFile(p) {
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		files = append(files, fileEntry{path: p, modTime: info.ModTime()})
	}

	if len(files) <= maxSize {
		return
	}

	for i := 1; i < len(files); i++ {
		for j := i; j > 0 && files[j].modTime.Before(files[j-1].modTime); j-- {
			files[j], files[j-1] = files[j-1], files[j]
		}
	}

	excess := len(files) - maxSize
	currentPath := ""
	if m.state.Current != nil {
		currentPath = m.state.Current.Path
	}

	deleted := 0
	for _, f := range files {
		if deleted >= excess {
			break
		}
		if f.path == currentPath {
			continue
		}
		_ = os.Remove(f.path)
		deleted++
	}
}

func copyFile(src, dst string) error {
	if _, err := os.Stat(dst); err == nil {
		ext := filepath.Ext(dst)
		base := strings.TrimSuffix(dst, ext)
		dst = fmt.Sprintf("%s_%d%s", base, time.Now().UnixNano(), ext)
	}

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
