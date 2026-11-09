package main

// Provider fixtures for crates/sources/tests/fixtures/<provider>/.
//
// Response bodies for bing, reddit and wallhaven are the exact bytes the
// httptest handlers in internal/providers/*/…_test.go send (same values, same
// encoder). apod and unsplash have no Go tests, so their bodies are
// synthesised from the structs in apod.go / unsplash.go.
//
// Every case is then run through the real Go provider. All HTTP dials made by
// the process are redirected to local httptest servers (http.DefaultTransport
// is patched before any request), so no request leaves the machine, and the
// recorded "expected" block is Go's actual output for that input.

import (
	"bytes"
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"log"
	"net"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/BurntSushi/toml"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers"
	"github.com/rielj/wallpaperd/internal/providers/apod"
	"github.com/rielj/wallpaperd/internal/providers/bing"
	"github.com/rielj/wallpaperd/internal/providers/reddit"
	"github.com/rielj/wallpaperd/internal/providers/unsplash"
	"github.com/rielj/wallpaperd/internal/providers/wallhaven"
)

const sourcesDir = "crates/sources/tests/fixtures"

// hostPlaceholder in a body file is replaced with the request's Host header
// when served (wallhaven_test.go formats r.Host into the search page).
const hostPlaceholder = "{{HOST}}"

type route struct {
	Method      string            `json:"method"`
	Host        string            `json:"host"`
	Path        string            `json:"path"`
	Status      int               `json:"status"`
	ContentType string            `json:"content_type,omitempty"`
	Headers     map[string]string `json:"headers,omitempty"`
	BodyFile    string            `json:"body_file,omitempty"`
}

type recordedRequest struct {
	Method    string `json:"method"`
	URL       string `json:"url"`
	UserAgent string `json:"user_agent"`
}

type result struct {
	SourceURL string `json:"source_url"`
	ImageURL  string `json:"image_url"`
	Title     string `json:"title"`
	Author    string `json:"author"`
	Source    string `json:"source"`
}

// expected holds exactly one of Results (possibly empty, rendered as []) or
// Error.
type expected struct {
	Results *[]result `json:"results,omitempty"`
	Error   string    `json:"error,omitempty"`
}

type caseFile struct {
	GoTest      string            `json:"go_test"`
	Provider    string            `json:"provider"`
	Config      map[string]any    `json:"config"`
	Query       string            `json:"query"`
	Routes      []route           `json:"routes"`
	Requests    []recordedRequest `json:"requests"`
	SortResults bool              `json:"sort_results,omitempty"`
	Expected    expected          `json:"expected"`
}

type providerCase struct {
	provider string
	name     string
	goTest   string
	config   any
	query    string
	routes   []route
	// sortResults: Go shuffles the order (wallhaven scrape), so results are
	// sorted by source_url before being recorded.
	sortResults bool
	fetch       func(ctx context.Context) ([]providers.WallpaperResult, error)
}

// ── fixture server ───────────────────────────────────────────────────────────

type fixtureServer struct {
	mu        sync.Mutex
	routes    []route
	bodies    map[string][]byte // "<provider>/<file>" → bytes
	provider  string
	requests  []recordedRequest
	unmatched []string
}

func (s *fixtureServer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.mu.Lock()
	defer s.mu.Unlock()

	scheme := "http"
	if r.TLS != nil {
		scheme = "https"
	}
	s.requests = append(s.requests, recordedRequest{
		Method:    r.Method,
		URL:       scheme + "://" + r.Host + r.URL.RequestURI(),
		UserAgent: r.UserAgent(),
	})

	for _, rt := range s.routes {
		if rt.Method != r.Method || rt.Host != r.Host || rt.Path != r.URL.Path {
			continue
		}
		for k, v := range rt.Headers {
			w.Header().Set(k, v)
		}
		if rt.ContentType != "" {
			w.Header().Set("Content-Type", rt.ContentType)
		}
		w.WriteHeader(rt.Status)
		if rt.BodyFile != "" {
			body := s.bodies[s.provider+"/"+rt.BodyFile]
			body = bytes.ReplaceAll(body, []byte(hostPlaceholder), []byte(r.Host))
			_, _ = w.Write(body)
		}
		return
	}
	s.unmatched = append(s.unmatched, r.Method+" "+r.Host+r.URL.Path)
	http.Error(w, "no fixture route", http.StatusNotFound)
}

