# Move from wallpaperd to kakemono

kakemono replaces the Go `wallpaperd`. It reads your wallpaperd config and history, so you can switch without setting anything up again. This guide covers the steps for macOS. On Linux, wallpaperd didn't run, so start at [Install kakemono](../README.md#install).

## What carries over

- **Config.** If `~/.config/kakemono/config.toml` doesn't exist, kakemono reads `~/.config/wallpaperd/config.toml` (under `$XDG_CONFIG_HOME` if set). It never moves, copies, or changes that file. `kakemono config path` prints the file in use and says when it is the wallpaperd one.
- **History and state.** On the first daemon start, if `~/.config/kakemono/state.json` doesn't exist, kakemono copies `~/.config/wallpaperd/state.json` to it: history, the current wallpaper, and the paused flag. kakemono never changes the wallpaperd file.
- **Wallpaper folders.** The cache, favorites, and trash folders come from the config (`download_dir`, `favorites.dir`, `trash_dir`), so kakemono uses the same folders as wallpaperd.
- **Commands.** `next`, `prev`, `set`, `favorite`, `trash`, `pause`, `resume`, `status`, `status --json`, `daemon`, `daemon --foreground`, and `settings` work as before. So do the `--config` and `--socket` flags. Replace `wallpaperd` with `kakemono` in your scripts.
- **The socket protocol.** The daemon still answers one JSON request per line, such as `{"cmd":"next"}`, with `{"ok":true,"data":...}` or `{"ok":false,"error":"..."}`. The socket path changed. See [The socket moved](#the-socket-moved).

kakemono ignores the wallpaperd `[sketchybar]` section. The `[hooks]` section replaces it. See [Update the sketchybar item](#update-the-sketchybar-item).

## Stop wallpaperd

If wallpaperd and kakemono both run, they both rotate the wallpaper. Stop wallpaperd first.

1. Find the wallpaperd LaunchAgent. The wallpaperd README used the label `com.rielj.wallpaperd`:

   ```sh
   launchctl list | grep -i wallpaperd
   ```

2. Stop it and remove it from launchd:

   ```sh
   launchctl bootout "gui/$(id -u)/com.rielj.wallpaperd"
   ```

3. Delete its plist so it doesn't start at the next login:

   ```sh
   rm ~/Library/LaunchAgents/com.rielj.wallpaperd.plist
   ```

4. Stop a wallpaperd that you started by hand:

   ```sh
   pkill -x wallpaperd
   ```

If your plist has another label or file name, use those in steps 2 and 3.

## Start kakemono at login

`kakemono service install` replaces the plist you wrote for wallpaperd:

```sh
kakemono service install
```

The command:

- writes `~/Library/LaunchAgents/dev.rielj.kakemono.daemon.plist` with the label `dev.rielj.kakemono.daemon`;
- runs `<path to kakemono> daemon -f` at login, and restarts it if it crashes;
- loads the service and starts the daemon at once;
- sends the daemon's log to `~/.local/state/kakemono/launchd.log` (under `$XDG_STATE_HOME` if set).

The plist records the path of the `kakemono` binary that ran the command. If you move the binary, run `kakemono service install` again.

Related commands:

| Command | What it does |
|---|---|
| `kakemono service status` | Shows whether the service is installed and running. Add `--json` for `{"installed":bool,"running":bool,"path":string\|null}`. |
| `kakemono service uninstall` | Stops the service and deletes the plist. Exits with code 0 when nothing is installed. |

If a daemon that you started by hand is already running, the service's daemon can't start until that one stops. `service install` prints a note when that happens.

## The socket moved

| | Socket |
|---|---|
| wallpaperd | `~/.config/wallpaperd/daemon.sock` |
| kakemono on macOS | `~/.config/kakemono/kakemono.sock` |
| kakemono on Linux | `$XDG_RUNTIME_DIR/kakemono/kakemono.sock`, or `~/.config/kakemono/kakemono.sock` when `XDG_RUNTIME_DIR` isn't set |

kakemono picks the socket in this order: `--socket`, then `daemon.socket_path` in the config, then `$KAKEMONO_SOCKET`, then the default above.

The wallpaperd config template set `socket_path = "~/.config/wallpaperd/daemon.sock"`. While kakemono reads the wallpaperd config, it ignores that value and uses its own default, so it never fights a wallpaperd that is still running. Any other `socket_path` value is used as is.

Update scripts that talk to the socket directly:

```sh
echo '{"cmd":"status"}' | nc -U ~/.config/kakemono/kakemono.sock
```

## Give kakemono its own config

kakemono keeps reading the wallpaperd config until `~/.config/kakemono/config.toml` exists. To move your settings over:

1. Copy the file:

   ```sh
   mkdir -p ~/.config/kakemono
   cp ~/.config/wallpaperd/config.toml ~/.config/kakemono/config.toml
   ```

2. Open the copy and delete the `socket_path` line, or comment it out. In a copied file, kakemono uses `socket_path` as written, so `~/.config/wallpaperd/daemon.sock` would stay in effect.

3. Delete the `[sketchybar]` section. kakemono ignores it.

4. Restart the daemon:

   ```sh
   launchctl kickstart -k "gui/$(id -u)/dev.rielj.kakemono.daemon"
   ```

To start from the commented default config instead, run `kakemono config init`. It writes `~/.config/kakemono/config.toml` and refuses to overwrite an existing file. `kakemono config edit` opens the config in `$VISUAL` or `$EDITOR`.

## Update the sketchybar item

wallpaperd's `[sketchybar]` section never fired the `wallpaper_changed` event, so the bar item updated only when sketchybar ran the plugin. kakemono runs a command after every wallpaper change instead. To fire the event, add this to the config that `kakemono config path` prints:

```toml
[hooks]
on_change = ["/opt/homebrew/bin/sketchybar", "--trigger", "wallpaper_changed"]
```

Use the full path that `command -v sketchybar` prints. Under launchd, the daemon's `PATH` is `/usr/bin:/bin:/usr/sbin:/sbin`, which doesn't include Homebrew. After you edit the config, restart the daemon with `launchctl kickstart -k "gui/$(id -u)/dev.rielj.kakemono.daemon"`.

The plugin scripts moved from `sketchybar/` to `contrib/sketchybar/` and now call `kakemono`. Copy them over the old ones. [`contrib/sketchybar/README.md`](../contrib/sketchybar/README.md) has the full setup.

The hook runs the argv directly, with no shell, and does not wait for it. A hook that fails or runs longer than 10 seconds is logged as a warning. It gets the variables `KAKEMONO_PATH`, `KAKEMONO_SOURCE`, `KAKEMONO_TITLE`, `KAKEMONO_AUTHOR`, `KAKEMONO_SOURCE_URL`, and `KAKEMONO_THEME`, and the argv can use the placeholders `{path}`, `{uri}`, `{fit}`, and `{theme}`.

## Spaces get the wallpaper only when they are current

macOS has no API that sets the wallpaper on every Space. kakemono, like wallpaperd, sets it on the current Space of each screen. Other Spaces keep their old wallpaper.

If you want each Space to pick up the current wallpaper when you switch to it, set:

```toml
[daemon]
reapply_on_space_change = true
```

The option is off by default because setting the wallpaper again turns off **Show on all Spaces** in System Settings.

## Check the setup with doctor

`kakemono doctor` prints the config file in use, the wallpaper backend, and whether the daemon is running. When the daemon runs, it also shows the daemon's version, backend, theme, and current wallpaper. `kakemono doctor --apply` asks the daemon to set the current wallpaper again, which tests the whole path from daemon to desktop.

`kakemono detect` matters on Linux only. It picks a wallpaper backend for the session. On macOS it prints the native backend and says that detection doesn't apply:

```text
backend:   macos (NSWorkspace setDesktopImageURL (current Space of every screen))
Backend detection is only used on Linux; macOS always uses its native backend.
```

## Other changes

- The Wails settings app is gone. `kakemono settings` and `kakemono pick` open the kakemono app (`kakemono.app`), which also has a menu bar icon. The CLI looks for the app in `$KAKEMONO_UI`, next to the `kakemono` binary, in `/Applications` and `~/Applications`, and on `PATH`.
- The detached daemon (`kakemono daemon` without `-f`) logs to a daily file `kakemono.<date>.log` in `~/.local/state/kakemono/` and keeps the newest 7. `KAKEMONO_LOG` sets the log level, for example `KAKEMONO_LOG=debug`.
- kakemono picks a new wallpaper when macOS switches between light and dark mode. wallpaperd waited for the next rotation. To keep the old behaviour, set `rotate_on_theme_change = false` in `[daemon]`.
- New commands: `open`, `pick`, `detect`, `doctor`, `service`, `config`, and `completions`. The [README](../README.md#commands) lists them.
