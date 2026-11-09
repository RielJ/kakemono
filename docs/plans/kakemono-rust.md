# kakemono: Rust, cross-platform rewrite of wallpaperd

> **Provenance.** This plan merges a multi-model planning run (`~/.pi/multi-runs/20261109-174615-plan/`). Plan A (Opus 5.5) is the base. Plans B (GPT 5.6 Sol) and C (GLM 5.2) contributed the changes listed under *Synthesis decisions*. Plan D (Opus 5) produced no output.

## Synthesis decisions

The user chose these:

| Decision | Choice | Source |
| --- | --- | --- |
| App name | **kakemono** (renamed from `wallpaperd`, which is too close to the existing Rust Wayland daemon `wpaperd`). Binary `kakemono`, crates `kakemono-*`, `~/.config/kakemono/`, app id `dev.rielj.kakemono`, env prefix `KAKEMONO_`. The Go config path `~/.config/wallpaperd/config.toml` is read as a legacy fallback and never moved automatically. Throughout this plan, read `kakemono` as `kakemono` wherever it is user-facing. | user |
| macOS Space switching | Re-applying on Space change is **opt-in**: `[daemon] reapply_on_space_change = false` by default. By default only the current Space changes, as in Go today. | B's warning, A/C's mechanism |
| Wayland picker (v1) | Centred window, documented compositor window rules, opt-in `force_x11`, and a time-boxed raw-wry layer-shell spike that ships nothing | A |
| Picker placement | The user can **drag** the picker anywhere on every OS. The position is remembered on macOS, Windows, X11, Hyprland and sway (see §H). | User request |
| Workspace layout | 7 crates: protocol / config / sources / platform / engine / cli / ui | A |
| Distribution | Tauri bundler (daemon as a sidecar) plus a plain GitHub Actions matrix for CLI-only archives. No signing in v1. | A |

Where the plans disagreed on a technical point, this plan takes A's position because A had evidence for it:

| Point | Taken | Rejected, and why |
| --- | --- | --- |
| Single instance | `File::try_lock` on `daemon.lock`, then bind the socket | B/C bind the socket as the lock. Two instances starting at once (for example launchd plus a manual start) can both unlink a stale socket and bind. |
| Interval parsing | Our own Go-duration parser (about 40 LOC) | B's `humantime` rejects `1h30m0s` and `1.5h`, which would break existing configs. |
| Linux autostart | Our own systemd user unit plus printed compositor snippets | `auto-launch` writes XDG `.desktop` files, which Hyprland, sway and niri ignore. |
| gsettings | Run the `gsettings` argv | B's direct D-Bus approach: writing GVariants through dconf is fragile, and `gio` would link glib into the daemon. (A and C agree.) |
| `Clock` trait | None. Use tokio paused time plus `Deps.now`. | B's trait duplicates what `tokio::time::pause` already provides. |
| Thumbnails | `thumb://` protocol with opaque ids and an allowlist | Go's `/local/<path>` handler could read any file on disk. B and C didn't address this. |
| TS bindings | ts-rs, checked in, and CI fails if they're stale | B and C had none, and the handwritten Go/TS types have already drifted. |

These came from B and C and were merged in:

- **Hooks (B):** at most 4 hooks run at once. Extra hook runs for the same event are dropped and logged. Hooks run with a cleared environment that holds only `PATH`, `HOME`, the session variables (`DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`) and `WALLPAPERD_*`. See §I and Task 22.
- **macOS spike first (C):** Task 24 starts with a 30-line `examples/spike_set.rs` that sets a wallpaper from a plain binary. The daemon is only built on top of it after the spike works.
- **Go-captured IPC goldens (C):** in addition to A's marshalled goldens, Task 1 also replays every command against a running Go daemon and records the wire bytes.
- **Tray health (B):** whether the tray is visible never counts toward daemon health. `doctor` reports it separately.
- **Non-goal (B):** moving API keys into the OS keychain. They stay in `config.toml` (0600) for v1.

## Summary

Rewrite `wallpaperd` (Go: cobra CLI and daemon, about 3.3k LOC in `cmd/` and `internal/`; plus two Wails apps in `gui/` and `picker/` with about 2.1k LOC of React/TS) as a Cargo workspace. The new version runs on macOS, Linux and Windows and needs no helper apps: no `osascript`, `defaults`, `open` or sketchybar.

- **Daemon and CLI.** A headless binary called `kakemono` hosts the engine and the CLI. It has no GUI dependencies.
  - The engine covers rotation, history, favorites, trash, cache, theme awareness and hooks.
  - It talks to the OS through a small platform seam: set the wallpaper, report the theme, and emit system signals.
- **Tray, settings and picker.** A separate, optional Tauri 2 app called `kakemono-ui` provides these. It reuses the existing React frontends, merged into one Vite multi-page app, and is a pure IPC client of the daemon.
- **Linux.** We do not draw the wallpaper ourselves. At first run (and on demand through `kakemono detect` and `kakemono doctor`) the daemon works out which desktop environment or compositor is running and which wallpaper tool is available. It saves the chosen backend and an argv `set_command` to `config.toml`, and the user can edit or pin that choice.
- **Compatibility.** `config.toml`, `state.json`, the newline-JSON IPC protocol and the CLI surface (including `status --json`, which the sketchybar scripts use) stay compatible. Existing setups keep working.
- **Hooks.** A user-defined `[hooks] on_change` command replaces the sketchybar coupling. The Go README claims the daemon triggers `wallpaper_changed`, but the Go code never does this, so this is effectively a new feature.

## Architecture

### A. Workspace layout and crate boundaries

```
Cargo.toml                    # [workspace] + [workspace.dependencies] (all versions pinned here)
rust-toolchain.toml           # stable >= 1.90 (std File::try_lock is 1.89; tauri-plugin-positioner needs 1.90)
deny.toml                     # cargo-deny: licenses (block GPL; see variety-reference risk), advisories
.github/workflows/ci.yml
crates/
  protocol/   kakemono-protocol   IPC wire types + codec + client + endpoint naming
  config/     kakemono-config     schema, defaults, load (serde/toml), edit (toml_edit), paths (etcetera), Go-duration parser
  sources/    kakemono-sources    Provider trait, http/retry/download helpers, 6 providers, registry
  platform/   kakemono-platform   seam traits + fake, exec (argv templating), host (main-thread ownership),
                                    macos/, windows/, linux/{detect,backends,…}, open (url/reveal), service install
  engine/     kakemono-engine     State (history), Engine (ops), rotation loop, Clock, files (fav/trash/cache), hooks, events
  cli/        kakemono (bin)        clap CLI + daemon host (IPC server, instance lock, logging)
  ui/src-tauri kakemono-ui (bin)  Tauri app: tray, settings window, picker window, thumb:// protocol
  ui/frontend                       Vite multi-page React app (settings.html, picker.html, shared/)
contrib/sketchybar/  contrib/waybar/   example integrations (moved from ./sketchybar)
docs/linux-backends.md docs/migration.md
```

Dependency direction: `protocol` ← `config` ← `sources` ← `engine` → `platform`; `cli` depends on all of these. `ui` depends only on `protocol` and `config`, so the UI never compiles reqwest, scraper or zbus. `platform` depends on `config` only for the `DesktopConfig` type; it must not depend on `engine`.

**Deletion test, applied to each crate or layer:**

| Layer | Delete it and… | Verdict |
|---|---|---|
| `protocol` | the CLI, daemon and UI each redefine the wire types and drift (Go already has this: `gui/app.go` duplicates `StatusDTO`) | keep |
| `config` | the UI would have to depend on `engine` to edit settings, pulling in reqwest and zbus | keep |
| `sources` | the providers fold into `engine`. We would lose the clear `Provider` seam (6 adapters, a real seam) and the per-file ownership that lets agents work in parallel | keep (cheap) |
| `platform` | `cfg` branches leak into the engine and nothing can be faked | keep |
| A separate `core` crate merging engine, sources and config | the UI pulls in the network stack | reject the merge |
| A `Store`/`StateRepository` trait | only one adapter (a JSON file) exists, so the trait is a hypothetical seam | delete; `engine::state` is a plain module |
| An `HttpClient` trait | only one real adapter; tests use real HTTP against wiremock with injected base URLs | delete |
| An `IpcTransport` trait | interprocess already unifies Unix sockets and named pipes | delete |
| A `Clock` trait | two adapters (system clock, and the tokio paused clock in tests). In fact `tokio::time::pause` covers it, so no trait is needed: use `tokio::time::Instant`/`sleep` and wall-clock `chrono::Utc::now()` passed in through `Deps.now: fn() -> DateTime<Utc>` | no trait, one function pointer |
| `CommandRunner` (the research suggests it) | real adapters: `tokio::process`, plus the recording fake used in insta tests of argv generation for every Linux preset | keep it, but only inside `platform::exec` |

### B. The platform seam (deep module, small interface)

```rust
// crates/platform/src/api.rs  — the only things the engine sees
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")] pub enum Theme { Light, Dark }
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")] pub enum Fit { #[default] Fill, Fit, Center, Stretch, Tile }
#[derive(Clone, Debug)] pub enum Signal { ThemeChanged(Theme), DisplaysChanged, SpaceChanged, Resumed }
#[derive(Clone, Debug, Serialize)] pub struct BackendInfo { pub name: String, pub detail: String }

#[async_trait::async_trait]
pub trait WallpaperSetter: Send + Sync + 'static {
    /// Set `image` on every display. Must be idempotent (the engine re-applies on display change and wake,
    /// and on Space change when `reapply_on_space_change` is on).
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError>;
    fn info(&self) -> BackendInfo;
}
pub trait SystemSignals: Send + Sync + 'static {
    fn theme(&self) -> Theme;
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Signal>;
}
pub struct Platform { pub setter: Arc<dyn WallpaperSetter>, pub signals: Arc<dyn SystemSignals> }

/// Owns the process main thread for the platform's needs (an NSApplication run loop on macOS,
/// a hidden message window on Windows, plain tokio on Linux) and runs `f` on a tokio runtime.
pub fn host<F, Fut>(cfg: &DesktopConfig, f: F) -> anyhow::Result<()>
where F: FnOnce(Platform) -> Fut + Send + 'static, Fut: Future<Output = anyhow::Result<()>> + Send;
```

- The seam is real, not hypothetical. `WallpaperSetter` has macOS, Windows, about 6 Linux native or preset backends, the user-command backend, and `fake::RecordingSetter`.
- Setting the wallpaper and reporting the theme are separate traits because Linux combines them independently: the theme comes from the portal, the wallpaper from awww.
- Per-monitor different images are not part of the interface (YAGNI; parity with Go). `Fit` is the only option.
- `host()` is the deep part. The main-thread rules of AppKit and the COM apartment rules of Windows sit behind one function, so `cli/src/daemon` stays OS-agnostic.

### C. Who hosts the engine: the headless daemon only

- **Decision.** `kakemono daemon` is the only host. `kakemono-ui` (tray, settings, picker) is an IPC client that spawns the daemon (bundled as a Tauri sidecar) if it isn't running.
- **Rejected: the GUI hosts the engine.** Linux users on sway, Hyprland or niri, and macOS users with sketchybar, want rotation without GTK, WebKit or a tray. Also, the Linux tray is unreliable: GNOME needs an extension, and tray-icon #336 fails silently on KDE Wayland. Rotation must not depend on it.
- **Rejected: both host, with single-instance locking.** This needs two hosting code paths and two lifecycles, makes engine ownership ambiguous during a UI crash, and fights over the main thread: on macOS both tao and our NSApplication observer want it.
- **Single instance.** The daemon takes `std::fs::File::try_lock` on `<runtime_dir>/daemon.lock`, then binds the socket.
  - I disagree with the research's "the socket acts as the lock". Connect-then-bind races when two instances start at once: launchd and a manual start can both see a stale socket, unlink it and bind.
  - The lock file is race-free, and the OS releases it on crash.
  - The UI uses `tauri-plugin-single-instance`, and second-launch argv (`--picker`, `--settings`) is forwarded to the running instance.

### D. Engine internals

```rust
pub struct Engine { inner: Arc<Inner> }            // cheap Clone, shared with IPC handlers
struct Inner {
    op: tokio::sync::Mutex<()>,                    // serialises mutating ops (fetch/set/trash)
    snap: std::sync::Mutex<Snapshot>,              // State + cfg + theme; status never waits on network (Go's mutex did)
    deps: Deps,                                    // Platform, Registry, http client, now()
    events: broadcast::Sender<EngineEvent>,        // Changed(WallpaperInfo) | Paused(bool) | ThemeChanged(Theme)
}
impl Engine {
  pub async fn next(&self) -> Result<WallpaperInfo, EngineError>;
  pub async fn prev(&self) -> Result<WallpaperInfo, EngineError>;
  pub async fn set_file(&self, p: &Path) -> Result<WallpaperInfo, EngineError>;
  pub async fn favorite(&self) -> Result<(), EngineError>;
  pub async fn trash(&self) -> Result<WallpaperInfo, EngineError>;
  pub fn pause(&self); pub fn resume(&self); pub fn status(&self) -> StatusData;
  pub async fn reload(&self, cfg: Config);
  pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent>;
  pub async fn run(&self, shutdown: CancellationToken);  // rotation + signal reactions
}
```

**Behaviour that is new or fixed compared with Go:**

