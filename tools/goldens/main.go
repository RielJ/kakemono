// Command goldens writes golden fixtures marshalled by the Go implementation
// so the Rust rewrite can assert byte-level and semantic compatibility.
//
// Run from the repository root:
//
//	go run ./tools/goldens
//
// Every value is fixed (no time.Now, no randomness), so re-running produces
// byte-identical files. Output:
//
//	crates/protocol/tests/fixtures/*.json, status_json.txt
//	crates/config/tests/fixtures/go_default_template.toml, go_defaults.toml,
//	go_template_loaded.toml
//	crates/sources/tests/fixtures/<provider>/* (see providers.go)
//
// Throwaway tool: deleted together with the Go tree in the final cleanup task.
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"time"

	"github.com/BurntSushi/toml"

	"github.com/rielj/wallpaperd/internal/config"
	"github.com/rielj/wallpaperd/internal/ipc"
	"github.com/rielj/wallpaperd/internal/wallpaper"
)

const (
	protocolDir = "crates/protocol/tests/fixtures"
	configDir   = "crates/config/tests/fixtures"

	// goldenHome is the fixed $HOME used while computing config defaults so
	// expanded paths are identical on every machine.
	goldenHome = "/home/golden"
)

// plus8 mimics a non-UTC local zone: Go formats set_at with
// time.Now().Format(time.RFC3339), which carries the local offset.
var plus8 = time.FixedZone("", 8*60*60)

// fullInfo sets every WallpaperInfo field to a non-zero value. Title and
// image_url contain <, > and & so the fixture shows Go's HTML escaping
// (\u003c, \u003e, \u0026).
func fullInfo() ipc.WallpaperInfo {
	return ipc.WallpaperInfo{
		Path:      "/Users/golden/Pictures/Wallpapers/cache/OHR.GoldenHour_UHD.jpg",
		Source:    "bing",
		SourceURL: "https://www.bing.com/search?q=Golden+Hour&form=hpcapt",
		ImageURL:  "https://www.bing.com/th?id=OHR.GoldenHour_UHD.jpg&rf=LaDigue_UHD.jpg",
		Title:     "Sunset <Golden Hour> & Co — café",
		Author:    "© Photographer \"Quoted\"",
		Width:     3840,
		Height:    2160,
		SetAt:     "2024-03-15T10:30:00+08:00",
	}
}

func localInfo(name, setAt string) ipc.WallpaperInfo {
	p := "/Users/golden/Pictures/local/" + name
	return ipc.WallpaperInfo{
		Path:      p,
		Source:    "local",
		SourceURL: "file://" + p,
		ImageURL:  p,
		Title:     name[:len(name)-len(filepath.Ext(name))],
		SetAt:     setAt,
	}
}

// wireLine is what ipc.Client / daemon.Server put on the socket:
// json.Encoder.Encode output, i.e. compact JSON plus "\n".
func wireLine(v any) []byte {
	var buf bytes.Buffer
	if err := json.NewEncoder(&buf).Encode(v); err != nil {
		log.Fatalf("encode %T: %v", v, err)
	}
	return buf.Bytes()
}

func mustMarshal(v any) json.RawMessage {
	raw, err := json.Marshal(v)
	if err != nil {
		log.Fatalf("marshal %T: %v", v, err)
	}
	return raw
}

func write(dir, name string, data []byte) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		log.Fatal(err)
	}
	p := filepath.Join(dir, name)
	if err := os.WriteFile(p, data, 0o644); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("wrote %s (%d bytes)\n", p, len(data))
}

// savedState returns the exact bytes State.Save writes to disk.
func savedState(tmp string, s *wallpaper.State) []byte {
	p := filepath.Join(tmp, "state.json")
	if err := s.Save(p); err != nil {
		log.Fatal(err)
	}
	data, err := os.ReadFile(p)
	if err != nil {
		log.Fatal(err)
	}
	return data
}

// statusJSONText reproduces cmd/status.go --json: the daemon marshals
// StatusData, the CLI unmarshals into `any` (keys become sorted) and prints
// json.MarshalIndent(pretty, "", "  ") with fmt.Println.
func statusJSONText(s ipc.StatusData) []byte {
	var pretty any
	if err := json.Unmarshal(mustMarshal(s), &pretty); err != nil {
		log.Fatal(err)
	}
	out, err := json.MarshalIndent(pretty, "", "  ")
	if err != nil {
		log.Fatal(err)
	}
	return append(out, '\n')
}

func encodeTOML(v any) []byte {
	var buf bytes.Buffer
	if err := toml.NewEncoder(&buf).Encode(v); err != nil {
		log.Fatal(err)
	}
	return buf.Bytes()
}

