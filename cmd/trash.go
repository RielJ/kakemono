package cmd

import (
	"encoding/json"
	"fmt"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/spf13/cobra"
)

var trashCmd = &cobra.Command{
	Use:   "trash",
	Short: "Trash the current wallpaper and advance to the next",
	Long: `Move the current wallpaper to the trash directory and immediately
switch to the next one.`,
	RunE: runTrash,
}

func init() {
	rootCmd.AddCommand(trashCmd)
}

func runTrash(_ *cobra.Command, _ []string) error {
	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{Cmd: "trash"})
	if err != nil {
		return daemonNotRunning(err)
	}
	if !resp.OK {
		return fmt.Errorf("daemon error: %s", resp.Error)
	}

	fmt.Println("🗑  Wallpaper trashed.  Switching to next…")

	// The daemon auto-advances after trash; the response carries the new wallpaper.
	if len(resp.Data) > 0 {
		var info ipc.WallpaperInfo
		if err := json.Unmarshal(resp.Data, &info); err == nil {
			printWallpaperInfo(&info)
		}
	}
	return nil
}
