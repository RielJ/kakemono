// Package reddit implements a wallpaper provider that fetches image posts
// from Reddit image subreddits.
package reddit

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"strings"
	"time"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers"
)

const (
	defaultFeedURLFmt = "https://www.reddit.com/r/%s/hot.json?limit=100"
	minScore          = 100
)

// Provider fetches wallpapers from Reddit image subreddits.
type Provider struct {
	cfg        *config.RedditConfig
	client     *http.Client
	feedURLFmt string // format string with one %s for subreddit name
}

// New creates a new Reddit Provider using the standard Reddit API endpoints.
func New(cfg *config.RedditConfig) *Provider {
	return &Provider{
		cfg:        cfg,
		client:     providers.DefaultHTTPClient,
		feedURLFmt: defaultFeedURLFmt,
	}
}

// NewWithFeedURL creates a Reddit Provider with a custom feed URL format string.
// feedURLFmt must contain one %s placeholder for the subreddit name.
func NewWithFeedURL(cfg *config.RedditConfig, feedURLFmt string) *Provider {
	return &Provider{
		cfg:        cfg,
		client:     providers.DefaultHTTPClient,
		feedURLFmt: feedURLFmt,
	}
}

func (p *Provider) Name() string { return "reddit" }

// Enabled returns true only when the provider is enabled AND at least one
// subreddit is configured.
func (p *Provider) Enabled() bool {
	return p.cfg.Enabled && len(p.cfg.Subreddits) > 0
}

// Fetch retrieves image posts from a random subreddit in the config.
func (p *Provider) Fetch(ctx context.Context, _ string) ([]providers.WallpaperResult, error) {
	if len(p.cfg.Subreddits) == 0 {
		return nil, fmt.Errorf("reddit: no subreddits configured")
	}

	rng := rand.New(rand.NewSource(time.Now().UnixNano()))
	subreddit := p.cfg.Subreddits[rng.Intn(len(p.cfg.Subreddits))]

	reqURL := fmt.Sprintf(p.feedURLFmt, subreddit)

	req, err := providers.NewRequest(ctx, reqURL)
	if err != nil {
		return nil, fmt.Errorf("reddit: request error: %w", err)
	}

	resp, err := p.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("reddit: HTTP error: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("reddit: unexpected status %d", resp.StatusCode)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reddit: failed to read response: %w", err)
	}

	var listing redditListing
	if err := json.Unmarshal(body, &listing); err != nil {
		return nil, fmt.Errorf("reddit: failed to parse response: %w", err)
	}

	return filterImagePosts(listing, subreddit), nil
}

// filterImagePosts extracts image posts from a Reddit listing.
func filterImagePosts(listing redditListing, subreddit string) []providers.WallpaperResult {
	var results []providers.WallpaperResult
	for _, child := range listing.Data.Children {
		post := child.Data

		if post.Score < minScore {
			continue
		}

		imageURL := resolveImageURL(post.URL)
		if imageURL == "" {
			continue
		}

		pageURL := post.Permalink
		if pageURL == "" {
			pageURL = "https://www.reddit.com/r/" + subreddit + "/comments/" + post.ID
		} else if !strings.HasPrefix(pageURL, "http") {
			pageURL = "https://www.reddit.com" + pageURL
		}

		results = append(results, providers.WallpaperResult{
			SourceURL: pageURL,
			ImageURL:  imageURL,
			Title:     post.Title,
			Author:    post.Author,
			Source:    "reddit",
		})
	}
	return results
}

// resolveImageURL returns a direct image URL from the post URL, or "" if the
// post does not link to a supported image.
func resolveImageURL(postURL string) string {
	lower := strings.ToLower(postURL)

	// Direct image links.
	for _, ext := range []string{".jpg", ".jpeg", ".png", ".webp"} {
		if strings.HasSuffix(lower, ext) {
			return postURL
		}
	}

	// Imgur album/image: convert to direct link.
	if strings.Contains(lower, "imgur.com/") && !strings.Contains(lower, "imgur.com/a/") {
		if !strings.Contains(lower, "i.imgur.com") {
			id := postURL[strings.LastIndex(postURL, "/")+1:]
			if id != "" && !strings.Contains(id, ".") {
				return "https://i.imgur.com/" + id + ".jpg"
			}
		}
		return postURL
	}

	return ""
}

// Reddit JSON types.
type redditListing struct {
	Data struct {
		Children []struct {
			Data redditPost `json:"data"`
		} `json:"children"`
	} `json:"data"`
}

type redditPost struct {
	ID        string `json:"id"`
	Title     string `json:"title"`
	Author    string `json:"author"`
	URL       string `json:"url"`
	Permalink string `json:"permalink"`
	Score     int    `json:"score"`
}
