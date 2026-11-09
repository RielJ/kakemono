// Package daemon implements the long-running wallpaperd process.
// It owns the IPC server and the rotation ticker.
package daemon

import (
	"context"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strconv"
	"time"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/providers"
	"github.com/rielj/wallpaperd/internal/providers/apod"
	"github.com/rielj/wallpaperd/internal/providers/bing"
	"github.com/rielj/wallpaperd/internal/providers/local"
	"github.com/rielj/wallpaperd/internal/providers/reddit"
	"github.com/rielj/wallpaperd/internal/providers/unsplash"
	"github.com/rielj/wallpaperd/internal/providers/wallhaven"
	"github.com/rielj/wallpaperd/internal/wallpaper"
)

// Daemon ties together the wallpaper Manager and the IPC Server into the
// long-running wallpaperd process.
type Daemon struct {
	config  *config.Config
	manager *wallpaper.Manager
	server  *Server
	pidPath string
}

// NewDaemon constructs a Daemon from cfg. It creates the provider registry,
// the wallpaper manager, and the IPC server.
// socketPath is the Unix domain socket path to listen on.
func NewDaemon(cfg *config.Config, socketPath string) (*Daemon, error) {
	// Build the provider registry.
	registry := providers.NewRegistry()
	registry.Register(bing.New(cfg.Bing))
	registry.Register(wallhaven.New(&cfg.Wallhaven))
	registry.Register(reddit.New(&cfg.Reddit))
	registry.Register(unsplash.New(&cfg.Unsplash))
	registry.Register(apod.New(&cfg.APOD))
	registry.Register(local.New(&cfg.Local))

	mgr, err := wallpaper.NewManager(cfg, registry)
	if err != nil {
		return nil, fmt.Errorf("daemon: init manager: %w", err)
	}

	srv := NewServer(socketPath, mgr)

	pidPath := filepath.Join(filepath.Dir(socketPath), "daemon.pid")

	return &Daemon{
		config:  cfg,
		manager: mgr,
		server:  srv,
		pidPath: pidPath,
	}, nil
}

// Run is the daemon's main entry point. It writes a PID file, starts the IPC
// server and rotation ticker, and blocks until ctx is cancelled.
func (d *Daemon) Run(ctx context.Context) error {
	// Write PID file.
	if err := d.writePID(d.pidPath); err != nil {
		return err
	}
	defer d.removePID()

	interval, err := time.ParseDuration(d.config.Daemon.Interval)
	if err != nil {
		log.Printf("daemon: invalid interval %q, defaulting to 30m: %v", d.config.Daemon.Interval, err)
		interval = 30 * time.Minute
	}

	// Start IPC server in background.
	errCh := make(chan error, 1)
	go func() {
		if err := d.server.Start(ctx); err != nil {
			errCh <- fmt.Errorf("daemon: IPC server: %w", err)
		}
	}()

	// Start the rotation ticker (only if rotation is enabled).
	if d.config.Daemon.RotationEnabled {
		go d.runTicker(ctx, interval)
		log.Printf("daemon: started (PID %d, interval %s, rotation enabled)", os.Getpid(), interval)
	} else {
		log.Printf("daemon: started (PID %d, rotation disabled)", os.Getpid())
	}

	// Block until context is cancelled or the server exits with an error.
	select {
	case <-ctx.Done():
		log.Printf("daemon: shutting down")
	case srvErr := <-errCh:
		return srvErr
	}

	if err := d.manager.SaveState(); err != nil {
		log.Printf("daemon: save state on shutdown: %v", err)
	}
	return nil
}

// runTicker fires auto-rotation events on every tick until ctx is cancelled.
func (d *Daemon) runTicker(ctx context.Context, interval time.Duration) {
	ticker := time.NewTicker(interval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if d.manager.IsPaused() {
				continue
			}
			info, err := d.manager.Next(ctx)
			if err != nil {
				log.Printf("daemon: auto-rotate: %v", err)
				continue
			}
			log.Printf("daemon: wallpaper changed: %s (%s)", info.Title, info.Source)
		}
	}
}

func (d *Daemon) writePID(path string) error {
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return fmt.Errorf("daemon: create PID dir: %w", err)
	}
	return os.WriteFile(path, []byte(strconv.Itoa(os.Getpid())), 0644)
}

func (d *Daemon) removePID() {
	_ = os.Remove(d.pidPath)
}
