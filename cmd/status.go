package cmd

import (
	"encoding/json"
	"fmt"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/spf13/cobra"
)

var jsonOutput bool

var statusCmd = &cobra.Command{
	Use:   "status",
	Short: "Show daemon and wallpaper status",
	Long: `Print the current wallpaper and daemon state.

Use --json to get machine-readable output (useful for sketchybar scripts).`,
	RunE: runStatus,
}

func init() {
	statusCmd.Flags().BoolVar(&jsonOutput, "json", false,
		"output raw JSON instead of pretty-printed text")
	rootCmd.AddCommand(statusCmd)
}

func runStatus(_ *cobra.Command, _ []string) error {
	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{Cmd: "status"})
	if err != nil {
		return daemonNotRunning(err)
	}
	if !resp.OK {
		return fmt.Errorf("daemon error: %s", resp.Error)
	}

	if jsonOutput {
		// Pretty-print the raw JSON payload so it's readable but still parseable.
		var pretty any
		if err := json.Unmarshal(resp.Data, &pretty); err != nil {
			// Fall back to raw bytes if unmarshal fails.
			fmt.Println(string(resp.Data))
			return nil
		}
		out, _ := json.MarshalIndent(pretty, "", "  ")
		fmt.Println(string(out))
		return nil
	}

	var s ipc.StatusData
	if err := json.Unmarshal(resp.Data, &s); err != nil {
		return fmt.Errorf("parsing status: %w", err)
	}

	printStatus(&s)
	return nil
}

// printStatus writes a human-readable status block to stdout.
func printStatus(s *ipc.StatusData) {
	paused := "running"
	if s.Paused {
		paused = "paused ⏸"
	}

	fmt.Printf("Daemon:   %s\n", paused)
	fmt.Printf("Theme:    %s\n", s.Theme)
	fmt.Printf("Interval: %s\n", s.Interval)
	fmt.Printf("History:  %d entries (index %d)\n", s.HistoryCount, s.HistoryIndex)

	if s.Current == nil {
		fmt.Println("Current:  (none)")
		return
	}
	fmt.Println()
	fmt.Println("Current wallpaper:")
	printWallpaperInfo(s.Current)
}
