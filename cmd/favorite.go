package cmd

import (
	"fmt"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/spf13/cobra"
)

var favoriteCmd = &cobra.Command{
	Use:   "favorite",
	Short: "Save the current wallpaper to favorites",
	Long: `Copy the current wallpaper to the favorites directory configured in
config.toml (defaults to ~/Pictures/Wallpapers/favorites).`,
	RunE: runFavorite,
}

func init() {
	rootCmd.AddCommand(favoriteCmd)
}

func runFavorite(_ *cobra.Command, _ []string) error {
	client := ipc.NewClient(getSocketPath())
	resp, err := client.Send(ipc.Command{Cmd: "favorite"})
	if err != nil {
		return daemonNotRunning(err)
	}
	if !resp.OK {
		return fmt.Errorf("daemon error: %s", resp.Error)
	}

	fmt.Println("✓ Wallpaper saved to favorites.")
	return nil
}
