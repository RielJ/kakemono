// Package unsplash implements a wallpaper provider that fetches high-resolution
// landscape photos from Unsplash.
//
// Two operational modes:
//
//   - API mode    — uses the official Unsplash API with an access key;
//     returns rich metadata (author, description, …).
//   - No-key mode — uses source.unsplash.com redirects; slower but requires
//     no registration.
package unsplash

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers"
)

const (
	apiURL    = "https://api.unsplash.com/photos/random"
	sourceURL = "https://source.unsplash.com/random/3840x2160/"
)

// ── API response types ────────────────────────────────────────────────────────

type apiPhoto struct {
	ID          string `json:"id"`
	Description string `json:"description"`
	AltDesc     string `json:"alt_description"`
	Links       struct {
		HTML     string `json:"html"`
		Download string `json:"download"`
	} `json:"links"`
	URLs struct {
		Raw  string `json:"raw"`
		Full string `json:"full"`
	} `json:"urls"`
	User struct {
		Name     string `json:"name"`
		Username string `json:"username"`
	} `json:"user"`
}

// ── Provider ──────────────────────────────────────────────────────────────────

// Provider is the Unsplash wallpaper provider.
type Provider struct {
	enabled   bool
	accessKey string
	client    *http.Client
	// noRedirectClient follows no redirects so we can capture the final image URL
	// from source.unsplash.com without downloading the full image body.
	noRedirectClient *http.Client
}

// New creates an Unsplash provider from the given config section.
func New(cfg *config.UnsplashConfig) *Provider {
	return &Provider{
		enabled:   cfg.Enabled,
		accessKey: cfg.AccessKey,
		client: &http.Client{
			Timeout: 20 * time.Second,
		},
		noRedirectClient: &http.Client{
			Timeout: 15 * time.Second,
			// Do not follow redirects; we only want the Location header.
			CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
	}
}

func (p *Provider) Name() string  { return "unsplash" }
func (p *Provider) Enabled() bool { return p.enabled }

// Fetch returns landscape wallpapers from Unsplash.
func (p *Provider) Fetch(ctx context.Context, query string) ([]providers.WallpaperResult, error) {
	if p.accessKey != "" {
		return p.fetchAPI(ctx, query)
	}
	return p.fetchNoKey(ctx, query)
}

// ── API mode ──────────────────────────────────────────────────────────────────

func (p *Provider) fetchAPI(ctx context.Context, query string) ([]providers.WallpaperResult, error) {
	params := url.Values{}
	params.Set("count", "10")
	params.Set("orientation", "landscape")
	params.Set("client_id", p.accessKey)
	if query != "" {
		params.Set("query", query)
	}

	reqURL := apiURL + "?" + params.Encode()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, reqURL, nil)
	if err != nil {
		return nil, fmt.Errorf("unsplash api: build request: %w", err)
	}
	req.Header.Set("User-Agent", providers.UserAgent)

	resp, err := p.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("unsplash api: GET: %w", err)
	}
	defer resp.Body.Close()

	switch resp.StatusCode {
	case http.StatusUnauthorized, http.StatusForbidden:
		return nil, fmt.Errorf("unsplash api: invalid or revoked access key (status %d)", resp.StatusCode)
	case http.StatusTooManyRequests:
		return nil, fmt.Errorf("unsplash api: rate limit exceeded")
	case http.StatusOK:
		// ok
	default:
		return nil, fmt.Errorf("unsplash api: unexpected status %d", resp.StatusCode)
	}

	// With count>1 the API always returns an array.
	var photos []apiPhoto
	if err := json.NewDecoder(resp.Body).Decode(&photos); err != nil {
		return nil, fmt.Errorf("unsplash api: decode: %w", err)
	}

	results := make([]providers.WallpaperResult, 0, len(photos))
	for _, photo := range photos {
		imgURL := photo.URLs.Raw
		if imgURL == "" {
			imgURL = photo.URLs.Full
		}
		if imgURL == "" {
			continue
		}
		// Append format/quality parameters for maximum resolution.
		if !strings.Contains(imgURL, "fm=") {
			imgURL += "&fm=jpg&q=95&w=3840"
		}

		title := photo.Description
		if title == "" {
			title = photo.AltDesc
		}

		results = append(results, providers.WallpaperResult{
			SourceURL: photo.Links.HTML,
			ImageURL:  imgURL,
			Title:     title,
			Author:    photo.User.Name,
			Source:    p.Name(),
		})
	}
	return results, nil
}

// ── No-key mode ───────────────────────────────────────────────────────────────

// fetchNoKey uses source.unsplash.com which redirects to a random image.
// We issue a HEAD request (no redirect) to capture the final image URL from
// the Location header, avoiding a full image download at this stage.
func (p *Provider) fetchNoKey(ctx context.Context, query string) ([]providers.WallpaperResult, error) {
	reqURL := sourceURL
	if query != "" {
		reqURL += url.QueryEscape(query)
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodHead, reqURL, nil)
	if err != nil {
		return nil, fmt.Errorf("unsplash no-key: build request: %w", err)
	}
	req.Header.Set("User-Agent", providers.UserAgent)

	resp, err := p.noRedirectClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("unsplash no-key: HEAD %s: %w", reqURL, err)
	}
	resp.Body.Close()

	// Expect a redirect (301/302/307).
	loc := resp.Header.Get("Location")
	if loc == "" {
		// Fall back to treating the current URL as the image URL.
		loc = reqURL
	}

	return []providers.WallpaperResult{
		{
			SourceURL: "https://unsplash.com",
			ImageURL:  loc,
			Source:    p.Name(),
		},
	}, nil
}