- The rotation timer resets after a manual change.
- On startup, the first tick is computed from `state.last_changed`.
- On `ThemeChanged`, the engine rotates immediately from the new theme's sources. This is controlled by `[daemon] rotate_on_theme_change`, default true.
- On `DisplaysChanged` or `Resumed`, the engine re-applies the current image. On `Resumed` it also rotates if a tick is overdue.
- On `SpaceChanged`, the engine re-applies the current image **only if** `[daemon] reapply_on_space_change = true` (default false). Otherwise the signal is ignored, because re-setting clears the user's "Show on all Spaces" toggle.
- If a provider fetch fails, the engine tries the other sources in the list before giving up. Go failed on the first error.

### E. Linux discovery design

- **Inputs.** A pure function `resolve(&Env, &Probe) -> Resolution` is fed an `Env` snapshot gathered by `linux::detect::env::capture()`:
  - **Env vars:** `XDG_CURRENT_DESKTOP` (colon-separated, lowercased), `XDG_SESSION_DESKTOP`, `DESKTOP_SESSION`, `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, `DISPLAY`, `HYPRLAND_INSTANCE_SIGNATURE`, `SWAYSOCK`, `NIRI_SOCKET`, `KDE_FULL_SESSION`.
  - **Binaries on `PATH`** (via the `which` crate): `gsettings`, `xfconf-query`, `awww`, `swww`, `hyprctl`, `hyprpaper`, `swaybg`, `swaymsg`, `wbg`, `xwallpaper`, `feh`, `nitrogen`, `hsetroot`, `pcmanfm-qt`, `pcmanfm`.
  - **Running processes:** names read from `/proc/*/comm`, with no extra crate: `awww-daemon`, `swww-daemon`, `hyprpaper`, `swaybg`, `wpaperd`, `mpvpaper`.
  - **D-Bus names:** `org.kde.plasmashell`, `org.xfce.Xfconf`, `org.freedesktop.portal.Desktop` and `org.kde.StatusNotifierWatcher`, read via zbus `ListNames`.
  - `resolve` is pure, so it can be table-tested with insta and no system access.

**Resolution order** (first match wins):
1. If `[desktop] backend` is set to anything other than `"auto"`, use it as-is (pinned by the user).
2. **Full desktop environments**, since they own the desktop surface and external setters are hidden behind it:

   | Desktop | Backend |
   |---|---|
   | KDE | `kde` (D-Bus) |
   | GNOME, ubuntu, pop, unity, budgie | `gnome` |
   | X-Cinnamon, cinnamon | `cinnamon` |
   | MATE | `mate` |
   | XFCE | `xfce` |
   | LXQt | `pcmanfm-qt` preset |
   | LXDE | `pcmanfm` preset |
   | COSMIC | `command` with no preset; `doctor` explains |

3. **An already-running wallpaper daemon**, which the user has already chosen:

   | Running process | Backend |
   |---|---|
   | `awww-daemon` / `swww-daemon` | `awww` / `swww` |
   | `hyprpaper` | `hyprpaper` |
   | `wpaperd` | `command` with a `wpaperctl` hint |

4. **Compositor signature:**
   - `HYPRLAND_INSTANCE_SIGNATURE`: hyprpaper if installed, else awww or swww, else swaybg.
   - `SWAYSOCK`: `sway` (`swaymsg output * bg {path} fill`; sway supervises swaybg itself).
   - `NIRI_SOCKET` or another Wayland session: awww or swww if installed, else swaybg (we supervise it), else wbg.
5. **X11 without a known DE:** xwallpaper > feh > nitrogen > hsetroot.
6. **Nothing found:** `Unconfigured`. `set` returns an actionable error: "No wallpaper backend detected. Run `kakemono doctor`, or set `[desktop] set_command`."

**Persisted shape.** New, optional keys; Go's BurntSushi/toml ignores them, so a parity build can read the same file.

```toml
[desktop]
backend = "awww"          # auto | native | gnome | kde | xfce | cinnamon | mate | sway | hyprpaper | awww | swww | swaybg | wbg | xwallpaper | feh | nitrogen | hsetroot | pcmanfm | pcmanfm-qt | command
set_command = ["awww", "img", "{path}", "--transition-type", "fade"]   # argv; shown/edited for preset+command backends
persistent = false        # true = command stays running (swaybg-style); we replace the previous instance
fit = "fill"
theme = "auto"            # auto | light | dark (override when no portal, e.g. bare X11 WMs)
managed_by = "detect"     # "detect" = re-detect may overwrite; "user" = never touch
detected_session = "hyprland:wayland"
detected_at = "2026-11-09T17:46:15Z"
```

**Writes and re-detection:**
- **First run.** If `[desktop]` is missing, or `backend = "auto"` with no saved result, the daemon resolves a backend and writes `[desktop]` through `toml_edit`, keeping the user's comments. If the config is read-only, it logs a warning and uses the result in memory.
- **Session drift.** Some users log into GNOME one day and Hyprland the next. At startup, if `managed_by = "detect"` and the current session key (desktop, compositor and session type) differs from `detected_session`, the daemon re-detects.
- **Never clobber user edits.** The UI sets `managed_by = "user"` when the user edits the backend. On the CLI side, `detect --write` refuses to overwrite `managed_by = "user"` unless `--force` is given.

**Safe command execution** (`platform::exec`):
- `set_command` and `on_change` are always argv vectors, run with `tokio::process::Command` and no shell.
- A string value is accepted for convenience but split with `shell-words` at load time; it is still not run through a shell.
- **Placeholders** are substituted per token, never re-split: `{path}`, `{uri}` (a percent-encoded `file://` URI), `{fit}`, `{theme}`.
- **Paths are always absolute**, so they can't be mistaken for flags. Downloaded filenames are already sanitized; keep Go's `sanitizeFilename`.
- **Timeouts.** Non-persistent commands get a 15 s timeout, and stderr is captured into `SetError::Command { argv0, status, stderr }`.
- **Shell features** are opt-in and documented: `["sh","-c","my-script \"$1\"","_","{path}"]`.
- **KDE** passes the path into the Plasma JS through `serde_json::to_string(&uri)`, so it cannot inject code.

**Long-running setters** (swaybg, wbg, and any user command with `persistent = true`), handled by `linux::backends::persistent::Supervisor`:
- Spawn the new instance in its own process group (`process_group(0)`) so it survives daemon restarts.
- Wait 300 ms, then SIGTERM the previous PID to avoid a black flash.
- Record PIDs in `<runtime_dir>/setter.pid`.
- Before killing, check that `/proc/<pid>/comm` still matches argv0, to guard against PID reuse.
- On daemon start, adopt any existing recorded PID.

**Failure, fallback and re-detection:**
- After 2 consecutive `SetError`s with `managed_by = "detect"`, the daemon re-runs detection in memory. If the result differs, it retries once with the new backend and logs a suggestion to run `kakemono detect --write`.
- It never silently rewrites a config the user pinned.
- `kakemono doctor` prints the env snapshot, the probes, the resolution and its reasons, and the tray watcher's presence. It runs a dry-run argv render, and with `--apply` it test-sets the current image.

### F. Per-OS notes

**macOS** (objc2 0.6, objc2-foundation and objc2-app-kit 0.3.x, dispatch2, block2):
- **Setting the wallpaper.** For each `NSScreen::screens(mtm)`, call `NSWorkspace::sharedWorkspace().setDesktopImageURL_forScreen_options_error` with `NSWorkspaceDesktopImageScalingKey` and `AllowClippingKey` mapped from `Fit`.
  - These calls must run on the main thread. `MainThreadMarker` enforces this in objc2's types; the exact 0.3.2 signatures are UNVERIFIED, so Task 24 confirms them.
  - The setter sends a closure to the main queue (`DispatchQueue::main().exec_async`) and awaits a oneshot.
- **Host.** `host()` builds a multi-thread tokio runtime, runs `f` on it, and runs `NSApplication::sharedApplication(mtm)` with `ActivationPolicy::Prohibited`, so there is no Dock icon. `app.run()` executes on the real main thread.
  - On shutdown, tokio dispatches `NSApp.stop` plus a dummy event.
- **Spaces.** There is no API to set the wallpaper on all Spaces: Apple DTS said so on 2026-06-14 (thread 834630). By default only the current Space changes, which matches Go's `tell every desktop`.
  - The platform always observes `NSWorkspaceActiveSpaceDidChangeNotification` and emits `SpaceChanged`. The engine re-applies the image only when `[daemon] reapply_on_space_change = true`.
  - Side effect when it's on: re-setting clears the "Show on all Spaces" toggle. The settings UI shows this next to the switch.
  - `doctor` and the README document the limitation.
- **Other signals:**
  - `NSWorkspaceDidWakeNotification` → `Resumed`
  - `NSApplicationDidChangeScreenParametersNotification` → `DisplaysChanged`
- **Theme.** dark-light 3.0 `stream()`. On macOS it polls every second, and I accept that for v1: the cost is a single defaults read, and it removes hand-written `NSDistributedNotificationCenter` unsafe code that the research flags.
  - I partly disagree with the research's framing: since we already write block-based NSWorkspace observers, a native `AppleInterfaceThemeChangedNotification` observer is a later, optional optimisation behind the same `SystemSignals` seam.
- **TCC.** NSWorkspace needs no Automation permission. Go's osascript approach did, so this is a UX win.

**Windows** (windows 0.62.x, features `Win32_UI_Shell`, `Win32_System_Com`, `Win32_System_Registry`, `Win32_UI_WindowsAndMessaging`):
- **Setting the wallpaper.** A dedicated "desktop thread" runs `CoInitializeEx(COINIT_APARTMENTTHREADED)` and holds `IDesktopWallpaper` from `CoCreateInstance(&DesktopWallpaper)`.
  - Calls: `SetPosition(DWPOS_FILL…)`, then `SetWallpaper(PCWSTR::null(), path)` for all monitors.
  - Fallback: `SystemParametersInfoW(SPI_SETDESKWALLPAPER, …, SPIF_UPDATEINIFILE|SPIF_SENDCHANGE)`.
  - Requests arrive over a channel. This actor pattern is the same as on macOS.
- **Theme.** dark-light 3.0. Its Windows watch mechanism is UNVERIFIED, so Task 27 tests it. If it polls, that's fine.
  - The registry value is `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\SystemUsesLightTheme`. Note dark-light may read `AppsUseLightTheme`; record which one it uses.
- **Signals.** A hidden message-only window on the host thread: `WM_DISPLAYCHANGE` → `DisplaysChanged`, `WM_POWERBROADCAST/PBT_APMRESUMEAUTOMATIC` → `Resumed`.
- **IPC.** Named pipe `\\.\pipe\kakemono-<USERNAME>` via interprocess. Restricting the pipe ACL to the current user SID is a risk to verify.

**Linux:**
- **Wallpaper:** the backends above.
- **Theme:** dark-light 3.0 with the `tokio` feature (Settings portal, `color-scheme`). If the portal is missing (bare WMs), use `[desktop] theme`, else Light.
  - Whether dark-light falls back to gsettings is UNVERIFIED. If it doesn't, add an `org.gnome.desktop.interface color-scheme` read through the `gsettings` argv.
- **Signals:** only `ThemeChanged`. Resume detection via logind `PrepareForSleep(false)` with zbus is cheap, so include it.
- **Tray caveats.** The UI app (not the daemon) owns the tray.
  - At startup it checks for `org.kde.StatusNotifierWatcher` on the session bus. If it's missing (stock GNOME), it skips the tray and tells the user to install the AppIndicator extension or launch with `--settings`.
  - Because tray-icon #336 fails silently on Plasma 6.7 Wayland, `doctor` reports this explicitly.
- **systemd user services** may lack `WAYLAND_DISPLAY` and `HYPRLAND_INSTANCE_SIGNATURE`. `service install` writes a unit bound to `graphical-session.target` and also prints compositor snippets (`exec-once = kakemono daemon` for Hyprland, `exec kakemono daemon` for sway, `spawn-at-startup` for niri). `doctor` warns when the daemon's env lacks the compositor variables.

### G. Library choices (one per decision)

Versions are from `docs/research/rust-libraries.md` (crates.io, 2026-11-09). Task 2 pins them in `[workspace.dependencies]`.

| Decision | Choice | Why the alternatives lost |
|---|---|---|
| GUI/tray | **Tauri 2.12.x** (not the 3.0 alpha) with plugins single-instance, autostart, dialog, opener, positioner (2.x lines) | Reuses about 2.1k LOC of React with Tailwind. egui (eframe 0.37), iced 0.14 and Slint 1.18 all mean a full UI rewrite and give no native look for free; iced has had no release in 11 months. Dioxus desktop is alpha and has the same WebKitGTK dependency. Raw tao, wry and tray-icon is Tauri minus its bundler and plugins. |
| Async runtime | **tokio 1.54** | Tauri, reqwest, interprocess and zbus (`tokio` feature) all use it. async-std is discontinued, and smol would need a second runtime. |
| HTTP | **reqwest 0.13**, `default-features=false, features=["rustls","json","gzip","stream"]` | ureq 3 is blocking and has no cancellation. **Disagreement:** I would pick ring over the default aws-lc provider (`rustls-no-provider`, installing the ring provider at startup) to avoid the aws-lc-sys cmake/NASM toolchain on Windows CI. Task 2 tries the default first; switch only if CI breaks. |
| Retry | Hand-rolled `sources::http::get_with_retry`: 429/503 with `Retry-After`, exponential backoff, 3 tries, `tokio::time` so tests can pause the clock | reqwest-retry ignores `Retry-After` (#146). |
| HTML | **scraper 0.29** | goquery-like CSS selectors. tl has weak selector support; lol_html is a streaming rewriter. |
| Config | **toml 1.2 + serde** for reads; **toml_edit 0.25** for writes from the UI, detection and `config set` | figment and config-rs give layering we don't need. Go rewrote the whole file and lost comments; toml_edit keeps them. |
| Durations | Own `config::duration::parse_go` (about 40 LOC) | humantime rejects Go forms like `1.5h` and `1h30m0s`. Compatibility with existing `interval` strings matters. |
| Paths | **etcetera 0.11** `choose_base_strategy` | Gives `~/.config/kakemono` on macOS too (the legacy `~/.config/wallpaperd/config.toml` is a read-only fallback). `directories` and `dirs` move it to `~/Library/Application Support`. |
| Time | **chrono** (serde) | Go's `time.Time` RFC 3339 output, including the zero value `0001-01-01T00:00:00Z`, round-trips. jiff would also work, but chrono is already in Tauri's tree. |
| CLI | **clap 4.6** derive (+ `clap_complete`) | Standard. bpaf and argh are fine but less familiar. |
| IPC | **interprocess 2.4** local sockets, `tokio` feature | One API for Unix sockets and named pipes. Hand-rolling means two adapters and more code. Its "maintenance: passive" status is acceptable: the API surface we use is small, and the fallback (tokio UnixListener plus `named_pipe`) is about 150 LOC behind `protocol::endpoint`. |
| Linux D-Bus | **zbus 6.2** (`tokio`) + **ashpd 0.14** (portal OpenURI) | dbus-rs needs libdbus, which breaks Linux `cargo check` from macOS. |
| GSettings | Run the `gsettings` argv (shipped with GNOME, Cinnamon and MATE) | The `gio` crate links glib into the headless daemon. Writing GVariants through dconf D-Bus is fragile. |
| Theme | **dark-light 3.0** (`tokio`) behind `SystemSignals` | Three hand-written watchers for one boolean fail the deletion test. Polling on macOS is acceptable. |
| macOS | **objc2 0.6 / objc2-app-kit 0.3 / objc2-foundation / dispatch2 / block2** | The `cocoa` crate is deprecated; Tauri itself moved to objc2. |
| Windows | **windows 0.62** | windows-sys has no COM interface wrappers, which `IDesktopWallpaper` needs. winapi is unmaintained. |
| `wallpaper` crate | **Rejected** | 3.2.0 dates from 2021, uses `osascript` on macOS, has no per-monitor support and no Hyprland, niri or awww. more-wallpapers 0.4 is barely used and wraps it. |
| Errors | **thiserror 2** in libraries (`SetError`, `EngineError`, `ProviderError`), **anyhow** in binaries | Errors cross IPC as strings, as in Go. |
| Logging | **tracing + tracing-subscriber (env-filter) + tracing-appender 0.2** with daily rotation and `max_log_files(7)` | Log dirs: `~/Library/Logs/kakemono`, `$XDG_STATE_HOME/kakemono`, `%LOCALAPPDATA%\kakemono\logs`. Foreground mode logs to stderr. |
| Autostart (daemon) | **auto-launch 0.6** for macOS (LaunchAgent mode) and Windows (HKCU Run). **Own systemd user unit** on Linux | **Disagreement with the research:** auto-launch on Linux writes XDG autostart `.desktop` files, which Hyprland, sway and niri ignore. A systemd unit with `graphical-session.target` plus printed compositor snippets covers both kinds of session. |
| Autostart (UI) | tauri-plugin-autostart | It already wraps auto-launch. |
| Single instance | std `File::try_lock` (daemon), tauri-plugin-single-instance (UI) | The `single-instance` crate would be one more dependency for what std now provides. |
| URL open / reveal | `platform::open`: NSWorkspace `openURL` and `activateFileViewerSelectingURLs` (macOS); `ShellExecuteW` and `SHOpenFolderAndSelectItems` (Windows); ashpd OpenURI, then `xdg-open` (Linux) | Removes the `open` and `open -R` shell-outs. The UI uses tauri-plugin-opener. |
| Thumbnails | **image** crate (jpeg, png and webp features), 384 px longest side, cached in `<cache>/thumbs/<hash>.jpg` | Go streamed full 4K images into the webview. |
| TS bindings | **ts-rs** derives on protocol and config types → `ui/frontend/src/shared/bindings/*.ts`, checked in; CI fails if they're stale | tauri-specta v2 is still RC. Handwritten types drift (they already have in Go). |
| HTTP mocks | **wiremock 0.6** | tokio-native and mature. httpmock 0.9 is more active and is the drop-in alternative if wiremock stalls. |
| Snapshots / CLI tests | **insta 1.49**, **assert_cmd**, **tempfile**, `tokio` `test-util` | |
| Packaging | **Tauri bundler** (dmg/app, deb/rpm/AppImage, msi/nsis) with the daemon as `externalBin` sidecar. CLI-only tarballs come from a plain GitHub Actions matrix (`cargo build --release`, then tar or zip) | cargo-packager has no rpm, and how well cargo-dist suits GUI bundles is UNVERIFIED. The updater and code signing are deferred. |

### H. UI

**Frontend structure:**
- Merge `gui/frontend` and `picker/frontend` into `crates/ui/frontend`: one Vite multi-page app (`settings.html`, `picker.html`, `src/shared/{api.ts,bindings/,tokens.css}`).
- Replace `wailsjs` and `services/wails.ts` with `@tauri-apps/api` `invoke` wrappers in `shared/api.ts`. Components port over almost unchanged.

**Tauri commands** (`src-tauri/src/commands.rs`):
- `get_config`, `save_config(patch)` (toml_edit), `status`, `next`, `prev`, `pause`, `resume`, `favorite`, `trash`, `set_file`, `thumbnails`, `detect`, `doctor`, `reveal`, `open_url`, `daemon_running`, `start_daemon`.
- File and directory pickers use tauri-plugin-dialog, which replaces the osascript dialogs.
- After saving, the UI sends IPC `reload` instead of restarting the daemon as Go did.

**Tray and windows:**
- **Tray-only startup:** no windows in `tauri.conf.json`, and `prevent_exit()` on `ExitRequested`.
- **Tray menu:** Next, Previous, Pause/Resume, Favorite, Move to Trash, Show Picker, Settings…, and Quit. The menu reflects the live status from IPC `subscribe`.
- **Live updates:** the UI keeps one IPC connection with `subscribe`, re-emits events as Tauri events (`wallpaper://changed`), and reconnects with backoff.

**Picker positioning, in tiers:**
1. **macOS, Windows and X11:** an undecorated, always-on-top window that skips the taskbar and is visible on all workspaces. It is positioned at the configured edge of the monitor under the cursor (`cursor_position` plus `available_monitors`); Go always used the primary monitor. On macOS also use `set_visible_on_all_workspaces` so the picker appears over the current Space.
2. **Wayland, all compositors (v1):** clients cannot position themselves, and Tauri/tao has no pre-realize hook for layer-shell (tao #1315; PR #15914 was closed). So the window opens centred-bottom wherever the compositor puts it, with a stable app_id/class `kakemono-picker`. `doctor` and the docs give one-line window rules:
   - Hyprland: `windowrulev2 = float, class:kakemono-picker` / `move …`
   - sway: `for_window [app_id="kakemono-picker"] floating enable, move position …`
   - niri: a `window-rule` block
   - Optional `[picker] force_x11 = true` relaunches the picker process with `GDK_BACKEND=x11` (XWayland). This is UNVERIFIED; it is behind a flag and off by default because of blur under fractional scaling.
3. **Layer-shell picker** (raw wry plus gtk-layer-shell): out of scope for v1. There is a time-boxed spike task, but no commitment.

**User-placed picker (all tiers):**
- The picker strip has a drag handle (`data-tauri-drag-region`, which calls `startDragging()`). The underlying calls are `NSWindow performWindowDragWithEvent` on macOS, `WM_NCLBUTTONDOWN`/`HTCAPTION` on Windows, `gtk_window_begin_move_drag` on X11, and `xdg_toplevel.move` on Wayland, where the compositor moves the window.
- The window must float on tiling compositors. The documented rules include `float` (Hyprland) and `floating enable` (sway), and `doctor` prints them.
- **Saving the position** adds `[picker] placement = "edge" | "custom"` and `[picker] custom = { x, y, monitor }` (the monitor is identified by name). When the user drops the window, `placement` becomes `custom`. The "Reset position" item in the tray menu or the context menu sets it back to `edge`.
  - **macOS, Windows, X11:** read the position with Tauri `outer_position()` on `Moved` (debounced 300 ms) and restore it with `set_position()`.
  - **Hyprland:** apps can't read or set their own position on Wayland, so ask the compositor. On hide, read `hyprctl clients -j` and match `class == "kakemono-picker"` to get `at`/`size`. On show, run `hyprctl dispatch movewindowpixel exact X Y,class:kakemono-picker`.
  - **sway:** on hide, read the rect of the `app_id == "kakemono-picker"` node from `swaymsg -t get_tree`. On show, run `swaymsg '[app_id="kakemono-picker"] move absolute position X Y'`.
  - The placement code lives in `crates/ui/src-tauri/src/picker_place.rs` behind a small `Placer` trait, so `ui` still depends only on `protocol` and `config`. The compositor comes from `HYPRLAND_INSTANCE_SIGNATURE` or `SWAYSOCK`. An injected argv runner lets tests snapshot the commands without running `hyprctl` or `swaymsg`.
  - **GNOME, KDE, niri and others:** the user can drag the window, but it isn't remembered, and it opens centred each time. The docs point KDE users to a "Remember position" window rule.
- If the saved monitor is gone, or the saved rect is off every screen, fall back to `edge` placement.

**Design standards (enforced in the review checklist, Task 44):**
- **Restraint:** system font stack (`-apple-system, BlinkMacSystemFont, "Segoe UI Variable", system-ui, Cantarell, sans-serif`), 13 px base on macOS and 14 px elsewhere, a neutral palette, and a single accent colour.
  - The accent comes from CSS `AccentColor` where the webview supports it, with a neutral blue fallback.
  - No gradients, glassmorphism, emoji icons, decorative illustrations or "AI dashboard" cards. Icons are one consistent stroke set (lucide), used only where they carry meaning.
- **Dark and light:** `color-scheme: light dark`, `prefers-color-scheme` tokens, Tauri window `theme: null` (follow the system), and no flash of the wrong theme. The background colour is set in the HTML `<head>` and in the window's `backgroundColor` for each theme.
- **Motion:** 120–160 ms ease-out for the picker reveal and hover; everything is disabled under `prefers-reduced-motion: reduce`.
- **Picker keyboard behaviour:**
  - The strip is `role="listbox"` with `aria-activedescendant`, and each thumbnail is `role="option"` with an `aria-label` of "title — source". The current wallpaper has `aria-selected`.
  - ←/→ (↑/↓ for left/right edges) move, Home/End jump, Enter sets, F favorites, Delete trashes (with an undo toast for 4 s), and Esc closes and restores focus.
  - Shift+F10 or the Menu key opens the context menu, which is fully keyboard-operable. Focus is visible (`:focus-visible` 2 px accent ring).
  - The picker closes on blur (configurable).
- **Settings:** a sidebar of sections (General, Sources, Themes, Desktop, Hooks, Favorites).
  - Each form field shows its validation inline; for example, the interval is validated by the same Go-duration grammar, mirrored in TS and covered by a shared test table.
  - Saving is explicit, with inline "Saved" confirmation, and there are no modal confirmations except for destructive actions.
- **Platform feel:** native window chrome everywhere (no custom title bars), a macOS-style source list only on macOS (`data-os` attribute on `<html>`), and standard shortcuts (⌘, / Ctrl+, opens settings; ⌘W closes).

**Thumbnails:** served through a `thumb://` custom protocol. Requests carry an opaque id: the hash of a path from an allowlist built from the history, the cache dir, the favorites dir and the local dirs. They never carry a raw path. This fixes Go's `/local/<any-path>` handler, which would serve any image-extension file on disk.

### I. Data compatibility

**config.toml:**
- Same path and keys, and the same defaults (copied from `config.Defaults()`). Fields with a leading `~` are expanded.
- New optional sections: `[desktop]`, `[hooks]`, `[ui]`. New keys: `[daemon] rotate_on_theme_change` (default true) and `reapply_on_space_change` (default false); `[picker] force_x11`, `close_on_blur`, `placement` and `custom`.
- `socket_path` is honoured if it is set.
- **Default socket location:**
  - macOS: `~/.config/kakemono/kakemono.sock`
  - Linux: `$XDG_RUNTIME_DIR/kakemono/kakemono.sock`, falling back to `~/.config/kakemono/kakemono.sock`
  - Windows: `\\.\pipe\kakemono-<user>`
  - The socket dir is created 0700 and the socket set to 0600.

**state.json:**
- Same shape: `current` (nullable), `history`, `history_index` (−1 when empty), `paused`, `last_changed`. `maxHistory = 100`.
- **Path:** `~/.config/kakemono/state.json`. If it is missing and the Go app's `~/.config/wallpaperd/state.json` exists (`paths::legacy_state_file`), read that once and write the new file; never modify the old one.
- **Write safety:** writes are atomic (temp file plus rename). Go wrote the file in place.

**IPC wire format:**
- Same newline JSON. A request is `{"cmd","args"?}` and a response is `{"ok","error"?,"data"?}`, with `error` and `data` omitted when empty, as in Go's `omitempty`.
- `WallpaperInfo` serialises **every** field, including `""` and `0` (Go has no omitempty there). `StatusData.current` serialises as `null`.
- The unknown-command message keeps Go's format: `unknown command: "x"`.
- New commands: `reload`, `subscribe` (the server keeps the connection open and streams `{"event":"changed","data":WallpaperInfo}` lines), `version`, `doctor`.

**CLI:**
- Same subcommands and flags: global `--config` and `--socket`; `daemon [-f/--foreground]`; `next`; `prev`; `set <path>`; `favorite`; `trash`; `status [--json]`; `pause`; `resume`; `open [--folder|--favorites]`; `pick`; `settings`.
- `status --json` prints the indented JSON of `StatusData`, as `json.MarshalIndent` does, so the sketchybar `jq` scripts keep working.
- New subcommands: `detect [--write] [--force]`, `doctor [--apply]`, `service install|uninstall|status`, `config init|path|edit`, `completions <shell>`.
- **`daemon` with no `-f`:** Go spawns a background child. Keep this on Unix (re-exec with `setsid`). On Windows use `DETACHED_PROCESS` and `CREATE_NO_WINDOW`.

**Hooks:**
- `[hooks] on_change = ["sketchybar","--trigger","wallpaper_changed"]` with environment variables `WALLPAPERD_PATH`, `WALLPAPERD_SOURCE`, `WALLPAPERD_TITLE`, `WALLPAPERD_AUTHOR`, `WALLPAPERD_SOURCE_URL` and `WALLPAPERD_THEME`.
- Placeholders work as in `set_command`.
- Hooks are fire-and-forget, with a 10 s timeout, and failures are only logged.
- At most 4 hooks run at once. If a hook for the same event is still running, the new run is dropped and logged.
- The environment is cleared first (`env_clear`). Only `PATH`, `HOME`, `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS` (and `SystemRoot` and `USERPROFILE` on Windows) are passed through, plus the `WALLPAPERD_*` variables.
- The examples move to `contrib/sketchybar` and `contrib/waybar` (a waybar `custom/wallpaper` module with `exec` and `signal`, plus a hook `["pkill","-RTMIN+8","waybar"]`).

### J. Test strategy

**Golden fixtures (Task 1):**
- A throwaway Go program `tools/goldens/main.go` marshals representative Go values to `crates/protocol/tests/fixtures/`: `WallpaperInfo` (full and empty), `StatusData` (with nil and non-nil current), `Response` (ok, error, data), `State`, the `status --json` output and the default config template.
- Rust tests assert that the JSON is semantically equal both ways, and that key sets and null handling match exactly.
- Provider fixtures (JSON and HTML responses) are copied from the inline fixtures in the Go `*_test.go` files into `crates/sources/tests/fixtures/<provider>/`.

**Fakes:**
- `platform::fake::{RecordingSetter, ManualSignals}` (behind the `fake` feature, used in `engine` dev-dependencies).
- `sources::fake::StaticProvider`.
- Fake Linux tools are shell scripts in `crates/platform/tests/bin/` that append argv to `$WPD_LOG`; tests prepend them to `PATH`.

**Time:** `#[tokio::test(start_paused = true)]` for the rotation, retry and supervisor delays. Wall time comes from `Deps.now`.

**Snapshots:** insta for the Linux resolution table (about 25 env scenarios), the argv of every preset, generated service files (plist, unit), the config template, `doctor` output and `status` text output.

**CI matrix** (`.github/workflows/ci.yml`):
- `{macos-14, windows-latest, ubuntu-24.04}` × (`cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace`).
- Ubuntu installs `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev`.
- A frontend job runs `npm ci && npm run typecheck && npm run build && npm test`.
- `cargo deny check` runs.
- A nightly job runs provider live smoke tests (`cargo test -p kakemono-sources --features live -- --ignored`). It is allowed to fail.

**Local cross-checks** (no linking needed; `platform` has no C dependencies):
- `rustup target add x86_64-unknown-linux-gnu x86_64-pc-windows-msvc`
- `cargo check -p kakemono-platform -p kakemono --target …`

## Risks and unknowns

Ranked by likelihood × impact.

1. **The Wayland picker can't be edge-anchored through Tauri.**
   - Mitigation: tiered approach (§H). The v1 promise on Wayland is a centred picker plus documented window rules. The layer-shell spike (Task 43) is time-boxed, and the result is reported, not shipped, unless it is clean.
2. **Linux tray is unreliable** (GNOME has no watcher; tray-icon #336 silent failure on Plasma 6.7 Wayland; #104 clicks).
   - Mitigation: the daemon never depends on the tray, the UI checks for a watcher, `doctor` reports, and every tray action also exists in the CLI and the settings window. Re-check whether tauri 2.12's tray-icon uses the `ksni` backend; if Tauri moves to tray-icon 0.26 with ksni, enable it.
3. **Session env missing under systemd user services** (Wayland and Hyprland variables), which leads to wrong detection or failed sets.
   - Mitigation: printed compositor snippets, `doctor` env checks, and re-detection on failure. The unit's documentation also covers `dbus-update-activation-environment --systemd --all`.
4. **macOS main-thread host and objc2 0.3.2 API details are UNVERIFIED.**
   - Mitigation: Task 24 is a spike-then-implement task with a manual check. If NSApplication-in-daemon misbehaves under launchd, fall back to `CFRunLoopRun` on the main thread plus `NSWorkspace.notificationCenter`; this needs no NSApplication, but loses screen-parameter notifications, which is acceptable because the space-change re-apply covers most cases.
5. **macOS Spaces limitation.** We can only set the current Space. Re-applying on switch is opt-in, because it clears the user's "Show on all Spaces" toggle. Document it, and don't over-promise.
6. **Provider API drift:**
   - The keyless Unsplash Source endpoint has been shut down; verify against the Go `unsplash.go` and require an `access_key`, otherwise mark the provider unavailable with a clear error.
   - Reddit throttles or 403s generic user agents; use a descriptive UA (`kakemono/<version>`).
   - Changes to Wallhaven's HTML break the scraper.
   - Mitigation: fixtures, the nightly live job, and multi-source fallback in the engine.
7. **The Linux backend matrix churns** (swww→awww rename, hyprpaper 0.8 rewrite with `hyprctl hyprpaper wallpaper` syntax UNVERIFIED, KDE screen indices, COSMIC).
   - Mitigation: presets are data (one table in `presets.rs`), users can override `set_command`, and `doctor --apply` tests the setup. Detect both awww and swww.
8. **WebKitGTK problems** (NVIDIA blank windows, EGL on Hyprland AppImage) and memory use (the research's rough figure of about 300 MB peak is AI-assisted and unverified).
   - Mitigation: the UI is optional and windows are destroyed on close, leaving the tray-only footprint. Document the `WEBKIT_DISABLE_DMABUF_RENDERER=1` workaround. A launcher wrapper could set it automatically when an NVIDIA driver is detected, but that is a follow-up.
9. **reqwest 0.13 with aws-lc** may cause Windows or ARM build friction. The fallback is ring via `rustls-no-provider` (§G).
10. **Windows named-pipe ACLs** may allow other local users to connect. Verify interprocess's security descriptor options, and restrict the pipe to the current user SID if possible. Unsigned binaries trigger SmartScreen; signing is deferred.
11. **GPL contamination.** `variety-reference/` (Variety, GPL-3) is reference only: implement from public API docs and don't copy code. `cargo deny` blocks GPL crates.
12. **Compatibility details:** the Go-duration grammar, the `omitempty` asymmetry and Go's zero time. All are covered by golden tests in Tasks 1, 3, 5 and 16.
13. **interprocess has passive maintenance.** The endpoint module isolates it (about 150 LOC to replace).
14. **Repository hygiene.** The folder is not a git repo, and there is a stray `~/` directory and committed binaries (`kakemono`, `gui/build/bin`, `coverage.out`). Task 1 creates the repo and a `.gitignore` before anything else.

**Open questions for the user** (defaults are chosen, so none of them block):
- (a) Auto-rotate on theme change: default yes.
- Answered in the synthesis: re-applying on macOS Space change is opt-in (default off); the Wayland picker is centred with a spike only; the picker can be dragged and its position is remembered where the OS allows; 7 crates; Tauri bundler plus a CI matrix.
- (b) A Linux-only "set the same image on the lock screen" option (GNOME `org.gnome.desktop.screensaver picture-uri`): default no.
- (c) App identifier `dev.rielj.kakemono`: decided.
- (d) Whether to ship Homebrew, AUR or winget manifests in v1: default no.

## What we're NOT doing

- Our own Wayland or X11 wallpaper renderer (no layer-shell surface, no root pixmap through x11rb).
- Different images per monitor, lock-screen or login-screen wallpapers, animated, video or HDR wallpapers.
- New providers beyond Bing, Wallhaven, Unsplash, Reddit, APOD and Local, and any plugin or dynamic-loading system for providers.
- Image effects (blur, dim, crop-to-fit pre-processing).
- Global hotkeys (they don't work on GNOME Wayland; plugins-workspace #3267).
- Shipping a layer-shell picker in v1 (spike only).
- Remembering the picker position on GNOME, KDE or niri (dragging works there, but the position isn't saved).
- Moving API keys into the OS keychain (they stay in `config.toml` at 0600).
- Code signing, notarization, the auto-updater, and Homebrew, AUR, winget or Flatpak publishing.
- A Tauri 3 alpha migration, or a non-web UI toolkit.
- Maintaining the Go version after cutover, or long-lived Go/Rust coexistence. Go stays only as a parity reference until Task 49 deletes it.
- Moving `state.json` or `config.toml` to new default locations.
- Rewriting the sketchybar plugin scripts (they keep working through the CLI); we only add hook examples.

## Overview

- [ ] Task 1: Git init, hygiene, and Go golden-fixture capture
- [ ] Task 2: Cargo workspace skeleton, pinned deps, CI, stub modules
- [ ] Task 3: protocol types + golden compatibility tests (contract)
- [ ] Task 4: protocol codec, endpoint naming, async client
- [ ] Task 5: config schema, defaults, load, Go-duration parser (contract)
- [ ] Task 6: config paths (etcetera), init template, toml_edit writer
- [ ] Task 7: platform seam traits, fake, `host()` default impl (contract)
- [ ] Task 8: platform::exec argv templating + CommandRunner
- [ ] Task 9: sources Provider trait, http client, retry, download
- [ ] Task 10: Bing provider
- [ ] Task 11: APOD provider
- [ ] Task 12: Reddit provider
- [ ] Task 13: Unsplash provider
- [ ] Task 14: Wallhaven API mode
- [ ] Task 15: Wallhaven scrape mode
- [ ] Task 16: Local provider
- [ ] Task 17: sources Registry built from config
- [ ] Task 18: engine State (history) with JSON compatibility
- [ ] Task 19: Engine core ops (next/prev/set/status) against fakes
- [ ] Task 20: Engine favorite/trash/cache trim
- [ ] Task 21: Engine rotation loop, pause/resume, signal reactions
- [ ] Task 22: Engine events + hooks
- [ ] Task 23: Daemon IPC server + instance lock
- [ ] Task 24: macOS host + NSWorkspace setter
- [ ] Task 25: macOS signals (space, wake, screens) + theme via dark-light
- [ ] Task 26: Windows host + IDesktopWallpaper setter
- [ ] Task 27: Windows signals + theme
- [ ] Task 28: Linux detection env capture + pure resolver
- [ ] Task 29: Linux command + preset backends (argv table)
- [ ] Task 30: Linux persistent-process supervisor (swaybg/wbg)
- [ ] Task 31: Linux gsettings backends (GNOME/Cinnamon/MATE)
- [ ] Task 32: Linux KDE Plasma D-Bus backend
- [ ] Task 33: Linux XFCE xfconf D-Bus backend
- [ ] Task 34: Linux theme/resume signals + `linux::open_platform`
- [ ] Task 35: CLI parity commands
- [ ] Task 36: `daemon` command wiring (host, logging, shutdown, detach)
- [ ] Task 37: `detect` / `doctor` / first-run persistence
- [ ] Task 38: platform::open (URL/reveal) + `open` command
- [ ] Task 39: `service install|uninstall|status`
- [ ] Task 40: Tauri app scaffold + merged frontend + tray
- [ ] Task 41: Settings window on Tauri commands
- [ ] Task 42: Settings Desktop + Hooks panels
- [ ] Task 43: Picker window, positioning tiers, layer-shell spike
- [ ] Task 44: Picker thumbnails protocol + keyboard a11y + design pass
- [ ] Task 45: Live updates via `subscribe`
- [ ] Task 46: UI ↔ daemon lifecycle (sidecar spawn, `pick`/`settings` commands)
- [ ] Task 47: Packaging + release workflow
- [ ] Task 48: Docs, migration guide, contrib hooks
- [ ] Task 49: Cutover — delete Go code

**Parallelism map:**
- Tasks 1→2 are sequential.
- Then the contracts, Tasks 3, 5, 7 and 9, can run in parallel (they own different crates).
- After 9: Tasks 10–16 run in parallel.
- After 7 and 8: platform Tasks 24–25 (macOS), 26–27 (Windows) and 28–34 (Linux) form three parallel lanes.
- After 4 and 6: the UI lane (Tasks 40–46) runs in parallel with the engine and platform lanes.
- Every crate's `Cargo.toml` and module list is created in Task 2, so parallel tasks never edit the same `Cargo.toml` or `lib.rs`. A task that needs a new dependency must ask for it in the Task 2 owner's file through a sequential follow-up; this should be rare.

## Task 1: Git init, hygiene, and Go golden-fixture capture
- **Files owned:** `.gitignore`, `tools/goldens/main.go`, `crates/protocol/tests/fixtures/*.json`, `crates/protocol/tests/fixtures/status_json.txt`, `crates/config/tests/fixtures/go_default_template.toml`, `crates/sources/tests/fixtures/**` (copied from the Go tests)
- **Depends on:** none
- **Mode:** sequential
- **Steps:**
  1. Run `git init`. Add a `.gitignore` covering `target/`, `node_modules/`, `dist/`, `coverage.out`, `/wallpaperd`, `gui/build/bin/`, `picker/build/bin/` and `/~/`.
  2. Ask the user to commit the Go baseline and tag `go-final`. Agents never commit without asking.
  2b. Write `tools/goldens/replay.sh`. It starts the Go daemon on a temp socket with a temp config, sends every command (`next prev set favorite trash pause resume status`, plus invalid JSON and an unknown command), and saves the raw request and response lines to `crates/protocol/tests/fixtures/wire/*.jsonl`. These wire captures sit alongside the marshalled goldens from step 3 and catch framing or `omitempty` differences that marshalling alone would miss. (From Plan C.)
  3. Write `tools/goldens/main.go` (package main, importing `internal/ipc`, `internal/wallpaper` and `internal/config`). It writes:
     - `wallpaper_info_full.json` / `wallpaper_info_empty.json`
     - `status_current_null.json` / `status_current_set.json`
     - `response_ok_nodata.json` / `response_ok_data.json` / `response_err.json`
     - `command_set.json` / `command_noargs.json`
     - `state_empty.json` (history_index −1, zero time) / `state_populated.json`
     - `status_json.txt` (output of `json.MarshalIndent` with two-space indent)
     - `go_default_template.toml` (from `config.InitConfig` into a temp dir)
  4. Copy the inline JSON and HTML fixture strings from `internal/providers/*/…_test.go` into `crates/sources/tests/fixtures/<provider>/`.
- **Automated verification:** `go run ./tools/goldens && go test ./... && bash tools/goldens/replay.sh && git status --porcelain --untracked-files=all | grep -c fixtures`
- **Manual verification:** none

## Task 2: Cargo workspace skeleton, pinned deps, CI, stub modules
- **Files owned:** `Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `.github/workflows/ci.yml`, `crates/*/Cargo.toml`, every `crates/*/src/lib.rs` or `main.rs`, and empty stub module files for every module named in Architecture §A (each with a `//! TODO(task N)` doc comment and compilable placeholder items)
- **Depends on:** Task 1
- **Mode:** sequential
- **Steps:**
  1. Create the workspace with members `crates/{protocol,config,sources,platform,engine,cli}` (`crates/ui/src-tauri` is added in Task 40).
  2. Fill `[workspace.dependencies]` with every crate from §G at the research versions, using `cargo add` to confirm the latest 2.x/1.x versions.
  3. Declare per-crate dependencies now, including `[target.'cfg(target_os="macos")'.dependencies]` (objc2 family, dispatch2, block2), Windows (`windows` with features) and Linux (`zbus` with `tokio`, `ashpd`, `which`).
  4. Add `platform` features `fake` and `sources` feature `live`.
  5. Write the CI workflow from §J.
  6. Make `lib.rs` declare all modules, e.g. `pub mod bing; pub mod apod; …`, with cfg-gated `mod macos;`, `mod windows;` and `mod linux;`.
