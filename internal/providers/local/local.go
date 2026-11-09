// Package local implements a wallpaper provider that sources images from
// directories on the local filesystem.
//
// Supported formats: .jpg, .jpeg, .png, .webp
// The returned list is shuffled on every call so callers receive a different
// ordering each time.
package local

import (
	"context"
	"fmt"
	"io/fs"
	"math/rand"
	"os"
	"path/filepath"
	"strings"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers"
)

// imageExts is the set of supported image file extensions (lowercase).
var imageExts = map[string]bool{
	".jpg":  true,
	".jpeg": true,
	".png":  true,
	".webp": true,
}

// Provider is the local-directory wallpaper provider.
type Provider struct {
	enabled   bool
	dirs      []string
	recursive bool
}

// New creates a Local provider from the given config section.
func New(cfg *config.LocalConfig) *Provider {
	// Expand ~ in directory paths.
	dirs := make([]string, 0, len(cfg.Dirs))
	for _, d := range cfg.Dirs {
		dirs = append(dirs, providers.ExpandPath(d))
	}
	return &Provider{
		enabled:   cfg.Enabled,
		dirs:      dirs,
		recursive: cfg.Recursive,
	}
}

func (p *Provider) Name() string  { return "local" }
func (p *Provider) Enabled() bool { return p.enabled }

// Fetch scans the configured directories and returns all found image files as
// WallpaperResult entries, shuffled randomly.
// The query parameter is ignored.
func (p *Provider) Fetch(ctx context.Context, _ string) ([]providers.WallpaperResult, error) {
	if len(p.dirs) == 0 {
		return []providers.WallpaperResult{}, nil
	}

	var paths []string
	for _, dir := range p.dirs {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}

		fi, err := os.Stat(dir)
		if err != nil {
			// Non-fatal: skip inaccessible directories.
			continue
		}
		if !fi.IsDir() {
			continue
		}

		found, err := collectImages(ctx, dir, p.recursive)
		if err != nil {
			return nil, fmt.Errorf("local: scan %s: %w", dir, err)
		}
		paths = append(paths, found...)
	}

	if len(paths) == 0 {
		return []providers.WallpaperResult{}, nil
	}

	// Shuffle for randomness on every call.
	rand.Shuffle(len(paths), func(i, j int) { paths[i], paths[j] = paths[j], paths[i] })

	results := make([]providers.WallpaperResult, 0, len(paths))
	for _, p := range paths {
		results = append(results, providers.WallpaperResult{
			// SourceURL uses the file:// scheme for display purposes.
			SourceURL: "file://" + p,
			// ImageURL is the bare filesystem path so the manager can detect
			// that no HTTP download is needed and use the file directly.
			ImageURL: p,
			// Title is the filename without extension.
			Title:  strings.TrimSuffix(filepath.Base(p), filepath.Ext(p)),
			Source: "local",
		})
	}
	return results, nil
}

// collectImages walks dir and returns all image file paths.
// If recursive is false, only the immediate children of dir are examined.
func collectImages(ctx context.Context, dir string, recursive bool) ([]string, error) {
	var paths []string

	err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			// Skip unreadable paths.
			return nil
		}
		if ctx.Err() != nil {
			return ctx.Err()
		}

		if d.IsDir() {
			if path == dir {
				return nil // always descend into the root dir
			}
			if !recursive {
				return filepath.SkipDir
			}
			return nil
		}

		if imageExts[strings.ToLower(filepath.Ext(path))] {
			paths = append(paths, path)
		}
		return nil
	})

	return paths, err
}
