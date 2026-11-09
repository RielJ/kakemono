package cmd

import (
	"errors"
	"fmt"
	"net"
	"strings"

	"github.com/rielj/wallpaperd/internal/ipc"
)

// daemonNotRunning converts a socket connection error into a friendly message
// with instructions on how to start the daemon.
func daemonNotRunning(err error) error {
	var netErr *net.OpError
	if errors.As(err, &netErr) {
		return fmt.Errorf(
			"wallpaperd daemon is not running.\n"+
				"Start it with:  wallpaperd daemon\n"+
				"(original error: %v)", err,
		)
	}
	return err
}

// printWallpaperInfo writes a human-readable summary of a WallpaperInfo to stdout.
func printWallpaperInfo(w *ipc.WallpaperInfo) {
	if w == nil {
		fmt.Println("(no wallpaper information)")
		return
	}

	var lines []string

	if w.Title != "" {
		lines = append(lines, fmt.Sprintf("Title:   %s", w.Title))
	}
	if w.Author != "" {
		lines = append(lines, fmt.Sprintf("Author:  %s", w.Author))
	}
	if w.Source != "" {
		lines = append(lines, fmt.Sprintf("Source:  %s", w.Source))
	}
	if w.SourceURL != "" {
		lines = append(lines, fmt.Sprintf("Page:    %s", w.SourceURL))
	}
	if w.Path != "" {
		lines = append(lines, fmt.Sprintf("File:    %s", w.Path))
	}
	if w.Width > 0 && w.Height > 0 {
		lines = append(lines, fmt.Sprintf("Size:    %dx%d", w.Width, w.Height))
	}
	if w.SetAt != "" {
		lines = append(lines, fmt.Sprintf("Set at:  %s", w.SetAt))
	}

	if len(lines) == 0 {
		fmt.Println("(wallpaper info empty)")
		return
	}

	fmt.Println(strings.Join(lines, "\n"))
}