- **Automated verification:** `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo check -p kakemono-platform --target x86_64-unknown-linux-gnu && cargo check -p kakemono-platform --target x86_64-pc-windows-msvc && cargo deny check`
- **Manual verification:** push to GitHub and confirm the CI matrix is green on all 3 OSes

## Task 3: protocol types + golden compatibility tests (contract)
- **Files owned:** `crates/protocol/src/types.rs`, `crates/protocol/tests/golden.rs`
- **Depends on:** Task 2
- **Mode:** contract-first
- **Steps:**
  1. Write the golden tests first. They load each fixture, deserialize it, re-serialize it, and compare `serde_json::Value` equality plus exact key sets.
  2. Define:
     - `Command { cmd: String, #[serde(default, skip_serializing_if="Map::is_empty")] args: Map }` and the typed `Request` enum (`Next | Prev | Set{path} | Favorite | Trash | Pause | Resume | Status | Reload | Subscribe | Version | Doctor`), with conversion from `Command`.
     - `Response { ok, #[skip_if empty] error: String, #[skip_if none] data: Option<RawValue> }`.
     - `StatusData` (with `current: Option<WallpaperInfo>` always serialised).
     - `WallpaperInfo`, with every field always present and `#[serde(default)]`.
     - `Event { event: String, data }`.
  3. Derive `ts_rs::TS` on all types.
  4. Implement `unknown_command_error(name)`, which reproduces `unknown command: "x"`.
