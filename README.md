# kakemono

kakemono is a wallpaper manager for macOS, Linux, and Windows. A small daemon downloads wallpapers from Bing, Wallhaven, Unsplash, Reddit, NASA APOD, or your own folders, and changes the wallpaper on a timer. It keeps a history you can step back through, saves favorites, and can prefer dark images in dark mode and light images in light mode.

The `kakemono` binary is both the daemon and the command-line client. The kakemono app adds a menu-bar (tray) icon, a thumbnail picker for browsing and setting wallpapers, and a settings window.

kakemono replaces the Go `wallpaperd`, and reads its config and history. To switch, follow [docs/migration.md](docs/migration.md).

## Install

Prebuilt downloads are on the [Releases](https://github.com/RielJ/kakemono/releases) page. Each release has two kinds of file:

- **The app** (`kakemono_<version>_<arch>.dmg`, `.deb`, `.rpm`, `.AppImage`, `.msi`, or `-setup.exe`). It contains the `kakemono` binary too, so most people need only this.
- **The command-line tool alone** (`kakemono-<version>-<target>.tar.gz`, or `.zip` on Windows), for servers and window managers where you don't want the app.

`SHA256SUMS` lists a checksum for every file.

The builds are not signed, so each system warns about them the first time.

### macOS

1. Open the `.dmg` for your Mac (`aarch64` for Apple silicon, `x64` for Intel) and drag **kakemono** to **Applications**.
2. Because the app is not signed, macOS says that it "is damaged" or "cannot be opened". To allow it, run:

   ```sh
   xattr -dr com.apple.quarantine /Applications/kakemono.app
   ```

3. Open kakemono. It starts the daemon and puts its icon in the menu bar.
4. To use the command-line tool, link it onto your `PATH`:

   ```sh
   ln -s /Applications/kakemono.app/Contents/MacOS/kakemono ~/.local/bin/kakemono
   ```

On a MacBook with a notch, macOS hides menu-bar icons that don't fit beside it. If you can't see the kakemono icon, quit some menu-bar apps or use a menu-bar manager such as Ice or Bartender.

### Linux

Install the package for your distribution:

```sh
sudo apt install ./kakemono_<version>_amd64.deb      # Debian, Ubuntu
sudo dnf install ./kakemono-<version>-1.x86_64.rpm   # Fedora
```

Or make the AppImage executable and run it: `chmod +x kakemono_<version>_amd64.AppImage`.

The packages install `kakemono` and `kakemono-ui` into `/usr/bin`. The tray icon needs a StatusNotifier host: KDE Plasma and most bars (waybar, for example) have one. On GNOME, install the AppIndicator extension.

On a Linux machine without a desktop app, or on ARM, use the `tar.gz` and copy `kakemono` onto your `PATH`.

### Windows

Run the `.msi` or the `-setup.exe`. SmartScreen warns that the publisher is unknown: click **More info**, then **Run anyway**. The installer adds kakemono to the Start menu. Its icon is in the notification area; Windows can hide it behind the **^** arrow.

The `.zip` contains `kakemono.exe` alone.

### Build from source

kakemono builds with stable Rust 1.99 or newer. For the command-line tool and daemon only, run this in a clone of this repository:

```sh
cargo install --locked --path crates/cli
```

The command installs `kakemono` into `~/.cargo/bin`.

The app is a Tauri 2 project in `crates/ui`, and also needs Node.js 26 or newer (`.node-version`). On Linux it needs the WebKitGTK development packages too. On Debian and Ubuntu:

```sh
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
```

To build the app bundle, which contains the `kakemono` binary:

```sh
scripts/build-sidecar.sh
cd crates/ui/frontend
npm ci
TAURI_APP_PATH=../src-tauri npm run tauri -- build --config ../src-tauri/tauri.bundle.conf.json
```

The bundles go to `target/release/bundle/`.

### Start the daemon at login

```sh
kakemono service install
```

| OS | What `service install` creates | Logs |
|---|---|---|
| macOS | LaunchAgent `dev.rielj.kakemono.daemon` in `~/Library/LaunchAgents/` | `~/.local/state/kakemono/launchd.log` |
| Linux | systemd user unit `~/.config/systemd/user/kakemono.service`, part of `graphical-session.target` | `journalctl --user -u kakemono.service` |
| Windows | the value `kakemono-daemon` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` | `%LOCALAPPDATA%\kakemono\logs\` |

On Linux, the systemd unit needs your session's environment, and many tiling compositors don't start `graphical-session.target`. [docs/linux-backends.md](docs/linux-backends.md#the-systemd-service-needs-your-session-environment) explains both cases.

`kakemono service status` shows whether the service is installed and running. `kakemono service uninstall` removes it.

## Quick start

```sh
kakemono daemon          # start the daemon in the background
kakemono next            # set a new wallpaper now
kakemono status          # show the current wallpaper
kakemono prev            # go back in the history
kakemono favorite        # copy the current wallpaper to the favorites folder
kakemono trash           # move the current wallpaper to the trash folder and set the next one
kakemono set ~/Downloads/photo.jpg
kakemono pause           # stop the timer; `kakemono resume` starts it again
```

If a command can't reach the daemon, it exits with code 1 and says how to start it.

On Linux, run `kakemono detect` once to see which wallpaper backend kakemono picked for your desktop. If wallpapers don't change, run `kakemono doctor`.

## Commands

Every command takes `--config <PATH>` and `--socket <PATH>`. `kakemono <command> --help` describes each one.

| Command | What it does |
|---|---|
| `daemon` | Starts the daemon in the background. `-f`, `--foreground` keeps it in the terminal. The daemon refuses to start twice. |
| `next` | Sets the next wallpaper: forward in the history, or a new download at the end of it. |
| `prev` | Sets the previous wallpaper from the history. |
| `set <IMAGE-PATH>` | Sets an image file as the wallpaper. |
| `favorite` | Copies the current wallpaper to the favorites folder. |
| `trash` | Moves the current wallpaper to the trash folder and sets the next one. |
| `status` | Shows the current wallpaper, the theme, the interval, and whether rotation is paused. `--json` prints the daemon's JSON. |
| `pause`, `resume` | Stops or restarts the rotation timer. |
| `open` | Reveals the current wallpaper in Finder or the file manager. `--folder` opens the download folder. `--favorites` opens the favorites folder. |
| `pick` | Opens the thumbnail picker in the kakemono app. |
| `settings` | Opens the settings window in the kakemono app. |
| `detect` | Shows the wallpaper backend for this session and why. `--write` saves it to `[desktop]`. `--force` overwrites a section with `managed_by = "user"`. Linux only. On other systems it prints the native backend. |
| `doctor` | Prints the config file, the session, the backend and the command it would run, the picker window rules, and the daemon's state. `--apply` asks the daemon to set the current wallpaper again. |
| `service install`, `service uninstall`, `service status [--json]` | Manages the login service. |
| `config path`, `config init`, `config edit` | Prints the config path, writes the commented default config (never overwrites), or opens the config in `$VISUAL` or `$EDITOR`. |
| `completions <SHELL>` | Prints a completion script for `bash`, `elvish`, `fish`, `powershell`, or `zsh`. |

`pick` and `settings` look for the `kakemono-ui` binary in `$KAKEMONO_UI`, next to `kakemono`, in `/Applications/kakemono.app` and `~/Applications/kakemono.app` (macOS), and on `PATH`.

## The picker

`kakemono pick`, or the tray, opens the picker: a strip of thumbnails along one screen edge (`[picker] position`). It has five tabs:

| Tab | What it shows |
|---|---|
| Sources | Previews from the enabled online sources, fetched as you scroll. Nothing is downloaded until you use an image. |
| Downloaded | The download folders of the enabled online sources, then the manual folder. |
| Local | The images of the enabled folder, album, and images sources. |
| Favorites | The favorites folder. |
| Cache | Files in the download folder that belong to no enabled online source: loose files and the folders of removed sources. |

The current wallpaper comes first in the tab that holds it.

### Keys

The tabs run across the strip, so the tab keys use the other arrow axis. In a horizontal strip, Left and Right move between thumbnails and Up and Down change the tab. In a vertical strip, it's the other way round.

| Key | Action |
|---|---|
| Left, Right (vertical strip: Up, Down) | Previous or next thumbnail. Home and End go to the first and the last. |
| Up, Down (vertical strip: Left, Right) | Previous or next tab. |
| 1–5 | Sources, Downloaded, Local, Favorites, or Cache. |
| `[`, `]` | Sources tab: previous or next source filter (All, then one per source). |
| Enter, Space | Set as wallpaper. |
| F | Add to favorites. |
| Delete, Backspace | Move to trash (not in Sources). |
| Menu key, Shift+F10 | Open the context menu. In Sources it also has Download. |
| Cmd+Z (Ctrl+Z) | Undo the last trash. |
| Esc | Close the picker. |

### Browsing sources

In the Sources tab, Set downloads the image to the source's download folder and sets it. Favorite downloads it to the favorites folder. Download only saves it.

| Source type | What it browses |
|---|---|
| Wallhaven | The row's search, with its purity, categories, and sorting. Without an API key, each request reads one search page, about 24 images. |
| Unsplash | The row's search. With no keywords, the editorial feed, up to 20 pages. Needs `access_key`. |
| Reddit | The subreddit's hot listing, 25 posts a page. Posts marked NSFW (`over_18`) and posts with a score below 100 are left out. |
| Bing | The current daily images, up to 16. |
| APOD | The archive, 14 days a page, newest first, back to 16 June 1995. Videos are skipped. |
| Folder, album, images | Not in Sources. Their images are in the Local tab. |

Each source reports its own state under the strip. A source that the provider is rate-limiting shows `type (id): message` with the provider's text. kakemono asks it again after the wait the provider gave, or after 60 seconds. A source that needs setup, such as a missing API key, shows what to add. With internet access off in Settings, the tab says that browsing is off.

`[picker] blur_nsfw` (default `true`, or the Blur NSFW previews switch in Settings) blurs previews that the source marks NSFW, such as Wallhaven sketchy and NSFW results, until you hover over or focus them.

Privacy: the daemon fetches previews, not the picker window. It requests them only from the sources' image hosts, does not use a system proxy, and caches them for 24 hours in `<cache>/previews` (see [Files](#files)).

## Configuration

kakemono reads the first config file that it finds:

1. `--config <PATH>`;
2. `$KAKEMONO_CONFIG`;
3. `~/.config/kakemono/config.toml` (`$XDG_CONFIG_HOME/kakemono/config.toml` if set; `%APPDATA%\kakemono\config.toml` on Windows);
4. the wallpaperd config, `~/.config/wallpaperd/config.toml`.

With no file, kakemono uses the defaults below. `kakemono config init` writes a commented config, with some example values that differ from the defaults. The daemon reads the config when it starts. After you edit the file, restart the daemon or send it `reload` (see [Talk to the daemon over the socket](#talk-to-the-daemon-over-the-socket)).

kakemono ignores unknown keys. A section with some keys missing uses the defaults for those keys. Paths can start with `~`.

### `[daemon]`

| Key | Default | Meaning |
|---|---|---|
| `rotation_enabled` | `true` | Change the wallpaper on a timer. |
| `interval` | `"30m"` | Time between changes, as a Go duration: `"45s"`, `"30m"`, `"2h30m"`. An invalid or zero value falls back to 30 minutes. |
| `download_dir` | `"~/Pictures/Wallpapers/cache"` | Where downloads go, one folder per source. |
| `trash_dir` | `"~/Pictures/Wallpapers/trash"` | Where `trash` moves files. |
| `socket_path` | unset | The daemon socket. Unset means the platform default (see [Files](#files)). Ignored on Windows. |
| `rotate_on_theme_change` | `true` | Set a new wallpaper when the system switches between light and dark mode. |
| `reapply_on_space_change` | `false` | macOS: set the current wallpaper again when you switch Spaces. See [docs/migration.md](docs/migration.md#spaces-get-the-wallpaper-only-when-they-are-current). |

### Change and download settings in `[daemon]`

| Key | Default | Meaning |
|---|---|---|
| `change_on_start` | `false` | Change the wallpaper once when the daemon starts. |
| `internet` | `true` | Allow online sources. With `false`, kakemono uses only the images already on disk. |
| `download_enabled` | `true` | Download new images from the online sources in the background. |
| `download_interval` | `"10m"` | Time between background downloads, one image each time. |
| `download_limit_mb_enabled`, `download_limit_mb` | `true`, `500` | Keep the download folder under this size. kakemono deletes the oldest downloads first, and never deletes favorites, the current wallpaper, or files in local sources. |
| `manual_dir` | `<download_dir>/manual` | Where images go that you drop on Settings or fetch from a URL. |

A wallpaper change picks an image that is already on disk, from the enabled sources. The background downloader keeps that pool fresh. If the pool is empty, the change downloads one image first.

### Sources: `[[sources]]`

Each source is one `[[sources]]` row. You can have several rows of the same type, for example two Wallhaven searches.

```toml
[[sources]]
id = "wh-nature"          # a-z, 0-9 and "-", at most 32; names the folder downloads/<id>/
enabled = true
type = "wallhaven"
location = "nature"

[[sources]]
id = "pictures"
type = "folder"
location = "~/Pictures/Wallpapers"
```

| `type` | `location` |
|---|---|
| `wallhaven` | Search keywords, or a wallhaven.cc search URL. Empty for the latest images. |
| `unsplash` | Search keywords. Empty for the editorial feed. Needs `[unsplash] access_key`. |
| `reddit` | A subreddit, such as `EarthPorn`. |
| `bing` | Empty. The images of the day. |
| `apod` | Empty. NASA's Astronomy Picture of the Day. |
| `folder` | A folder. kakemono picks its images at random, subfolders included. |
| `album-name`, `album-date` | A folder, shown in file-name or file-date order. |
| `images` | Image files. |

With no `[[sources]]` rows, kakemono uses Bing, Wallhaven "nature", and the favorites folder. Settings has a sources table with **Add…**, which is the easiest way to edit them.

Settings shared by every row of a type go in a section named after the type:

| Section | Keys and defaults |
|---|---|
| `[wallhaven]` | `api_key = ""`, `purity = "100"`, `sorting = "random"`, `categories = "111"` |
| `[unsplash]` | `access_key = ""` |
| `[bing]` | `market = "en-US"` |
| `[apod]` | `api_key = "DEMO_KEY"` |

Wallhaven works without an API key. `purity` is three digits for SFW, sketchy, and NSFW. `categories` is three digits for general, anime, and people. `sorting` is one of `random`, `toplist`, `date_added`, `relevance`, `views`, `favorites`, and `hot`.

Config files from older versions, with `[light]`, `[dark]`, and an `enabled` key per provider, are converted to `[[sources]]` rows on the first start. [docs/migration.md](docs/migration.md#sources-list-2026-07) describes the conversion.

### `[filters]`

Filters are soft: if no image passes all of them, kakemono uses the image that passes the most.

| Key | Default | Meaning |
|---|---|---|
| `landscape` | `false` | Only images wider than they are tall. |
| `min_size_enabled`, `min_size_percent` | `false`, `80` | Only images at least this percentage (50, 80, or 100) of the screen size. |
| `brightness` | `"off"` | `dark`, `light`, or `system`: dark images in dark mode and light images in light mode. |
| `color_enabled`, `color` | `false`, `"#3b82f6"` | Prefer images whose main colour is close to this colour. |
| `rating_enabled`, `min_rating` | `false`, `4` | Only images with an EXIF or XMP rating of at least this (1–5). |
| `name_regex_enabled`, `name_regex` | `false`, `""` | Only images whose file name matches this regular expression. |

### `[favorites]`

| Key | Default |
|---|---|
| `dir` | `"~/Pictures/Wallpapers/favorites"` |

### `[picker]`

| Key | Default | Meaning |
|---|---|---|
| `position` | `"bottom"` | The screen edge for the picker: `bottom`, `top`, `left`, or `right`. |
| `orientation` | `"auto"` | `horizontal`, `vertical`, or `auto`: vertical on the left and right edges. |
| `close_on_blur` | `true` | Hide the picker when it loses focus. |
| `placement` | `"edge"` | `edge` uses `position`. `custom` uses `custom`. Dragging the picker sets both. |
| `custom` | unset | `{ x = 10, y = 20, monitor = "DP-1" }` |
| `force_x11` | `false` | Linux, Wayland: run the picker under XWayland so kakemono can place it. Experimental. |
| `blur_nsfw` | `true` | Blur NSFW previews in the Sources tab until you hover over or focus them. |

### `[desktop]`

Linux only. It selects the wallpaper backend: `backend`, `set_command`, `persistent`, `fit`, `theme`, and `managed_by`. The daemon fills it in on the first start. [docs/linux-backends.md](docs/linux-backends.md) describes every key and backend. On macOS and Windows, kakemono always uses the system API and ignores `backend` and `set_command`.

### `[ui]`

| Key | Default | Meaning |
|---|---|---|
| `appearance` | `"light"` | The colour scheme of the settings window: `light`, `dark`, or `system`. |

### `[hooks]`

| Key | Default | Meaning |
|---|---|---|
| `on_change` | unset | A command to run after every wallpaper change. |

`on_change` is an argv array, or a string that kakemono splits like a shell would. kakemono runs it without a shell and doesn't wait for it. A hook that fails or runs longer than 10 seconds is logged as a warning. At most 4 hooks run at once.

The hook gets `KAKEMONO_PATH`, `KAKEMONO_SOURCE`, `KAKEMONO_TITLE`, `KAKEMONO_AUTHOR`, `KAKEMONO_SOURCE_URL`, and `KAKEMONO_THEME`, plus `PATH`, `HOME`, `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, and `DBUS_SESSION_BUS_ADDRESS` from the daemon. Arguments can use `{path}`, `{uri}`, `{fit}`, and `{theme}`.

```toml
[hooks]
# sketchybar (macOS). The `kakemono service install` daemon has Homebrew on its PATH; from
# other launchers, use the full path (`command -v sketchybar`).
on_change = ["sketchybar", "--trigger", "wallpaper_changed"]
# waybar (Linux)
# on_change = ["pkill", "-RTMIN+8", "waybar"]
```

[contrib/sketchybar](contrib/sketchybar/README.md) and [contrib/waybar](contrib/waybar/README.md) have complete bar items that use these hooks.

## Files

| | macOS | Linux | Windows |
|---|---|---|---|
| Config | `~/.config/kakemono/config.toml` | `$XDG_CONFIG_HOME/kakemono/config.toml` | `%APPDATA%\kakemono\config.toml` |
| History and state | `~/.config/kakemono/state.json` | `$XDG_CONFIG_HOME/kakemono/state.json` | `%APPDATA%\kakemono\state.json` |
| Socket | `~/.config/kakemono/kakemono.sock` | `$XDG_RUNTIME_DIR/kakemono/kakemono.sock` | named pipe `\\.\pipe\kakemono-<user>` |
| Lock and PID files | `~/.config/kakemono/` | `$XDG_RUNTIME_DIR/kakemono/` | `%LOCALAPPDATA%\kakemono\run\` |
| Daemon logs | `~/.local/state/kakemono/` | `$XDG_STATE_HOME/kakemono/` | `%LOCALAPPDATA%\kakemono\logs\` |
| Thumbnails (app) | `~/.cache/kakemono/` | `$XDG_CACHE_HOME/kakemono/` | `%LOCALAPPDATA%\kakemono\cache\` |
| Browse previews (daemon, 24 h) | `~/.cache/kakemono/previews/` | `$XDG_CACHE_HOME/kakemono/previews/` | `%LOCALAPPDATA%\kakemono\cache\previews\` |

`XDG_CONFIG_HOME`, `XDG_STATE_HOME`, and `XDG_CACHE_HOME` default to `~/.config`, `~/.local/state`, and `~/.cache`, on macOS too. Without `XDG_RUNTIME_DIR`, Linux puts the socket at `~/.config/kakemono/kakemono.sock` and the lock and PID files in the config folder. `$KAKEMONO_SOCKET` overrides the default socket.

The background daemon writes one log file per day, `kakemono.<date>.log`, and keeps the newest 7. `KAKEMONO_LOG` sets the level in `tracing` filter syntax, for example `KAKEMONO_LOG=debug`.

## Talk to the daemon over the socket

The daemon reads one JSON request per line and answers with one JSON line:

```sh
echo '{"cmd":"status"}' | nc -U ~/.config/kakemono/kakemono.sock
echo '{"cmd":"set","args":{"path":"/tmp/a.jpg"}}' | nc -U ~/.config/kakemono/kakemono.sock
```

A reply is `{"ok":true,"data":{...}}` or `{"ok":false,"error":"..."}`. The commands are `next`, `prev`, `set` (needs `args.path`), `favorite`, `trash`, `pause`, `resume`, `status`, `reload`, `subscribe`, `version`, and `doctor`. `reload` reads the config again without a restart. Changes to `socket_path` and `[desktop]` still need a restart.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd crates/ui/frontend && npm ci && npm test && npm run build
```

`scripts/gate.sh full` runs the Rust checks that CI runs, plus cross-target clippy. To run CI's Linux and Windows jobs from a Mac before you push:

| Script | What it runs | Needs |
|---|---|---|
| `scripts/linux-check.sh` | The Rust tests, the frontend checks, and a `.deb` build, in an Ubuntu 24.04 container | Docker |
| `scripts/windows-check.sh` | Clippy and the Rust tests in a Windows 11 ARM64 VM | QEMU (`brew install qemu`) and about 60 GB of disk |

To create the Windows VM, download the Windows 11 ARM64 ISO from [Microsoft](https://www.microsoft.com/software-download/windows11arm64), then run `scripts/windows-vm/vm.sh install <iso>` and `scripts/windows-vm/vm.sh setup`. Both run unattended and take about an hour in total. The VM is in `~/VMs/kakemono-win`. To remove it, run `vm.sh destroy`. Run either script with `--help` to see its options.

| Crate | Contents |
|---|---|
| `crates/cli` | The `kakemono` binary: commands and the daemon host |
| `crates/engine` | Rotation, history, favorites, trash, and hooks |
| `crates/sources` | The wallpaper sources and downloads |
| `crates/platform` | Wallpaper setters, theme and display signals, login services, and Linux backend detection |
| `crates/config` | The config schema, defaults, paths, and editing |
| `crates/protocol` | The socket protocol and client |
| `crates/ui` | The Tauri app: tray, picker, and settings |

## License

MIT. See [LICENSE](LICENSE).
