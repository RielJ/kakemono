package cmd

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"os/signal"
	"syscall"

	"github.com/spf13/cobra"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/daemon"
	"github.com/rielj/wallpaperd/internal/ipc"
)

var foreground bool

var daemonCmd = &cobra.Command{
	Use:   "daemon",
	Short: "Start the wallpaperd background daemon",
	Long: `Start the wallpaperd daemon.

Without --foreground the process detaches from the terminal (new session,
stdin/stdout/stderr closed). Pass --foreground to keep it attached — useful
for launchctl service definitions or debugging.

  # Background (default):
  wallpaperd daemon

  # Foreground (e.g. in a launchctl plist):
  wallpaperd daemon --foreground`,
	RunE: runDaemon,
}

func init() {
	daemonCmd.Flags().BoolVarP(&foreground, "foreground", "f", false,
		"run the daemon in the foreground (do not detach)")
	rootCmd.AddCommand(daemonCmd)
}

func runDaemon(_ *cobra.Command, _ []string) error {
	if !foreground {
		return spawnBackground()
	}
	return runForeground()
}

// spawnBackground re-execs the current binary with --foreground in a new
// session, fully detached from the controlling terminal.
func spawnBackground() error {
	// Resolve the effective socket path (same priority as runForeground) so we
	// can check whether a daemon is already listening on it.
	cfg, _ := config.LoadOrDefault(getConfigPath())
	sock := socketFile // empty when --socket was not explicitly set
	if sock == "" && cfg.Daemon.SocketPath != "" {
		sock = cfg.Daemon.SocketPath
	}
	if sock == "" {
		sock = config.DefaultSocketPath()
	}

	// Refuse to start a second daemon instance.
	client := ipc.NewClient(sock)
	if client.IsRunning() {
		return fmt.Errorf("daemon is already running (socket: %s)", sock)
	}

	exe, err := os.Executable()
	if err != nil {
		return fmt.Errorf("resolving executable path: %w", err)
	}

	spawnArgs := []string{"daemon", "--foreground", "--config", getConfigPath()}
	// Only forward --socket when it was explicitly set by the caller.
	// If omitted, the child process resolves the path from its own config
	// (matching the logic in runForeground).
	if socketFile != "" {
		spawnArgs = append(spawnArgs, "--socket", socketFile)
	}

	child := exec.Command(exe, spawnArgs...)
	child.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	child.Stdin = nil
	child.Stdout = nil
	child.Stderr = nil

	if err := child.Start(); err != nil {
		return fmt.Errorf("spawning daemon: %w", err)
	}

	fmt.Printf("wallpaperd daemon started (pid %d)\n", child.Process.Pid)
	fmt.Printf("Socket: %s\n", sock)
	return nil
}

// runForeground loads configuration, creates the daemon, and runs it until
// SIGTERM or SIGINT.
func runForeground() error {
	cfgPath := getConfigPath()

	// Auto-generate config on first run.
	if _, statErr := os.Stat(cfgPath); os.IsNotExist(statErr) {
		if initErr := config.InitConfig(cfgPath); initErr != nil {
			fmt.Fprintf(os.Stderr, "warning: could not write default config: %v\n", initErr)
		} else {
			fmt.Fprintf(os.Stderr, "Created default config at %s\n", cfgPath)
		}
	}

	cfg, err := config.LoadOrDefault(cfgPath)
	if err != nil {
		return fmt.Errorf("loading config: %w", err)
	}

	// Resolve the socket path with this priority:
	//   1. --socket flag (explicitly passed by the user; socketFile != "")
	//   2. socket_path from config.toml
	//   3. built-in default (~/.config/wallpaperd/daemon.sock)
	sock := socketFile // raw flag value; empty when not explicitly set
	if sock == "" && cfg.Daemon.SocketPath != "" {
		sock = cfg.Daemon.SocketPath
	}
	if sock == "" {
		sock = config.DefaultSocketPath()
	}

	d, err := daemon.NewDaemon(cfg, sock)
	if err != nil {
		return fmt.Errorf("initialising daemon: %w", err)
	}

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer stop()

	return d.Run(ctx)
}