func main() {
	if _, err := os.Stat("go.mod"); err != nil {
		log.Fatal("run from the repository root: go run ./tools/goldens")
	}
	// Fixed HOME so config.Defaults()/expandPath produce stable paths.
	if err := os.Setenv("HOME", goldenHome); err != nil {
		log.Fatal(err)
	}

	tmp, err := os.MkdirTemp("", "wallpaperd-goldens-")
	if err != nil {
		log.Fatal(err)
	}
	defer os.RemoveAll(tmp)

	// ── WallpaperInfo ───────────────────────────────────────────────────
	full := fullInfo()
	write(protocolDir, "wallpaper_info_full.json", wireLine(full))
	write(protocolDir, "wallpaper_info_empty.json", wireLine(ipc.WallpaperInfo{}))

	// ── StatusData ──────────────────────────────────────────────────────
	statusNull := ipc.StatusData{
		Current:      nil,
		Paused:       false,
		Interval:     "30m",
		Theme:        "light",
		HistoryCount: 0,
		HistoryIndex: -1,
	}
	statusSet := ipc.StatusData{
		Current:      &full,
		Paused:       true,
		Interval:     "1h30m",
		Theme:        "dark",
		HistoryCount: 3,
		HistoryIndex: 2,
	}
	write(protocolDir, "status_current_null.json", wireLine(statusNull))
	write(protocolDir, "status_current_set.json", wireLine(statusSet))

	// ── Response (built exactly as daemon/server.go handleConn does) ────
	write(protocolDir, "response_ok_nodata.json", wireLine(ipc.Response{OK: true}))
	write(protocolDir, "response_ok_data.json", wireLine(ipc.Response{OK: true, Data: mustMarshal(full)}))
	write(protocolDir, "response_ok_status.json", wireLine(ipc.Response{OK: true, Data: mustMarshal(statusNull)}))
	write(protocolDir, "response_err.json", wireLine(ipc.Response{
		OK:    false,
		Error: fmt.Errorf("unknown command: %q", "bogus").Error(),
	}))
	// ipc.ErrResponse is the other Go constructor; same shape.
	write(protocolDir, "response_err_constructor.json", wireLine(ipc.ErrResponse(errors.New("manager: no current wallpaper"))))

	// ── Command (as ipc.Client.Send encodes it) ─────────────────────────
	write(protocolDir, "command_set.json", wireLine(ipc.Command{
		Cmd:  "set",
		Args: map[string]any{"path": "/Users/golden/Pictures/mountain & lake.jpg"},
	}))
	write(protocolDir, "command_noargs.json", wireLine(ipc.Command{Cmd: "status"}))

	// ── State (exact bytes of State.Save: MarshalIndent, no trailing \n) ─
	empty, err := wallpaper.LoadState(filepath.Join(tmp, "missing.json"))
	if err != nil {
		log.Fatal(err)
	}
	write(protocolDir, "state_empty.json", savedState(tmp, empty))

	populated := &wallpaper.State{HistoryIndex: -1}
	populated.Push(localInfo("alpha.png", "2024-03-15T10:00:00Z"))
	populated.Push(full)
	populated.Push(localInfo("gamma.jpg", "2024-03-15T11:30:00+08:00"))
	populated.Prev() // history_index 1, current == history[1]: shows forward history
	populated.Paused = true
	populated.LastChanged = time.Date(2024, 3, 15, 10, 30, 0, 123456789, plus8)
	write(protocolDir, "state_populated.json", savedState(tmp, populated))

	// ── status --json CLI output ────────────────────────────────────────
	write(protocolDir, "status_json.txt", statusJSONText(statusSet))

	// ── Config ──────────────────────────────────────────────────────────
	tmplPath := filepath.Join(tmp, "config.toml")
	if err := config.InitConfig(tmplPath); err != nil {
		log.Fatal(err)
	}
	tmpl, err := os.ReadFile(tmplPath)
	if err != nil {
		log.Fatal(err)
	}
	write(configDir, "go_default_template.toml", tmpl)

	// config.Defaults() with HOME=/home/golden, re-encoded as TOML.
	write(configDir, "go_defaults.toml", encodeTOML(config.Defaults()))

	// What config.Load produces from the template (HOME=/home/golden).
	// NOTE: this differs from Defaults() (reddit subreddits, light/dark
	// queries) and socket_path keeps its literal "~" because Load does not
	// expand it.
	loaded, err := config.Load(tmplPath)
	if err != nil {
		log.Fatal(err)
	}
	write(configDir, "go_template_loaded.toml", encodeTOML(loaded))

	// ── Provider fixtures ───────────────────────────────────────────────
	writeProviderFixtures()
}