- **Automated verification:** `cargo test -p kakemono-protocol golden`
- **Manual verification:** none

## Task 4: protocol codec, endpoint naming, async client
- **Files owned:** `crates/protocol/src/codec.rs`, `crates/protocol/src/endpoint.rs`, `crates/protocol/src/client.rs`, `crates/protocol/tests/client.rs`
- **Depends on:** Task 3
- **Mode:** sequential
- **Steps:**
  1. `codec`: `read_line_json<T>(BufReader)` and `write_line_json`, with a 1 MiB line cap.
  2. `endpoint::default_endpoint(socket_override: Option<&Path>) -> Endpoint` implements the defaults in §I. On Unix, `prepare()` creates the directory with mode 0700.
  3. `Client::connect(&Endpoint).await`, `request(Request) -> Result<Response>`, and `subscribe() -> impl Stream<Item=Event>`.
  4. Map connection-refused or not-found to `ClientError::DaemonNotRunning`, whose message matches Go's `daemonNotRunning` hint.
  5. Write a test that starts an in-test interprocess listener echoing canned responses.
- **Automated verification:** `cargo test -p kakemono-protocol`
- **Manual verification:** none

## Task 5: config schema, defaults, load, Go-duration parser (contract)
- **Files owned:** `crates/config/src/schema.rs`, `crates/config/src/defaults.rs`, `crates/config/src/load.rs`, `crates/config/src/duration.rs`, `crates/config/tests/compat.rs`
- **Depends on:** Task 2
- **Mode:** contract-first
- **Steps:**
  1. Mirror the Go structs: `Config{daemon, wallhaven, unsplash, reddit, bing, apod, local, light, dark, favorites, picker}`, plus the new `desktop: DesktopConfig`, `hooks: HooksConfig` and `ui`. Add the new keys `daemon.rotate_on_theme_change` (default true), `daemon.reapply_on_space_change` (default false), `picker.force_x11`, `picker.close_on_blur`, `picker.placement` (`edge` | `custom`, default `edge`) and `picker.custom: Option<{x, y, monitor}>`.
  2. Use `#[serde(default)]` everywhere and ignore unknown keys.
  3. Copy the defaults exactly from `config.Defaults()` (bing enabled, `market` "en-US", interval "30m", cache_size 50, and so on).
  4. Write `load(path)` and `load_or_default(path)`, with `~` expansion for download_dir, trash_dir, favorites.dir and local.dirs.
  5. Write `duration::parse_go("1h30m" | "1.5h" | "90s" | "300ms") -> Duration`, with a table test.
  6. `CommandSpec` deserializes from either an array or a string (split with shell-words).
  7. The compat test parses `go_default_template.toml` and asserts that the result equals `defaults()` after expansion.
