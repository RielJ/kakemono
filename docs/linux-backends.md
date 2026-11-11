# Linux backends

On Linux, kakemono sets the wallpaper through a backend: a desktop API, or a command such as `swww` or `feh`. This page lists how kakemono picks the backend, the command each backend runs, and the setup that some sessions need.

Run `kakemono detect` to see the backend for the current session and why kakemono picked it. Run `kakemono doctor` to see the environment, the command a `set` would run, and what the running daemon uses.

## How kakemono picks a backend

The `[desktop]` section of `config.toml` controls the backend:

| Key | Values | Default |
|---|---|---|
| `backend` | `auto`, `native`, `gnome`, `kde`, `xfce`, `cinnamon`, `mate`, `sway`, `hyprpaper`, `awww`, `swww`, `swaybg`, `wbg`, `xwallpaper`, `feh`, `nitrogen`, `hsetroot`, `pcmanfm`, `pcmanfm-qt`, `command` | `auto` |
| `set_command` | An argv array, or a string that kakemono splits like a shell would. kakemono never runs it through a shell. | unset |
| `persistent` | `true` if `set_command` keeps running, like `swaybg` | `false` |
| `fit` | `fill`, `fit`, `center`, `stretch`, `tile` | `fill` |
| `theme` | `auto`, `light`, `dark`. Overrides the system theme when kakemono can't read it, for example on a bare X11 window manager. | `auto` |
| `managed_by` | `detect` lets kakemono rewrite `[desktop]` when the session changes. `user` means kakemono never touches it. | `detect` |
| `detected_session` | Written by kakemono, for example `"hyprland:wayland"` | unset |
| `detected_at` | Written by kakemono, an RFC 3339 time | unset |

On Linux, `native` means the same as `auto`.

kakemono resolves the backend in this order. The first match wins.

1. **A pinned backend.** Any `backend` other than `auto` or `native` is used as is. One exception: with `managed_by = "detect"`, if `detected_session` doesn't match the current session (for example, you logged into GNOME after Hyprland), kakemono detects again.
2. **A full desktop environment**, from the first of `XDG_CURRENT_DESKTOP`, `XDG_SESSION_DESKTOP`, and `DESKTOP_SESSION` that is set. kakemono also picks `kde` when `KDE_FULL_SESSION=true`, or when `org.kde.plasmashell` is on the session bus and no compositor variable is set.
3. **A wallpaper daemon that is already running.** kakemono checks for `awww-daemon`, `swww-daemon`, `hyprpaper`, and `wpaperd`, in that order.
4. **The compositor.** kakemono checks `HYPRLAND_INSTANCE_SIGNATURE`, `SWAYSOCK`, and `NIRI_SOCKET`, then any Wayland session.
5. **An X11 setter**, in an X11 session with no known desktop environment.
6. **Unconfigured.** kakemono can't set a wallpaper. `doctor` and the daemon's error say what to install or configure.

### Detection results

| What kakemono finds | Backend |
|---|---|
| Desktop name `kde`, `plasma`, `plasmawayland`, or `plasmax11` | `kde` |
| Desktop name `gnome`, `gnome-xorg`, `gnome-wayland`, `gnome-classic`, `gnome-flashback`, `ubuntu`, `ubuntu-wayland`, `ubuntu-xorg`, `pop`, `unity`, `budgie`, or `budgie-desktop` | `gnome` |
| Desktop name `x-cinnamon`, `cinnamon`, `cinnamon2d`, or `cinnamon-wayland` | `cinnamon` |
| Desktop name `mate` | `mate` |
| Desktop name `xfce` or `xfce4` | `xfce` |
| Desktop name `lxqt` | `pcmanfm-qt` |
| Desktop name `lxde` | `pcmanfm` |
| Desktop name `cosmic` | `command`. COSMIC has no wallpaper command that kakemono can drive, so you must set `set_command`. |
| `awww-daemon` is running | `awww` |
| `swww-daemon` is running | `swww` |
| `hyprpaper` is running | `hyprpaper` |
| `wpaperd` is running | `command`. wpaperd manages the wallpaper itself. Stop wpaperd, or set `set_command`. |
| Hyprland | the first installed of `hyprpaper`, `awww`, `swww`, and `swaybg`. If none is installed, the next row applies. |
| sway | `sway` (`swaymsg output * bg`) |
| niri, or another Wayland session | the first installed of `awww`, `swww`, `swaybg`, and `wbg` |
| X11, no known desktop | the first installed of `xwallpaper`, `feh`, `nitrogen`, and `hsetroot` |
| No `WAYLAND_DISPLAY`, `DISPLAY`, or `XDG_SESSION_TYPE` | unconfigured |

kakemono matches desktop names without regard to case. It splits `XDG_CURRENT_DESKTOP` on `:` and uses the first name it knows, so `ubuntu:GNOME` selects `gnome`.

