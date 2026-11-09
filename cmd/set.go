package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/spf13/cobra"
)

var setCmd = &cobra.Command{
	Use:   "set <image-path>",
	Short: "Set a specific image as the wallpaper",
	Long: `Tell the daemon to set a specific local image file as the desktop wallpaper.

The path is added to the rotation history so that prev/next work as expected.

Example:
  wallpaperd set ~/Pictures/mountain.jpg`,
	Args: cobra.ExactArgs(1),
	RunE: runSet,
}

func init() {
	rootCmd.AddCommand(setCmd)
}

func runSet(_ *cobra.Command, args []string) error {
	imagePath, err := expandPath(args[0])
	if err != nil {
		return fmt.Errorf("resolving path: %w", err)
	}

	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{
		Cmd:  "set",
		Args: map[string]any{"path": imagePath},
	})
	if err != nil {
		return daemonNotRunning(err)
	}
	if !resp.OK {
		return fmt.Errorf("daemon error: %s", resp.Error)
	}

	var info ipc.WallpaperInfo
	if err := json.Unmarshal(resp.Data, &info); err != nil {
		return fmt.Errorf("parsing response: %w", err)
	}

	printWallpaperInfo(&info)
	return nil
}

// expandPath expands ~ and resolves to an absolute path.
func expandPath(p string) (string, error) {
	if len(p) >= 2 && p[:2] == "~/" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("home directory: %w", err)
		}
		p = filepath.Join(home, p[2:])
	}
	return filepath.Abs(p)
}
