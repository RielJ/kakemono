// Package apod implements a wallpaper provider that fetches Astronomy Picture
// of the Day (APOD) images from the NASA API.
//
// Only entries with media_type == "image" are returned; video entries are
// silently skipped.  The HD image URL is preferred over the standard URL.
//
// Reference: https://api.nasa.gov/
package apod

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"time"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers"
)

const (
	// apodAPI fetches count random APOD entries.
	apodAPI = "https://api.nasa.gov/planetary/apod?api_key=%s&count=5"

	// demoKey is used when no API key is configured.
	// It is rate-limited to 30 requests/hour.
	demoKey = "DEMO_KEY"
)

// ── API response types ────────────────────────────────────────────────────────

type apodEntry struct {
	Title       string `json:"title"`
	Date        string `json:"date"`
	Explanation string `json:"explanation"`
	MediaType   string `json:"media_type"`
	URL         string `json:"url"`
	HDURL       string `json:"hdurl"`
	Copyright   string `json:"copyright"`
}

// ── Provider ──────────────────────────────────────────────────────────────────

// Provider is the NASA APOD wallpaper provider.
type Provider struct {
	enabled bool
	apiKey  string
	client  *http.Client
}

// New creates an APOD provider from the given config section.
func New(cfg *config.APODConfig) *Provider {
	apiKey := cfg.APIKey
	if apiKey == "" {
		apiKey = demoKey
	}
	return &Provider{
		enabled: cfg.Enabled,
		apiKey:  apiKey,
		client: &http.Client{
			Timeout: 20 * time.Second,
		},
	}
}

func (p *Provider) Name() string  { return "apod" }
func (p *Provider) Enabled() bool { return p.enabled }

// Fetch retrieves up to 5 random APOD images.
// The query parameter is ignored — APOD has no search interface.
func (p *Provider) Fetch(ctx context.Context, _ string) ([]providers.WallpaperResult, error) {
	reqURL := fmt.Sprintf(apodAPI, p.apiKey)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, reqURL, nil)
	if err != nil {
		return nil, fmt.Errorf("apod: build request: %w", err)
	}
	req.Header.Set("User-Agent", providers.UserAgent)

	resp, err := p.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("apod: GET: %w", err)
	}
	defer resp.Body.Close()

	switch resp.StatusCode {
	case http.StatusTooManyRequests:
		return nil, fmt.Errorf("apod: rate limit exceeded (DEMO_KEY allows 30 req/hr; set a real API key)")
	case http.StatusForbidden, http.StatusUnauthorized:
		return nil, fmt.Errorf("apod: invalid API key (status %d)", resp.StatusCode)
	case http.StatusOK:
		// ok
	default:
		return nil, fmt.Errorf("apod: unexpected status %d", resp.StatusCode)
	}

	var entries []apodEntry
	if err := json.NewDecoder(resp.Body).Decode(&entries); err != nil {
		return nil, fmt.Errorf("apod: decode response: %w", err)
	}

	results := make([]providers.WallpaperResult, 0, len(entries))
	for _, e := range entries {
		// Skip videos (e.g. YouTube embeds).
		if e.MediaType != "image" {
			continue
		}

		// Prefer HD URL.
		imgURL := e.HDURL
		if imgURL == "" {
			imgURL = e.URL
		}
		if imgURL == "" {
			continue
		}

		// Build a canonical APOD page URL from the date (format: "YYYY-MM-DD").
		sourceURL := ""
		if len(e.Date) == 10 {
			compact := e.Date[0:4] + e.Date[5:7] + e.Date[8:10] // "20240315"
			sourceURL = "https://apod.nasa.gov/apod/ap" + compact + ".html"
		}

		results = append(results, providers.WallpaperResult{
			SourceURL: sourceURL,
			ImageURL:  imgURL,
			Title:     e.Title,
			Author:    e.Copyright,
			Source:    p.Name(),
		})
	}
	return results, nil
}
