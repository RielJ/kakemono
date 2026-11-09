# kakemono and sketchybar

These files add a wallpaper item to [sketchybar](https://github.com/FelixKratz/SketchyBar). The item shows the title (or the source) of the current wallpaper, a moon or sun for the theme, and a pause mark while rotation is paused. Click the item to open a popup with **Next**, **Previous**, **Favorite**, **Trash**, **Pause/Resume**, and **Settings**.

The item updates when the kakemono daemon runs its `[hooks] on_change` command, which fires the sketchybar event `wallpaper_changed`.

## Files

| File | What it does |
|---|---|
| `sketchybarrc.sh` | Adds the `wallpaper_changed` event, the `wallpaper` item, and the popup buttons. |
| `plugins/wallpaper.sh` | Reads `kakemono status --json` and sets the item's label. |
| `plugins/wallpaper_popup.sh` | Runs `kakemono next`, `prev`, `favorite`, `trash`, `pause`, `resume`, or `settings` for the clicked button. |

## Prerequisites

```sh
brew install sketchybar jq
```

You also need the `kakemono` binary. See the [installation section of the main README](../../README.md#install).

## Set up the item

1. Copy the plugins to your sketchybar plugin directory:

   ```sh
   PLUGIN_DIR="$HOME/.config/sketchybar/plugins"
   mkdir -p "$PLUGIN_DIR"
   cp contrib/sketchybar/plugins/wallpaper.sh contrib/sketchybar/plugins/wallpaper_popup.sh "$PLUGIN_DIR/"
   chmod +x "$PLUGIN_DIR/wallpaper.sh" "$PLUGIN_DIR/wallpaper_popup.sh"
   ```

2. Add the item to `~/.config/sketchybar/sketchybarrc`. Either paste the contents of `contrib/sketchybar/sketchybarrc.sh`, or copy the file and source it:

   ```sh
   cp contrib/sketchybar/sketchybarrc.sh "$HOME/.config/sketchybar/wallpaper.sh"
   echo 'source "$HOME/.config/sketchybar/wallpaper.sh"' >> "$HOME/.config/sketchybar/sketchybarrc"
   ```

3. Find the full path of `sketchybar`:

   ```sh
   command -v sketchybar
   ```

   On Apple silicon with Homebrew, the path is `/opt/homebrew/bin/sketchybar`.

4. Add the hook to your kakemono config. Run `kakemono config path` to see which file kakemono reads, then add:

   ```toml
   [hooks]
   on_change = ["/opt/homebrew/bin/sketchybar", "--trigger", "wallpaper_changed"]
   ```

   Use the full path from step 3. The login service that `kakemono service install` creates runs the daemon under launchd, and launchd's `PATH` is `/usr/bin:/bin:/usr/sbin:/sbin`. A bare `"sketchybar"` works only when the daemon was started from a shell that has Homebrew on its `PATH`.

5. Restart the daemon so it reads the new `[hooks]` section:

   ```sh
   launchctl kickstart -k "gui/$(id -u)/dev.rielj.kakemono.daemon"
   ```

   If you started the daemon yourself with `kakemono daemon`, stop that process and start it again.

6. Reload sketchybar:

   ```sh
   sketchybar --reload
   ```

To test the hook, run `kakemono next`. The label changes when the new wallpaper is set.

## How it works

- After every wallpaper change (a timed rotation, `next`, `prev`, `set`, `trash`, or a light or dark switch with `rotate_on_theme_change` on), the daemon runs the `on_change` argv. It runs the command directly, with no shell, and does not wait for it. A hook that fails or runs longer than 10 seconds is logged as a warning and does not affect the wallpaper.
- `on_change` replaces Go wallpaperd's `[sketchybar]` section. The Go daemon never fired the trigger, and kakemono ignores `[sketchybar]`.
- `wallpaper.sh` runs `kakemono status --json` and builds the label from `.current.title`, `.current.source`, `.paused`, and `.theme`. When the daemon isn't running, `status` exits with code 1 and the label shows "No daemon".
- Pause and resume don't change the wallpaper, so the hook doesn't fire for them. `wallpaper_popup.sh` fires `wallpaper_changed` itself after it pauses or resumes.
- The hook gets these environment variables, so a script can use them instead of calling `status`: `KAKEMONO_PATH`, `KAKEMONO_SOURCE`, `KAKEMONO_TITLE`, `KAKEMONO_AUTHOR`, `KAKEMONO_SOURCE_URL`, and `KAKEMONO_THEME`. The argv can also use the placeholders `{path}`, `{uri}`, `{fit}`, and `{theme}`.

## Customize the item

- If sketchybar's `PATH` doesn't include `kakemono`, set `KAKEMONO` to the full path at the top of both plugin scripts, for example `KAKEMONO=/usr/local/bin/kakemono`.
- To change the icon, edit `icon="🖼"` in `sketchybarrc.sh` and `wallpaper.sh`.
- To move the item, change `right` to `left` or `center` in `sketchybar --add item wallpaper right`.
- To change the label length, edit `${TITLE:0:25}` in `wallpaper.sh`.
