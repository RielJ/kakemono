package cmd

import (
	"encoding/json"
	"fmt"
	"os/exec"

	"github.com/spf13/cobra"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/ipc"
)

var (
	openFolderFlag    bool
	openFavoritesFlag bool
)

var openCmd = &cobra.Command{
	Use:   "open",
	Short: "Reveal the current wallpaper in Finder",
	Long: `Reveal the current wallpaper file in Finder.

Use --folder to open the download directory instead.
Use --favorites to open the favorites directory instead.`,
	RunE: runOpen,
}

func init() {
	openCmd.Flags().BoolVar(&openFolderFlag, "folder", false, "open the download directory instead of the current wallpaper")
	openCmd.Flags().BoolVar(&openFavoritesFlag, "favorites", false, "open the favorites directory instead of the current wallpaper")
	rootCmd.AddCommand(openCmd)
}

func runOpen(_ *cobra.Command, _ []string) error {
	if openFolderFlag || openFavoritesFlag {
		cfg, err := config.LoadOrDefault(getConfigPath())
		if err != nil {
			return fmt.Errorf("loading config: %w", err)
		}

		if openFavoritesFlag {
			dir := cfg.ExpandedFavoritesDir()
			fmt.Println("Opening:", dir)
			return exec.Command("open", dir).Run()
		}

		dir := cfg.ExpandedDownloadDir()
		fmt.Println("Opening:", dir)
		return exec.Command("open", dir).Run()
	}

	// Default: reveal the current wallpaper in Finder.
	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{Cmd: "status"})
	if err != nil {
		return daemonNotRunning(err)
	}
	if !resp.OK {
		return fmt.Errorf("daemon error: %s", resp.Error)
	}

	var status ipc.StatusData
	if err := json.Unmarshal(resp.Data, &status); err != nil {
		return fmt.Errorf("parsing status: %w", err)
	}

	if status.Current == nil || status.Current.Path == "" {
		return fmt.Errorf("no wallpaper currently set")
	}

	fmt.Println("Revealing:", status.Current.Path)
	return exec.Command("open", "-R", status.Current.Path).Run()
}
