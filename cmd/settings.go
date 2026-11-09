package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"

	"github.com/spf13/cobra"
)

var settingsCmd = &cobra.Command{
	Use:   "settings",
	Short: "Open the settings GUI",
	Long: `Launch the wallpaperd settings graphical interface.

The GUI binary (wallpaperd-settings) must be installed. It is looked up in:
  1. The same directory as the wallpaperd binary
  2. The system PATH

The current --config and --socket values are forwarded to the GUI via
environment variables (WALLPAPERD_CONFIG and WALLPAPERD_SOCKET).`,
	RunE: runSettings,
}

func init() {
	rootCmd.AddCommand(settingsCmd)
}

func runSettings(_ *cobra.Command, _ []string) error {
	guiBin, err := findGUIBinary()
	if err != nil {
		return fmt.Errorf(
			"wallpaperd-settings binary not found.\n"+
				"Build it with:  cd gui && wails build\n"+
				"Then copy the binary to the same directory as wallpaperd, or add it to PATH.\n"+
				"(original error: %v)", err,
		)
	}

	cmd := exec.Command(guiBin)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	cmd.Env = append(os.Environ(),
		"WALLPAPERD_CONFIG="+getConfigPath(),
		"WALLPAPERD_SOCKET="+getSocketPath(),
	)

	// On macOS, if the binary is inside an .app bundle, open it via the bundle
	// so it gets proper window management.  exec.Command handles this fine for
	// a raw binary too.
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("launching settings GUI: %w", err)
	}

	// Don't wait — let the GUI run independently.
	fmt.Printf("Settings GUI launched (pid %d)\n", cmd.Process.Pid)
	return nil
}

// findGUIBinary locates the wallpaperd-settings binary.
func findGUIBinary() (string, error) {
	binName := "wallpaperd-settings"

	// 1. Next to the running wallpaperd binary.
	selfPath, err := os.Executable()
	if err == nil {
		selfPath, _ = filepath.EvalSymlinks(selfPath)
		candidate := filepath.Join(filepath.Dir(selfPath), binName)
		if isExecutable(candidate) {
			return candidate, nil
		}

		// macOS .app bundle: look for the binary inside the app.
		if runtime.GOOS == "darwin" {
			appBundle := filepath.Join(
				filepath.Dir(selfPath), binName+".app",
				"Contents", "MacOS", binName,
			)
			if isExecutable(appBundle) {
				return appBundle, nil
			}
		}
	}

	// 2. ~/.local/share/wallpaperd/
	if home, err := os.UserHomeDir(); err == nil {
		appBundle := filepath.Join(home, ".local", "share", "wallpaperd",
			binName+".app", "Contents", "MacOS", binName)
		if isExecutable(appBundle) {
			return appBundle, nil
		}
	}

	// 3. In PATH.
	if p, err := exec.LookPath(binName); err == nil {
		return p, nil
	}

	return "", fmt.Errorf("%s not found next to wallpaperd, in ~/.local/share/wallpaperd/, or in PATH", binName)
}

// isExecutable reports whether path exists and is executable.
func isExecutable(path string) bool {
	fi, err := os.Stat(path)
	if err != nil {
		return false
	}
	return !fi.IsDir() && fi.Mode()&0o111 != 0
}
