package wallhaven_test

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers/wallhaven"
)

// ── API mode tests ────────────────────────────────────────────────────────────

func TestWallhavenAPI_ParsesResponse(t *testing.T) {
	type mockWallpaper struct {
		ID       string `json:"id"`
		URL      string `json:"url"`
		Path     string `json:"path"`
		Uploader struct {
			Username string `json:"username"`
		} `json:"uploader"`
	}
	payload := struct {
		Data []mockWallpaper `json:"data"`
	}{
		Data: []mockWallpaper{
			{
				ID:   "abc123",
				URL:  "https://wallhaven.cc/w/abc123",
				Path: "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg",
				Uploader: struct {
					Username string `json:"username"`
				}{Username: "artist1"},
			},
			{
				ID:   "def456",
				URL:  "https://wallhaven.cc/w/def456",
				Path: "https://w.wallhaven.cc/full/de/wallhaven-def456.png",
				Uploader: struct {
					Username string `json:"username"`
				}{Username: "artist2"},
			},
		},
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(payload)
	}))
	defer srv.Close()

	cfg := config.WallhavenConfig{
		Enabled: true,
		APIKey:  "testkey",
		Purity:  "100",
		Sorting: "random",
	}
	p := wallhaven.NewWithSearchURL(&cfg, srv.URL+"/api/v1/search", srv.URL+"/search")
	results, err := p.Fetch(context.Background(), "nature")
	if err != nil {
		t.Fatalf("Fetch returned error: %v", err)
	}
	if len(results) != 2 {
		t.Fatalf("expected 2 results, got %d", len(results))
	}
	if results[0].ImageURL != "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg" {
		t.Errorf("result[0] ImageURL: got %q", results[0].ImageURL)
	}
	if results[0].Author != "artist1" {
		t.Errorf("result[0] Author: expected 'artist1', got %q", results[0].Author)
	}
	if results[0].Source != "wallhaven" {
		t.Errorf("result[0] Source: expected 'wallhaven', got %q", results[0].Source)
	}
}

func TestWallhavenAPI_EmptyData(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"data":[]}`))
	}))
	defer srv.Close()

	cfg := config.WallhavenConfig{
		Enabled: true,
		APIKey:  "testkey",
		Purity:  "100",
		Sorting: "random",
	}
	p := wallhaven.NewWithSearchURL(&cfg, srv.URL+"/api/v1/search", srv.URL+"/search")
	results, err := p.Fetch(context.Background(), "")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("expected 0 results, got %d", len(results))
	}
}

func TestWallhavenAPI_HTTPError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "server error", http.StatusInternalServerError)
	}))
	defer srv.Close()

	cfg := config.WallhavenConfig{
		Enabled: true,
		APIKey:  "testkey",
		Purity:  "100",
		Sorting: "random",
	}
	p := wallhaven.NewWithSearchURL(&cfg, srv.URL+"/api/v1/search", srv.URL+"/search")
	_, err := p.Fetch(context.Background(), "")
	if err == nil {
		t.Error("expected error for HTTP 500, got nil")
	}
}

// ── HTML scrape mode tests ────────────────────────────────────────────────────

func TestWallhavenScrape_FindsImages(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/search":
			w.Header().Set("Content-Type", "text/html")
			fmt.Fprintf(w, `<!DOCTYPE html><html><body>
<ul id="wallpapers">
  <li><figure class="thumb"><a class="preview" href="http://%s/w/abc123"><img src="t.jpg"/></a></figure></li>
  <li><figure class="thumb"><a class="preview" href="http://%s/w/def456"><img src="t2.jpg"/></a></figure></li>
</ul>
</body></html>`, r.Host, r.Host)
		default:
			// Wallpaper detail page.
			w.Header().Set("Content-Type", "text/html")
			w.Write([]byte(`<!DOCTYPE html><html><body>
<img id="wallpaper" src="https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg"/>
</body></html>`))
		}
	}))
	defer srv.Close()

	cfg := config.WallhavenConfig{
		Enabled: true,
		APIKey:  "", // no key → scrape mode
		Purity:  "100",
		Sorting: "random",
	}
	p := wallhaven.NewWithSearchURL(&cfg, srv.URL+"/api/v1/search", srv.URL+"/search")
	results, err := p.Fetch(context.Background(), "nature")
	if err != nil {
		t.Fatalf("Fetch returned error: %v", err)
	}
	if len(results) == 0 {
		t.Fatal("expected at least one result from scrape mode")
	}
	for i, r := range results {
		if r.Source != "wallhaven" {
			t.Errorf("result[%d]: expected source 'wallhaven', got %q", i, r.Source)
		}
		if r.ImageURL == "" {
			t.Errorf("result[%d]: empty ImageURL", i)
		}
	}
}

func TestWallhavenScrape_NoThumbs(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html")
		w.Write([]byte(`<html><body><p>No results.</p></body></html>`))
	}))
	defer srv.Close()

	cfg := config.WallhavenConfig{
		Enabled: true,
		APIKey:  "",
		Purity:  "100",
		Sorting: "random",
	}
	p := wallhaven.NewWithSearchURL(&cfg, srv.URL+"/api/v1/search", srv.URL+"/search")
	results, err := p.Fetch(context.Background(), "")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("expected 0 results, got %d", len(results))
	}
}

// ── Meta tests ────────────────────────────────────────────────────────────────

func TestWallhavenName(t *testing.T) {
	p := wallhaven.New(&config.WallhavenConfig{Enabled: true})
	if p.Name() != "wallhaven" {
		t.Errorf("expected 'wallhaven', got %q", p.Name())
	}
}

func TestWallhavenEnabled(t *testing.T) {
	enabled := wallhaven.New(&config.WallhavenConfig{Enabled: true})
	disabled := wallhaven.New(&config.WallhavenConfig{Enabled: false})

	if !enabled.Enabled() {
		t.Error("expected enabled provider to report Enabled() == true")
	}
	if disabled.Enabled() {
		t.Error("expected disabled provider to report Enabled() == false")
	}
}

func TestWallhavenAPI_QueryParam(t *testing.T) {
	var capturedQuery string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		capturedQuery = r.URL.Query().Get("q")
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"data":[]}`))
	}))
	defer srv.Close()

	cfg := &config.WallhavenConfig{
		Enabled: true,
		APIKey:  "testkey",
		Purity:  "100",
		Sorting: "random",
	}
	p := wallhaven.NewWithSearchURL(cfg, srv.URL+"/api/v1/search", srv.URL+"/search")
	_, _ = p.Fetch(context.Background(), "mountains")

	if capturedQuery != "mountains" {
		t.Errorf("expected query 'mountains' in request, got %q", capturedQuery)
	}
}