- **Automated verification:** `cargo test -p kakemono-config`
- **Manual verification:** none

## Task 6: config paths (etcetera), init template, toml_edit writer
- **Files owned:** `crates/config/src/paths.rs`, `crates/config/src/template.toml`, `crates/config/src/edit.rs`, `crates/config/tests/edit.rs`
- **Depends on:** Task 5
- **Mode:** sequential
- **Steps:**
  1. `paths::{config_file, state_file, runtime_dir, log_dir, cache_dir}` using `etcetera::choose_base_strategy`, honouring `XDG_CONFIG_HOME`.
  2. `template.toml` is the Go template plus commented `[desktop]` and `[hooks]` sections. `init(path)` refuses to overwrite, with Go's message.
  3. `edit::patch(path, |doc: &mut toml_edit::DocumentMut| …)` writes atomically. Helpers: `set_desktop(&DesktopConfig)` and `apply_patch(serde_json::Value)` for UI saves.
  4. Write an insta test showing that comments survive a patch.
- **Automated verification:** `cargo test -p kakemono-config && cargo insta test -p kakemono-config --check`
- **Manual verification:** none

## Task 7: platform seam traits, fake, `host()` default impl (contract)
- **Files owned:** `crates/platform/src/api.rs`, `crates/platform/src/fake.rs`, `crates/platform/src/host.rs`, `crates/platform/src/lib.rs` (re-exports only)
- **Depends on:** Task 2
- **Mode:** contract-first
- **Steps:**
  1. Write the code from §B exactly.
  2. `SetError` (thiserror) has variants `Unconfigured{hint}`, `Command{argv0,status,stderr}`, `Dbus(String)`, `Os(String)` and `Io`.
  3. `fake::RecordingSetter` stores calls in `Arc<Mutex<Vec<(PathBuf,Fit)>>>`, can be told to fail the next N calls, and `ManualSignals` has `emit(Signal)` and `set_theme()`.
  4. The default `host()` (non-macOS/Windows path) builds the tokio runtime and `block_on(f(platform))`. The per-OS `open_platform` functions are stubbed `todo`-free, returning `Unconfigured`.
  5. Unit-test the fake.
- **Automated verification:** `cargo test -p kakemono-platform --features fake`
- **Manual verification:** none

## Task 8: platform::exec argv templating + CommandRunner
- **Files owned:** `crates/platform/src/exec.rs`, `crates/platform/tests/exec.rs`, `crates/platform/tests/bin/record-argv` (sh script)
- **Depends on:** Task 7
- **Mode:** sequential
- **Steps:**
  1. `render(spec:&[String], vars:&Vars) -> Vec<String>` does per-token substitution of `{path}`, `{uri}`, `{fit}` and `{theme}`, and rejects unknown `{…}` placeholders at config validation time.
  2. `trait CommandRunner { async fn run(&self, argv:&[String], env:&[(..)], timeout) -> Result<Output>; fn spawn_detached(&self, argv) -> Result<u32>; }`, with a `TokioRunner` implementation and a `RecordingRunner`.
  3. Tests:
     - paths containing spaces, quotes, `$()`, `;` and leading `-` survive as a single argv element (asserted through the record-argv script)
     - a timeout kills the child
     - the `SetError::Command` stderr capture is correct
- **Automated verification:** `cargo test -p kakemono-platform exec` (Unix); `cargo check -p kakemono-platform --target x86_64-pc-windows-msvc`
- **Manual verification:** none

## Task 9: sources Provider trait, http client, retry, download
- **Files owned:** `crates/sources/src/provider.rs`, `crates/sources/src/http.rs`, `crates/sources/src/download.rs`, `crates/sources/src/fake.rs`, `crates/sources/tests/http.rs`
- **Depends on:** Task 2
- **Mode:** contract-first
- **Steps:**
  1. `#[async_trait] trait Provider { fn name(&self)->&'static str; fn enabled(&self)->bool; async fn fetch(&self, query:&str)->Result<Vec<WallpaperResult>, ProviderError>; }` with `WallpaperResult{source_url,image_url,title,author,source}`.
  2. `http::client()` uses UA `kakemono/<version>` (`http::USER_AGENT`) and a 30 s timeout. Providers build URLs with `http::url_with_query` (reqwest's `.query()` needs a feature the workspace doesn't enable) and accept an injected client + base URL for tests.
  3. `get_with_retry` honours `Retry-After` (seconds and HTTP-date), with backoff 1 s, 2 s, 4 s.
  4. `download(url, dir)`:
     - port of Go `DownloadFile`: content-type check, 10 KB minimum, `sanitize_filename`, write to temp then rename
     - a filename collision gets a suffix
  5. Also port `is_image_file`.
  6. Wiremock tests: a 429 with `Retry-After: 2` is retried after a 2 s paused clock; non-image is rejected; too-small is rejected; the filename is sanitized.
- **Automated verification:** `cargo test -p kakemono-sources -- http download`
- **Manual verification:** none

## Task 10: Bing provider
- **Files owned:** `crates/sources/src/bing.rs`, `crates/sources/tests/bing.rs`
- **Depends on:** Task 9
- **Mode:** parallel-safe
- **Steps:**
  1. Port `internal/providers/bing/bing.go`: the HPImageArchive JSON with `mkt` from config, and the UHD URL construction. Use an injectable base URL (`Bing::with_base_url`).
  2. Port the cases from `bing_test.go` against wiremock serving `fixtures/bing/*.json`.
- **Automated verification:** `cargo test -p kakemono-sources --test bing`
- **Manual verification:** none

