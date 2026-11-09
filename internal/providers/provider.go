// Package providers defines the Provider interface and shared types used by
// all wallpaper source implementations.
package providers

import "context"

// WallpaperResult is a single wallpaper candidate returned by a Provider.
type WallpaperResult struct {
	// SourceURL is the human-readable page where the image was found.
	SourceURL string
	// ImageURL is the direct URL to the image file itself.
	ImageURL string
	// Title is an optional human-readable title for the wallpaper.
	Title string
	// Author is the creator of the wallpaper (may be empty).
	Author string
	// Source is the provider name (e.g. "wallhaven", "bing").
	Source string
}

// Provider is implemented by every wallpaper source.
type Provider interface {
	// Name returns the canonical, lower-case provider identifier.
	Name() string
	// Enabled reports whether the provider is turned on in the config.
	Enabled() bool
	// Fetch fetches a batch of wallpaper candidates matching query.
	// query may be empty for providers that don't support search.
	Fetch(ctx context.Context, query string) ([]WallpaperResult, error)
}
