// Package theme provides macOS appearance (light/dark) detection.
package theme

import (
	"os/exec"
	"strings"
)

// Theme represents the macOS system appearance.
type Theme string

const (
	// ThemeDark indicates macOS is in Dark Mode.
	ThemeDark Theme = "dark"

	// ThemeLight indicates macOS is in Light Mode (the default).
	ThemeLight Theme = "light"
)

// Current reads the current macOS system appearance by querying the global
// preference "AppleInterfaceStyle" via the `defaults` command.
//
// If the preference is set to "Dark" the system is in Dark Mode; any other
// value (including absence of the key) means Light Mode.  If the command
// fails for any reason, ThemeLight is returned as the safe default.
func Current() Theme {
	out, err := exec.Command("defaults", "read", "-g", "AppleInterfaceStyle").Output()
	if err != nil {
		// The key is absent in Light Mode — `defaults` exits non-zero.
		return ThemeLight
	}

	if strings.TrimSpace(strings.ToLower(string(out))) == "dark" {
		return ThemeDark
	}
	return ThemeLight
}
