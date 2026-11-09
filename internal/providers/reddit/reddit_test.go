package reddit_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers/reddit"
)

// buildListing creates a minimal Reddit JSON listing payload.
func buildListing(posts []map[string]interface{}) []byte {
	children := make([]interface{}, len(posts))
	for i, p := range posts {
		children[i] = map[string]interface{}{"data": p}
	}
	listing := map[string]interface{}{
		"data": map[string]interface{}{"children": children},
	}
	b, _ := json.Marshal(listing)
	return b
}

// newProvider creates a Provider backed by the given httptest.Server.
// It uses NewWithFeedURL to avoid depending on the potentially-changing New() signature.
func newProvider(t *testing.T, cfg config.RedditConfig, srv *httptest.Server) *reddit.Provider {
	t.Helper()
	return reddit.NewWithFeedURL(&cfg, srv.URL+"/r/%s/hot.json")
}

// newProviderNoSrv creates a Provider with a dummy URL (for non-network tests).
func newProviderNoSrv(cfg config.RedditConfig) *reddit.Provider {
	return reddit.NewWithFeedURL(&cfg, "http://localhost:1/r/%s/hot.json")
}

func TestReddit_FiltersNonImages(t *testing.T) {
	posts := []map[string]interface{}{
		// Included: direct jpg, high score.
		{"title": "Mountain", "url": "https://example.com/mountain.jpg", "author": "user1",
			"permalink": "/r/EarthPorn/comments/abc/mountain/", "score": 500, "id": "abc"},
		// Excluded: not an image URL.
		{"title": "Article", "url": "https://example.com/article", "author": "user2",
			"permalink": "/r/EarthPorn/comments/def/article/", "score": 500, "id": "def"},
		// Excluded: low score.
		{"title": "Low score", "url": "https://example.com/lowscore.jpg", "author": "user3",
			"permalink": "/r/EarthPorn/comments/ghi/low/", "score": 5, "id": "ghi"},
		// Included: direct png, high score.
		{"title": "Ocean", "url": "https://example.com/ocean.png", "author": "user4",
			"permalink": "/r/EarthPorn/comments/jkl/ocean/", "score": 200, "id": "jkl"},
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write(buildListing(posts))
	}))
	defer srv.Close()

	cfg := config.RedditConfig{Enabled: true, Subreddits: []string{"EarthPorn"}}
	p := newProvider(t, cfg, srv)

	results, err := p.Fetch(context.Background(), "")
	if err != nil {
		t.Fatalf("Fetch returned error: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("expected 2 image results, got %d", len(results))
		for i, r := range results {
			t.Logf("result[%d]: %+v", i, r)
		}
	}
}

func TestReddit_ImgurConversion(t *testing.T) {
	posts := []map[string]interface{}{
		// imgur.com link should be converted to i.imgur.com/ID.jpg.
		{"title": "Imgur", "url": "https://imgur.com/abc123", "author": "user1",
			"permalink": "/r/w/comments/x/imgur/", "score": 500, "id": "x"},
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write(buildListing(posts))
	}))
	defer srv.Close()

	cfg := config.RedditConfig{Enabled: true, Subreddits: []string{"wallpapers"}}
	p := newProvider(t, cfg, srv)
	results, err := p.Fetch(context.Background(), "")
	if err != nil {
		t.Fatalf("Fetch returned error: %v", err)
	}
	if len(results) == 0 {
		t.Fatal("expected imgur link to be converted, got no results")
	}
	imgURL := results[0].ImageURL
	if !strings.HasPrefix(imgURL, "https://i.imgur.com/") {
		t.Errorf("expected i.imgur.com URL, got %q", imgURL)
	}
}

