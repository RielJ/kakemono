# kakemono and waybar

These files add a `custom/wallpaper` module to [waybar](https://github.com/Alexays/Waybar). The module shows the title (or the source) of the current wallpaper and a pause mark while rotation is paused. Its tooltip has the title, the author, the source, the file path, and the theme.

Waybar refreshes the module when it gets `SIGRTMIN+8`. The kakemono daemon sends that signal through its `[hooks] on_change` command after every wallpaper change.

## Files

| File | What it does |
|---|---|
| `kakemono.sh` | Reads `kakemono status --json` and prints one line of waybar JSON. |
| `config.jsonc` | The `custom/wallpaper` module block. |
| `style.css` | Optional styles for the module's CSS classes. |

## Set up the module

1. Install `jq`, and make sure `kakemono` is on your `PATH`.

2. Copy the script:

   ```sh
   mkdir -p ~/.config/waybar/scripts
   cp contrib/waybar/kakemono.sh ~/.config/waybar/scripts/
   chmod +x ~/.config/waybar/scripts/kakemono.sh
   ```

3. Merge the `custom/wallpaper` block from `config.jsonc` into `~/.config/waybar/config.jsonc`, and add `"custom/wallpaper"` to one of `modules-left`, `modules-center`, or `modules-right`.

4. Add the hook to your kakemono config. Run `kakemono config path` to see which file kakemono reads, then add:

   ```toml
   [hooks]
   on_change = ["pkill", "-RTMIN+8", "waybar"]
   ```

   If another module already uses signal 8, pick a free number and change both `"signal"` in the waybar config and `-RTMIN+8` in the hook.

5. Restart the daemon so it reads the new `[hooks]` section. If you installed the login service:

   ```sh
   systemctl --user restart kakemono.service
   ```

   If you start the daemon from your compositor config, stop it and run `kakemono daemon` again.

6. Restart waybar.

To test the hook, run `kakemono next`. The module changes when the new wallpaper is set.

## Clicks

| Action | Command |
|---|---|
| Left click | `kakemono next` |
| Right click | `kakemono pick` (needs the kakemono app) |
| Middle click | `kakemono favorite` |
| Scroll up | `kakemono prev` |
| Scroll down | `kakemono next` |

Pause and resume don't change the wallpaper, so the hook doesn't fire for them. The module picks up the new state within `"interval"` seconds (60). To update it at once, run `pkill -RTMIN+8 waybar` after `kakemono pause` or `kakemono resume`.

## How it works

- The daemon runs the `on_change` argv directly, with no shell, and does not wait for it. A hook that fails or runs longer than 10 seconds is logged as a warning and does not affect the wallpaper.
- The hook gets `PATH`, `HOME`, `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, and `DBUS_SESSION_BUS_ADDRESS` from the daemon, plus `KAKEMONO_PATH`, `KAKEMONO_SOURCE`, `KAKEMONO_TITLE`, `KAKEMONO_AUTHOR`, `KAKEMONO_SOURCE_URL`, and `KAKEMONO_THEME`. The daemon clears every other variable.
- `kakemono.sh` gives the module the classes `light` or `dark`, and `running` or `paused`. When the daemon isn't running, `kakemono status` exits with code 1 and the script prints "🖼 off" with the class `stopped`.
- `"escape": true` makes waybar show `<`, `>`, and `&` in titles as text instead of reading them as Pango markup.
- If waybar's `PATH` doesn't include `kakemono`, set `KAKEMONO` to the full path at the top of `kakemono.sh`.