### When kakemono saves the result

On start, the daemon detects the backend and writes it to `[desktop]` with `detected_session` and `detected_at`. A later start with the same session reuses the saved backend. The daemon doesn't write to `[desktop]` when:

- `managed_by = "user"`;
- `backend = "native"`, which makes the daemon detect again on every start;
- `backend` is already pinned for this session (step 1 above);
- the detected backend can't set a wallpaper (unconfigured, or `command` without `set_command`);
- `set_command` is set and the detected backend is not `command`;
- the config is a legacy file that kakemono reads but doesn't write.

`kakemono detect --write` saves the detected backend from your shell. It refuses a section with `managed_by = "user"` unless you pass `--force`, and it refuses the legacy config. Run `kakemono config init` first in that case.

## Commands that each backend runs

The desktop backends set the wallpaper through the desktop's own settings:

| Backend | How | `fill`, `fit`, `center`, `stretch`, `tile` |
|---|---|---|
| `gnome` | `gsettings set org.gnome.desktop.background` `picture-uri` and `picture-uri-dark` to `{uri}`, then `picture-options` | `zoom`, `scaled`, `centered`, `stretched`, `wallpaper` |
| `cinnamon` | `gsettings set org.cinnamon.desktop.background` `picture-uri` to `{uri}`, then `picture-options` | as `gnome` |
| `mate` | `gsettings set org.mate.background` `picture-filename` to `{path}`, then `picture-options` | as `gnome` |
| `kde` | A Plasma script sent over D-Bus (`org.kde.plasmashell` `evaluateScript`) that sets `Image` and `FillMode` on every desktop | `FillMode` 2, 1, 6, 0, 3 |
| `xfce` | xfconf over D-Bus: every `/backdrop/.../last-image` key of `xfce4-desktop`, plus its `image-style` | `image-style` 5, 4, 1, 3, 2 |

GNOME older than 42 has no `picture-uri-dark` key. That one step fails and the `set` still succeeds.

XFCE has no `last-image` keys until you set a wallpaper once in **Desktop Settings**. Until then, `set` fails and says so.

The preset backends run one command per `set`. `{path}` is the absolute image path.

| Backend | `fill` | `fit` | `center` | `stretch` | `tile` |
|---|---|---|---|---|---|
| `sway` | `swaymsg output * bg {path} fill` | `... fit` | `... center` | `... stretch` | `... tile` |
| `hyprpaper` | `hyprctl hyprpaper wallpaper ,{path}` | `hyprctl hyprpaper wallpaper ,contain:{path}` | as `fill` | as `fill` | `hyprctl hyprpaper wallpaper ,tile:{path}` |
| `awww` | `awww img {path} --resize crop --transition-type fade` | `--resize fit` | `--resize no` | `--resize stretch` | `--resize no` |
| `swww` | `swww img {path} --resize crop --transition-type fade` | `--resize fit` | `--resize no` | `--resize stretch` | `--resize no` |
| `swaybg` | `swaybg -i {path} -m fill` | `-m fit` | `-m center` | `-m stretch` | `-m tile` |
| `wbg` | `wbg {path}` | same | same | same | same |
| `xwallpaper` | `xwallpaper --zoom {path}` | `--maximize` | `--center` | `--stretch` | `--tile` |
| `feh` | `feh --bg-fill {path}` | `--bg-max` | `--bg-center` | `--bg-scale` | `--bg-tile` |
| `nitrogen` | `nitrogen --set-zoom-fill --save {path}` | `--set-zoom` | `--set-centered` | `--set-scaled` | `--set-tiled` |
| `hsetroot` | `hsetroot -cover {path}` | `-full` | `-center` | `-fill` | `-tile` |
| `pcmanfm` | `pcmanfm --set-wallpaper {path} --wallpaper-mode=crop` | `=fit` | `=center` | `=stretch` | `=tile` |
| `pcmanfm-qt` | `pcmanfm-qt --set-wallpaper {path} --wallpaper-mode=zoom` | `=fit` | `=center` | `=stretch` | `=tile` |

The snapshot `crates/platform/tests/it/snapshots/it__linux_presets__presets.snap` holds the full argv of every preset. `cargo test -p kakemono-platform --test linux_presets` checks it.

`swaybg` and `wbg` keep running while they show the wallpaper. kakemono starts a new instance for each `set`, waits for it to draw, and then stops the old one. It records the PID in `<runtime dir>/setter.pid` and takes over that process when the daemon restarts.