// redirectAllDials points every connection made through http.DefaultTransport
// (used directly by the provider clients, and wrapped by
// providers.DefaultHTTPClient) at the local fixture servers: port 443 goes to
// the TLS server, everything else to the plain one.
func redirectAllDials(plainAddr, tlsAddr string) {
	tr, ok := http.DefaultTransport.(*http.Transport)
	if !ok {
		log.Fatal("http.DefaultTransport is not *http.Transport")
	}
	tr.Proxy = nil
	tr.TLSClientConfig = &tls.Config{InsecureSkipVerify: true} //nolint:gosec // local fixture server only
	tr.DialContext = func(ctx context.Context, network, addr string) (net.Conn, error) {
		_, port, err := net.SplitHostPort(addr)
		if err != nil {
			return nil, err
		}
		target := plainAddr
		if port == "443" {
			target = tlsAddr
		}
		var d net.Dialer
		return d.DialContext(ctx, "tcp", target)
	}
}

// ── body helpers ─────────────────────────────────────────────────────────────

// encoderBody is json.NewEncoder(w).Encode(v): compact JSON plus "\n".
func encoderBody(v any) []byte { return wireLine(v) }

// buildListing is copied verbatim from reddit_test.go.
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

// httpErrorBody is what http.Error writes.
func httpErrorBody(msg string) []byte { return []byte(msg + "\n") }

func tomlMap(v any) map[string]any {
	var buf bytes.Buffer
	if err := toml.NewEncoder(&buf).Encode(v); err != nil {
		log.Fatal(err)
	}
	m := map[string]any{}
	if _, err := toml.Decode(buf.String(), &m); err != nil {
		log.Fatal(err)
	}
	return m
}

const (
	ctJSON = "application/json"
	ctHTML = "text/html"
	ctText = "text/plain; charset=utf-8"
)

// ── bodies ───────────────────────────────────────────────────────────────────

