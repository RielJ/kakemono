package ipc

import "encoding/json"

// Command is a request sent to the daemon.
type Command struct {
	Cmd  string         `json:"cmd"`
	Args map[string]any `json:"args,omitempty"`
}

// Response is returned by the daemon for each command.
type Response struct {
	OK    bool            `json:"ok"`
	Error string          `json:"error,omitempty"`
	Data  json.RawMessage `json:"data,omitempty"`
}

// StatusData is the payload for the "status" command response.
type StatusData struct {
	Current      *WallpaperInfo `json:"current"`
	Paused       bool           `json:"paused"`
	Interval     string         `json:"interval"`
	Theme        string         `json:"theme"`
	HistoryCount int            `json:"history_count"`
	HistoryIndex int            `json:"history_index"`
}

// WallpaperInfo carries metadata about a wallpaper.
type WallpaperInfo struct {
	Path      string `json:"path"`
	Source    string `json:"source"`
	SourceURL string `json:"source_url"`
	ImageURL  string `json:"image_url"`
	Title     string `json:"title"`
	Author    string `json:"author"`
	Width     int    `json:"width"`
	Height    int    `json:"height"`
	SetAt     string `json:"set_at"`
}

// OKResponse is a convenience constructor for a successful response.
func OKResponse(data any) Response {
	raw, _ := json.Marshal(data)
	return Response{OK: true, Data: raw}
}

// ErrResponse is a convenience constructor for an error response.
func ErrResponse(err error) Response {
	return Response{OK: false, Error: err.Error()}
}
