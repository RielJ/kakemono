package cmd

import (
	"encoding/json"
	"fmt"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/spf13/cobra"
)

var prevCmd = &cobra.Command{
	Use:   "prev",
	Short: "Switch to the previous wallpaper",
	Long:  `Ask the daemon to revert to the previous wallpaper from history.`,
	RunE:  runPrev,
}

func init() {
	rootCmd.AddCommand(prevCmd)
}

func runPrev(_ *cobra.Command, _ []string) error {
	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{Cmd: "prev"})
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