func providerBodies() map[string][]byte {
	b := map[string][]byte{}

	// bing_test.go
	b["bing/parses_json.json"] = encoderBody(map[string]interface{}{
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
	})
	b["bing/empty_images.json"] = encoderBody(map[string]interface{}{"images": []interface{}{}})
	b["bing/uhd_resolution.json"] = encoderBody(map[string]interface{}{
		"images": []map[string]interface{}{
			{
				"url":     "/th?id=OHR.Test_1920x1080.jpg",
				"urlbase": "/th?id=OHR.Test",
				"title":   "Test",
			},
		},
	})
	b["bing/server_error.txt"] = httpErrorBody("server error")

	// reddit_test.go
	b["reddit/filters_non_images.json"] = buildListing([]map[string]interface{}{
		{"title": "Mountain", "url": "https://example.com/mountain.jpg", "author": "user1",
			"permalink": "/r/EarthPorn/comments/abc/mountain/", "score": 500, "id": "abc"},
		{"title": "Article", "url": "https://example.com/article", "author": "user2",
			"permalink": "/r/EarthPorn/comments/def/article/", "score": 500, "id": "def"},
		{"title": "Low score", "url": "https://example.com/lowscore.jpg", "author": "user3",
			"permalink": "/r/EarthPorn/comments/ghi/low/", "score": 5, "id": "ghi"},
		{"title": "Ocean", "url": "https://example.com/ocean.png", "author": "user4",
			"permalink": "/r/EarthPorn/comments/jkl/ocean/", "score": 200, "id": "jkl"},
	})
	b["reddit/imgur_conversion.json"] = buildListing([]map[string]interface{}{
		{"title": "Imgur", "url": "https://imgur.com/abc123", "author": "user1",
			"permalink": "/r/w/comments/x/imgur/", "score": 500, "id": "x"},
	})
	// Used by TestReddit_Source and TestReddit_PermalinkURL (identical posts).
	b["reddit/single_post.json"] = buildListing([]map[string]interface{}{
		{"title": "Test", "url": "https://example.com/test.jpg", "author": "user1",
			"permalink": "/r/EarthPorn/comments/abc/test/", "score": 500, "id": "abc"},
	})
	b["reddit/empty.json"] = buildListing(nil)
	b["reddit/webp_included.json"] = buildListing([]map[string]interface{}{
		{"title": "webp image", "url": "https://example.com/image.webp", "author": "user1",
			"permalink": "/r/w/comments/abc/img/", "score": 500, "id": "abc"},
		{"title": "gif anim", "url": "https://example.com/anim.gif", "author": "user2",
			"permalink": "/r/w/comments/def/gif/", "score": 500, "id": "def"},
	})
	b["reddit/forbidden.txt"] = httpErrorBody("forbidden")

	// wallhaven_test.go
	type mockWallpaper struct {
		ID       string `json:"id"`
		URL      string `json:"url"`
		Path     string `json:"path"`
		Uploader struct {
			Username string `json:"username"`
		} `json:"uploader"`
	}
	mw := func(id, path, user string) mockWallpaper {
		m := mockWallpaper{ID: id, URL: "https://wallhaven.cc/w/" + id, Path: path}
		m.Uploader.Username = user
		return m
	}
	b["wallhaven/api_search.json"] = encoderBody(struct {
		Data []mockWallpaper `json:"data"`
	}{Data: []mockWallpaper{
		mw("abc123", "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg", "artist1"),
		mw("def456", "https://w.wallhaven.cc/full/de/wallhaven-def456.png", "artist2"),
	}})
	b["wallhaven/api_empty.json"] = []byte(`{"data":[]}`)
	b["wallhaven/server_error.txt"] = httpErrorBody("server error")
	b["wallhaven/scrape_search.html"] = []byte(`<!DOCTYPE html><html><body>
<ul id="wallpapers">
  <li><figure class="thumb"><a class="preview" href="http://` + hostPlaceholder + `/w/abc123"><img src="t.jpg"/></a></figure></li>
  <li><figure class="thumb"><a class="preview" href="http://` + hostPlaceholder + `/w/def456"><img src="t2.jpg"/></a></figure></li>
</ul>
</body></html>`)
	b["wallhaven/scrape_wallpaper_page.html"] = []byte(`<!DOCTYPE html><html><body>
<img id="wallpaper" src="https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg"/>
</body></html>`)
	b["wallhaven/scrape_no_thumbs.html"] = []byte(`<html><body><p>No results.</p></body></html>`)

	// apod (synthesised; shape of apodEntry in apod.go, count=5 → JSON array)
	type apodEntry struct {
		Copyright   string `json:"copyright,omitempty"`
		Date        string `json:"date"`
		Explanation string `json:"explanation"`
		HDURL       string `json:"hdurl,omitempty"`
		MediaType   string `json:"media_type"`
		Title       string `json:"title"`
		URL         string `json:"url,omitempty"`
	}
	b["apod/count5.json"] = encoderBody([]apodEntry{
		{Copyright: "Jane Astro", Date: "2024-03-15", Explanation: "A galaxy.", MediaType: "image",
			Title: "Spiral Galaxy", URL: "https://apod.nasa.gov/apod/image/2403/galaxy_1024.jpg",
			HDURL: "https://apod.nasa.gov/apod/image/2403/galaxy_4096.jpg"},
		{Date: "2024-03-16", Explanation: "No HD version, no copyright.", MediaType: "image",
			Title: "Moon Rise", URL: "https://apod.nasa.gov/apod/image/2403/moon.jpg"},
		{Date: "2024-03-17", Explanation: "A video day.", MediaType: "video",
			Title: "Eclipse Video", URL: "https://www.youtube.com/embed/abc"},
		{Date: "2024-03-18", Explanation: "Image entry without any URL.", MediaType: "image",
			Title: "Broken Entry"},
		{Copyright: "Odd Date", Date: "2024-3-19", Explanation: "Date is not 10 chars.", MediaType: "image",
			Title: "Nebula", HDURL: "https://apod.nasa.gov/apod/image/2403/nebula.png"},
	})
	b["apod/video_only.json"] = encoderBody([]apodEntry{
		{Date: "2024-03-17", Explanation: "A video day.", MediaType: "video",
			Title: "Eclipse Video", URL: "https://www.youtube.com/embed/abc"},
	})
	b["apod/rate_limited.json"] = []byte(`{"error":{"code":"OVER_RATE_LIMIT","message":"You have exceeded your rate limit."}}`)

	// unsplash (synthesised; shape of apiPhoto in unsplash.go, count=10 → array)
	type unsplashPhoto struct {
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
	ph := func(id, desc, alt, raw, full, name string) unsplashPhoto {
		p := unsplashPhoto{ID: id, Description: desc, AltDesc: alt}
		p.Links.HTML = "https://unsplash.com/photos/" + id
		p.Links.Download = "https://unsplash.com/photos/" + id + "/download"
		p.URLs.Raw = raw
		p.URLs.Full = full
		p.User.Name = name
		p.User.Username = strings.ToLower(strings.ReplaceAll(name, " ", ""))
		return p
	}
	b["unsplash/api_random.json"] = encoderBody([]unsplashPhoto{
		ph("aaa111", "Snowy peaks", "mountain range", "https://images.unsplash.com/photo-aaa111?ixid=x1",
			"https://images.unsplash.com/photo-aaa111?ixid=x1&fm=jpg&q=85", "Ann Peak"),
		ph("bbb222", "", "calm lake at dawn", "https://images.unsplash.com/photo-bbb222?ixid=x2&fm=png",
			"", "Bo Lake"),
		ph("ccc333", "", "", "", "https://images.unsplash.com/photo-ccc333?ixid=x3", "Cy Full"),
		ph("ddd444", "No URLs", "", "", "", "Di Empty"),
	})
	b["unsplash/unauthorized.json"] = []byte(`{"errors":["OAuth error: The access token is invalid"]}`)

	return b
}

// ── cases ────────────────────────────────────────────────────────────────────

func providerCases() []providerCase {
	var cs []providerCase

	// bing
	bingCfg := config.BingConfig{Enabled: true, Market: "en-US"}
	bingRoute := func(status int, ct, file string) []route {
		return []route{{Method: "GET", Host: "www.bing.com", Path: "/HPImageArchive.aspx", Status: status, ContentType: ct, BodyFile: file}}
	}
	bingFetch := func(ctx context.Context) ([]providers.WallpaperResult, error) {
		return bing.New(bingCfg).Fetch(ctx, "")
	}
	cs = append(cs,
		providerCase{provider: "bing", name: "parses_json", goTest: "bing_test.go TestBingFetch_ParsesJSON",
			config: bingCfg, routes: bingRoute(200, ctJSON, "parses_json.json"), fetch: bingFetch},
		providerCase{provider: "bing", name: "http_error", goTest: "bing_test.go TestBingFetch_HTTPError",
			config: bingCfg, routes: bingRoute(500, ctText, "server_error.txt"), fetch: bingFetch},
		providerCase{provider: "bing", name: "empty_images", goTest: "bing_test.go TestBingFetch_EmptyImages",
			config: bingCfg, routes: bingRoute(200, ctJSON, "empty_images.json"), fetch: bingFetch},
		providerCase{provider: "bing", name: "uhd_resolution", goTest: "bing_test.go TestBingFetch_UHDResolution",
			config: bingCfg, routes: bingRoute(200, ctJSON, "uhd_resolution.json"), fetch: bingFetch},
	)

	// reddit (one subreddit per case so the random pick is deterministic)
	redditCase := func(name, goTest, sub string, status int, ct, file string) providerCase {
		cfg := config.RedditConfig{Enabled: true, Subreddits: []string{sub}}
		return providerCase{
			provider: "reddit", name: name, goTest: goTest, config: cfg,
			routes: []route{{Method: "GET", Host: "www.reddit.com", Path: "/r/" + sub + "/hot.json", Status: status, ContentType: ct, BodyFile: file}},
			fetch: func(ctx context.Context) ([]providers.WallpaperResult, error) {
				return reddit.New(&cfg).Fetch(ctx, "")
			},
		}
	}
	cs = append(cs,
		redditCase("filters_non_images", "reddit_test.go TestReddit_FiltersNonImages", "EarthPorn", 200, ctJSON, "filters_non_images.json"),
		redditCase("imgur_conversion", "reddit_test.go TestReddit_ImgurConversion", "wallpapers", 200, ctJSON, "imgur_conversion.json"),
		redditCase("source_and_permalink", "reddit_test.go TestReddit_Source + TestReddit_PermalinkURL", "EarthPorn", 200, ctJSON, "single_post.json"),
		redditCase("http_error", "reddit_test.go TestReddit_HTTPError", "EarthPorn", 403, ctText, "forbidden.txt"),
		redditCase("empty_results", "reddit_test.go TestReddit_EmptyResults", "EarthPorn", 200, ctJSON, "empty.json"),
		redditCase("webp_included", "reddit_test.go TestReddit_WebpIncluded", "wallpapers", 200, ctJSON, "webp_included.json"),
	)

	// wallhaven: NewWithSearchURL with the real endpoints, so scrapeDelay is 0
	// exactly as in the Go tests.
	whAPICfg := config.WallhavenConfig{Enabled: true, APIKey: "testkey", Purity: "100", Sorting: "random"}
	whScrapeCfg := config.WallhavenConfig{Enabled: true, APIKey: "", Purity: "100", Sorting: "random"}
	whCase := func(name, goTest string, cfg config.WallhavenConfig, query string, routes []route, sortResults bool) providerCase {
		return providerCase{
			provider: "wallhaven", name: name, goTest: goTest, config: cfg, query: query, routes: routes, sortResults: sortResults,
			fetch: func(ctx context.Context) ([]providers.WallpaperResult, error) {
				c := cfg
				return wallhaven.NewWithSearchURL(&c, "https://wallhaven.cc/api/v1/search", "https://wallhaven.cc/search").Fetch(ctx, query)
			},
		}
	}
	apiRoute := func(status int, ct, file string) []route {
		return []route{{Method: "GET", Host: "wallhaven.cc", Path: "/api/v1/search", Status: status, ContentType: ct, BodyFile: file}}
	}
	cs = append(cs,
		whCase("api_parses_response", "wallhaven_test.go TestWallhavenAPI_ParsesResponse", whAPICfg, "nature", apiRoute(200, ctJSON, "api_search.json"), false),
		whCase("api_empty_data", "wallhaven_test.go TestWallhavenAPI_EmptyData", whAPICfg, "", apiRoute(200, ctJSON, "api_empty.json"), false),
		whCase("api_http_error", "wallhaven_test.go TestWallhavenAPI_HTTPError", whAPICfg, "", apiRoute(500, ctText, "server_error.txt"), false),
		whCase("api_query_param", "wallhaven_test.go TestWallhavenAPI_QueryParam", whAPICfg, "mountains", apiRoute(200, ctJSON, "api_empty.json"), false),
		whCase("scrape_finds_images", "wallhaven_test.go TestWallhavenScrape_FindsImages", whScrapeCfg, "nature", []route{
			{Method: "GET", Host: "wallhaven.cc", Path: "/search", Status: 200, ContentType: ctHTML, BodyFile: "scrape_search.html"},
			{Method: "GET", Host: "wallhaven.cc", Path: "/w/abc123", Status: 200, ContentType: ctHTML, BodyFile: "scrape_wallpaper_page.html"},
			{Method: "GET", Host: "wallhaven.cc", Path: "/w/def456", Status: 200, ContentType: ctHTML, BodyFile: "scrape_wallpaper_page.html"},
		}, true),
		whCase("scrape_no_thumbs", "wallhaven_test.go TestWallhavenScrape_NoThumbs", whScrapeCfg, "", []route{
			{Method: "GET", Host: "wallhaven.cc", Path: "/search", Status: 200, ContentType: ctHTML, BodyFile: "scrape_no_thumbs.html"},
		}, false),
	)

	// apod (synthesised)
	apodCfg := config.APODConfig{Enabled: true, APIKey: "DEMO_KEY"}
	apodRoute := func(status int, ct, file string) []route {
		return []route{{Method: "GET", Host: "api.nasa.gov", Path: "/planetary/apod", Status: status, ContentType: ct, BodyFile: file}}
	}
	apodFetch := func(ctx context.Context) ([]providers.WallpaperResult, error) {
		c := apodCfg
		return apod.New(&c).Fetch(ctx, "")
	}
	cs = append(cs,
		providerCase{provider: "apod", name: "mixed_entries", goTest: "synthesised (apod.go has no tests)",
			config: apodCfg, routes: apodRoute(200, ctJSON, "count5.json"), fetch: apodFetch},
		providerCase{provider: "apod", name: "video_only", goTest: "synthesised (apod.go has no tests)",
			config: apodCfg, routes: apodRoute(200, ctJSON, "video_only.json"), fetch: apodFetch},
		providerCase{provider: "apod", name: "rate_limited", goTest: "synthesised (apod.go has no tests)",
			config: apodCfg, routes: apodRoute(429, ctJSON, "rate_limited.json"), fetch: apodFetch},
	)

	// unsplash (synthesised)
	unsCfg := config.UnsplashConfig{Enabled: true, AccessKey: "test-access-key"}
	unsNoKeyCfg := config.UnsplashConfig{Enabled: true}
	unsFetch := func(cfg config.UnsplashConfig, query string) func(context.Context) ([]providers.WallpaperResult, error) {
		return func(ctx context.Context) ([]providers.WallpaperResult, error) {
			c := cfg
			return unsplash.New(&c).Fetch(ctx, query)
		}
	}
	cs = append(cs,
		providerCase{provider: "unsplash", name: "api_random", goTest: "synthesised (unsplash.go has no tests)",
			config: unsCfg, query: "misty forest",
			routes: []route{{Method: "GET", Host: "api.unsplash.com", Path: "/photos/random", Status: 200, ContentType: ctJSON, BodyFile: "api_random.json"}},
			fetch:  unsFetch(unsCfg, "misty forest")},
		providerCase{provider: "unsplash", name: "api_unauthorized", goTest: "synthesised (unsplash.go has no tests)",
			config: unsCfg,
			routes: []route{{Method: "GET", Host: "api.unsplash.com", Path: "/photos/random", Status: 401, ContentType: ctJSON, BodyFile: "unauthorized.json"}},
			fetch:  unsFetch(unsCfg, "")},
		providerCase{provider: "unsplash", name: "nokey_redirect", goTest: "synthesised (unsplash.go has no tests)",
			config: unsNoKeyCfg, query: "ocean waves",
			routes: []route{{Method: "HEAD", Host: "source.unsplash.com", Path: "/random/3840x2160/ocean+waves", Status: 302,
				Headers: map[string]string{"Location": "https://images.unsplash.com/photo-zzz999?ixid=nokey&w=3840"}}},
			fetch: unsFetch(unsNoKeyCfg, "ocean waves")},
	)

	return cs
}

// ── driver ───────────────────────────────────────────────────────────────────

func writeProviderFixtures() {
	bodies := providerBodies()
	srv := &fixtureServer{bodies: bodies}
	plain := httptest.NewServer(srv)
	defer plain.Close()
	tlsSrv := httptest.NewTLSServer(srv)
	defer tlsSrv.Close()
	redirectAllDials(plain.Listener.Addr().String(), tlsSrv.Listener.Addr().String())

	names := make([]string, 0, len(bodies))
	for k := range bodies {
		names = append(names, k)
	}
	sort.Strings(names)
	for _, k := range names {
		write(filepath.Join(sourcesDir, filepath.Dir(k)), filepath.Base(k), bodies[k])
	}

	for _, c := range providerCases() {
		for _, rt := range c.routes {
			if rt.BodyFile != "" {
				if _, ok := bodies[c.provider+"/"+rt.BodyFile]; !ok {
					log.Fatalf("%s/%s: missing body %s", c.provider, c.name, rt.BodyFile)
				}
			}
		}

		srv.mu.Lock()
		srv.routes, srv.provider, srv.requests, srv.unmatched = c.routes, c.provider, nil, nil
		srv.mu.Unlock()

		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		got, err := c.fetch(ctx)
		cancel()

		srv.mu.Lock()
		reqs, unmatched := srv.requests, srv.unmatched
		srv.mu.Unlock()
		if len(unmatched) > 0 {
			log.Fatalf("%s/%s: requests with no fixture route: %v", c.provider, c.name, unmatched)
		}
		if len(reqs) == 0 {
			log.Fatalf("%s/%s: provider made no request", c.provider, c.name)
		}
		sort.Slice(reqs, func(i, j int) bool { return reqs[i].Method+reqs[i].URL < reqs[j].Method+reqs[j].URL })

		var exp expected
		if err != nil {
			exp.Error = err.Error()
		} else {
			rs := make([]result, 0, len(got))
			for _, r := range got {
				rs = append(rs, result{
					SourceURL: r.SourceURL, ImageURL: r.ImageURL, Title: r.Title, Author: r.Author, Source: r.Source,
				})
			}
			if c.sortResults {
				sort.Slice(rs, func(i, j int) bool { return rs[i].SourceURL < rs[j].SourceURL })
			}
			exp.Results = &rs
		}

		cf := caseFile{
			GoTest:      c.goTest,
			Provider:    c.provider,
			Config:      tomlMap(c.config),
			Query:       c.query,
			Routes:      c.routes,
			Requests:    reqs,
			SortResults: c.sortResults,
			Expected:    exp,
		}
		out, mErr := json.MarshalIndent(cf, "", "  ")
		if mErr != nil {
			log.Fatal(mErr)
		}
		write(filepath.Join(sourcesDir, c.provider), c.name+".case.json", append(out, '\n'))
		fmt.Printf("  %s/%s: %s\n", c.provider, c.name, summary(exp))
	}
}

func summary(e expected) string {
	if e.Error != "" {
		return "error: " + e.Error
	}
	return fmt.Sprintf("%d results", len(*e.Results))
}
