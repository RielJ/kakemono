# App icons

`source/icon-night.png` is the app icon. `source/icon-day.png` is its daytime twin, which the
README and the docs site show on light backgrounds. `tray.png` is separate: a monochrome
template for the menu bar.

To regenerate the bundle icons, run these in `crates/ui/frontend`:

```sh
# Windows, Linux and the Welcome window: the icon fills the whole square.
npx tauri icon ../src-tauri/icons/source/icon-night.png -o /tmp/flat
cp /tmp/flat/{32x32.png,128x128.png,128x128@2x.png,icon.ico,icon.png} ../src-tauri/icons/

# macOS: Apple's grid puts the shape at 824 of 1024 pixels, so pad it first.
magick ../src-tauri/icons/source/icon-night.png -resize 824x824 -background none \
  -gravity center -extent 1024x1024 /tmp/mac-src.png
npx tauri icon /tmp/mac-src.png -o /tmp/mac
cp /tmp/mac/icon.icns ../src-tauri/icons/
```
