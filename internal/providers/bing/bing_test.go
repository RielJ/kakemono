package bing_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers/bing"
)

// TestBingFetch_ParsesJSON verifies that Fetch correctly parses the Bing API JSON response.
func TestBingFetch_ParsesJSON(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := map[string]interface{}{
			"images": []map[string]interface{}{
				{
					"url":       "/th?id=OHR.SomePic_1920x1080.jpg&rf=LaDigue.jpg",
					"urlbase":   "/th?id=OHR.SomePic",
					"title":     "Beautiful Landscape",
					"copyright": "© Photographer",
				},
				{
					"url":       "/th?id=OHR.AnotherPic_1920x1080.jpg",
					"urlbase":   "/th?id=OHR.AnotherPic",
					"title":     "City Lights",
					"copyright": "© Another",
				},
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	}))
	defer srv.Close()

	cfg := config.BingConfig{Enabled: true, Market: "en-US"}
	p := bing.NewWithBaseURL(cfg, srv.URL)

	results, err := p.Fetch(context.Background(), "")
	if err != nil {
		t.Fatalf("Fetch() failed: %v", err)
	}

	if len(results) != 2 {
		t.Errorf("expected 2 results, got %d", len(results))
	}
	if results[0].Title != "Beautiful Landscape" {
		t.Errorf("unexpected title: %q", results[0].Title)
	}
	if results[0].Source != "bing" {
		t.Errorf("unexpected source: %q", results[0].Source)
	}
	// ImageURL should reference the test server.
	if !strings.HasPrefix(results[0].ImageURL, srv.URL) {
		t.Errorf("expected ImageURL to start with server URL %q, got %q", srv.URL, results[0].ImageURL)
	}
}

// TestBingFetch_HTTPError verifies that HTTP errors are propagated.
func TestBingFetch_HTTPError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "server error", http.StatusInternalServerError)
	}))
	defer srv.Close()

	cfg := config.BingConfig{Enabled: true, Market: "en-US"}
	p := bing.NewWithBaseURL(cfg, srv.URL)

	_, err := p.Fetch(context.Background(), "")
	if err == nil {
		t.Error("expected error for HTTP 500, got nil")
	}
}

// TestBingFetch_EmptyImages verifies that the provider reports an error when no images
// are returned (indicates an API issue).
func TestBingFetch_EmptyImages(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{"images": []interface{}{}})
	}))
	defer srv.Close()

	cfg := config.BingConfig{Enabled: true, Market: "en-US"}
	p := bing.NewWithBaseURL(cfg, srv.URL)

	// Empty images → provider returns an error (no wallpapers available).
	_, err := p.Fetch(context.Background(), "")
	if err == nil {
		t.Error("expected error when API returns no images")
	}
}

// TestBingProvider_Name verifies basic metadata.
func TestBingProvider_Name(t *testing.T) {
	p := bing.New(config.BingConfig{Enabled: true, Market: "en-US"})
	if p.Name() != "bing" {
		t.Errorf("expected 'bing', got %q", p.Name())
	}
}

// TestBingProvider_Enabled verifies the enabled flag.
func TestBingProvider_Enabled(t *testing.T) {
	enabled := bing.New(config.BingConfig{Enabled: true})
	disabled := bing.New(config.BingConfig{Enabled: false})

	if !enabled.Enabled() {
		t.Error("expected enabled provider to report Enabled() == true")
	}
	if disabled.Enabled() {
		t.Error("expected disabled provider to report Enabled() == false")
	}
}

// TestBingFetch_UHDResolution verifies that image URLs are upgraded to UHD quality.
func TestBingFetch_UHDResolution(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := map[string]interface{}{
			"images": []map[string]interface{}{
				{
					"url":     "/th?id=OHR.Test_1920x1080.jpg",
					"urlbase": "/th?id=OHR.Test",
					"title":   "Test",
				},
			},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	}))
	defer srv.Close()

	cfg := config.BingConfig{Enabled: true, Market: "en-US"}
	p := bing.NewWithBaseURL(cfg, srv.URL)

	results, err := p.Fetch(context.Background(), "")
	if err != nil {
		t.Fatalf("Fetch() failed: %v", err)
	}
	if len(results) == 0 {
		t.Fatal("expected results")
	}
	// Should have UHD in URL.
	if !strings.Contains(results[0].ImageURL, "UHD") {
		t.Errorf("expected UHD in image URL, got %q", results[0].ImageURL)
	}
}
