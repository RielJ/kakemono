// Package wallpaper provides state management, wallpaper setting, and the
// Manager that orchestrates rotation, history, favorites, and trash.
package wallpaper

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/rielj/wallpaperd/internal/ipc"
)

// maxHistory is the maximum number of entries kept in the history slice.
const maxHistory = 100

// State is the persisted runtime state of the wallpaperd daemon.
// It is serialised to / deserialised from a JSON file at statePath.
type State struct {
	Current      *ipc.WallpaperInfo  `json:"current"`
	History      []ipc.WallpaperInfo `json:"history"`
	HistoryIndex int                 `json:"history_index"`
	Paused       bool                `json:"paused"`
	LastChanged  time.Time           `json:"last_changed"`
}

// LoadState reads and parses state from path.  If the file does not exist a
// fresh, empty State is returned without an error.
func LoadState(path string) (*State, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return &State{HistoryIndex: -1}, nil
		}
		return nil, fmt.Errorf("state: read %s: %w", path, err)
	}

	var s State
	if err := json.Unmarshal(data, &s); err != nil {
		return nil, fmt.Errorf("state: parse %s: %w", path, err)
	}
	return &s, nil
}

// Save serialises s to path as indented JSON, creating the parent directory if
// necessary.
func (s *State) Save(path string) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return fmt.Errorf("state: create dir for %s: %w", path, err)
	}

	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return fmt.Errorf("state: marshal state: %w", err)
	}

	if err := os.WriteFile(path, data, 0o644); err != nil {
		return fmt.Errorf("state: write %s: %w", path, err)
	}
	return nil
}

// Push appends w to history.
//
// If the index is not at the end (i.e. the user previously went back with
// Prev), any "future" history entries beyond the current index are discarded
// first — just like a browser's forward history after navigating to a new page.
//
// After appending, the history is trimmed to maxHistory entries by removing
// the oldest entries. HistoryIndex is set to the last position and Current is
// updated to point to the new entry.
func (s *State) Push(w ipc.WallpaperInfo) {
	// Truncate forward history when branching from a non-terminal position.
	if s.HistoryIndex >= 0 && s.HistoryIndex < len(s.History)-1 {
		s.History = s.History[:s.HistoryIndex+1]
	}

	s.History = append(s.History, w)

	// Trim oldest entries if we exceed the cap.
	if len(s.History) > maxHistory {
		trim := len(s.History) - maxHistory
		s.History = s.History[trim:]
	}

	s.HistoryIndex = len(s.History) - 1
	s.Current = &s.History[s.HistoryIndex]
	s.LastChanged = time.Now()
}

// Prev moves HistoryIndex one step backward and returns the WallpaperInfo at
// that position, or nil if already at the beginning.
func (s *State) Prev() *ipc.WallpaperInfo {
	if !s.CanGoPrev() {
		return nil
	}
	s.HistoryIndex--
	item := s.History[s.HistoryIndex]
	s.Current = &s.History[s.HistoryIndex]
	return &item
}

// Next moves HistoryIndex one step forward (within existing history) and
// returns the WallpaperInfo at that position, or nil if already at the end.
// This method only navigates within the existing history; fetching a brand-new
// wallpaper is the Manager's responsibility.
func (s *State) Next() *ipc.WallpaperInfo {
	if !s.HasFutureHistory() {
		return nil
	}
	s.HistoryIndex++
	item := s.History[s.HistoryIndex]
	s.Current = &s.History[s.HistoryIndex]
	return &item
}

// CanGoPrev reports whether there is an older entry in the history.
func (s *State) CanGoPrev() bool {
	return s.HistoryIndex > 0 && len(s.History) > 0
}

// HasFutureHistory reports whether there is a newer entry already in the
// history that has not yet been displayed (i.e. the user went back with Prev).
func (s *State) HasFutureHistory() bool {
	return s.HistoryIndex < len(s.History)-1
}
