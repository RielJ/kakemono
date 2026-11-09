package cmd

import (
	"fmt"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/spf13/cobra"
)

var resumeCmd = &cobra.Command{
	Use:   "resume",
	Short: "Resume automatic wallpaper rotation",
	Long:  `Tell the daemon to restart the auto-rotation timer after a pause.`,
	RunE:  runResume,
}

func init() {
	rootCmd.AddCommand(resumeCmd)
}

func runResume(_ *cobra.Command, _ []string) error {
	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{Cmd: "resume"})
	if err != nil {
		return daemonNotRunning(err)
	}
	if !resp.OK {
		return fmt.Errorf("daemon error: %s", resp.Error)
	}

	fmt.Println("▶  Auto-rotation resumed.")
	return nil
}
