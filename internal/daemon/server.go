// Package daemon implements the long-running wallpaperd process: an IPC server
// that listens on a Unix domain socket and dispatches JSON commands to the
// wallpaper Manager.
package daemon

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net"
	"os"

	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/rielj/wallpaperd/internal/wallpaper"
)

// Server accepts IPC connections on a Unix domain socket and delegates each
// command to the wallpaper Manager.
type Server struct {
	socketPath string
	manager    *wallpaper.Manager

	// listener is set by Start; kept so tests can close it directly.
	listener net.Listener

	// ctx is stored from Start so that dispatch can use it for operations
	// that are context-aware (e.g. Manager.Next which may do network I/O).
	ctx context.Context
}

// NewServer returns a Server that will listen on socketPath and route all
// commands to manager.
func NewServer(socketPath string, manager *wallpaper.Manager) *Server {
	return &Server{
		socketPath: socketPath,
		manager:    manager,
	}
}

// Start removes any stale socket file, begins listening, and runs the accept
// loop until ctx is cancelled.  It returns nil when the context is done and
// the listener has been cleaned up, or a non-nil error if setup fails.
func (s *Server) Start(ctx context.Context) error {
	s.ctx = ctx

	// Remove a stale socket left over from a previous (crashed) daemon run.
	if err := os.Remove(s.socketPath); err != nil && !errors.Is(err, os.ErrNotExist) {
		return fmt.Errorf("server: remove stale socket %s: %w", s.socketPath, err)
	}

	ln, err := net.Listen("unix", s.socketPath)
	if err != nil {
		return fmt.Errorf("server: listen on %s: %w", s.socketPath, err)
	}
	s.listener = ln

	// Close the listener when the context is cancelled so that Accept unblocks.
	go func() {
		<-ctx.Done()
		ln.Close()
	}()

	log.Printf("server: listening on %s", s.socketPath)

	// Accept loop — each connection is handled in its own goroutine so that
	// slow commands don't block other callers.
	for {
		conn, err := ln.Accept()
		if err != nil {
			// A cancelled context is the expected shutdown path.
			if ctx.Err() != nil {
				return nil
			}
			log.Printf("server: accept error: %v", err)
			continue
		}
		go s.handleConn(conn)
	}
}

// handleConn reads newline-delimited JSON commands from conn, dispatches each
// one, and writes a JSON response back.  The connection is closed when the
// client disconnects or an unrecoverable write error occurs.
func (s *Server) handleConn(conn net.Conn) {
	defer conn.Close()

	scanner := bufio.NewScanner(conn)
	enc := json.NewEncoder(conn)

	for scanner.Scan() {
		line := scanner.Bytes()

		var cmd ipc.Command
		if err := json.Unmarshal(line, &cmd); err != nil {
			resp := ipc.Response{
				OK:    false,
				Error: fmt.Sprintf("invalid JSON: %v", err),
			}
			if writeErr := enc.Encode(resp); writeErr != nil {
				log.Printf("server: write error response: %v", writeErr)
				return
			}
			continue
		}

		data, dispatchErr := s.dispatch(cmd)

		var resp ipc.Response
		if dispatchErr != nil {
			resp = ipc.Response{OK: false, Error: dispatchErr.Error()}
		} else {
			resp = ipc.Response{OK: true}
			if data != nil {
				raw, err := json.Marshal(data)
				if err != nil {
					resp = ipc.Response{
						OK:    false,
						Error: fmt.Sprintf("internal: marshal response: %v", err),
					}
				} else {
					resp.Data = raw
				}
			}
		}

		if err := enc.Encode(resp); err != nil {
			log.Printf("server: write response for %q: %v", cmd.Cmd, err)
			return
		}
	}

	if err := scanner.Err(); err != nil {
		log.Printf("server: connection read error: %v", err)
	}
}

// dispatch routes a Command to the appropriate Manager method and returns the
// result payload (any type that can be JSON-marshalled) or an error.
func (s *Server) dispatch(cmd ipc.Command) (any, error) {
	switch cmd.Cmd {
	case "next":
		return s.manager.Next(s.ctx)

	case "prev":
		return s.manager.Prev()

	case "set":
		path, _ := cmd.Args["path"].(string)
		if path == "" {
			return nil, fmt.Errorf("set: missing required argument 'path'")
		}
		return s.manager.SetFile(path)

	case "favorite":
		return nil, s.manager.Favorite()

	case "trash":
		return s.manager.Trash(s.ctx)

	case "pause":
		s.manager.Pause()
		return nil, nil

	case "resume":
		s.manager.Resume()
		return nil, nil

	case "status":
		status := s.manager.Status()
		return status, nil

	default:
		return nil, fmt.Errorf("unknown command: %q", cmd.Cmd)
	}
}
