//! Where the picker goes: edge geometry, saved-position checks and the per-platform
//! [`Placer`]s.
//!
//! Wayland clients can't read or set their own position, so on Hyprland and sway the
//! compositor is asked through `hyprctl` / `swaymsg`. Those calls go through an injected
//! [`Runner`], so tests replay fixture JSON and snapshot the argv without either tool.
//!
//! Compositor rules and the IPC lookups match the window **title**, [`PICKER_TITLE`]. GTK gives
//! every window of the process the same app_id/class (`kakemono-ui`), so the title is the only
//! per-window key. It is a constant and never changes at runtime.

use std::process::Command;

use kakemono_config::schema::{PickerCustom, PickerOrientation, PickerPosition};
use serde::Deserialize;

/// Window title of the picker. Hyprland/sway rules and the placers match on it.
pub const PICKER_TITLE: &str = "kakemono-picker";

/// Thickness of a horizontal strip (logical px). The Go picker used 160.
pub const STRIP_HEIGHT: f64 = 160.0;
/// Width of a vertical strip (logical px). The Go picker used 200.
pub const STRIP_WIDTH: f64 = 200.0;
/// Window size on Wayland, where the compositor decides the position (logical px).
pub const FLOATING_HORIZONTAL: (f64, f64) = (1000.0, STRIP_HEIGHT);
/// See [`FLOATING_HORIZONTAL`].
pub const FLOATING_VERTICAL: (f64, f64) = (STRIP_WIDTH, 640.0);
/// A saved rect must overlap a monitor by at least this much on both axes to count as on
/// screen; a 2 px sliver at the edge can't be grabbed again.
pub const MIN_VISIBLE: i64 = 32;

// ── Geometry ────────────────────────────────────────────────────────────────

/// A rectangle in one placer's coordinate space (physical px for Tauri, logical layout px for
/// Hyprland and sway).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.w)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.h)
    }

    /// Overlap on (x, y), zero or negative when the rects don't meet.
    fn overlap(&self, other: &Rect) -> (i64, i64) {
        let w = self.right().min(other.right()) - i64::from(self.x.max(other.x));
        let h = self.bottom().min(other.bottom()) - i64::from(self.y.max(other.y));
        (w, h)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && i64::from(x) < self.right() && y >= self.y && i64::from(y) < self.bottom()
    }
}

/// One monitor as a placer sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
    pub rect: Rect,
}

/// Tier 1 (macOS, Windows, X11): where the picker goes on `position` of `work_area`, in
/// physical px. The strip runs vertically when `orientation.is_vertical(position)`.
///
/// When the orientation runs along the edge (`auto`, horizontal on top/bottom, vertical on
/// left/right) the strip spans the whole edge, as the Go picker did. Off-axis (vertical on
/// top/bottom, horizontal on left/right) it is a [`FLOATING_VERTICAL`] /
/// [`FLOATING_HORIZONTAL`] panel docked to the edge and centred along it, clamped to the work
/// area.
pub fn edge_rect(
    position: PickerPosition,
    orientation: PickerOrientation,
    work_area: Rect,
    scale: f64,
) -> Rect {
    let px = |logical: f64, max: u32| {
        // Saturating float → int; the picker never exceeds its monitor.
        let px = (logical * scale).round() as u32;
        px.clamp(1, max.max(1))
    };
    // Offset that puts a `size` span at the end of / centred in a `total` span.
    let end = |total: u32, size: u32| total.saturating_sub(size) as i32;
    let centre = |total: u32, size: u32| (total.saturating_sub(size) / 2) as i32;
    let Rect { x, y, w, h } = work_area;
    let vertical = orientation.is_vertical(position);
    match (position, vertical) {
        // Along the edge: the full-edge strip.
        (PickerPosition::Bottom, false) => {
            let t = px(STRIP_HEIGHT, h);
            Rect::new(x, y + end(h, t), w, t)
        }
        (PickerPosition::Top, false) => Rect::new(x, y, w, px(STRIP_HEIGHT, h)),
        (PickerPosition::Left, true) => Rect::new(x, y, px(STRIP_WIDTH, w), h),
        (PickerPosition::Right, true) => {
            let t = px(STRIP_WIDTH, w);
            Rect::new(x + end(w, t), y, t, h)
        }
        // Off-axis: a panel docked to the edge, centred along it.
        (PickerPosition::Top | PickerPosition::Bottom, true) => {
            let (pw, ph) = (px(FLOATING_VERTICAL.0, w), px(FLOATING_VERTICAL.1, h));
            let dy = if position == PickerPosition::Bottom {
                end(h, ph)
            } else {
                0
            };
            Rect::new(x + centre(w, pw), y + dy, pw, ph)
        }
        (PickerPosition::Left | PickerPosition::Right, false) => {
            let (pw, ph) = (px(FLOATING_HORIZONTAL.0, w), px(FLOATING_HORIZONTAL.1, h));
            let dx = if position == PickerPosition::Right {
                end(w, pw)
            } else {
                0
            };
            Rect::new(x + dx, y + centre(h, ph), pw, ph)
        }
    }
}

