// Package wallhaven implements a wallpaper provider that fetches images from
// Wallhaven (https://wallhaven.cc).
//
// Two operational modes are supported:
//
//   - API mode   — requires a valid API key; uses the v1 search endpoint.
//   - Scrape mode — no key needed; parses the public HTML search results.
//
// Scrape mode respects Wallhaven's servers by inserting a 1–2 s delay between
// page fetches and backing off exponentially on HTTP 429 responses.
package wallhaven

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"net/url"
	"time"

	"github.com/PuerkitoBio/goquery"
	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers"
)

const (
	defaultAPIURL  = "https://wallhaven.cc/api/v1/search"
	defaultHTMLURL = "https://wallhaven.cc/search"
	maxScrapePages = 3
	maxRetries     = 3
	baseBackoff    = time.Second
	scrapeMinDelay = 500 * time.Millisecond
)

// ── API response types ────────────────────────────────────────────────────────

type apiResponse struct {
	Data []apiWallpaper `json:"data"`
}

type apiWallpaper struct {
	ID       string `json:"id"`
	URL      string `json:"url"`  // page URL
	Path     string `json:"path"` // direct image URL
	Uploader struct {
		Username string `json:"username"`
	} `json:"uploader"`
}

// ── Provider ──────────────────────────────────────────────────────────────────

// Provider is the Wallhaven wallpaper provider.
type Provider struct {
	enabled     bool
	apiKey      string
	purity      string
	sorting     string
	categories  string
	apiURL      string
	htmlURL     string
	scrapeDelay time.Duration
	client      *http.Client
}

// New creates a Wallhaven provider from the given config section.
func New(cfg *config.WallhavenConfig) *Provider {
	purity := cfg.Purity
	if purity == "" {
		purity = "100"
	}
	sorting := cfg.Sorting
	if sorting == "" {
		sorting = "random"
	}
	categories := cfg.Categories
	if categories == "" {
		categories = "111"
	}
	return &Provider{
		enabled:     cfg.Enabled,
		apiKey:      cfg.APIKey,
		purity:      purity,
		sorting:     sorting,
		categories:  categories,
		apiURL:      defaultAPIURL,
		htmlURL:     defaultHTMLURL,
		scrapeDelay: 1000 * time.Millisecond,
		client: &http.Client{
			Timeout: 30 * time.Second,
		},
	}
}

// NewWithSearchURL creates a Provider with custom API and HTML search URLs.
// Intended for testing — overrides the default Wallhaven endpoints.
func NewWithSearchURL(cfg *config.WallhavenConfig, apiSearchURL, htmlSearchURL string) *Provider {
	p := New(cfg)
	p.apiURL = apiSearchURL
	p.htmlURL = htmlSearchURL
	// Reduce scrape delay to zero in tests so they run quickly.
	p.scrapeDelay = 0
	return p
}

func (p *Provider) Name() string  { return "wallhaven" }
func (p *Provider) Enabled() bool { return p.enabled }

// Fetch returns wallpaper results from Wallhaven.
// If an API key is configured, the v1 API is used; otherwise HTML scraping is used.
func (p *Provider) Fetch(ctx context.Context, query string) ([]providers.WallpaperResult, error) {
	if p.apiKey != "" {
		return p.fetchAPI(ctx, query)
	}
	return p.fetchScrape(ctx, query)
}

// ── API mode ──────────────────────────────────────────────────────────────────

func (p *Provider) fetchAPI(ctx context.Context, query string) ([]providers.WallpaperResult, error) {
	params := url.Values{}
	params.Set("sorting", p.sorting)
	params.Set("purity", p.purity)
	params.Set("categories", p.categories)
	params.Set("apikey", p.apiKey)
	if query != "" {
		params.Set("q", query)
	}

	reqURL := p.apiURL + "?" + params.Encode()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, reqURL, nil)
	if err != nil {
		return nil, fmt.Errorf("wallhaven api: build request: %w", err)
	}
	req.Header.Set("User-Agent", providers.UserAgent)

	resp, err := p.doWithRetry(req)
	if err != nil {
		return nil, fmt.Errorf("wallhaven api: %w", err)
	}
	defer resp.Body.Close()

	var ar apiResponse
	if err := json.NewDecoder(resp.Body).Decode(&ar); err != nil {
		return nil, fmt.Errorf("wallhaven api: decode: %w", err)
	}

	results := make([]providers.WallpaperResult, 0, len(ar.Data))
	for _, w := range ar.Data {
		if w.Path == "" {
			continue
		}
		results = append(results, providers.WallpaperResult{
			SourceURL: w.URL,
			ImageURL:  w.Path,
			Author:    w.Uploader.Username,
			Source:    p.Name(),
		})
	}
	return results, nil
}

