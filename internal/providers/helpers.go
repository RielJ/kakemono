package providers

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	// UserAgent is the HTTP User-Agent header sent with every outbound request.
	UserAgent = "wallpaperd/1.0"

	// minImageSize is the minimum acceptable downloaded file size in bytes.
	// Files smaller than this are considered invalid (e.g. error pages).
	minImageSize = 10 * 1024 // 10 KB
)

// DefaultHTTPClient is a shared HTTP client with a sensible timeout and the
// wallpaperd User-Agent set on every request via a custom Transport.
var DefaultHTTPClient = &http.Client{
	Timeout:   30 * time.Second,
	Transport: &uaTransport{wrapped: http.DefaultTransport},
}

// SharedHTTPClient is an alias for DefaultHTTPClient provided for
// compatibility with provider implementations that reference this name.
var SharedHTTPClient = DefaultHTTPClient

// uaTransport injects the wallpaperd User-Agent header into every request.
type uaTransport struct {
	wrapped http.RoundTripper
}

func (t *uaTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	// Clone the request so we never mutate the caller's copy.
	clone := req.Clone(req.Context())
	clone.Header.Set("User-Agent", UserAgent)
	return t.wrapped.RoundTrip(clone)
}

// NewRequest creates a new GET request with the given context and URL.
// This helper exists so that sub-packages can build requests without needing
// to import "net/http" directly (and to centralise any future header setup).
func NewRequest(ctx context.Context, url string) (*http.Request, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, fmt.Errorf("providers: build request for %s: %w", url, err)
	}
	return req, nil
}

// DownloadFile downloads the image at url into destDir, validates that the
// response content-type is an image and that the file is larger than minImageSize,
// and returns the local file path.
//
// The client parameter may be nil, in which case DefaultHTTPClient is used.
// On any validation failure the partially-written file is removed.
func DownloadFile(ctx context.Context, url, destDir string, client *http.Client) (string, error) {
	if client == nil {
		client = DefaultHTTPClient
	}

	if err := EnsureDir(destDir); err != nil {
		return "", err
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return "", fmt.Errorf("providers: build request for %s: %w", url, err)
	}

	resp, err := client.Do(req)
	if err != nil {
		return "", fmt.Errorf("providers: GET %s: %w", url, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("providers: GET %s returned status %d", url, resp.StatusCode)
	}

	// Validate content-type before touching the disk.
	ct := resp.Header.Get("Content-Type")
	if !isImageContentType(ct) {
		return "", fmt.Errorf("providers: %s returned non-image content-type %q", url, ct)
	}

	// Derive a safe filename from the URL path.
	name := sanitizeFilename(filepath.Base(strings.Split(url, "?")[0]))
	if name == "" || name == "." {
		name = fmt.Sprintf("wallpaper-%d.jpg", time.Now().UnixNano())
	}
	destPath := filepath.Join(destDir, name)

	f, err := os.Create(destPath)
	if err != nil {
		return "", fmt.Errorf("providers: create %s: %w", destPath, err)
	}

	n, err := io.Copy(f, resp.Body)
	f.Close() // close before any potential removal
	if err != nil {
		os.Remove(destPath)
		return "", fmt.Errorf("providers: write %s: %w", destPath, err)
	}

	if n < minImageSize {
		os.Remove(destPath)
		return "", fmt.Errorf("providers: downloaded file %s is too small (%d bytes)", destPath, n)
	}

	return destPath, nil
}

// IsImageFile reports whether path has a recognised image file extension.
func IsImageFile(path string) bool {
	ext := strings.ToLower(filepath.Ext(path))
	switch ext {
	case ".jpg", ".jpeg", ".png", ".webp", ".gif", ".bmp", ".tiff", ".tif":
		return true
	}
	return false
}

// ExpandPath expands a leading ~ to the current user's home directory.
func ExpandPath(path string) string {
	if strings.HasPrefix(path, "~/") {
		home, err := os.UserHomeDir()
		if err == nil {
			return filepath.Join(home, path[2:])
		}
	}
	return path
}

// EnsureDir creates dir (and any parents) if it does not already exist.
func EnsureDir(dir string) error {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("providers: ensure dir %s: %w", dir, err)
	}
	return nil
}

// isImageContentType reports whether ct represents an image MIME type.
func isImageContentType(ct string) bool {
	ct = strings.ToLower(strings.Split(ct, ";")[0])
	ct = strings.TrimSpace(ct)
	switch ct {
	case "image/jpeg", "image/jpg", "image/png", "image/webp",
		"image/gif", "image/bmp", "image/tiff":
		return true
	}
	return false
}

// sanitizeFilename removes characters that are unsafe in file names.
func sanitizeFilename(name string) string {
	replacer := strings.NewReplacer(
		"/", "_",
		"\\", "_",
		":", "_",
		"*", "_",
		"?", "_",
		"\"", "_",
		"<", "_",
		">", "_",
		"|", "_",
	)
	return replacer.Replace(name)
}