/// Tier 2 (Wayland): the window size (logical px). Only the effective orientation matters;
/// the compositor picks the position.
pub fn floating_size(position: PickerPosition, orientation: PickerOrientation) -> (f64, f64) {
    if orientation.is_vertical(position) {
        FLOATING_VERTICAL
    } else {
        FLOATING_HORIZONTAL
    }
}

/// The monitor containing `(x, y)`.
pub fn monitor_at(monitors: &[Monitor], x: i32, y: i32) -> Option<&Monitor> {
    monitors.iter().find(|m| m.rect.contains(x, y))
}

/// Why a saved position isn't used; the picker goes to its edge instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// No monitor has the saved name (unplugged, renamed).
    MonitorMissing,
    /// The saved rect doesn't overlap any monitor by [`MIN_VISIBLE`].
    OffScreen,
    /// The compositor doesn't list the picker window (not mapped yet).
    NotMapped,
    /// This platform can't place windows (GNOME, KDE, niri, …).
    Unsupported,
}

/// Checks `custom` (at window `size`) against the current monitors. `Ok` is the rect to use.
pub fn check_custom(
    custom: &PickerCustom,
    size: (u32, u32),
    monitors: &[Monitor],
) -> Result<Rect, Fallback> {
    if !monitors.iter().any(|m| m.name == custom.monitor) {
        return Err(Fallback::MonitorMissing);
    }
    let rect = Rect::new(custom.x, custom.y, size.0, size.1);
    let visible = monitors.iter().any(|m| {
        let (w, h) = rect.overlap(&m.rect);
        w >= MIN_VISIBLE.min(i64::from(size.0)) && h >= MIN_VISIBLE.min(i64::from(size.1))
    });
    if visible {
        Ok(rect)
    } else {
        Err(Fallback::OffScreen)
    }
}

// ── Platform selection ──────────────────────────────────────────────────────

/// Which [`Placer`] the session gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacerKind {
    /// macOS, Windows, X11 (also XWayland after `force_x11`): Tauri positions the window.
    Tauri,
    /// Hyprland: `hyprctl`.
    Hyprland,
    /// sway: `swaymsg`.
    Sway,
    /// Other Wayland compositors: draggable, not remembered, centred by the compositor.
    Noop,
}

impl PlacerKind {
    /// Tier 1: the app positions the window at the edge itself.
    pub fn positions_itself(self) -> bool {
        self == Self::Tauri
    }
}

fn set(env: &dyn Fn(&str) -> Option<String>, key: &str) -> bool {
    env(key).is_some_and(|v| !v.is_empty())
}

/// A native Wayland session for GTK: `WAYLAND_DISPLAY` is set and `GDK_BACKEND` doesn't
/// start with `x11`.
pub fn is_wayland(env: &dyn Fn(&str) -> Option<String>) -> bool {
    set(env, "WAYLAND_DISPLAY") && !env("GDK_BACKEND").is_some_and(|b| b.starts_with("x11"))
}

/// Picks the placer for `os` (`std::env::consts::OS`) and the environment.
pub fn placer_kind(os: &str, env: &dyn Fn(&str) -> Option<String>) -> PlacerKind {
    if os != "linux" || !is_wayland(env) {
        PlacerKind::Tauri
    } else if set(env, "HYPRLAND_INSTANCE_SIGNATURE") {
        PlacerKind::Hyprland
    } else if set(env, "SWAYSOCK") {
        PlacerKind::Sway
    } else {
        PlacerKind::Noop
    }
}

/// Env var set on the relaunched process so `force_x11` can't loop.
pub const X11_RELAUNCH_MARKER: &str = "KAKEMONO_UI_X11";

