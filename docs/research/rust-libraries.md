# Rust library research for the wallpaperd rewrite

Date: 2026-11-09. Versions and dates come from crates.io and GitHub API reads made during research. Items marked UNVERIFIED could not be confirmed; the note says where the researcher looked.

## Summary

| # | Category | Recommendation | Runner-up | Key reason | Evidence |
|---|---|---|---|---|---|
| 1 | GUI + tray | Tauri 2.12.5 (3.0.0-alpha.6 exists; stay on 2.x), with daemon/CLI in a separate GUI-free binary | raw `wry` + `tao` + `tray-icon` (+ `ksni` backend) | Reuses the React/Vite frontends, has the plugin set, same WebView model as Wails. Linux tray and Wayland positioning need workarounds. | [tray-icon #336](https://github.com/tauri-apps/tray-icon/issues/336), [tao #1315](https://github.com/tauri-apps/tao/issues/1315), [Tauri Linux graphics](https://v2.tauri.app/develop/debug/linux-graphics/) |
| 2 | Wallpaper setting | Hand-rolled per-OS backends behind one trait | none (`wallpaper` and `more-wallpapers` rejected) | `wallpaper` shells out to `osascript` on macOS, has no per-monitor support, and neither crate covers Hyprland or niri. | [wallpaper macos.rs](https://docs.rs/crate/wallpaper/3.2.0/source/src/macos.rs), [more-wallpapers](https://github.com/LuckyTurtleDev/more-wallpapers) |
| 3 | Theme detect + watch | dark-light 3.0.0 (`tokio` feature) | own ashpd Settings portal + objc2 observer | Provides `detect`, `subscribe`, `stream`. macOS watching polls once per second. | [dark-light README](https://raw.githubusercontent.com/rust-dark-light/rust-dark-light/master/README.md) |
| 4 | Linux D-Bus | zbus 6.2.1 (`tokio` feature) + ashpd 0.14.0 for portals | dbus-rs 0.9.12 | Pure Rust and active. dbus-rs wraps the libdbus C library. | crates.io API |
| 5 | HTTP + scrape | reqwest 0.13.5 (rustls) + scraper 0.29.0 + hand-rolled 429/`Retry-After` handling | ureq 3.5.1 | Already on tokio. `reqwest-retry` ignores `Retry-After`. | [reqwest changelog](https://raw.githubusercontent.com/seanmonstar/reqwest/master/CHANGELOG.md), [reqwest-middleware #146](https://github.com/TrueLayer/reqwest-middleware/issues/146) |
| 6 | Async runtime | tokio 1.54.1 | smol | Tauri depends on `tokio ^1`; zbus has a `tokio` feature. | [docs.rs tauri](https://docs.rs/tauri/latest/tauri/tray/struct.TrayIconBuilder.html) |
| 7 | IPC | interprocess 2.4.8 (local sockets, `tokio` feature) | hand-rolled `tokio::net::UnixListener` + `tokio::net::windows::named_pipe` | One API for Unix sockets and Windows named pipes. Newline-delimited JSON is unchanged. | [interprocess README](https://raw.githubusercontent.com/kotauskas/interprocess/main/README.md) |
| 8 | Config/paths | toml 1.2.1 + serde; toml_edit 0.25.19 for GUI writes; etcetera 0.11 `choose_base_strategy` | `directories` 6.0.0 / `dirs` 7.0.0 | etcetera gives XDG paths (`~/.config`) on macOS and Linux and Known Folders on Windows. `directories`/`dirs` force `~/Library/Application Support` on macOS. | [etcetera README](https://raw.githubusercontent.com/lunacookies/etcetera/master/README.md) |
| 9 | CLI | clap 4.6.7 derive | bpaf / argh | Standard choice, completions; wallflow uses it too. | crates.io, [wallflow Cargo.toml](https://raw.githubusercontent.com/MKSG-MugunthKumar/wallflow/main/Cargo.toml) |
| 10 | Errors/logging | thiserror + anyhow; tracing + tracing-appender 0.2.5 | miette / color-eyre | Typed errors in libraries, anyhow in binaries. tracing-appender rotates daily and has `max_log_files`. | [tracing-appender Builder](https://docs.rs/tracing-appender/latest/tracing_appender/rolling/struct.Builder.html) |
| 11 | Autostart / single instance | `tauri-plugin-autostart` in the GUI; `auto-launch` 0.6.0 in the headless daemon; the daemon's IPC socket acts as the single-instance lock | `single-instance` 0.3.3 | The socket already exists, so no extra crate is needed. | crates.io |
| 12 | Testing | wiremock 0.6.5 + insta 1.49.0 + `tokio::time::pause` (`test-util`) | httpmock 0.9.0 (2026-10-20) | Async-native mocks. wiremock's last release was 2025-08-24, so httpmock is the more active option. | crates.io |
| 13 | Packaging | Tauri bundler + `tauri-plugin-updater` | cargo-packager (no RPM) | Same toolchain as the GUI. | [Tauri distribute](https://v2.tauri.app/distribute/), [cargo-packager README](https://raw.githubusercontent.com/crabnebula-dev/cargo-packager/main/README.md) |

## Top risks

1. **Linux tray reliability.** Tauri's tray uses libayatana-appindicator, which now logs a deprecation warning. tray-icon #336 (2026-07-04) reports that on KDE Plasma 6.7.1 Wayland the icon never appears, while `.build()` returns `Ok`, so the failure is silent. tray-icon #104 reports that click events do not fire on Linux. tray-icon now has a GTK-free `ksni` (StatusNotifierItem) backend, but tauri 2.12.5 lists `tray-icon ^0.25` while the latest is 0.26.1, so it is not clearly the default (UNVERIFIED). The `doctor` command should check that a StatusNotifierWatcher exists on the session bus. GNOME needs an AppIndicator extension.
2. **An edge-anchored picker on Wayland is not possible through Tauri.** Wayland toplevel windows cannot position themselves. Layer-shell must be set before the GTK window is realised, and Tauri/tao provide no hook for that (tao #1315; Tauri PR #15914 was closed without merging). The workaround is to move the WebView into your own GTK window, which has reported ghosting issues. GNOME has no wlr-layer-shell. Plan three tiers: a positioned window (macOS, Windows, X11); layer-shell via raw `wry` + `gtk-layer-shell` (wlroots, KDE); a centred or popover fallback on GNOME Wayland. Forcing XWayland with `GDK_BACKEND=x11` is a possible fourth route (UNVERIFIED).
3. **macOS limits.** `NSWorkspace setDesktopImageURL:forScreen:options:error:` sets one screen, on the current Space only. An Apple engineer wrote on 2026-06-14: "There is currently no API to allow setting desktop images for all spaces on a screen" ([thread 834630](https://developer.apple.com/forums/thread/834630)). The same thread reports that re-setting the wallpaper clears the user's "Show on all Spaces" setting. dark-light polls on macOS; a native observer needs hand-written objc2 `NSDistributedNotificationCenter` code. The exact objc2-app-kit 0.3.2 signature and `MainThreadMarker` requirements are UNVERIFIED.
4. **The Linux backend matrix changes often.** swww was renamed to `awww` and moved to Codeberg, so detect both binary names. hyprpaper 0.8.x is a rewrite (v0.8.5, 2026-11-01), with IPC via `hyprctl hyprpaper wallpaper …` (UNVERIFIED, from a search snippet). KDE per-screen scripting uses an unstable screen index. On GNOME, setting only `picture-uri` leaves dark mode unchanged, so also set `picture-uri-dark`. niri has no built-in wallpaper setter; users run swaybg or awww. more-wallpapers reports that the wallpaper resets after restart on X11 and Cinnamon. Mitigations: user-editable `set_command` templates, a `doctor` command, and test shims on `PATH`.
5. **The WebKitGTK stack and memory use.** Tauri's docs list blank windows and Wayland protocol errors on NVIDIA, with workarounds `WEBKIT_DISABLE_DMABUF_RENDERER=1` and `__NV_DISABLE_EXPLICIT_SYNC=1`. A Tauri 2.0 AppImage hit EGL failures on Hyprland. `tauri-plugin-global-shortcut` does not work on GNOME Wayland (plugins-workspace #3267), so do not promise global hotkeys on Wayland. One AI-assisted benchmark (August 2026) shows Tauri peaking at about 310–350 MB against about 105–165 MB for Slint, egui and iced; treat this as a trend, not a measurement. reqwest 0.13 uses aws-lc by default, which may cause cross-compilation friction (UNVERIFIED); `rustls-no-provider` with ring is the escape hatch.

## 1. GUI and tray

- **Tauri** 2.12.5 (docs.rs 2026-10-21); 3.0.0-alpha.6 (2026-11-06). **tray-icon** 0.26.1 (2026-10-07), with a new `ksni` feature. **muda** 0.21.2 (2026-10-08).
- **eframe** 0.37.0 (2026-10-15): rejected. No reuse of the web frontends, and no tray of its own.
- **iced** 0.14.0 (2025-12-07): rejected. No release in 11 months, and no frontend reuse.
- **Slint** 1.18.1 (2026-09-21): rejected for the same reason. License terms UNVERIFIED.
- **dioxus-desktop** 0.8.0-alpha.3 (2026-10-16): rejected. Alpha, and it has the same WebKitGTK dependency because it is WebView-based.
- **Tauri plugins:** autostart, single-instance, global-shortcut, updater and positioner all have 3.0.0-alpha.3 releases on crates.io, and 2.x stable lines exist. The positioner docs say it needs Rust 1.90 or later. The dialog and notification plugins were not checked.
- **Tray-only startup:** use an empty `windows` array and call `api.prevent_exit()` on `RunEvent::ExitRequested` ([discussion 5364](https://github.com/orgs/tauri-apps/discussions/5364)). Discussion 11489 shows friction with this in v2, so test it.
- **Wayland:** Tauri 2.6.0 added an `x11` cargo feature, so Wayland-only builds are possible ([release notes](https://v2.tauri.app/release/tauri/v2.6.0/)). The positioner has no documented Wayland support (UNVERIFIED). For layer-shell, raw `wry`'s GTK build path is the escape hatch.
- **Workspace split:** a `core` library, a `wallpaperd` CLI/daemon with no GUI dependencies, and a Tauri GUI binary. This keeps the daemon installable on headless or minimal Linux systems without GTK or WebKit.

## 2. Wallpaper setting

- **`wallpaper` crate.** The latest crates.io release is 3.2.0 (2021-07-09); the latest GitHub commit is "v4.0.0" (2022-11-28).
  - macOS: runs `osascript … tell every desktop to set picture`. `set_mode` is unsupported.
  - Windows: `SystemParametersInfoW` through `winapi`. No per-monitor support.
  - Linux: shells out to `gsettings`, `dconf`, KDE and XFCE tools by `XDG_CURRENT_DESKTOP`, then falls back to spawning `swaybg` or `feh`. No Hyprland, niri or awww support.
  - Rejected.
- **`more-wallpapers` 0.4.0** (2024-03-29, about 127 recent downloads). Per-screen support only on X11 (xwallpaper + libxrandr), Cinnamon, KDE, sway and XFCE. macOS and Windows go through the `wallpaper` crate as a fallback. Rejected.
- **Hand-rolled backends:**
  - **macOS:** `objc2-app-kit` 0.3.2 (2025-10-04). Call `NSWorkspace` once per `NSScreen`, with per-screen options. `NSScreen` probably needs a `MainThreadMarker` (UNVERIFIED).
  - **Windows:** `windows` 0.62.2 (2025-10-06). Call `IDesktopWallpaper::SetWallpaper(monitorID, path)` in `Win32::UI::Shell`; a NULL monitor ID sets all monitors, and `GetMonitorDevicePathAt` lists monitor IDs. Fall back to `SystemParametersInfoW` for all monitors at once.
  - **GNOME:** `gsettings set org.gnome.desktop.background picture-uri` and also `picture-uri-dark`. Calling libgio from Rust would add a native dependency.
  - **KDE Plasma:** a script run through `org.kde.PlasmaShell` (`desktops()`, then `writeConfig("Image", …)` for each desktop), sorted by screen position. `plasma-apply-wallpaperimage` is simpler, but there are reports of it failing silently on Plasma 6 (UNVERIFIED).
  - **XFCE:** xfconf over D-Bus (not verified).
  - **X11:** `x11rb` 0.14.0 (2026-07-16) or `feh`. Root-window wallpapers do not persist across restarts.
  - **Hyprland:** hyprpaper 0.8.5 (2026-11-01), set through `hyprctl hyprpaper wallpaper`.
  - **awww (formerly swww):** needs wlr-layer-shell and its daemon. Does not work on GNOME.
  - **swaybg:** one long-running process per output.
  - **niri:** uses swaybg or awww. The niri wiki describes running two wallpaper tools: one for the overview backdrop, one for the workspace background.
- **Portal Wallpaper interface** (`SetWallpaperURI`/`SetWallpaperFile`, with `show-preview` and `set-on` options). Implemented by xdg-desktop-portal-gtk, -kde and -gnome. xdg-desktop-portal-wlr implements only Screenshot and ScreenCast. Whether non-sandboxed apps get a confirmation prompt each time is UNVERIFIED; the docs say the portal "may decide to show a preview". Use it as the last-resort backend for Flatpak builds, not as a generic backend.
- **Desktop-environment detection:** the `detect-desktop-environment` crate exists, but its coverage of Sway, niri and XFCE and its release date are UNVERIFIED. Recommendation: write a small enum of our own that checks `XDG_CURRENT_DESKTOP` first, then compositor environment variables.

## 3. Theme

- **dark-light 3.0.0** (2026-08-12). Features: `async-io` (default) and `tokio`. API: `detect()`, `subscribe()` (returns a `Watcher`) and `stream()`.
- **Linux:** dark-light reads the portal `color-scheme` setting. Whether it falls back to gsettings is UNVERIFIED.
- **macOS:** dark-light polls once per second, because objc2 does not expose `NSDistributedNotificationCenter` safely.
- **Windows:** the watch mechanism is UNVERIFIED. The expected options are `RegNotifyChangeKeyValue` or `UISettings.ColorValuesChanged`.
- **Own ashpd route:** read the Settings portal `color-scheme` value and subscribe to its change signal.
- **Tauri theme events** need an open window, so the headless daemon cannot use them.

## 4. Linux D-Bus

- **zbus** 6.2.1 (2026-11-01); the repository is now `z-galaxy/zbus`. Use it with `default-features = false, features = ["tokio"]`.
- **ashpd** 0.14.0 (2026-11-05).
- **dbus-rs** 0.9.12 (2026-07-03): active, but needs libdbus.
- **ksni** 0.3.7 (2026-10-20): StatusNotifierItem tray support.

## 5. HTTP and scraping

- **reqwest 0.13** made rustls the default TLS backend, with aws-lc as the crypto provider. The `rustls-tls` feature was renamed to `rustls`. Use `default-features = false, features = ["rustls","json","gzip"]`.
- **ureq** 3.5.1 (2026-11-02): a good blocking client, but the app is already on tokio.
- **scraper** 0.29.0 (2026-10-21): html5ever plus CSS selectors. The alternatives were rejected from prior knowledge, not checked during this research: `tl` has limited selector support, `lol_html` is a streaming rewriter rather than a query library, and raw html5ever means writing your own selector engine.
- **Retry:** reqwest-retry 0.9.1 (2026-02-05) does not handle `Retry-After` (issue #146). Hand-roll about 40 lines: handle 429 with `Retry-After`, with backoff tracked per source.

## 6. Async runtime

tokio 1.54.1 (2026-10-21). Enable `test-util` in tests to get `tokio::time::pause`. Tauri, interprocess and reqwest all integrate with tokio, so switching to smol isn't worth it.

## 7. IPC

- **interprocess** 2.4.8 (2026-11-08). Its README badge says "maintenance: passive".
- Local sockets use Unix domain sockets on Unix and named pipes on Windows.
- Confirm the `tokio` feature on crates.io before depending on it.
- Keep the wire format as newline-delimited JSON, read with `BufReader::lines()`.

## 8. Config and paths

- **toml** 1.2.1 and **toml_edit** 0.25.19 are released together. toml_edit preserves comments, so use it when the GUI or auto-detection writes to the config.
- **etcetera** 0.11.0 (2025-10-28). `choose_base_strategy` uses XDG paths everywhere except Windows, so on macOS the config stays at `~/.config/wallpaperd`. The master README mentions 0.12 with MSRV 1.95, which is not on crates.io yet.
- **directories** 6.0.0 and **dirs** 7.0.0 follow each platform's native conventions instead. Tauri itself depends on `dirs ^7`.
- **figment** and **config-rs** were not examined. Layered configuration is unnecessary for a single TOML file.

## 9–11. CLI, errors and logging, autostart

- **clap** 4.6.7 (2026-09-14). wallflow, a Rust wallpaper tool, uses almost the same stack: clap derive, anyhow, tracing, tracing-appender, dirs, tokio and reqwest with rustls.
- **tracing-appender:** rotate log files by time, and use `Builder::max_log_files` for retention. Pick each OS's log directory explicitly (`~/Library/Logs`, `%LOCALAPPDATA%`, XDG state).
- **auto-launch** 0.6.0 (2026-01-10). `tauri-plugin-autostart` wraps it.

## 12. Testing

- **Versions:** wiremock 0.6.5 (2025-08-24), httpmock 0.9.0 (2026-10-20), mockito 1.7.2 (2026-02-02), insta 1.49.0 (2026-10-03).
- **Platform backends:** put a `CommandRunner` trait between the backend and `std::process::Command`, and snapshot the argv it receives with insta.
- **Fake tools:** put fake `gsettings`, `hyprctl` and `swww`/`awww` scripts on `PATH` in tests.
- **CI:** run a matrix on macOS, Windows and Ubuntu.

## 13. Packaging

- **Tauri bundler:** macOS `.app` and `.dmg`; Linux deb, rpm, AppImage, Snap, Flatpak and AUR; Windows MSI and NSIS.
- **macOS signing:** needs an Apple Developer account ($99 per year) and a Mac.
- **Windows signing:** certificate requirements and SmartScreen behaviour UNVERIFIED.
- **cargo-packager:** supports dmg, `.app`, deb, AppImage, pacman, NSIS and MSI, but not rpm.
- **cargo-dist:** how well it fits GUI app bundles is UNVERIFIED.

## Prior art

| Project | Stack | Lesson |
|---|---|---|
| wallflow (Rust, v0.6.1, MIT) | clap, tokio, reqwest-rustls, tracing-appender, dirs; shells out to awww or `feh`/nitrogen | Nearly the same sources as ours (Wallhaven, Unsplash, Reddit, APOD, Bing). Supports wlroots, KDE and X11 only: "Sorry, GNOME". |
| wayper (Python) | CLI + GUI + tray | Runs on Windows, macOS and Linux, but its Linux support covers only Hyprland and Sway. Closing the GUI leaves the tray running and rotation continues. |
| swww / awww (Rust) | wlr-layer-shell daemon | Renamed to awww on 2025-10-29 and moved to Codeberg. Does not work on GNOME. |
