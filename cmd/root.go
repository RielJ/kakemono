package cmd

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/cobra"

	"github.com/rielj/wallpaperd/internal/config"
)

var (
	cfgFile    string
	socketFile string
)

var rootCmd = &cobra.Command{
	Use:   "wallpaperd",
	Short: "A macOS wallpaper manager daemon",
	Long: `wallpaperd is a macOS desktop wallpaper manager that supports multiple
download sources (Wallhaven, Unsplash, Bing, Reddit, APOD, Local),
auto-rotation with light/dark theme awareness, history/favorites/trash
management, and a floating picker HUD.`,
}

// Execute runs the root cobra command.
func Execute() {
	if err := rootCmd.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func init() {
	defaultCfg := filepath.Join(os.Getenv("HOME"), ".config", "wallpaperd", "config.toml")

	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", defaultCfg, "config file path")
	// socket default is intentionally empty so that runForeground() can prefer
	// the value from config.toml before falling back to DefaultSocketPath().
	rootCmd.PersistentFlags().StringVar(&socketFile, "socket", "", "daemon socket path")
}

// getSocketPath returns the effective socket path.
// If --socket was not explicitly set it falls back to config.DefaultSocketPath()
// so that CLI commands always have a usable path even without a config file.
func getSocketPath() string {
	if socketFile != "" {
		return socketFile
	}
	return config.DefaultSocketPath()
}

// getConfigPath returns the config path from flags.
func getConfigPath() string {
	return cfgFile
}
