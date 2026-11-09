package wallpaper

import (
	"fmt"
	"os/exec"
	"strings"
)

// SetWallpaper sets the desktop wallpaper on every macOS desktop/space to
// imagePath using osascript.  The path must be an absolute path to a local
// image file.
func SetWallpaper(imagePath string) error {
	// Escape any double-quotes in the path so it is safe to embed in AppleScript.
	escaped := strings.ReplaceAll(imagePath, `"`, `\"`)

	script := fmt.Sprintf(`tell application "System Events"
	tell every desktop
		set picture to "%s"
	end tell
end tell`, escaped)

	out, err := exec.Command("osascript", "-e", script).CombinedOutput()
	if err != nil {
		return fmt.Errorf("wallpaper: osascript set: %w (output: %s)", err, strings.TrimSpace(string(out)))
	}
	return nil
}

// GetCurrentWallpaper returns the file path of the wallpaper currently set on
// the first desktop, as reported by System Events via osascript.
func GetCurrentWallpaper() (string, error) {
	script := `tell application "System Events"
	get picture of first desktop
end tell`

	out, err := exec.Command("osascript", "-e", script).CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("wallpaper: osascript get: %w (output: %s)", err, strings.TrimSpace(string(out)))
	}

	path := strings.TrimSpace(string(out))
	if path == "" {
		return "", fmt.Errorf("wallpaper: osascript returned empty path")
	}
	return path, nil
}