// ── HTML scrape mode ──────────────────────────────────────────────────────────

func (p *Provider) fetchScrape(ctx context.Context, query string) ([]providers.WallpaperResult, error) {
	params := url.Values{}
	params.Set("sorting", p.sorting)
	params.Set("purity", p.purity)
	params.Set("categories", p.categories)
	if query != "" {
		params.Set("q", query)
	}

	searchURL := p.htmlURL + "?" + params.Encode()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, searchURL, nil)
	if err != nil {
		return nil, fmt.Errorf("wallhaven scrape: build request: %w", err)
	}
	req.Header.Set("User-Agent", providers.UserAgent)

	resp, err := p.doWithRetry(req)
	if err != nil {
		return nil, fmt.Errorf("wallhaven scrape: search page: %w", err)
	}
	defer resp.Body.Close()

	doc, err := goquery.NewDocumentFromReader(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("wallhaven scrape: parse HTML: %w", err)
	}

	// Collect preview page links.
	var pageURLs []string
	doc.Find("figure.thumb a.preview").Each(func(_ int, s *goquery.Selection) {
		if href, ok := s.Attr("href"); ok && href != "" {
			pageURLs = append(pageURLs, href)
		}
	})

	if len(pageURLs) == 0 {
		return []providers.WallpaperResult{}, nil
	}

	// Shuffle and cap.
	rand.Shuffle(len(pageURLs), func(i, j int) { pageURLs[i], pageURLs[j] = pageURLs[j], pageURLs[i] })
	if len(pageURLs) > maxScrapePages {
		pageURLs = pageURLs[:maxScrapePages]
	}

	results := make([]providers.WallpaperResult, 0, len(pageURLs))
	for _, pageURL := range pageURLs {
		if p.scrapeDelay > 0 {
			select {
			case <-ctx.Done():
				return results, ctx.Err()
			case <-time.After(p.scrapeDelay + time.Duration(rand.Intn(int(p.scrapeDelay)))):
			}
		}

		imageURL, err := p.scrapeWallpaperPage(ctx, pageURL)
		if err != nil {
			continue // non-fatal
		}
		results = append(results, providers.WallpaperResult{
			SourceURL: pageURL,
			ImageURL:  imageURL,
			Source:    p.Name(),
		})
	}
	return results, nil
}

// scrapeWallpaperPage fetches a Wallhaven wallpaper page and extracts the
// direct image URL from img#wallpaper[src].
func (p *Provider) scrapeWallpaperPage(ctx context.Context, pageURL string) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, pageURL, nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("User-Agent", providers.UserAgent)

	resp, err := p.doWithRetry(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	doc, err := goquery.NewDocumentFromReader(resp.Body)
	if err != nil {
		return "", fmt.Errorf("parse %s: %w", pageURL, err)
	}

	src, ok := doc.Find("img#wallpaper").Attr("src")
	if !ok || src == "" {
		return "", fmt.Errorf("img#wallpaper not found on %s", pageURL)
	}
	return src, nil
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

// doWithRetry executes req, retrying on HTTP 429 with exponential back-off.
func (p *Provider) doWithRetry(req *http.Request) (*http.Response, error) {
	backoff := baseBackoff
	for attempt := 0; attempt <= maxRetries; attempt++ {
		clone := req.Clone(req.Context())

		resp, err := p.client.Do(clone)
		if err != nil {
			if attempt == maxRetries {
				return nil, fmt.Errorf("request failed after %d attempts: %w", maxRetries+1, err)
			}
			select {
			case <-req.Context().Done():
				return nil, req.Context().Err()
			case <-time.After(backoff):
				backoff *= 2
			}
			continue
		}

		if resp.StatusCode == http.StatusTooManyRequests {
			io.Copy(io.Discard, resp.Body) //nolint:errcheck
			resp.Body.Close()
			if attempt == maxRetries {
				return nil, fmt.Errorf("rate limited after %d attempts", maxRetries+1)
			}
			select {
			case <-req.Context().Done():
				return nil, req.Context().Err()
			case <-time.After(backoff):
				backoff *= 2
			}
			continue
		}

		if resp.StatusCode != http.StatusOK {
			resp.Body.Close()
			return nil, fmt.Errorf("unexpected status %d for %s", resp.StatusCode, req.URL)
		}

		return resp, nil
	}
	return nil, fmt.Errorf("wallhaven: max retries exceeded")
}
