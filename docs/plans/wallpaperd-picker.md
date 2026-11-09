# Plan — Floating Wallpaper Picker (Go + Wails + React)

## Summary
Build a lightweight Wails v2 + React floating HUD app (`picker/`) that displays a horizontal strip of wallpaper thumbnails at the bottom of the screen — modeled on Variety's ThumbsWindow. Toggled via `wallpaperd pick` CLI command (bound to alt-shift-period in AeroSpace).

## Architecture
- **Separate Go module** in `picker/` (like `gui/`) with `replace` directive to parent
- **Wails frameless window**: always-on-top, no dock icon, translucent, positioned at screen bottom
- **LocalFileHandler**: serves local images at `/local/<url-encoded-path>` to webview
- **React frontend**: single horizontal scrollable thumbnail strip with Catppuccin Macchiato theme
- **CLI**: `cmd/pick.go` launches the picker binary detached

## Sections

### Section 1: Go Module + Wails Scaffold
Independent — creates `picker/` directory structure

1. Create `picker/go.mod` with replace directive to parent module
2. Create `picker/wails.json` with app ID `com.rielj.wallpaperd-picker`
3. Create `picker/build/darwin/Info.plist` with LSUIElement=true (no dock icon)
4. Create `picker/main.go` — Wails entry with frameless, always-on-top, translucent window
5. Create `picker/app.go` — PickerApp struct with startup positioning, LocalFileHandler, GetThumbnails, SetWallpaper, FavoriteWallpaper, Close methods

### Section 2: React Frontend
Depends on Section 1 (needs wails bindings types)

6. Create `picker/frontend/package.json`, `index.html`, `vite.config.ts`, `tsconfig.json`, `tsconfig.node.json`
7. Create `picker/frontend/src/main.tsx` + `picker/frontend/src/App.tsx` — mounts strip, handles Escape + blur-to-close
8. Create `picker/frontend/src/App.css` — Catppuccin Macchiato theme, frosted glass, horizontal strip
9. Create `picker/frontend/src/components/ThumbnailStrip.tsx` — horizontal scrollable strip with click/right-click/hover
10. Create `picker/frontend/src/components/ContextMenu.tsx` — right-click popup for favorite

### Section 3: CLI Integration
Independent of Section 2

11. Create `cmd/pick.go` — `wallpaperd pick` command that launches picker binary detached
12. Rebuild wallpaperd CLI and install to ~/.local/bin

### Section 4: Build & Install
Depends on Sections 1+2+3

13. npm install + wails build for picker
14. Install picker binary to ~/.local/bin
15. Manual verification