/// `[picker] force_x11`: relaunch under XWayland (`GDK_BACKEND=x11`) when on native Wayland
/// and not relaunched already.
pub fn needs_x11_relaunch(os: &str, force_x11: bool, env: &dyn Fn(&str) -> Option<String>) -> bool {
    os == "linux" && force_x11 && is_wayland(env) && !set(env, X11_RELAUNCH_MARKER)
}

// ── Placer ──────────────────────────────────────────────────────────────────

/// Result of [`Placer::restore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// Moved to `(x, y)`.
    Placed { x: i32, y: i32 },
    /// Not moved; use the edge placement.
    Fallback(Fallback),
}

/// A failed `hyprctl`/`swaymsg` call or window query.
#[derive(Debug, thiserror::Error)]
pub enum PlaceError {
    #[error("cannot run {cmd}: {source}")]
    Spawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{cmd} failed: {message}")]
    Failed { cmd: String, message: String },
    #[error("{cmd}: unexpected JSON: {source}")]
    Parse {
        cmd: String,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Window(#[from] tauri::Error),
}

/// Reads and restores the picker's position on one platform. Both calls need the window
/// shown (mapped): compositors don't list hidden windows.
pub trait Placer: Send + Sync {
    /// Moves the picker to `custom`, unless its monitor is gone or it would be off screen.
    fn restore(&self, custom: &PickerCustom) -> Result<Restore, PlaceError>;
    /// Where the picker is now. `None` when this platform can't tell.
    fn capture(&self) -> Result<Option<PickerCustom>, PlaceError>;
}

/// Runs an argv and returns stdout. Injected so tests don't need `hyprctl` or `swaymsg`.
pub trait Runner: Send + Sync {
    fn run(&self, argv: &[&str]) -> Result<String, PlaceError>;
}

/// Runs the real command. A non-zero exit is an error carrying stderr.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, argv: &[&str]) -> Result<String, PlaceError> {
        let cmd = argv.join(" ");
        let (program, args) = argv.split_first().ok_or_else(|| PlaceError::Failed {
            cmd: cmd.clone(),
            message: "empty argv".into(),
        })?;
        let out = Command::new(program)
            .args(args)
            .output()
            .map_err(|source| PlaceError::Spawn {
                cmd: cmd.clone(),
                source,
            })?;
        if !out.status.success() {
            return Err(PlaceError::Failed {
                cmd,
                message: format!(
                    "{}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

fn parse<T: for<'de> Deserialize<'de>>(argv: &[&str], json: &str) -> Result<T, PlaceError> {
    serde_json::from_str(json).map_err(|source| PlaceError::Parse {
        cmd: argv.join(" "),
        source,
    })
}

/// GNOME, KDE, niri, others: draggable, never remembered.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopPlacer;

impl Placer for NoopPlacer {
    fn restore(&self, _: &PickerCustom) -> Result<Restore, PlaceError> {
        Ok(Restore::Fallback(Fallback::Unsupported))
    }

    fn capture(&self) -> Result<Option<PickerCustom>, PlaceError> {
        Ok(None)
    }
}

// ── Hyprland ────────────────────────────────────────────────────────────────

const HYPR_CLIENTS: [&str; 3] = ["hyprctl", "clients", "-j"];
const HYPR_MONITORS: [&str; 3] = ["hyprctl", "monitors", "-j"];

#[derive(Debug, Deserialize)]
struct HyprClient {
    at: [i32; 2],
    size: [i32; 2],
    #[serde(default)]
    title: String,
    #[serde(default)]
    pid: i64,
    #[serde(default = "yes")]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    monitor: i64,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct HyprMonitor {
    id: i64,
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    #[serde(default = "one")]
    scale: f64,
    #[serde(default)]
    transform: u8,
}

fn one() -> f64 {
    1.0
}

impl HyprMonitor {
    /// The monitor in layout (logical) px, which is what `at` and `movewindowpixel` use.
    fn monitor(&self) -> Monitor {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) = if self.transform % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let logical = |px: u32| (f64::from(px) / scale).round() as u32;
        Monitor {
            name: self.name.clone(),
            rect: Rect::new(self.x, self.y, logical(w), logical(h)),
        }
    }
}

/// Hyprland: `hyprctl clients -j` to read, `hyprctl dispatch movewindowpixel exact` to move.
pub struct HyprPlacer<R: Runner> {
    runner: R,
    pid: u32,
}

impl<R: Runner> HyprPlacer<R> {
    /// `pid` is this process; only its picker window is matched.
    pub fn new(runner: R, pid: u32) -> Self {
        Self { runner, pid }
    }

    fn picker(&self) -> Result<Option<HyprClient>, PlaceError> {
        let clients: Vec<HyprClient> = parse(&HYPR_CLIENTS, &self.runner.run(&HYPR_CLIENTS)?)?;
        Ok(clients.into_iter().find(|c| {
            c.title == PICKER_TITLE && c.pid == i64::from(self.pid) && c.mapped && !c.hidden
        }))
    }

    fn monitors(&self) -> Result<Vec<HyprMonitor>, PlaceError> {
        parse(&HYPR_MONITORS, &self.runner.run(&HYPR_MONITORS)?)
    }
}

impl<R: Runner> Placer for HyprPlacer<R> {
    fn restore(&self, custom: &PickerCustom) -> Result<Restore, PlaceError> {
        let Some(client) = self.picker()? else {
            return Ok(Restore::Fallback(Fallback::NotMapped));
        };
        let monitors: Vec<Monitor> = self.monitors()?.iter().map(HyprMonitor::monitor).collect();
        let size = (client.size[0].max(1) as u32, client.size[1].max(1) as u32);
        let rect = match check_custom(custom, size, &monitors) {
            Ok(rect) => rect,
            Err(fallback) => return Ok(Restore::Fallback(fallback)),
        };
        let arg = format!("exact {} {},title:^({PICKER_TITLE})$", rect.x, rect.y);
        let argv = ["hyprctl", "dispatch", "movewindowpixel", &arg];
        let out = self.runner.run(&argv)?;
        if out.trim() != "ok" {
            return Err(PlaceError::Failed {
                cmd: argv.join(" "),
                message: out.trim().to_owned(),
            });
        }
        Ok(Restore::Placed {
            x: rect.x,
            y: rect.y,
        })
    }

    fn capture(&self) -> Result<Option<PickerCustom>, PlaceError> {
        let Some(client) = self.picker()? else {
            return Ok(None);
        };
        let monitors = self.monitors()?;
        let [x, y] = client.at;
        let monitor = monitors
            .iter()
            .find(|m| m.id == client.monitor)
            .map(|m| m.name.clone())
            .or_else(|| {
                let all: Vec<Monitor> = monitors.iter().map(HyprMonitor::monitor).collect();
                monitor_at(&all, x, y).map(|m| m.name.clone())
            })
            .unwrap_or_default();
        Ok(Some(PickerCustom { x, y, monitor }))
    }
}

// ── sway ────────────────────────────────────────────────────────────────────

const SWAY_TREE: [&str; 4] = ["swaymsg", "-r", "-t", "get_tree"];
const SWAY_OUTPUTS: [&str; 4] = ["swaymsg", "-r", "-t", "get_outputs"];

#[derive(Debug, Clone, Copy, Deserialize)]
struct SwayRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl From<SwayRect> for Rect {
    fn from(r: SwayRect) -> Self {
        Rect::new(r.x, r.y, r.width, r.height)
    }
}

#[derive(Debug, Deserialize)]
struct SwayNode {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    pid: Option<i64>,
    rect: SwayRect,
    #[serde(default)]
    nodes: Vec<SwayNode>,
    #[serde(default)]
    floating_nodes: Vec<SwayNode>,
}

#[derive(Debug, Deserialize)]
struct SwayOutput {
    name: String,
    #[serde(default = "yes")]
    active: bool,
    rect: SwayRect,
}

/// The picker node and the name of the output it is on.
fn find_sway<'a>(node: &'a SwayNode, output: &str, pid: u32) -> Option<(&'a SwayNode, String)> {
    let output = if node.kind == "output" {
        node.name.as_deref().unwrap_or(output)
    } else {
        output
    };
    if node.name.as_deref() == Some(PICKER_TITLE) && node.pid == Some(i64::from(pid)) {
        return Some((node, output.to_owned()));
    }
    node.nodes
        .iter()
        .chain(&node.floating_nodes)
        .find_map(|child| find_sway(child, output, pid))
}

/// sway: `swaymsg -t get_tree` to read, `move absolute position` to move.
pub struct SwayPlacer<R: Runner> {
    runner: R,
    pid: u32,
}

impl<R: Runner> SwayPlacer<R> {
    /// `pid` is this process; only its picker window is matched.
    pub fn new(runner: R, pid: u32) -> Self {
        Self { runner, pid }
    }

    fn picker(&self) -> Result<Option<(SwayRect, String)>, PlaceError> {
        let tree: SwayNode = parse(&SWAY_TREE, &self.runner.run(&SWAY_TREE)?)?;
        Ok(find_sway(&tree, "", self.pid).map(|(node, output)| (node.rect, output)))
    }
}

impl<R: Runner> Placer for SwayPlacer<R> {
    fn restore(&self, custom: &PickerCustom) -> Result<Restore, PlaceError> {
        let Some((current, _)) = self.picker()? else {
            return Ok(Restore::Fallback(Fallback::NotMapped));
        };
        let outputs: Vec<SwayOutput> = parse(&SWAY_OUTPUTS, &self.runner.run(&SWAY_OUTPUTS)?)?;
        let monitors: Vec<Monitor> = outputs
            .into_iter()
            .filter(|o| o.active)
            .map(|o| Monitor {
                name: o.name,
                rect: o.rect.into(),
            })
            .collect();
        let rect = match check_custom(custom, (current.width, current.height), &monitors) {
            Ok(rect) => rect,
            Err(fallback) => return Ok(Restore::Fallback(fallback)),
        };
        let cmd = format!(
            "[title=\"^{PICKER_TITLE}$\" pid={}] move absolute position {} {}",
            self.pid, rect.x, rect.y
        );
        let argv = ["swaymsg", "-r", &cmd];
        let out = self.runner.run(&argv)?;
        #[derive(Deserialize)]
        struct Reply {
            success: bool,
            #[serde(default)]
            error: Option<String>,
        }
        let replies: Vec<Reply> = parse(&argv, &out)?;
        if let Some(failed) = replies.into_iter().find(|r| !r.success) {
            return Err(PlaceError::Failed {
                cmd: argv.join(" "),
                message: failed.error.unwrap_or_else(|| "success: false".into()),
            });
        }
        Ok(Restore::Placed {
            x: rect.x,
            y: rect.y,
        })
    }

    fn capture(&self) -> Result<Option<PickerCustom>, PlaceError> {
        Ok(self.picker()?.map(|(rect, monitor)| PickerCustom {
            x: rect.x,
            y: rect.y,
            monitor,
        }))
    }
}

// ── Tauri (macOS, Windows, X11) ─────────────────────────────────────────────

/// macOS, Windows and X11: Tauri reads `outer_position` and calls `set_position`.
pub struct TauriPlacer<Rt: tauri::Runtime> {
    window: tauri::WebviewWindow<Rt>,
}

impl<Rt: tauri::Runtime> TauriPlacer<Rt> {
    pub fn new(window: tauri::WebviewWindow<Rt>) -> Self {
        Self { window }
    }

    fn monitors(&self) -> Result<Vec<Monitor>, PlaceError> {
        Ok(self
            .window
            .available_monitors()?
            .iter()
            .map(tauri_monitor)
            .collect())
    }
}

/// A Tauri monitor in physical px. Unnamed monitors get `""`.
pub fn tauri_monitor(m: &tauri::Monitor) -> Monitor {
    let (p, s) = (m.position(), m.size());
    Monitor {
        name: m.name().cloned().unwrap_or_default(),
        rect: Rect::new(p.x, p.y, s.width, s.height),
    }
}

impl<Rt: tauri::Runtime> Placer for TauriPlacer<Rt> {
    fn restore(&self, custom: &PickerCustom) -> Result<Restore, PlaceError> {
        let size = self.window.outer_size()?;
        match check_custom(custom, (size.width, size.height), &self.monitors()?) {
            Ok(rect) => {
                self.window
                    .set_position(tauri::PhysicalPosition::new(rect.x, rect.y))?;
                Ok(Restore::Placed {
                    x: rect.x,
                    y: rect.y,
                })
            }
            Err(fallback) => Ok(Restore::Fallback(fallback)),
        }
    }

    fn capture(&self) -> Result<Option<PickerCustom>, PlaceError> {
        let pos = self.window.outer_position()?;
        let monitors = self.monitors()?;
        let monitor = self
            .window
            .current_monitor()?
            .map(|m| tauri_monitor(&m).name)
            .or_else(|| monitor_at(&monitors, pos.x, pos.y).map(|m| m.name.clone()))
            .unwrap_or_default();
        Ok(Some(PickerCustom {
            x: pos.x,
            y: pos.y,
            monitor,
        }))
    }
}