## Task 11: APOD provider
- **Files owned:** `crates/sources/src/apod.rs`, `crates/sources/tests/apod.rs`
- **Depends on:** Task 9
- **Mode:** parallel-safe
- **Steps:**
  1. Port `apod.go`, skipping `media_type != "image"` and preferring `hdurl`.
  2. Test with wiremock fixtures, including a video day, which should yield an empty result or an error per the Go behaviour.
- **Automated verification:** `cargo test -p kakemono-sources --test apod`
- **Manual verification:** none

## Task 12: Reddit provider
- **Files owned:** `crates/sources/src/reddit.rs`, `crates/sources/tests/reddit.rs`
- **Depends on:** Task 9
- **Mode:** parallel-safe
- **Steps:**
  1. Port `reddit.go`: pick a random subreddit, filter for direct image URLs, and HTML-unescape preview URLs if Go did.
  2. Port the `reddit_test.go` cases.
- **Automated verification:** `cargo test -p kakemono-sources --test reddit`
- **Manual verification:** none

## Task 13: Unsplash provider
- **Files owned:** `crates/sources/src/unsplash.rs`, `crates/sources/tests/unsplash.rs`
- **Depends on:** Task 9
- **Mode:** parallel-safe
- **Steps:**
  1. Port `unsplash.go`. If Go uses the dead keyless Source endpoint, `fetch` without an `access_key` returns `ProviderError::NotConfigured("unsplash requires access_key")`.
  2. With a key, use `/photos/random?query=&orientation=landscape`.
  3. Test with wiremock.
- **Automated verification:** `cargo test -p kakemono-sources --test unsplash`
- **Manual verification:** none

## Task 14: Wallhaven API mode
- **Files owned:** `crates/sources/src/wallhaven/mod.rs`, `crates/sources/src/wallhaven/api.rs`, `crates/sources/tests/wallhaven_api.rs`
- **Depends on:** Task 9
- **Mode:** parallel-safe
- **Steps:**
  1. `Wallhaven::new(cfg)` uses defaults purity "100", sorting "random" and categories "111". `fetch` dispatches on `api_key`.
  2. The API mode maps `data[].{url,path,uploader.username}`.
  3. `mod.rs` declares `mod scrape;`, which was already stubbed in Task 2.
  4. Port the API cases from `wallhaven_test.go`.
- **Automated verification:** `cargo test -p kakemono-sources --test wallhaven_api`
- **Manual verification:** none

## Task 15: Wallhaven scrape mode
- **Files owned:** `crates/sources/src/wallhaven/scrape.rs`, `crates/sources/tests/wallhaven_scrape.rs`
- **Depends on:** Task 14
- **Mode:** sequential
- **Steps:**
  1. Port the goquery selectors to `scraper::Selector`, using the same CSS selectors as Go.
  2. Fetch up to 3 pages with `scrape_delay`, which is 0 in tests, as in Go's `NewWithSearchURL`.
  3. Build the full-image URL from the thumbnail id the same way Go does.
  4. Test with the HTML fixtures.
- **Automated verification:** `cargo test -p kakemono-sources --test wallhaven_scrape`
- **Manual verification:** none

## Task 16: Local provider
- **Files owned:** `crates/sources/src/local.rs`, `crates/sources/tests/local.rs`
- **Depends on:** Task 9
- **Mode:** parallel-safe
- **Steps:**
  1. Port `local.go` with recursive walking (std, or the `walkdir` dependency from Task 2), filtering by `is_image_file`. `image_url` is the absolute path.
  2. Use a tempdir tree in the test.
- **Automated verification:** `cargo test -p kakemono-sources --test local`
- **Manual verification:** none

## Task 17: sources Registry built from config
- **Files owned:** `crates/sources/src/registry.rs`, `crates/sources/tests/registry.rs`
- **Depends on:** Tasks 10–16
- **Mode:** sequential
- **Steps:**
  1. `Registry::from_config(&Config, http)` registers all six providers.
  2. `candidates(sources:&[String]) -> Vec<Arc<dyn Provider>>` returns the enabled providers in random order. Port the Go `Random` semantics and add ordering for the fallback.
  3. Write tests: an unknown name is ignored; disabled providers are excluded.
- **Automated verification:** `cargo test -p kakemono-sources`
- **Manual verification:** none

## Task 18: engine State (history) with JSON compatibility
- **Files owned:** `crates/engine/src/state.rs`, `crates/engine/tests/state.rs`
- **Depends on:** Task 3
- **Mode:** parallel-safe
- **Steps:**
  1. Port `state.go` (`push` truncates forward history, cap 100, `prev`, `next`, `can_go_prev`, `has_future_history`). `load` returns `history_index: -1` when the file is missing.
  2. Saving is atomic.
  3. Port all of `state_test.go`.
  4. Add a golden round-trip of `state_empty.json` and `state_populated.json`, including the zero `last_changed`.
- **Automated verification:** `cargo test -p kakemono-engine state`
- **Manual verification:** none