The list of desktops and commands was checked against [Variety](https://github.com/varietywalls/variety), which supports many of the same desktops. Variety is GPL-3, so kakemono contains none of its code. The backends are written from each desktop's public documentation.

### Use your own command

Set `backend = "command"` and `set_command`:

```toml
[desktop]
backend = "command"
set_command = ["awww", "img", "{path}", "--transition-type", "grow"]
managed_by = "user"
```

`set_command` takes these placeholders: `{path}` (the absolute path), `{uri}` (a `file://` URI), `{fit}`, and `{theme}` (`light` or `dark`). A placeholder value never splits into two arguments, even if it has spaces. For shell features, call the shell yourself: `["sh", "-c", "my-setter \"$1\"", "_", "{path}"]`.

Set `persistent = true` if the command keeps running, as `swaybg` does. kakemono then replaces the old instance on every `set`.

`set_command` with a preset backend (for example `backend = "swww"`) replaces the preset's argv.

`managed_by = "user"` stops the daemon from replacing your section when the session changes.

## The systemd service needs your session environment

`kakemono service install` writes `~/.config/systemd/user/kakemono.service` (under `$XDG_CONFIG_HOME` if set), enables it, and starts it. The unit runs `kakemono daemon -f`, is part of `graphical-session.target`, and restarts on failure after 5 seconds. Logs go to the journal: `journalctl --user -u kakemono`.

A systemd user service sees only the environment of the systemd user manager, not of your compositor. If the manager doesn't have `WAYLAND_DISPLAY`, `DISPLAY`, `XDG_CURRENT_DESKTOP`, or `HYPRLAND_INSTANCE_SIGNATURE`, the daemon detects the wrong backend or none, and commands like `swww` can't reach the compositor. GNOME and KDE import these variables for you. Most tiling compositors don't.

To fix it, import the variables before the service starts. In Hyprland:

```text
exec-once = dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP HYPRLAND_INSTANCE_SIGNATURE
```

In sway:

```text
exec dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP SWAYSOCK
```

Many compositors don't start `graphical-session.target` at all. In that case, start the daemon from the compositor config instead of the service. The daemon refuses to run twice, so this is safe even with the service installed:

```text
# Hyprland (hyprland.conf)
exec-once = kakemono daemon
# sway (config)
exec kakemono daemon
```

For niri, add `spawn-at-startup "kakemono" "daemon"` to `config.kdl`. `kakemono service install` prints these lines with the full path to your `kakemono` binary.

To check what the daemon sees, run `kakemono doctor`. The **Session (this shell)** section shows your shell's environment. The **Daemon** section shows the backend that the running daemon uses. If the two differ, the service is missing part of your session environment.

## Picker window rules

The picker is a small window with the title `kakemono-picker`. On X11, macOS, and Windows, kakemono places it at the edge set by `[picker] position`. On native Wayland, the compositor decides where windows go, so add window rules that match the title. Match the title, not the app ID or class: the picker and the Settings window share one app ID.

`kakemono doctor` prints the rules for Hyprland, sway, and niri when it detects one of them.

sway (`~/.config/sway/config`):

```text
for_window [title="^kakemono-picker$"] floating enable, sticky enable, border none, move position center
```

Hyprland (`~/.config/hypr/hyprland.conf`):

```text
windowrulev2 = float, title:^(kakemono-picker)$
windowrulev2 = pin, title:^(kakemono-picker)$
windowrulev2 = noborder, title:^(kakemono-picker)$
# Optional: a full-width strip at the bottom instead of a centred window.
windowrulev2 = size 100% 160, title:^(kakemono-picker)$
windowrulev2 = move 0 100%-160, title:^(kakemono-picker)$
```

niri (`~/.config/niri/config.kdl`):

```text
window-rule {
    match title="^kakemono-picker$"
    open-floating true
}
```

On Hyprland and sway, if you drag the picker somewhere else, kakemono saves the spot as `[picker] placement = "custom"` and moves the picker there the next time it opens.

`[picker] force_x11 = true` runs the picker under XWayland, where kakemono can place it without window rules. The option is experimental and off by default, because XWayland windows can look blurry with fractional scaling.

## Blank windows on NVIDIA

WebKitGTK, which draws the picker and Settings windows, can show blank or white windows on NVIDIA drivers. Set `WEBKIT_DISABLE_DMABUF_RENDERER=1` in the environment that starts the kakemono app:

```sh
WEBKIT_DISABLE_DMABUF_RENDERER=1 kakemono-ui
```

`kakemono pick` and `kakemono settings` pass their environment to the app when they start it, so this works too:

```sh
WEBKIT_DISABLE_DMABUF_RENDERER=1 kakemono settings
```

If the app is already running, it keeps its own environment. Quit it first.

To set the variable for every start in a session that starts through systemd (GNOME, KDE Plasma), add it to `~/.config/environment.d/kakemono.conf`. In other sessions, export it in the compositor config or the shell profile that starts the app.

```text
WEBKIT_DISABLE_DMABUF_RENDERER=1
```

The daemon doesn't use WebKit, so wallpaper rotation works without this setting.
