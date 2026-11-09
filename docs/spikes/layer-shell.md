# Spike: the picker as a wlr-layer-shell surface (Task 43, step 6)

**Question.** On wlroots compositors (sway, Hyprland), can the picker be a layer-shell surface
anchored to a screen edge, like a panel, instead of a floating toplevel that needs window
rules?

**Answer: yes on sway. Hyprland was not run.** GTK3 + `gtk-layer-shell` 0.8 + a wry webview
gives a clean, full-width, 160 px strip at the bottom of the output, drawn over normal
windows. Nothing from the spike ships. The shipped picker stays a normal Tauri window, placed
with compositor rules plus the `hyprctl`/`swaymsg` placers (`picker_place.rs`).

Time box: 30 minutes. Used: about 10 (2026-11-10, 05:04 to 05:13).

## Setup

The spike was a scratch crate outside the repo (`/tmp/layer-spike`). `gtk-layer-shell` is
marked unmaintained on crates.io and is **not** a kakemono dependency. Adding it would need a
cargo-deny exception.

The host was macOS (arm64), so the spike ran in Docker on Debian bookworm:

- sway 1.7, headless (`WLR_BACKENDS=headless WLR_RENDERER=pixman`, one 1920x1080 output)
- libgtk-layer-shell0 0.8.0 and libwebkit2gtk-4.1 2.50.6
- run as a non-root user, because sway refuses to start as root

Images were captured with `grim`. The window tree came from `swaymsg -t get_tree`, and the
layer surfaces from sway's debug log.

`Cargo.toml`:

```toml
[package]
name = "layer-spike"
version = "0.0.0"
edition = "2021"
publish = false

[[bin]]
name = "layer_picker"
path = "src/main.rs"

[dependencies]
gtk = "0.18"
# v0_6 for set_keyboard_mode / is_supported; Debian bookworm ships the 0.8.0 C library.
gtk-layer-shell = { version = "0.8", features = ["v0_6"] }
wry = "0.57"
```

`src/main.rs` (this is the `examples/layer_picker.rs` from the plan. It is kept here
instead of in the repo):

```rust
//! Layer-shell spike (kakemono Task 43): a picker-shaped GTK3 window turned into a
//! wlr-layer-shell surface anchored to the bottom edge, with a wry webview inside.
//! Not part of kakemono; run under a wlroots compositor (sway, Hyprland).
use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use wry::{WebViewBuilder, WebViewBuilderExtUnix};

const HTML: &str = r#"<!doctype html><html><body style="margin:0;background:#1e1e2e;color:#cdd6f4;
font:24px sans-serif;display:flex;align-items:center;justify-content:center;height:100vh">
kakemono-picker layer surface <input id="q" placeholder="type here" autofocus></body></html>"#;

fn main() {
    gtk::init().expect("gtk init");
    println!("layer-shell supported: {}", gtk_layer_shell::is_supported());

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("kakemono-picker");
    // SPIKE_TOPLEVEL=1 keeps a normal toplevel (to test compositor window rules instead).
    if std::env::var_os("SPIKE_TOPLEVEL").is_some() {
        window.set_default_size(1000, 160);
        window.add(&gtk::Box::new(gtk::Orientation::Vertical, 0));
        show_webview(&window);
        return;
    }
    // Must happen before the window is realized.
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace("kakemono-picker");
    for edge in [Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.set_layer_shell_margin(Edge::Bottom, 0);
    window.set_exclusive_zone(-1); // float over panels, don't push windows
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_size_request(-1, 160);
    window.add(&gtk::Box::new(gtk::Orientation::Vertical, 0));
    show_webview(&window);
}

fn show_webview(window: &gtk::Window) {
    let vbox = window.child().and_downcast::<gtk::Box>().expect("vbox");
    let _webview = WebViewBuilder::new()
        .with_html(HTML)
        .build_gtk(&vbox)
        .expect("webview");
    window.show_all();
    window.connect_destroy(|_| gtk::main_quit());
    gtk::main();
}
```

Build image (`Dockerfile`, then a second layer that adds a uid 1000 `spike` user):

```dockerfile
FROM rust:1-bookworm
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    libwebkit2gtk-4.1-dev libgtk-3-dev libgtk-layer-shell-dev sway grim jq \
    fonts-dejavu-core dbus-x11 && rm -rf /var/lib/apt/lists/*
```

Run script, condensed:

```sh
export XDG_RUNTIME_DIR=/tmp/xdg WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
printf 'output HEADLESS-1 resolution 1920x1080\n%s\n' "${SWAY_RULE:-}" > /tmp/sway.conf
sway -c /tmp/sway.conf -d >/tmp/sway.log 2>&1 &
export WAYLAND_DISPLAY=wayland-1 SWAYSOCK=$(ls $XDG_RUNTIME_DIR/sway-ipc.*)
export GDK_BACKEND=wayland WEBKIT_DISABLE_COMPOSITING_MODE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1
dbus-launch --exit-with-session ./target/release/layer_picker &
sleep 8
swaymsg -r -t get_tree | jq -c '[.. | objects | select(.type? == "con" or .type? == "floating_con") | {name, app_id, rect}]'
grep -iE 'layer.surface|namespace' /tmp/sway.log | tail
grim /w/shot.png
```

## What was observed

### Run 1: layer surface on sway

```
layer-shell supported: true
--- swaymsg get_tree (app windows)
[]
--- sway layer surfaces (debug log)
[types/wlr_layer_shell_v1.c:452] new layer_surface 0xaaab09975b30 (res 0xaaab099417b0)
[sway/desktop/layer_shell.c:618] new layer surface: namespace kakemono-picker layer 3 anchor 14 size 0x160 margin 0,0,0,0,
```

