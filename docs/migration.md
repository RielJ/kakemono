# Move from wallpaperd to kakemono

kakemono replaces the Go `wallpaperd`. It reads your wallpaperd config and history, so you can switch without setting anything up again. This guide covers the steps for macOS. On Linux, wallpaperd didn't run, so start at [Install kakemono](../README.md#install).

## What carries over

- **Config.** If `~/.config/kakemono/config.toml` doesn't exist, the first daemon start (or the first time the settings window opens) converts `~/.config/wallpaperd/config.toml` (under `$XDG_CONFIG_HOME` if set) and writes the result to `~/.config/kakemono/config.toml`. kakemono never changes the wallpaperd file, so it stays as your backup. See [Sources list (2026-07)](#sources-list-2026-07) for what the conversion does.
- **History and state.** On the first daemon start, if `~/.config/kakemono/state.json` doesn't exist, kakemono copies `~/.config/wallpaperd/state.json` to it: history, the current wallpaper, and the paused flag. kakemono never changes the wallpaperd file.
- **Wallpaper folders.** The cache, favorites, and trash folders come from the config (`download_dir`, `favorites.dir`, `trash_dir`), so kakemono uses the same folders as wallpaperd.
- **Commands.** `next`, `prev`, `set`, `favorite`, `trash`, `pause`, `resume`, `status`, `status --json`, `daemon`, `daemon --foreground`, `open`, `open --folder`, `open --favorites`, `pick`, and `settings` work as before. So do the `--config` and `--socket` flags. Replace `wallpaperd` with `kakemono` in your scripts.
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
- sets the daemon's `PATH` to `/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`, so hooks find Homebrew tools;
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

You don't need to copy anything. When `~/.config/kakemono/config.toml` doesn't exist, the daemon writes it from the wallpaperd config on its first start. The daemon log shows a line like this one:

```text
migrated ~/.config/wallpaperd/config.toml to ~/.config/kakemono/config.toml; the old file is left as is
```

The converted file has no `socket_path` when the wallpaperd file used the wallpaperd default socket, so kakemono uses its own socket. You can delete the `[sketchybar]` section by hand. kakemono ignores it.

To start from the commented default config instead, run `kakemono config init` before the first daemon start. It writes `~/.config/kakemono/config.toml` and refuses to overwrite an existing file. `kakemono config edit` opens the config in `$VISUAL` or `$EDITOR`.

## Sources list (2026-07)

kakemono replaced the per-provider switches and the `[light]` and `[dark]` lists with one list of sources, as in Variety. Each `[[sources]]` row has its own on/off switch, a type, and a location:

```toml
[[sources]]
id = "wh-2051"
enabled = true
type = "wallhaven"
location = "landscape"
```

An older config converts the first time the daemon starts or the settings window opens. kakemono also converts it in memory on every load, so an older file always works, even when it can't be written.

### What the conversion does

- Rows are added in this order: Bing, Wallhaven, Unsplash, Reddit, APOD, then local folders.
- An enabled provider gives enabled rows. A disabled provider that is listed in `[light].sources` or `[dark].sources` gives disabled rows. A disabled provider that isn't listed gives no rows.
- Wallhaven and Unsplash give one row per distinct query: the `[light]` queries first, then the `[dark]` queries. With no queries, they give one row with an empty location.
- Reddit gives one row per subreddit in `[reddit] subreddits`.
- Each folder in `[local] dirs` becomes a `folder` row. Paths in your home folder are written with `~`.
- `[light]`, `[dark]`, `[daemon] cache_size`, each provider's `enabled`, `[reddit] subreddits`, and `[local] dirs` and `recursive` are removed. Comments elsewhere in the file are kept.

Behaviour that changes:

- **Folder sources always include subfolders.** If you had `[local] recursive = false`, the log notes this. To use only the top-level images, add the images as an `images` source instead.
- **Light and dark lists are gone.** All enabled sources are used in both modes. To keep dark images in dark mode, set the brightness filter to `system` ("Match system appearance" in settings):

  ```toml
  [filters]
  brightness = "system"
  ```

- **The download folder is limited by size, not by file count.** `cache_size` is replaced by `download_limit_mb` (500 MB by default). Favorites, the current wallpaper, and files in local sources are never deleted.
- **Each online source downloads into its own folder**, `download_dir/<source id>/`. A wallpaper change picks from images that are already on disk, so it works offline.

### Where the backup is

- **Converted from wallpaperd.** `~/.config/wallpaperd/config.toml` is the backup. kakemono never writes to it.
- **Converted in place.** An older `~/.config/kakemono/config.toml` (or a file passed with `--config` or `$KAKEMONO_CONFIG`) is rewritten, and the original bytes are kept next to it as `config.toml.bak`. If a different `config.toml.bak` already exists, the backup is named `config.toml.bak.<unix time>`.

### Restore the old config

1. Stop the daemon:

   ```sh
   launchctl bootout "gui/$(id -u)/dev.rielj.kakemono.daemon"
   ```