## Task 19: Engine core ops (next/prev/set/status) against fakes
- **Files owned:** `crates/engine/src/engine.rs`, `crates/engine/src/lib.rs`, `crates/engine/tests/engine_core.rs`
- **Depends on:** Tasks 7, 17, 18
- **Mode:** sequential
- **Steps:**
  1. Implement the §D structure: `next` uses future history first, then picks sources for the current theme and tries candidates in order. Port `resolve_local_path` (file://, absolute path, else download).
  2. `prev`, `set_file` and `status` (whose `theme` comes from `signals.theme()`).
  3. The engine creates download, trash and favorites dirs on construction.
  4. Port the relevant `manager_test.go` cases using `RecordingSetter` and `StaticProvider` (serving a local wiremock image).
  5. Test that `status()` doesn't block while `next()` is awaiting a slow provider.
- **Automated verification:** `cargo test -p kakemono-engine engine_core`
- **Manual verification:** none

## Task 20: Engine favorite/trash/cache trim
- **Files owned:** `crates/engine/src/files.rs`, `crates/engine/tests/files.rs`
- **Depends on:** Task 19
- **Mode:** sequential
- **Steps:**
  1. Port `copyFile` (with the collision suffix) and `Favorite`.
  2. `Trash` is rename, falling back to copy-then-remove across devices, followed by `next`.
  3. `trimCache` keeps the newest `cache_size` files and never deletes the current image or any image referenced in history within the last 2 entries. The second rule is new and protects GNOME's dark-key image.
  4. Wire these into `Engine::{favorite, trash}` through `pub(crate)` functions. `engine.rs` calls `files::…`; the call sites were stubbed in Task 19.
- **Automated verification:** `cargo test -p kakemono-engine files`
- **Manual verification:** none

## Task 21: Engine rotation loop, pause/resume, signal reactions
- **Files owned:** `crates/engine/src/rotation.rs`, `crates/engine/tests/rotation.rs`
- **Depends on:** Task 19
- **Mode:** sequential
- **Steps:**
  1. `Engine::run(shutdown)` uses `tokio::select!` over the sleep until the next due time, `signals.subscribe()`, a reset notify (fired by manual ops) and shutdown.
  2. Pause and resume persist. Interval changes take effect on `reload`.
  3. Tests (paused clock):
     - ticks at 30 m
     - a manual `next` resets the timer
     - pause suppresses ticks
     - `ThemeChanged(Dark)` triggers `next` with dark sources
     - `SpaceChanged` does nothing when `reapply_on_space_change = false` (the default)
     - `SpaceChanged` with `reapply_on_space_change = true` re-sets the current path without adding to history
     - `Resumed` after an overdue tick rotates
- **Automated verification:** `cargo test -p kakemono-engine rotation`
- **Manual verification:** none

## Task 22: Engine events + hooks
- **Files owned:** `crates/engine/src/events.rs`, `crates/engine/src/hooks.rs`, `crates/engine/tests/hooks.rs`
- **Depends on:** Tasks 8, 19
- **Mode:** sequential
- **Steps:**
  1. Broadcast `EngineEvent::{Changed, Paused, ThemeChanged}` from ops.
  2. `hooks::fire(on_change, info, theme, runner)` renders argv with `platform::exec::render`, runs it with `env_clear()` plus the pass-through list in §I and the `WALLPAPERD_*` env, runs fire-and-forget with a 10 s timeout, and logs failures.
  3. Limit concurrency with a `Semaphore(4)`. A new run for an event whose previous run is still going is dropped and logged at `warn`.
  4. Test with `RecordingRunner`: argv and env after `next()`. `HOME` is passed through, an unrelated parent variable (for example `WPD_SECRET_TEST`) is absent, and five quick `next()` calls with a slow hook record at most 4 in-flight runs.
- **Automated verification:** `cargo test -p kakemono-engine hooks`
- **Manual verification:** none

## Task 23: Daemon IPC server + instance lock
- **Files owned:** `crates/cli/src/daemon/server.rs`, `crates/cli/src/daemon/lock.rs`, `crates/cli/tests/server.rs`
- **Depends on:** Tasks 4, 21, 22
- **Mode:** sequential
- **Steps:**
  1. `lock::acquire(runtime_dir) -> Result<LockGuard, AlreadyRunning>` uses `File::try_lock`.
  2. Server: unlink the stale socket only after the lock is held. Accept loop; one task per connection; dispatch the typed `Request` onto `Engine`.
  3. `subscribe` streams events until the client disconnects. `reload` re-reads the config from disk.
  4. Responses match Go's serialisation exactly (§I). Write every line through `protocol::codec::write_line_json` / `to_vec_go`, never plain `serde_json::to_writer`: Go escapes `<`, `>`, `&` as `\u003c` etc. No crate may enable serde_json's `preserve_order` (sorted `args` keys depend on it).
     - `subscribe`: reply with one `{"ok":true}` line, then event lines (the client expects exactly this handshake; see `.pi/contracts/protocol.md`).
     - Bind via `Endpoint::bind()`. It creates the parent dir with mode 0700 and sets the socket to 0600, but does not remove a stale socket file. The server does that, only while holding the lock (step 2).
  5. Integration test: start the server on a tempdir socket with a fake engine platform and drive it through `protocol::Client` for every command. Snapshot the raw response lines with insta. A second lock attempt fails.
- **Automated verification:** `cargo test -p kakemono --test server`
- **Manual verification:** none

## Task 24: macOS host + NSWorkspace setter
- **Files owned:** `crates/platform/src/macos/mod.rs`, `crates/platform/src/macos/mainloop.rs`, `crates/platform/src/macos/setter.rs`
- **Depends on:** Task 7
- **Mode:** parallel-safe (macOS lane)
- **Steps:**
  0. **Spike first (from Plan C).** Write `examples/spike_set.rs`, about 30 lines: a plain binary that, from a tokio worker thread, hops to the main thread and calls `setDesktopImageURL` for each screen. Confirm it works when started from a terminal and from a LaunchAgent before building `mainloop`. If NSApplication misbehaves under launchd, take the `CFRunLoopRun` fallback in Risk 4.
  1. `mainloop::host` builds the tokio runtime, spawns `f` on it, and runs NSApplication with the Prohibited policy on the main thread. When the future completes, it stops the app.
  2. `MacSetter::set` dispatches to the main queue: for each `NSScreen`, `setDesktopImageURL_forScreen_options_error` with options from `Fit`. A oneshot returns the result.
  3. Confirm the objc2-app-kit 0.3.x signatures and `MainThreadMarker` usage (UNVERIFIED in the research).
  4. Add an `#[ignore]` test that sets a generated PNG and reads it back with `desktopImageURLForScreen`.
- **Automated verification:** `cargo build -p kakemono-platform && cargo test -p kakemono-platform --features fake -- --ignored macos_set_roundtrip` (on macOS)
- **Manual verification:** with two displays, run the ignored test and confirm both screens change and no Automation permission prompt appears

## Task 25: macOS signals (space, wake, screens) + theme via dark-light
- **Files owned:** `crates/platform/src/macos/signals.rs`
- **Depends on:** Task 24
- **Mode:** sequential
- **Steps:**
  1. On the main thread, register block observers on `NSWorkspace.notificationCenter` for ActiveSpaceDidChange and DidWake, and on `NSNotificationCenter.defaultCenter` for `NSApplicationDidChangeScreenParametersNotification`. Forward each into a `broadcast::Sender<Signal>`.
  2. Theme: start a `dark_light::stream()` task mapped to `ThemeChanged`, deduplicated.
  3. Make `macos::open_platform()` return `Platform`.
- **Automated verification:** `cargo clippy -p kakemono-platform -- -D warnings` (macOS)
- **Manual verification:** with a debug `examples/signals.rs` printing signals: switch Space, sleep and wake, toggle Appearance, and plug in a display. Each prints its signal within about 1 s.

## Task 26: Windows host + IDesktopWallpaper setter
- **Files owned:** `crates/platform/src/windows/mod.rs`, `crates/platform/src/windows/setter.rs`
- **Depends on:** Task 7
- **Mode:** parallel-safe (Windows lane)
- **Steps:**
  1. A desktop thread with an STA COM apartment and an mpsc request loop calls `IDesktopWallpaper::SetPosition` and `SetWallpaper(null, path)`, falling back to `SystemParametersInfoW`.
  2. `windows::host` runs the tokio runtime on a worker thread and the message window (Task 27) on the main thread.
  3. Add an `#[ignore]` test that sets an image and reads it back with `GetWallpaper` for monitor 0.
- **Automated verification:** `cargo check -p kakemono-platform --target x86_64-pc-windows-msvc`; on Windows CI, `cargo test -p kakemono-platform`
- **Manual verification:** on Windows 10 and 11 with two monitors, run the ignored test and confirm both monitors change

## Task 27: Windows signals + theme
- **Files owned:** `crates/platform/src/windows/signals.rs`
- **Depends on:** Task 26
- **Mode:** sequential
- **Steps:**
  1. Create a hidden message-only window; `WM_DISPLAYCHANGE` → `DisplaysChanged` and `WM_POWERBROADCAST` resume → `Resumed`.
  2. Theme: use `dark_light::stream()`. If it turns out not to watch, or reads `AppsUseLightTheme`, write a `RegNotifyChangeKeyValue` watcher on `SystemUsesLightTheme` on a blocking thread instead.
  3. `windows::open_platform()`.
- **Automated verification:** `cargo check -p kakemono-platform --target x86_64-pc-windows-msvc`
- **Manual verification:** on Windows, toggle Settings → Personalization → Colors → "Choose your mode" and change the display resolution; both print signals in `examples/signals.rs`

## Task 28: Linux detection env capture + pure resolver
- **Files owned:** `crates/platform/src/linux/detect/mod.rs`, `crates/platform/src/linux/detect/env.rs`, `crates/platform/src/linux/detect/resolve.rs`, `crates/platform/tests/linux_resolve.rs`, `crates/platform/tests/snapshots/linux_resolve__*.snap`
- **Depends on:** Task 7
- **Mode:** parallel-safe (Linux lane)
- **Steps:**
  1. Write `Env` (§E) and `capture()`, which reads env vars, runs `which` probes and reads `/proc/*/comm`. Probing D-Bus names is async and optional.
  2. `resolve(&Env, &DesktopConfig) -> Resolution { backend: BackendKind, set_command: Option<Vec<String>>, persistent: bool, reasons: Vec<String>, session_key: String }`.
  3. A table test with at least 25 scenarios and an insta snapshot. Scenarios include:
     - GNOME on Wayland
     - Ubuntu (`ubuntu:GNOME`)
     - KDE on X11 and Wayland
     - XFCE
     - X-Cinnamon
     - MATE
     - LXQt
     - Hyprland with a running hyprpaper; Hyprland with only awww; Hyprland with only swww
     - sway
     - niri with swaybg only; niri with nothing
     - i3 with feh; i3 with xwallpaper
     - a pinned backend
     - COSMIC
     - an empty environment
     - session drift
  4. The module is cfg-gated to Linux, but `resolve` is pure, so expose it under `cfg(any(target_os="linux", test))` to run the table test on all CI OSes.
- **Automated verification:** `cargo test -p kakemono-platform --test linux_resolve && cargo insta test -p kakemono-platform --check`
- **Manual verification:** none

## Task 29: Linux command + preset backends (argv table)
- **Files owned:** `crates/platform/src/linux/backends/command.rs`, `crates/platform/src/linux/backends/presets.rs`, `crates/platform/tests/linux_presets.rs`
- **Depends on:** Tasks 8, 28
- **Mode:** sequential
- **Steps:**
  1. `presets::argv(kind, fit) -> Vec<String>` covers:
     - sway: `swaymsg output * bg {path} <fill>`
     - hyprpaper: `hyprctl hyprpaper wallpaper ,{path}`. The syntax is UNVERIFIED against 0.8.x; keep it in one table entry so a fix is one line.
     - awww and swww: `<bin> img {path} --transition-type fade`
     - xwallpaper: `--zoom`/`--maximize`… mapped from `Fit`
     - feh: `--bg-fill`/`--bg-max`…
     - nitrogen: `--set-zoom-fill --save`
     - hsetroot: `-fill`
     - pcmanfm-qt and pcmanfm: `--set-wallpaper {path} --wallpaper-mode=…`
  2. `CommandSetter` renders through `exec` and runs it with the runner.
  3. Snapshot every preset's argv. Integration test with `tests/bin/record-argv` symlinked as `awww` on `PATH`.
- **Automated verification:** `cargo test -p kakemono-platform --test linux_presets`
- **Manual verification:** none

## Task 30: Linux persistent-process supervisor (swaybg/wbg)
- **Files owned:** `crates/platform/src/linux/backends/persistent.rs`, `crates/platform/tests/linux_persistent.rs`, `crates/platform/tests/bin/fake-swaybg`
- **Depends on:** Task 29
- **Mode:** sequential
- **Steps:**
  1. Implement §E: spawn the new process in its own group, wait 300 ms on the paused clock, check the old PID's `/proc/comm`, SIGTERM it (via `nix` or `rustix`, as chosen in Task 2), and write `setter.pid`. On start, adopt an existing PID.
  2. Test with `fake-swaybg`, a script that sleeps: after two sets only the second process is alive, and a PID whose comm doesn't match is not killed.
- **Automated verification:** `cargo test -p kakemono-platform --test linux_persistent` (Linux CI)
- **Manual verification:** on niri or a wlroots compositor with swaybg, run `kakemono next` three times and confirm one swaybg process (`pgrep -c swaybg` = 1) and no black flash

## Task 31: Linux gsettings backends (GNOME/Cinnamon/MATE)
- **Files owned:** `crates/platform/src/linux/backends/gsettings.rs`, `crates/platform/tests/linux_gsettings.rs`
- **Depends on:** Task 29
- **Mode:** parallel-safe (with Tasks 32 and 33)
- **Steps:**
  1. Build these argv sequences:
     - GNOME: `gsettings set org.gnome.desktop.background picture-uri {uri}`, the same for `picture-uri-dark`, and `picture-options zoom|scaled|centered|stretched|wallpaper`
     - Cinnamon: `org.cinnamon.desktop.background picture-uri`
     - MATE: `org.mate.background picture-filename {path}`
  2. Snapshot the argv sequences with `RecordingRunner`, and test with a fake `gsettings` on `PATH`.
- **Automated verification:** `cargo test -p kakemono-platform --test linux_gsettings`
- **Manual verification:** on GNOME 4x Wayland, run `kakemono set img.jpg`, then toggle dark mode; the wallpaper stays the same

## Task 32: Linux KDE Plasma D-Bus backend
- **Files owned:** `crates/platform/src/linux/backends/kde.rs`, `crates/platform/tests/linux_kde.rs`
- **Depends on:** Task 28
- **Mode:** parallel-safe
- **Steps:**
  1. `script_for(uri, fit) -> String` produces JS that sets `org.kde.image` on all `desktops()`. The URI is embedded with `serde_json::to_string`.
  2. Call it with zbus: `org.kde.plasmashell` `/PlasmaShell` `org.kde.PlasmaShell.evaluateScript`.
  3. Snapshot the script. An injection test checks that a path containing `"); evil(); ("` stays inside a string literal.
- **Automated verification:** `cargo test -p kakemono-platform --test linux_kde`
- **Manual verification:** on Plasma 6 (Wayland and X11) with two screens, both change

## Task 33: Linux XFCE xfconf D-Bus backend
- **Files owned:** `crates/platform/src/linux/backends/xfce.rs`, `crates/platform/tests/linux_xfce.rs`
- **Depends on:** Task 28
- **Mode:** parallel-safe
- **Steps:**
  1. Over zbus `org.xfce.Xfconf`: `GetAllProperties("xfce4-desktop","/backdrop")`, then for every key ending in `/last-image`, `SetProperty(…, path)` and the matching `/image-style` from `Fit`.
  2. Make key selection a pure function and unit-test it with sample property maps.
- **Automated verification:** `cargo test -p kakemono-platform --test linux_xfce`
- **Manual verification:** on XFCE 4.18+ with two monitors, both change

## Task 34: Linux theme/resume signals + `linux::open_platform`
- **Files owned:** `crates/platform/src/linux/mod.rs`, `crates/platform/src/linux/theme.rs`
- **Depends on:** Tasks 29–33
- **Mode:** sequential
- **Steps:**
  1. Theme: dark-light `stream()` (`tokio`), falling back to `[desktop] theme`, then to a `gsettings get org.gnome.desktop.interface color-scheme` read if the portal is absent.
  2. Resume: logind `PrepareForSleep(false)` via zbus, on the system bus.
  3. `open_platform(cfg)` runs resolve and constructs the matching backend.
  4. Track consecutive `SetError`s and re-detect in memory (§E) via a wrapper setter, `ResilientSetter`.
  5. Unit-test `ResilientSetter` with fakes.
- **Automated verification:** `cargo test -p kakemono-platform` (Linux CI); `cargo check -p kakemono-platform --target x86_64-unknown-linux-gnu` (macOS)
- **Manual verification:** on GNOME, toggle dark mode; `kakemono status` shows the theme flip and the wallpaper rotates

## Task 35: CLI parity commands
- **Files owned:** `crates/cli/src/main.rs`, `crates/cli/src/cli.rs`, `crates/cli/src/commands/{next,prev,set,favorite,trash,pause,resume,status}.rs`, `crates/cli/src/output.rs`, `crates/cli/tests/cli.rs`
- **Depends on:** Task 23
- **Mode:** sequential
- **Prerequisite (from Task 18):** add a public indented Go-format writer to `kakemono-protocol::codec` (`to_vec_go_pretty`, matching `json.MarshalIndent` with two spaces and Go escaping). Use it for `status --json`, and switch `engine/state.rs` from its private copy of that printer to this one. The Go fixtures `status_json.txt` and `state_*.json` must still round-trip.
- **Steps:**
  1. Write the clap derive tree from §I, with global `--config` and `--socket`.
  2. Port `printWallpaperInfo` and the status text exactly.
  3. `status --json` prints two-space-indented JSON.
  4. `set` canonicalises the path to an absolute path.
  5. assert_cmd tests start the in-process server from Task 23 on a temp socket. `status --json` is compared against `fixtures/status_json.txt` (semantically). Also test that the daemon-not-running message is printed.
- **Automated verification:** `cargo test -p kakemono --test cli`
- **Manual verification:** none

## Task 36: `daemon` command wiring (host, logging, shutdown, detach)
- **Files owned:** `crates/cli/src/daemon/mod.rs`, `crates/cli/src/commands/daemon.rs`, `crates/cli/src/logging.rs`
- **Depends on:** Tasks 25, 27, 34, 35
- **Mode:** sequential
- **Steps:**
  1. `daemon -f` loads the config, then `platform::host(&cfg.desktop, |p| async { engine + server + run })`.
  2. Shut down on SIGINT, SIGTERM or Ctrl-C (`tokio::signal`), and save state on exit.
  3. Without `-f`, re-exec detached (§I).
  4. Logging goes to the file appender, or to stderr in foreground mode.
  5. Write the PID file for compatibility with `daemon.pid`.
- **Automated verification:** `cargo build -p kakemono && cargo test -p kakemono` (the CLI tests use the fake platform behind the `fake` feature through `WALLPAPERD_FAKE_PLATFORM=1`, test builds only)
- **Manual verification:** on each OS, run `kakemono daemon -f`, then `kakemono next` changes the wallpaper and `Ctrl-C` exits cleanly. On macOS, there's no Dock icon.

## Task 37: `detect` / `doctor` / first-run persistence
- **Files owned:** `crates/cli/src/commands/detect.rs`, `crates/cli/src/commands/doctor.rs`, `crates/cli/src/firstrun.rs`, `crates/cli/tests/detect.rs`
- **Depends on:** Tasks 6, 34, 36
- **Mode:** sequential
- **Steps:**
  1. `detect` prints the resolution and reasons. `--write` persists it through `config::edit::set_desktop` and respects `managed_by` (`--force`).
  2. `firstrun::ensure_desktop(&mut cfg)` is called by the daemon at start; it handles the missing-section and session-drift cases.
  3. `doctor` prints:
     - the env snapshot
     - probes
     - the resolution
     - a rendered argv
     - tray watcher presence (Linux)
     - missing compositor variables under systemd
     - picker window-rule snippets
     - the daemon status
   `--apply` re-sets the current image.
  4. On macOS and Windows, `doctor` prints the native backend info.
  5. Tests drive a captured `Env` through a hidden `--env-json` flag, then snapshot the config file after `--write`.
- **Automated verification:** `cargo test -p kakemono --test detect`
- **Manual verification:** on a Hyprland box, delete `[desktop]`, start the daemon, and confirm `config.toml` gains `[desktop] backend = "hyprpaper"|"awww"` with comments preserved

## Task 38: platform::open (URL/reveal) + `open` command
- **Files owned:** `crates/platform/src/open.rs`, `crates/cli/src/commands/open.rs`
- **Depends on:** Tasks 25, 27, 34, 35
- **Mode:** sequential
- **Steps:**
  1. Write `open_url(&str)` and `reveal(&Path)` per §G.
  2. The `open` command offers `--folder` (download dir), `--favorites`, and by default reveals the current wallpaper (from IPC status), as Go did.
- **Automated verification:** `cargo clippy --workspace -- -D warnings && cargo check -p kakemono-platform --target x86_64-pc-windows-msvc --target x86_64-unknown-linux-gnu`
- **Manual verification:** `kakemono open` selects the file in Finder, Explorer or the Linux file manager

## Task 39: `service install|uninstall|status`
- **Files owned:** `crates/platform/src/service/{mod.rs,launchd.rs,systemd.rs,windows.rs}`, `crates/cli/src/commands/service.rs`, `crates/platform/tests/service.rs`
- **Depends on:** Task 36
- **Mode:** sequential
- **Steps:**
  1. macOS uses auto-launch in LaunchAgent mode, or its own plist at `~/Library/LaunchAgents/dev.rielj.kakemono.plist` with `RunAtLoad` and `KeepAlive` if auto-launch can't set KeepAlive. The plist runs `kakemono daemon -f`.
  2. Linux uses `~/.config/systemd/user/kakemono.service` (`PartOf`/`WantedBy=graphical-session.target`, `Restart=on-failure`), then `systemctl --user enable --now`, and prints the compositor snippets.
  3. Windows uses auto-launch (HKCU Run) with `kakemono.exe daemon`.
  4. Snapshot the generated plist and unit with insta.
- **Automated verification:** `cargo test -p kakemono-platform --test service`
- **Manual verification:** on each OS, install the service, log out and back in, and confirm `kakemono status` reports a running daemon

## Task 40: Tauri app scaffold + merged frontend + tray
- **Files owned:** `crates/ui/src-tauri/**` (`Cargo.toml`, `tauri.conf.json`, `capabilities/default.json`, `src/main.rs`, `src/tray.rs`, `src/daemon_link.rs`), `crates/ui/frontend/**` (moved from `gui/frontend` and `picker/frontend`), and the root `Cargo.toml` members line
- **Depends on:** Tasks 4, 6
- **Mode:** sequential (UI lane start)
- **Steps:**
  1. Run `git mv gui/frontend crates/ui/frontend` and merge the picker sources into `src/picker/`. Set up Vite multi-page.
  2. Delete `wailsjs/` and `services/wails.ts`. Generate ts-rs bindings into `src/shared/bindings`.
  3. Tauri 2.12: tray-only startup with `prevent_exit`, and plugins single-instance, dialog, opener and autostart.
  4. `daemon_link` holds a shared `protocol::Client`.
  5. Tray menu items call IPC. On Linux, check for the StatusNotifierWatcher and skip the tray if it's missing.
- **Automated verification:** `cd crates/ui/frontend && npm ci && npm run build && npm run typecheck && cd - && cargo build -p kakemono-ui`
- **Manual verification:** launch `kakemono-ui` with the daemon running; the tray shows and Next works (macOS menubar, Windows notification area, KDE)

## Task 41: Settings window on Tauri commands
- **Files owned:** `crates/ui/src-tauri/src/commands.rs`, `crates/ui/frontend/src/shared/api.ts`, `crates/ui/frontend/src/settings/**`
- **Depends on:** Task 40
- **Mode:** sequential
- **Steps:**
  1. Implement the commands in §H.
  2. `save_config` takes a JSON patch, applies it with `config::edit::apply_patch`, then sends IPC `reload`.
  3. Replace the osascript directory and file pickers with plugin-dialog. "Reveal" and "Open folder" use the opener plugin.
  4. Port the components: General, Providers (renamed Sources), Themes, Favorites, StatusBar.
  4b. On macOS only, General gets a "Keep wallpaper on every Space" switch bound to `daemon.reapply_on_space_change` (default off). The helper text below it reads: "Re-applies the wallpaper when you switch Spaces. This turns off 'Show on all Spaces' in System Settings."
  5. Add a vitest for interval validation that shares its table with `duration.rs` (a JSON file with cases, read by both).
- **Automated verification:** `cd crates/ui/frontend && npm test && npm run typecheck && cd - && cargo test -p kakemono-config duration`
- **Manual verification:** change the interval in the UI and save; `config.toml` keeps its comments and `kakemono status` shows the new interval without a daemon restart

## Task 42: Settings Desktop + Hooks panels
- **Files owned:** `crates/ui/frontend/src/settings/DesktopSettings.tsx`, `crates/ui/frontend/src/settings/HooksSettings.tsx`, `crates/ui/src-tauri/src/commands_desktop.rs`
- **Depends on:** Tasks 37, 41
- **Mode:** sequential
- **Steps:**
  1. The Desktop panel shows the detected backend, its reasons, and an editable argv list (one token per row, so there's no quoting confusion). It has "Re-detect", "Test" (doctor `--apply`) and "Pin" (`managed_by = "user"`) controls.
  2. The Hooks panel edits `on_change` and shows the env variable reference.
  3. On macOS and Windows the Desktop panel shows only the native backend info and the Fit option.
- **Automated verification:** `cd crates/ui/frontend && npm run typecheck && npm test`
- **Manual verification:** on Linux, edit `set_command`, press Test, and the wallpaper changes

## Task 43: Picker window, positioning tiers, layer-shell spike
- **Files owned:** `crates/ui/src-tauri/src/picker.rs`, `crates/ui/src-tauri/src/picker_place.rs`, `crates/ui/src-tauri/tests/picker_place.rs`, `docs/spikes/layer-shell.md`
- **Depends on:** Task 40
- **Mode:** sequential
- **Steps:**
  1. Pre-create the picker window hidden (decorations off, always on top, skip taskbar, visible on all workspaces, app_id/class `kakemono-picker`).
  2. `show_picker()` places it on the monitor under the cursor at `[picker] position` (Tier 1). On Wayland it doesn't position, and the size depends on orientation (Tier 2).
  3. Close on blur if configured.
  4. `force_x11` relaunches with `GDK_BACKEND=x11`.
  5. **User placement (§H).** `picker_place::Placer` has `restore(&Custom)` and `capture() -> Option<Custom>`, with these implementations:
     - `TauriPlacer` (macOS, Windows, X11): `outer_position` on `Moved`, debounced 300 ms; `set_position` on show.
     - `HyprPlacer`: `hyprctl clients -j` and `hyprctl dispatch movewindowpixel exact X Y,class:kakemono-picker`.
     - `SwayPlacer`: `swaymsg -t get_tree` and `swaymsg '[app_id="kakemono-picker"] move absolute position X Y'`.
     - `NoopPlacer` (GNOME, KDE, niri, others).
     When the user drops the window, write `placement = "custom"` and `custom` through `config::edit`. `reset_picker_position` sets `placement = "edge"`. If the saved monitor is missing or the rect is off every screen, fall back to `edge`.
  6. **Spike** (time-boxed): build a separate `examples/layer_picker.rs` with raw wry and gtk-layer-shell, and record whether the result is clean in `docs/spikes/layer-shell.md`. Ship nothing from the spike.
- **Automated verification:** `cargo build -p kakemono-ui && cargo clippy -p kakemono-ui -- -D warnings && cargo test -p kakemono-ui picker_place` (parses fixture `hyprctl clients -j` and `swaymsg get_tree` JSON, snapshots the restore argv, and checks that an off-screen rect falls back to `edge`)
- **Manual verification:**
  - macOS: the picker appears at the bottom of the display under the cursor, including over a fullscreen Space. Drag it, close it, reopen it: it reopens where it was dropped.
  - Windows: correct edge, and drag plus reopen keeps the position.
  - X11 i3 (floating): correct edge, and drag plus reopen keeps the position.
  - Hyprland: centred, and it docks at the bottom with the documented window rule. Drag it (with the `float` rule) and reopen: the position is restored.
  - sway: drag plus reopen restores the position.
  - GNOME: dragging works, and it reopens centred.

## Task 44: Picker thumbnails protocol + keyboard a11y + design pass
- **Files owned:** `crates/ui/src-tauri/src/thumbs.rs`, `crates/ui/frontend/src/picker/**`, `crates/ui/frontend/src/shared/tokens.css`, `docs/ui-checklist.md`
- **Depends on:** Task 43
- **Mode:** sequential
- **Steps:**
  1. Register the `thumb://` protocol with the id→path allowlist and generate thumbnails with `image` (384 px JPEG cached by content hash). Unit-test that disallowed paths return 403.
  2. Picker: listbox semantics and the key map from §H, roving focus, an undo toast for trash, a reduced-motion guard and system colour tokens. Remove the emoji and any gradients.
  2b. Add a thin drag handle (`data-tauri-drag-region`, cursor `grab`, labelled "Move picker") at the strip's leading edge, and a "Reset position" item in the context menu that calls `reset_picker_position`. Dragging must not start from thumbnails, so click-to-set still works.
  3. Write vitest and testing-library tests for keyboard navigation (arrows, Home/End, Enter calls `set_file`, Esc closes).
  4. Write `docs/ui-checklist.md`, the design-standards checklist from §H, and tick it.
- **Automated verification:** `cargo test -p kakemono-ui thumbs && cd crates/ui/frontend && npm test`
- **Manual verification:**
  - VoiceOver (macOS) and Narrator (Windows) read "title — source, selected".
  - Toggle reduce motion; there is no animation.
  - Light and dark modes have no flash on open.
  - Complete the checklist review.

## Task 45: Live updates via `subscribe`
- **Files owned:** `crates/ui/src-tauri/src/events.rs`, `crates/ui/frontend/src/shared/useDaemonEvents.ts`
- **Depends on:** Tasks 23, 41
- **Mode:** sequential
- **Steps:**
  1. A background task subscribes, reconnects with backoff (0.5 s to 10 s), and emits `wallpaper://changed`, `wallpaper://paused` and `daemon://connection`. The tray menu label (Pause/Resume) updates.
  2. The settings StatusBar and the picker's current-selection highlight subscribe to these events.
- **Automated verification:** `cargo build -p kakemono-ui && cd crates/ui/frontend && npm run typecheck`
- **Manual verification:** with settings open, run `kakemono next` in a terminal; the StatusBar updates in under 1 s. Kill the daemon; the UI shows "Daemon not running" and recovers when it's restarted.

## Task 46: UI ↔ daemon lifecycle (sidecar spawn, `pick`/`settings` commands)
- **Files owned:** `crates/cli/src/commands/{pick,settings}.rs`, `crates/ui/src-tauri/src/lifecycle.rs`
- **Depends on:** Tasks 36, 43, 45
- **Mode:** sequential
- **Steps:**
  1. At startup the UI spawns the sidecar `kakemono daemon` if IPC is unavailable. The "Start at login" toggle drives `service install` (daemon) and plugin-autostart (UI).
  2. `kakemono pick` and `kakemono settings` locate `kakemono-ui`:
     - next to the current exe
     - in the .app `Contents/MacOS`
     - via `WALLPAPERD_UI`
     - on `PATH`
     It spawns the UI with `--picker` or `--settings`. single-instance forwards these flags to the running UI.
  3. If the UI isn't found, print an install hint.
- **Automated verification:** `cargo test -p kakemono --test cli -- pick_missing_ui_hint`
- **Manual verification:** with the tray running, `kakemono pick` shows the picker within 200 ms; from cold, within about 1 s

## Task 47: Packaging + release workflow
- **Files owned:** `.github/workflows/release.yml`, `crates/ui/src-tauri/tauri.conf.json` (bundle section), `crates/ui/src-tauri/icons/**`, `scripts/build-sidecar.sh`
- **Depends on:** Task 46
- **Mode:** sequential
- **Steps:**
  1. Build `kakemono` per target triple and copy it to `src-tauri/binaries/kakemono-<triple>` for `externalBin`.
  2. Tauri bundles dmg, app, deb, rpm, AppImage, msi and nsis.
  3. CLI tarballs and zips cover macOS (arm64, x64), Linux (x64, arm64 with `cross`) and Windows (x64).
  4. Reuse the Go app icons (`gui/build/appicon.png`).
- **Automated verification:** `cargo tauri build --debug` locally; the release workflow runs on `workflow_dispatch` and produces artifacts
- **Manual verification:** install the dmg, deb and msi on clean machines or VMs; first run detects the backend and rotates

## Task 48: Docs, migration guide, contrib hooks
- **Files owned:** `README.md`, `docs/migration.md`, `docs/linux-backends.md`, `contrib/sketchybar/**` (moved from `sketchybar/`), `contrib/waybar/**`
- **Depends on:** Task 37
- **Mode:** parallel-safe
- **Steps:**
  1. `README.md` covers install, the CLI and the config reference.
  2. `migration.md` says that existing config and state work, how to add the `[hooks]` sketchybar trigger, socket path notes, and the Spaces limitation.
  3. `linux-backends.md` has a table of detection results, presets, the systemd env caveat, picker window rules and the NVIDIA WebKit workaround.
  4. The waybar example is a module plus a hook.
- **Automated verification:** `test -f docs/migration.md && grep -q "on_change" contrib/sketchybar/README.md`
- **Manual verification:** a human proofreads the docs

## Task 49: Cutover — delete Go code
- **Files owned:** deletes `cmd/`, `internal/`, `gui/` (remaining), `picker/` (remaining), `main.go`, `go.mod`, `go.sum`, `coverage.out`, `kakemono` (binary), `tools/goldens/`, `~/`. The golden fixtures stay.
- **Depends on:** Tasks 1–48
- **Mode:** sequential
- **Steps:**
  1. Confirm the parity checklist: every Go cobra command has a Rust equivalent, and every Go test case was ported. Grep the Go `_test.go` function names against a Rust test name map in `docs/migration.md`.
  2. Delete the Go code.
  3. Keep the `go-final` tag for reference.
  4. Decide whether to keep `variety-reference/`. Recommendation: remove it from the repo (GPL-3) and link to upstream from `docs/linux-backends.md`.
- **Automated verification:** `test ! -e go.mod && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && (cd crates/ui/frontend && npm run build)`
- **Manual verification:** a one-week daily-driver run on the primary macOS machine with the sketchybar hook; then a Linux session (Hyprland or GNOME) and a Windows VM for smoke tests
