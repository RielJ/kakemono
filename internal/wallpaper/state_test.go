package wallpaper

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/rielj/wallpaperd/internal/ipc"
)

func makeInfo(n int) ipc.WallpaperInfo {
	return ipc.WallpaperInfo{
		Path:   fmt.Sprintf("/tmp/wallpaper%d.jpg", n),
		Source: "test",
		Title:  fmt.Sprintf("Wallpaper %d", n),
	}
}

// TestPush_MaxSize verifies that history is trimmed to maxHistory entries.
func TestPush_MaxSize(t *testing.T) {
	s := &State{HistoryIndex: -1}
	for i := 0; i < maxHistory+10; i++ {
		s.Push(makeInfo(i))
	}
	if len(s.History) != maxHistory {
		t.Errorf("expected %d entries, got %d", maxHistory, len(s.History))
	}
	if s.HistoryIndex != maxHistory-1 {
		t.Errorf("expected index %d, got %d", maxHistory-1, s.HistoryIndex)
	}
}

// TestHistoryNavigation verifies prev/next index movement.
func TestHistoryNavigation(t *testing.T) {
	s := &State{HistoryIndex: -1}

	s.Push(makeInfo(1))
	s.Push(makeInfo(2))
	s.Push(makeInfo(3))

	if s.HistoryIndex != 2 {
		t.Fatalf("expected index 2, got %d", s.HistoryIndex)
	}
	if !s.CanGoPrev() {
		t.Error("expected CanGoPrev() == true after 3 pushes")
	}
	if s.HasFutureHistory() {
		t.Error("expected HasFutureHistory() == false at end")
	}

	// Go back.
	prev := s.Prev()
	if prev == nil {
		t.Fatal("Prev() returned nil")
	}
	if prev.Title != "Wallpaper 2" {
		t.Errorf("expected 'Wallpaper 2', got %q", prev.Title)
	}
	if s.HistoryIndex != 1 {
		t.Errorf("expected index 1, got %d", s.HistoryIndex)
	}
	if !s.HasFutureHistory() {
		t.Error("expected HasFutureHistory() == true after going back")
	}

	// Go back again.
	prev = s.Prev()
	if prev == nil {
		t.Fatal("second Prev() returned nil")
	}
	if prev.Title != "Wallpaper 1" {
		t.Errorf("expected 'Wallpaper 1', got %q", prev.Title)
	}
	if s.CanGoPrev() {
		t.Error("expected CanGoPrev() == false at beginning")
	}

	// Prev at start returns nil.
	if s.Prev() != nil {
		t.Error("Prev() at beginning should return nil")
	}

	// Go forward.
	next := s.Next()
	if next == nil {
		t.Fatal("Next() returned nil")
	}
	if next.Title != "Wallpaper 2" {
		t.Errorf("expected 'Wallpaper 2', got %q", next.Title)
	}
}

// TestStatePersistence verifies save + load round-trip.
func TestStatePersistence(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "state.json")

	s := &State{HistoryIndex: -1}
	s.Push(makeInfo(1))
	s.Push(makeInfo(2))

	if err := s.Save(path); err != nil {
		t.Fatalf("Save() failed: %v", err)
	}

	loaded, err := LoadState(path)
	if err != nil {
		t.Fatalf("LoadState() failed: %v", err)
	}

	if len(loaded.History) != 2 {
		t.Errorf("expected 2 history entries, got %d", len(loaded.History))
	}
	if loaded.HistoryIndex != 1 {
		t.Errorf("expected index 1, got %d", loaded.HistoryIndex)
	}
	if loaded.Current == nil {
		t.Fatal("Current should not be nil after load")
	}
	if loaded.Current.Title != "Wallpaper 2" {
		t.Errorf("expected 'Wallpaper 2', got %q", loaded.Current.Title)
	}
}

// TestLoadState_MissingFile verifies that a missing state file returns an empty state.
func TestLoadState_MissingFile(t *testing.T) {
	s, err := LoadState("/nonexistent/path/state.json")
	if err != nil {
		t.Fatalf("expected no error for missing file, got %v", err)
	}
	if s == nil {
		t.Fatal("expected empty state, got nil")
	}
	if len(s.History) != 0 {
		t.Errorf("expected empty history, got %d entries", len(s.History))
	}
}

// TestPush_TruncatesForwardHistory verifies that pushing mid-history discards future entries.
func TestPush_TruncatesForwardHistory(t *testing.T) {
	s := &State{HistoryIndex: -1}
	s.Push(makeInfo(1))
	s.Push(makeInfo(2))
	s.Push(makeInfo(3))

	// Go back twice.
	s.Prev()
	s.Prev()

	// Now push a new entry — should truncate wallpapers 2 and 3.
	s.Push(makeInfo(99))

	if len(s.History) != 2 {
		t.Errorf("expected 2 entries after truncation, got %d", len(s.History))
	}
	if s.History[1].Title != "Wallpaper 99" {
		t.Errorf("expected 'Wallpaper 99', got %q", s.History[1].Title)
	}
	if s.HasFutureHistory() {
		t.Error("should not have future history after new push")
	}
}

// TestSave_CreatesDirectory verifies that Save creates parent directories.
func TestSave_CreatesDirectory(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "subdir", "state.json")

	s := &State{HistoryIndex: -1}
	if err := s.Save(path); err != nil {
		t.Fatalf("Save() failed: %v", err)
	}

	if _, err := os.Stat(path); err != nil {
		t.Errorf("expected state file to exist: %v", err)
	}
}
