// Package bing implements a wallpaper provider that fetches images from the
// Bing Daily Wallpaper feed.  Each call to Fetch returns up to 8 images
// (one per day for the past 8 days).
package bing

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"regexp"
	"strings"
	"time"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers"
)

const (
	// bingAPIPath is the path appended to the base URL for the Bing HP Image Archive.
	// idx=0 means "starting from today", n=8 means "return 8 days".
	bingAPIPath = "/HPImageArchive.aspx?format=js&idx=0&n=8&mkt=%s"

	// bingBase is the default Bing host; extracted so tests can override it.
	bingBase = "https://www.bing.com"
)

// resRE matches the resolution token in Bing image URLs (e.g. _1920x1080.jpg).
var resRE = regexp.MustCompile(`_\d+x\d+\.jpg`)

// bingResponse is the top-level JSON structure returned by the Bing API.
type bingResponse struct {
	Images []bingImage `json:"images"`
}

// bingImage represents a single day's image entry.
type bingImage struct {
	URL           string `json:"url"`
	URLBase       string `json:"urlbase"`
	Title         string `json:"title"`
	Copyright     string `json:"copyright"`
	CopyrightLink string `json:"copyrightlink"`
}

// Provider is the Bing Daily wallpaper provider.
type Provider struct {
	cfg     config.BingConfig
	baseURL string // default: bingBase; overridable in tests
	client  *http.Client
}

// New creates a Bing provider from the given config section.
func New(cfg config.BingConfig) *Provider {
	return NewWithBaseURL(cfg, bingBase)
}

// NewWithBaseURL creates a Bing provider with a custom base URL.
// This is primarily used in tests to point the provider at a mock HTTP server.
func NewWithBaseURL(cfg config.BingConfig, baseURL string) *Provider {
	return &Provider{
		cfg:     cfg,
		baseURL: baseURL,
		client: &http.Client{
			Timeout: 15 * time.Second,
		},
	}
}

func (p *Provider) Name() string  { return "bing" }
func (p *Provider) Enabled() bool { return p.cfg.Enabled }

// Fetch retrieves up to 8 Bing daily wallpapers.
// The query parameter is ignored — Bing chooses its own editorial images.
func (p *Provider) Fetch(ctx context.Context, _ string) ([]providers.WallpaperResult, error) {
	market := p.cfg.Market
	if market == "" {
		market = "en-US"
	}
	apiURL := p.baseURL + fmt.Sprintf(bingAPIPath, market)

	req, err := providers.NewRequest(ctx, apiURL)
	if err != nil {
		return nil, fmt.Errorf("bing: build request: %w", err)
	}

	resp, err := p.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("bing: GET %s: %w", apiURL, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("bing: unexpected status %d", resp.StatusCode)
	}

	var br bingResponse
	if err := json.NewDecoder(resp.Body).Decode(&br); err != nil {
		return nil, fmt.Errorf("bing: decode response: %w", err)
	}

	results := make([]providers.WallpaperResult, 0, len(br.Images))
	for _, img := range br.Images {
		if img.URL == "" {
			continue
		}

		// Prepend the base host to the relative URL.
		rawURL := p.baseURL + img.URL

		// Replace the resolution token with _UHD.jpg for maximum quality.
		uhdURL := replaceResolution(rawURL)

		// If the URL had no resolution token, build from urlbase instead.
		if uhdURL == rawURL && img.URLBase != "" {
			uhdURL = p.baseURL + img.URLBase + "_UHD.jpg"
		}

		results = append(results, providers.WallpaperResult{
			SourceURL: img.CopyrightLink,
			ImageURL:  uhdURL,
			Title:     img.Title,
			Author:    img.Copyright,
			Source:    p.Name(),
		})
	}

	if len(results) == 0 {
		return nil, fmt.Errorf("bing: no images returned from API")
	}
	return results, nil
}

// replaceResolution swaps the resolution suffix (e.g. _1920x1080.jpg) with
// _UHD.jpg and strips any trailing query-string junk after the .jpg token.
func replaceResolution(url string) string {
	result := resRE.ReplaceAllString(url, "_UHD.jpg")
	// Strip extra query params that may appear after _UHD.jpg (some Bing URLs).
	if idx := strings.Index(result, "_UHD.jpg&"); idx != -1 {
		result = result[:idx+len("_UHD.jpg")]
	}
	return result
}
