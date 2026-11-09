# Build Assets

This directory contains build-time assets consumed by `wails build`.

## Files

| File | Purpose |
|---|---|
| `appicon.png` | Application icon (1024×1024 PNG); Wails converts it to `.icns` |
| `darwin/Info.plist` | macOS bundle metadata for release builds |
| `darwin/Info.dev.plist` | macOS bundle metadata for development builds (`wails dev`) |

## App Icon

`appicon.png` must be a **1024×1024 PNG** with an alpha channel.
Wails generates the required `.icns` file automatically during `wails build`.

To create a simple placeholder icon you can use ImageMagick:

```bash
convert -size 1024x1024 xc:'#1C1C1E' \
  -fill white -pointsize 400 -gravity center -annotate 0 '🖼' \
  gui/build/appicon.png
```

Or export from any design tool (Figma, Sketch, etc.) and drop it here.
If `appicon.png` is missing, `wails build` will use the Wails default icon.