2. Put the old file back. For an in-place conversion:

   ```sh
   mv ~/.config/kakemono/config.toml.bak ~/.config/kakemono/config.toml
   ```

   For a conversion from wallpaperd, delete the new file:

   ```sh
   rm ~/.config/kakemono/config.toml
   ```

3. Start an older kakemono build. The current build converts the file again on its next start.

## Update the sketchybar item

wallpaperd's `[sketchybar]` section never fired the `wallpaper_changed` event, so the bar item updated only when sketchybar ran the plugin. kakemono runs a command after every wallpaper change instead. To fire the event, add this to the config that `kakemono config path` prints:

```toml
[hooks]
on_change = ["sketchybar", "--trigger", "wallpaper_changed"]
```

The service that `kakemono service install` creates puts Homebrew's `bin` directories on the daemon's `PATH`, so a bare `sketchybar` works. A plist written by an earlier kakemono version has no `PATH`; run `kakemono service install` again to rewrite it. If you start the daemon another way (for example `kakemono daemon -f` from a script or another launcher), use the full path that `command -v sketchybar` prints. After you edit the config, restart the daemon with `launchctl kickstart -k "gui/$(id -u)/dev.rielj.kakemono.daemon"`.

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
- New commands: `detect`, `doctor`, `service`, `config`, and `completions`. The [README](../README.md#commands) lists them.

## Appendix: Parity with the Go code

The Go code (`cmd/`, `internal/`, `gui/`, `picker/`) was deleted after this check. The tag `go-final` points at the last commit that has it.

### Commands

Every wallpaperd cobra command and flag has a kakemono equivalent.

| wallpaperd command and flags | kakemono command |
|---|---|
| root: `--config` (default `~/.config/wallpaperd/config.toml`), `--socket` | `--config` (falls back to the wallpaperd file), `--socket` |
| `daemon [-f\|--foreground]` | `daemon [-f\|--foreground]` |
| `next` | `next` |
| `prev` | `prev` |
| `set <image-path>` | `set <IMAGE-PATH>` |
| `favorite` | `favorite` |
| `trash` | `trash` |
| `pause` | `pause` |
| `resume` | `resume` |
| `status [--json]` | `status [--json]` |
| `open [--folder] [--favorites]` | `open [--folder] [--favorites]` |
| `pick` (launched `wallpaperd-picker.app`) | `pick` (opens `kakemono.app`) |
| `settings` (launched `wallpaperd-settings`) | `settings` (opens `kakemono.app`) |

The socket commands `next`, `prev`, `set`, `favorite`, `trash`, `pause`, `resume`, and `status` keep the same names and arguments (`crates/protocol`).

### Tests

The Go suite had 56 test functions: 38 ported, 18 covered by a broader Rust test, 0 dropped. Rust test names are `<file>::<test>`. Files are under `crates/<crate>/tests/`.

| Go test | Status | Rust test |
|---|---|---|
| `config.TestDefaults_Interval` | covered | `config compat.rs::default_values_spelled_out`, `compat.rs::defaults_match_go_defaults` |
| `config.TestDefaults_CacheSize` | covered | `config compat.rs::default_values_spelled_out` |
| `config.TestDefaults_SocketPath` | covered | `config compat.rs::defaults_match_go_defaults` (kakemono leaves `socket_path` unset on purpose), `paths.rs::effective_socket_path::unset_socket_uses_default_for_every_source` |
| `config.TestDefaults_WallhavenPurity` | covered | `config compat.rs::defaults_match_go_defaults` (whole struct equals Go's `Defaults()` golden) |
| `config.TestDefaults_WallhavenCategories` | covered | `config compat.rs::defaults_match_go_defaults` |
| `config.TestDefaults_APODKey` | covered | `config compat.rs::default_values_spelled_out` |
| `config.TestDefaults_Reddit` | covered | `config compat.rs::defaults_match_go_defaults` |
| `config.TestLoad_ValidFile` | covered | `config compat.rs::missing_keys_in_present_section_keep_section_defaults`, `compat.rs::go_template_loads_like_go` |
| `config.TestLoad_MissingFile_ReturnsError` | ported | `config compat.rs::load_missing_file` |
| `config.TestLoad_InvalidTOML` | ported | `config compat.rs::bad_values_are_errors`, `compat.rs::load_or_default_propagates_parse_errors` |
| `config.TestLoad_ExpandsTilde` | ported | `config compat.rs::tilde_paths_are_expanded` |
| `config.TestLoadOrDefault_MissingFile` | ported | `config compat.rs::load_missing_file` |
| `config.TestDefaultConfigPath_Default` | covered | `config paths.rs::macos_layout`, `paths.rs::locate_prefers_new_then_legacy` (the wallpaperd path is now the legacy fallback) |
| `config.TestDefaultConfigPath_XDG` | covered | `config paths.rs::linux_layout_honours_xdg` (checks the legacy `wallpaperd/config.toml` under `XDG_CONFIG_HOME`) |
| `config.TestInitConfig_CreatesFile` | ported | `config paths.rs::init_writes_template_and_refuses_overwrite` |
| `config.TestInitConfig_CreatesParentDirs` | covered | `config paths.rs::init_writes_template_and_refuses_overwrite` (writes into a missing directory) |
| `config.TestInitConfig_RefusesOverwrite` | ported | `config paths.rs::init_writes_template_and_refuses_overwrite` |
| `config.TestConfig_AllFieldsAccessible` | covered | Compile-time field check; the Rust compiler enforces it, and `config compat.rs::default_values_spelled_out` reads the fields |
| `bing.TestBingFetch_ParsesJSON` | ported | `sources bing.rs::parses_json` |
| `bing.TestBingFetch_HTTPError` | ported | `sources bing.rs::http_error` |
| `bing.TestBingFetch_EmptyImages` | ported | `sources bing.rs::empty_images` |
| `bing.TestBingProvider_Name` | ported | `sources bing.rs::name_and_enabled_come_from_config` |
| `bing.TestBingProvider_Enabled` | ported | `sources bing.rs::name_and_enabled_come_from_config` |
| `bing.TestBingFetch_UHDResolution` | ported | `sources bing.rs::uhd_resolution` |
| `reddit.TestReddit_FiltersNonImages` | ported | `sources reddit.rs::filters_non_images` |
| `reddit.TestReddit_ImgurConversion` | ported | `sources reddit.rs::imgur_conversion` |
| `reddit.TestReddit_Source` | ported | `sources reddit.rs::source_and_permalink` |
| `reddit.TestReddit_HTTPError` | ported | `sources reddit.rs::http_error` |
| `reddit.TestReddit_EmptyResults` | ported | `sources reddit.rs::empty_results` |
| `reddit.TestRedditName` | ported | `sources reddit.rs::name_is_reddit` |
| `reddit.TestRedditEnabled` | ported | `sources reddit.rs::enabled_requires_subreddits` |
| `reddit.TestReddit_PermalinkURL` | ported | `sources reddit.rs::source_and_permalink` |
| `reddit.TestReddit_WebpIncluded` | ported | `sources reddit.rs::webp_included` |
| `wallhaven.TestWallhavenAPI_ParsesResponse` | ported | `sources wallhaven_api.rs::api_parses_response` |
| `wallhaven.TestWallhavenAPI_EmptyData` | ported | `sources wallhaven_api.rs::api_empty_data` |
| `wallhaven.TestWallhavenAPI_HTTPError` | ported | `sources wallhaven_api.rs::api_http_error` |
| `wallhaven.TestWallhavenScrape_FindsImages` | ported | `sources wallhaven_scrape.rs::scrape_finds_images` |
| `wallhaven.TestWallhavenScrape_NoThumbs` | ported | `sources wallhaven_scrape.rs::scrape_no_thumbs` |
| `wallhaven.TestWallhavenName` | ported | `sources wallhaven_api.rs::name_and_enabled` |
| `wallhaven.TestWallhavenEnabled` | ported | `sources wallhaven_api.rs::name_and_enabled` |
| `wallhaven.TestWallhavenAPI_QueryParam` | ported | `sources wallhaven_api.rs::api_query_param` |
| `wallpaper.TestManagerFavorite_CopiesFile` | ported | `engine files.rs::files::favorite_copies_the_current_file` |
| `wallpaper.TestManagerPauseResume` | ported | `engine engine_core.rs::engine_core::pause_resume_persist_and_emit_on_change` |
| `wallpaper.TestManagerStatus` | covered | `engine engine_core.rs::engine_core::engine_starts_from_loaded_state`, `engine_core::status_of_fresh_engine` |
| `wallpaper.TestManagerPrev_EmptyHistory` | ported | `engine engine_core.rs::engine_core::prev_on_empty_history_errors` |
| `wallpaper.TestManagerStatePush_IncreasesHistory` | covered | `engine engine_core.rs::engine_core::next_downloads_sets_and_pushes_history`, `state.rs::state_history_navigation` |
| `wallpaper.TestResolveLocalPath_AbsolutePath` | covered | `engine engine_core.rs::engine_core::next_uses_local_absolute_path_without_download` |
| `wallpaper.TestResolveLocalPath_FileURL` | covered | `engine engine_core.rs::engine_core::next_accepts_file_url` |
| `wallpaper.TestResolveLocalPath_MissingFile` | covered | `engine engine_core.rs::engine_core::next_with_missing_local_file_fails_with_go_message` |
| `wallpaper.TestManagerSaveState` | covered | `engine engine_core.rs::engine_core::engine_starts_from_loaded_state` (reloads the state the engine saved), `state.rs::state_persistence` |
| `wallpaper.TestPush_MaxSize` | ported | `engine state.rs::state_push_max_size` |
| `wallpaper.TestHistoryNavigation` | ported | `engine state.rs::state_history_navigation` |
| `wallpaper.TestStatePersistence` | ported | `engine state.rs::state_persistence` |
| `wallpaper.TestLoadState_MissingFile` | ported | `engine state.rs::state_load_missing_file` |
| `wallpaper.TestPush_TruncatesForwardHistory` | ported | `engine state.rs::state_push_truncates_forward_history` |
| `wallpaper.TestSave_CreatesDirectory` | ported | `engine state.rs::state_save_creates_directory` |
