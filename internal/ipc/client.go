package ipc

import (
	"encoding/json"
	"fmt"
	"net"
	"time"
)

const dialTimeout = 3 * time.Second

// Client communicates with the wallpaperd daemon over a Unix domain socket.
type Client struct {
	SocketPath string
}

// NewClient creates a new IPC client for the given socket path.
func NewClient(socketPath string) *Client {
	return &Client{SocketPath: socketPath}
}

// Send dials the daemon socket, sends cmd as a newline-terminated JSON message,
// and returns the decoded Response.
func (c *Client) Send(cmd Command) (*Response, error) {
	conn, err := net.DialTimeout("unix", c.SocketPath, dialTimeout)
	if err != nil {
		return nil, fmt.Errorf("cannot connect to daemon socket %q: %w", c.SocketPath, err)
	}
	defer conn.Close()

	// Set a generous deadline for the whole round-trip.
	_ = conn.SetDeadline(time.Now().Add(30 * time.Second))

	enc := json.NewEncoder(conn)
	if err := enc.Encode(cmd); err != nil {
		return nil, fmt.Errorf("sending command: %w", err)
	}

	var resp Response
	dec := json.NewDecoder(conn)
	if err := dec.Decode(&resp); err != nil {
		return nil, fmt.Errorf("decoding response: %w", err)
	}

	return &resp, nil
}

// IsRunning returns true when the daemon is reachable on the socket.
func (c *Client) IsRunning() bool {
	conn, err := net.DialTimeout("unix", c.SocketPath, dialTimeout)
	if err != nil {
		return false
	}
	conn.Close()
	return true
}