func TestReddit_Source(t *testing.T) {
	posts := []map[string]interface{}{
		{"title": "Test", "url": "https://example.com/test.jpg", "author": "user1",
			"permalink": "/r/EarthPorn/comments/abc/test/", "score": 500, "id": "abc"},
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write(buildListing(posts))
	}))
	defer srv.Close()

	cfg := config.RedditConfig{Enabled: true, Subreddits: []string{"EarthPorn"}}
	p := newProvider(t, cfg, srv)
	results, _ := p.Fetch(context.Background(), "")
	if len(results) == 0 {
		t.Fatal("expected 1 result")
	}
	if results[0].Source != "reddit" {
		t.Errorf("expected source 'reddit', got %q", results[0].Source)
	}
}

func TestReddit_HTTPError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "forbidden", http.StatusForbidden)
	}))
	defer srv.Close()

	cfg := config.RedditConfig{Enabled: true, Subreddits: []string{"EarthPorn"}}
	p := newProvider(t, cfg, srv)
	_, err := p.Fetch(context.Background(), "")
	if err == nil {
		t.Error("expected error for HTTP 403, got nil")
	}
}

func TestReddit_EmptyResults(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write(buildListing(nil))
	}))
	defer srv.Close()

	cfg := config.RedditConfig{Enabled: true, Subreddits: []string{"EarthPorn"}}
	p := newProvider(t, cfg, srv)
	results, err := p.Fetch(context.Background(), "")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("expected 0 results, got %d", len(results))
	}
}

func TestRedditName(t *testing.T) {
	cfg := config.RedditConfig{Enabled: true, Subreddits: []string{"test"}}
	p := newProviderNoSrv(cfg)
	if p.Name() != "reddit" {
		t.Errorf("expected 'reddit', got %q", p.Name())
	}
}

func TestRedditEnabled(t *testing.T) {
	enabled := newProviderNoSrv(config.RedditConfig{Enabled: true, Subreddits: []string{"test"}})
	disabledFlag := newProviderNoSrv(config.RedditConfig{Enabled: false, Subreddits: []string{"test"}})
	disabledNoSubs := newProviderNoSrv(config.RedditConfig{Enabled: true, Subreddits: []string{}})

	if !enabled.Enabled() {
		t.Error("expected enabled provider, got disabled")
	}
	if disabledFlag.Enabled() {
		t.Error("expected provider with Enabled=false to be disabled")
	}
	if disabledNoSubs.Enabled() {
		t.Error("expected provider with no subreddits to be disabled")
	}
}

func TestReddit_PermalinkURL(t *testing.T) {
	posts := []map[string]interface{}{
		{"title": "Test", "url": "https://example.com/test.jpg", "author": "user1",
			"permalink": "/r/EarthPorn/comments/abc/test/", "score": 500, "id": "abc"},
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write(buildListing(posts))
	}))
	defer srv.Close()

	cfg := config.RedditConfig{Enabled: true, Subreddits: []string{"EarthPorn"}}
	p := newProvider(t, cfg, srv)
	results, _ := p.Fetch(context.Background(), "")
	if len(results) == 0 {
		t.Fatal("expected 1 result")
	}
	if !strings.HasPrefix(results[0].SourceURL, "https://www.reddit.com") {
		t.Errorf("expected reddit.com SourceURL, got %q", results[0].SourceURL)
	}
}

func TestReddit_WebpIncluded(t *testing.T) {
	posts := []map[string]interface{}{
		// .webp should be included.
		{"title": "webp image", "url": "https://example.com/image.webp", "author": "user1",
			"permalink": "/r/w/comments/abc/img/", "score": 500, "id": "abc"},
		// .gif should NOT be included (not in the allowed extensions).
		{"title": "gif anim", "url": "https://example.com/anim.gif", "author": "user2",
			"permalink": "/r/w/comments/def/gif/", "score": 500, "id": "def"},
	}

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write(buildListing(posts))
	}))
	defer srv.Close()

	cfg := config.RedditConfig{Enabled: true, Subreddits: []string{"wallpapers"}}
	p := newProvider(t, cfg, srv)
	results, _ := p.Fetch(context.Background(), "")
	if len(results) != 1 {
		t.Errorf("expected 1 result (webp only), got %d", len(results))
	}
}
