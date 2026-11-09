package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/spf13/cobra"
)

var pickCmd = &cobra.Command{
	Use:   "pick",
	Short: "Open the floating wallpaper thumbnail picker",
	Long: `Opens a floating HUD window showing wallpaper thumbnails at the bottom
of the screen. Click a thumbnail to set it as wallpaper, right-click to
add to favorites.

The picker is a Wails .app bundle (wallpaperd-picker.app).
Build with: cd picker && wails build`,
	RunE: func(cmd *cobra.Command, args []string) error {
		appPath, err := findPickerApp()
		if err != nil {
			return fmt.Errorf("wallpaperd-picker.app not found: %w\n\nBuild with: cd picker && wails build", err)
		}

		// Use macOS `open` to launch the .app bundle (handles LSUIElement, etc.)
		proc := exec.Command("open", "-a", appPath)
		proc.Stdout = os.Stdout
		proc.Stderr = os.Stderr
		return proc.Run()
	},
}

func findPickerApp() (string, error) {
	// Search locations for wallpaperd-picker.app bundle
	home, _ := os.UserHomeDir()

	candidates := []string{}

	// 1. Same directory as the current wallpaperd binary
	if exe, err := os.Executable(); err == nil {
		dir := filepath.Dir(exe)
		candidates = append(candidates,
			filepath.Join(dir, "wallpaperd-picker.app"),
		)
	}

	// 2. Common install locations
	candidates = append(candidates,
		filepath.Join(home, ".local", "share", "wallpaperd", "wallpaperd-picker.app"),
		filepath.Join(home, ".local", "bin", "wallpaperd-picker.app"),
		"/Applications/wallpaperd-picker.app",
		filepath.Join(home, "Applications", "wallpaperd-picker.app"),
	)

	// 3. Build directory (for development)
	if exe, err := os.Executable(); err == nil {
		// Walk up from binary location to find project root
		dir := filepath.Dir(exe)
		candidates = append(candidates,
			filepath.Join(dir, "..", "picker", "build", "bin", "wallpaperd-picker.app"),
		)
	}
	// Also check the project source directly
	candidates = append(candidates,
		filepath.Join(home, "personal", "projects", "macos-wallpapers", "picker", "build", "bin", "wallpaperd-picker.app"),
	)

	for _, c := range candidates {
		if info, err := os.Stat(c); err == nil && info.IsDir() {
			return c, nil
		}
	}

	return "", fmt.Errorf("not found in any standard location")
}

func init() {
	rootCmd.AddCommand(pickCmd)
}