- The layer is 3 (overlay). The anchor is 14 (bottom 2 + left 4 + right 8). The size is 0x160, where a width of 0
  means "stretch between the left and right anchors".
- `grim` showed a dark strip 1920x160 at y = 920 to 1080, the full width of the output, with
  the webview's text and `<input>` rendered and centred. The rest of the output was the empty
  sway background. There were no decorations, borders or gaps, and no artefacts in software
  rendering.
- A layer surface is **not** in `get_tree`. sway has no IPC to move it or to read its
  position. It is placed only by anchors and margins.

### Run 2: normal toplevel with the documented sway rule (`SPIKE_TOPLEVEL=1`)

The sway config contained the `for_window` rule from "Window rules" below.

```
[{"name":"kakemono-picker","app_id":"layer_picker","rect":{"x":460,"y":460,"width":1000,"height":160}}]
```

The picker was floating and centred (1000x160 on 1920x1080), with no border. Then the
`SwayPlacer` restore command ran with the window's pid:

```
$ swaymsg -r '[title="^kakemono-picker$" pid=30] move absolute position 100 700'
[ { "success": true } ]
[{"name":"kakemono-picker","pid":30,"rect":{"x":100,"y":700,"width":1000,"height":160}}]
```

This is the exact argv shape snapshotted in `crates/ui/src-tauri/tests/picker_place.rs`.

### Hyprland: not run

Hyprland was not run because no package was available in the time box. There is no arm64
`archlinux` image, and Debian bookworm and trixie don't package Hyprland. Building it from
source would not fit in 30 minutes. The Hyprland rules below are not verified.

### Not checked

- Keyboard focus with `KeyboardMode::OnDemand`. The headless output has no input devices, so
  focus was not checked. Typing into the strip, and closing on blur when it is a layer
  surface, are unverified.
- A layer surface inside a **Tauri** window, as opposed to raw wry with a GTK window. Tauri's
  Linux window is a `gtk::ApplicationWindow` that tao realizes on `show_all()`. With
  `.visible(false)` it is not realized at build time, so calling
  `window.gtk_window()?.init_layer_shell()` before the first `show()` probably works. This
  was not tried.

## Conclusion and why nothing ships

- Layer shell gives the best Wayland result on wlroots compositors. The strip docks to the
  edge, needs no user window rules, and floats over fullscreen-ish layouts.
- Costs:
  - It adds an unmaintained crate (cargo-deny exception) and a GTK FFI path.
  - It only works on wlroots and KDE. GNOME has no layer shell.
  - The strip can't be dragged, so `placement = "custom"` doesn't apply.
  - Keyboard focus needs separate testing.
- The shipped Tier 2 behaviour on Wayland: a floating window sized by orientation, the window
  rules below, and the `hyprctl`/`swaymsg` placers. It works on every compositor and keeps
  drag-to-place.

## Window rules (copy into the user docs, Task 48)

The picker's **title** is the constant `kakemono-picker` (`picker::TITLE`, asserted by tests).
It never contains a wallpaper name or count. Match on the title, not on app_id or class.
Tauri sets one app_id for the whole process (from the bundle identifier), and the settings
window shares it.

### sway

Verified in run 2 above:

```
for_window [title="^kakemono-picker$"] floating enable, sticky enable, border none, move position center
```

sway can't anchor a floating window to the bottom edge with a relative rule. To dock it, use
absolute pixels for your output instead of `center`, for example
`move position 0 px 920 px, resize set width 1920 px height 160 px` on a 1920x1080 output.
A position you drag to is restored by kakemono itself (`[picker] placement = "custom"`).

### Hyprland

Not run (see above). This is the `windowrulev2` syntax. Hyprland 0.48 and later also accept
it as `windowrule`.

```
windowrulev2 = float, title:^(kakemono-picker)$
windowrulev2 = pin, title:^(kakemono-picker)$
windowrulev2 = noborder, title:^(kakemono-picker)$
# Optional: dock as a full-width strip at the bottom instead of centred.
windowrulev2 = size 100% 160, title:^(kakemono-picker)$
windowrulev2 = move 0 100%-160, title:^(kakemono-picker)$
```

`float` is required for drag-to-place. kakemono restores a dragged position with
`hyprctl dispatch movewindowpixel exact X Y,title:^(kakemono-picker)$` after the window maps.

### GNOME, KDE, niri, others

No rule is needed. The picker opens where the compositor puts it (usually centred). It can
be dragged, but the position is not remembered (`NoopPlacer`).

## Follow-ups (not code now)

1. **Per-window app_id** so rules can match `app_id`/`class` = `kakemono-picker` as the plan
   first said. There are two ways:
   - GTK FFI: on the picker's `gtk_window()`, call `gdk_wayland_window_set_application_id`
     (Wayland, GTK 3.24.22+, after realize) and `gtk_window_set_wmclass` (X11, before
     realize). This needs a `gdk-wayland` FFI binding that Tauri doesn't expose.
   - A separate picker process with its own app id. It is cleaner for compositors but
     doubles the webview memory and needs IPC with the tray process.
   Until then, the title is the match key.
2. **Opt-in layer-shell mode** on wlroots (`[picker] layer_shell = true`), if users ask for a
   docked strip without rules. This needs a maintained layer-shell binding (or a small FFI
   shim), a Tauri realize-order check, and a keyboard focus test on real hardware.
3. **Run the Hyprland rules on a real Hyprland session** before Task 48 publishes them.
