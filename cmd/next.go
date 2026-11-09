package cmd

import (
	"encoding/json"
	"fmt"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/spf13/cobra"
)

var nextCmd = &cobra.Command{
	Use:   "next",
	Short: "Switch to the next wallpaper",
	Long: `Ask the daemon to advance to the next wallpaper.

If there is a future entry in the rotation history it is restored;
otherwise a fresh wallpaper is fetched from the configured provider.`,
	RunE: runNext,
}

func init() {
	rootCmd.AddCommand(nextCmd)
}

func runNext(_ *cobra.Command, _ []string) error {
	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{Cmd: "next"})
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
