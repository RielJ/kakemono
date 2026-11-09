package cmd

import (
	"fmt"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/spf13/cobra"
)

var pauseCmd = &cobra.Command{
	Use:   "pause",
	Short: "Pause automatic wallpaper rotation",
	Long:  `Tell the daemon to stop the auto-rotation timer.  Use "resume" to restart it.`,
	RunE:  runPause,
}

func init() {
	rootCmd.AddCommand(pauseCmd)
}

func runPause(_ *cobra.Command, _ []string) error {
	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{Cmd: "pause"})
	if err != nil {
		return daemonNotRunning(err)
	}
	if !resp.OK {
		return fmt.Errorf("daemon error: %s", resp.Error)
	}

	fmt.Println("⏸  Auto-rotation paused.")
	return nil
}
