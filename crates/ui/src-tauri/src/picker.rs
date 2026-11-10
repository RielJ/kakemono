//! The picker window (plan §H, Task 43).
//!
//! The window is created hidden at startup and reused. `show` places it, `hide` remembers
//! where the user dragged it. Placement comes in two tiers:
//!
//! 1. **macOS, Windows, X11:** the app puts the strip on `[picker] position` of the monitor
//!    under the cursor, or at the saved `custom` spot ([`TauriPlacer`]). `[picker]
//!    orientation` picks the strip's direction; off-axis it is a panel centred on the edge
//!    ([`edge_rect`]), and the window size follows it in both tiers.
//! 2. **Wayland:** the app can't position itself. It only sets an orientation-dependent size;
//!    the compositor (and the user's window rules) decide where it goes. Hyprland and sway
//!    can still restore and read a dragged position through their IPC ([`HyprPlacer`],
//!    [`SwayPlacer`]); elsewhere nothing is remembered.
//!
//! A drag counts as a "drop" when the captured position differs from where the picker was
//! put. Tier 1 captures on `Moved` (debounced 300 ms) and on hide; compositors only on hide,
//! because they don't report moves. A drop writes `placement = "custom"` and `custom` through
//! `kakemono_config::edit` (comments survive).

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kakemono_config::ConfigError;
use kakemono_config::paths::Paths;
use kakemono_config::schema::{PickerConfig, PickerCustom, PickerPlacement};
use serde_json::json;
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Runtime, Theme, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent, window::Color,
};

use crate::commands::CommandError;
use crate::picker_place::{
    self, Fallback, HyprPlacer, NoopPlacer, PICKER_TITLE, Placer, PlacerKind, Rect, Restore,
    SwayPlacer, SystemRunner, TauriPlacer, edge_rect, floating_size, tauri_monitor,
};
use crate::windows::PICKER_LABEL;

/// Window title. Compositor rules match it, so it is a constant (see `picker_place`).
pub const TITLE: &str = PICKER_TITLE;
/// Blur right after showing is ignored. The Go picker had the same 800 ms guard "to avoid
/// closing immediately on launch": focus can bounce while the window maps.
pub const BLUR_GRACE: Duration = Duration::from_millis(800);
/// Tier 1 captures a drag this long after the last `Moved` event.
pub const MOVE_DEBOUNCE: Duration = Duration::from_millis(300);
/// A captured position within this many px of where the picker was put is not a drag.
const DROP_TOLERANCE: i32 = 2;
/// Wayland: how often and how long to wait for the compositor to map the window.
const SETTLE_STEP: Duration = Duration::from_millis(50);
const SETTLE_TRIES: u32 = 12;

// ── State ───────────────────────────────────────────────────────────────────

/// What the picker controller remembers between events. Pure, so the blur and drop rules
/// are unit-tested without a window.
#[derive(Debug, Default, Clone)]
pub struct Session {
    shown_at: Option<Instant>,
    close_on_blur: bool,
    /// Where the app put the window (or first saw it, on Wayland). `None` = unknown, so no
    /// capture counts as a drop.
    placed_at: Option<(i32, i32)>,
    move_gen: u64,
}

impl Session {
    /// The picker was shown at `now`; `close_on_blur` is `[picker] close_on_blur`.
    pub fn shown(&mut self, now: Instant, close_on_blur: bool) {
        self.shown_at = Some(now);
        self.close_on_blur = close_on_blur;
        self.placed_at = None;
    }

    pub fn hidden(&mut self) {
        self.shown_at = None;
    }

    /// Whether `WindowEvent::Focused(false)` at `now` hides the picker: only when configured,
    /// visible, and past [`BLUR_GRACE`].
    pub fn hide_on_blur(&self, now: Instant) -> bool {
        self.close_on_blur
            && self
                .shown_at
                .is_some_and(|at| now.saturating_duration_since(at) >= BLUR_GRACE)
    }

    pub fn placed(&mut self, x: i32, y: i32) {
        self.placed_at = Some((x, y));
    }

    /// Whether `captured` is a user drag away from where the picker was put.
    pub fn is_drop(&self, captured: &PickerCustom) -> bool {
        self.placed_at.is_some_and(|(x, y)| {
            (captured.x - x).abs() > DROP_TOLERANCE || (captured.y - y).abs() > DROP_TOLERANCE
        })
    }

    /// Starts a new debounce window; returns its generation.
    pub fn bump_move(&mut self) -> u64 {
        self.move_gen = self.move_gen.wrapping_add(1);
        self.move_gen
    }

    /// No `Moved` event arrived since `generation`.
    pub fn move_settled(&self, generation: u64) -> bool {
        self.move_gen == generation
    }
}

/// Managed state: the session's placer kind and the [`Session`].
pub struct PickerState {
    kind: PlacerKind,
    session: Mutex<Session>,
}

impl PickerState {
    /// Detects the placer from the OS and environment.
    pub fn detect() -> Self {
        let kind = picker_place::placer_kind(std::env::consts::OS, &|key| std::env::var(key).ok());
        tracing::info!(?kind, "picker placer");
        Self {
            kind,
            session: Mutex::new(Session::default()),
        }
    }

    pub fn kind(&self) -> PlacerKind {
        self.kind
    }

    fn with<T>(&self, f: impl FnOnce(&mut Session) -> T) -> T {
        // A poisoned lock only means a panic elsewhere; the session data is still usable.
        let mut guard = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }
}

fn placer_for<R: Runtime>(kind: PlacerKind, window: &WebviewWindow<R>) -> Box<dyn Placer> {
    let pid = std::process::id();
    match kind {
        PlacerKind::Tauri => Box::new(TauriPlacer::new(window.clone())),
        PlacerKind::Hyprland => Box::new(HyprPlacer::new(SystemRunner, pid)),
        PlacerKind::Sway => Box::new(SwayPlacer::new(SystemRunner, pid)),
        PlacerKind::Noop => Box::new(NoopPlacer),
    }
}

// ── Config ──────────────────────────────────────────────────────────────────

fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(Paths::detect()?.locate_config(None).path().to_owned())
}

/// `[picker]`, or the defaults when the config can't be read (logged).
fn load_picker() -> PickerConfig {
    config_path()
        .and_then(|path| kakemono_config::load_or_default(&path))
        .map(|config| config.picker)
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, "cannot read config; picker uses defaults");
            PickerConfig::default()
        })
}

fn save_custom(custom: &PickerCustom) -> Result<(), ConfigError> {
    let patch = json!({ "picker": {
        "placement": "custom",
        "custom": { "x": custom.x, "y": custom.y, "monitor": custom.monitor },
    }});
    kakemono_config::edit::apply_patch(&config_path()?, &patch)
}

fn save_edge() -> Result<(), ConfigError> {
    let patch = json!({ "picker": { "placement": "edge" } });
    kakemono_config::edit::apply_patch(&config_path()?, &patch)
}

// ── Window ──────────────────────────────────────────────────────────────────

/// Creates the hidden picker window and wires its events. Called once at startup.
pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    if let Some(window) = app.get_webview_window(PICKER_LABEL) {
        return Ok(window);
    }
    let (w, h) = picker_place::FLOATING_HORIZONTAL;
    let window =
        WebviewWindowBuilder::new(app, PICKER_LABEL, WebviewUrl::App("picker.html".into()))
            .title(TITLE)
            .inner_size(w, h)
            .decorations(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible_on_all_workspaces(true)
            .visible(false)
            // Image drops are HTML5 events in the page (`useImageDrop`); the native handler
            // would swallow them. `data-tauri-drag-region` is unaffected.
            .disable_drag_drop_handler()
            .build()?;

    let handle = app.clone();
    window.on_window_event(move |event| on_event(&handle, event));
    Ok(window)
}

fn on_event<R: Runtime>(app: &AppHandle<R>, event: &WindowEvent) {
    let state = app.state::<PickerState>();
    match event {
        WindowEvent::Focused(false) => {
            if state.with(|s| s.hide_on_blur(Instant::now())) {
                spawn_hide(app);
            }
        }
        WindowEvent::Moved(_) if state.kind().positions_itself() => {
            let generation = state.with(Session::bump_move);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(MOVE_DEBOUNCE).await;
                let state = app.state::<PickerState>();
                if !state.with(|s| s.move_settled(generation)) {
                    return;
                }
                if let Some(window) = app.get_webview_window(PICKER_LABEL) {
                    let kind = state.kind();
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        let state = window.state::<PickerState>();
                        capture_and_save(&state, placer_for(kind, &window).as_ref());
                    })
                    .await;
                }
            });
        }
        // Alt+F4 and the like: keep the pre-created window, just hide it.
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            spawn_hide(app);
        }
        _ => {}
    }
}

fn spawn_hide<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = hide(app).await {
            tracing::warn!(error = %err, "cannot hide picker");
        }
    });
}

/// Reads the window's position and saves it when the user dragged it.
fn capture_and_save(state: &PickerState, placer: &dyn Placer) {
    match placer.capture() {
        Ok(Some(captured)) if state.with(|s| s.is_drop(&captured)) => {
            match save_custom(&captured) {
                Ok(()) => {
                    tracing::info!(?captured, "picker position saved");
                    state.with(|s| s.placed(captured.x, captured.y));
                }
                Err(err) => tracing::warn!(error = %err, "cannot save picker position"),
            }
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "cannot read picker position"),
    }
}

/// Picker background (`--bg-light` / `--bg-dark` in `frontend/src/shared/tokens.css`).
pub const BG_LIGHT: Color = Color(0xf4, 0xf4, 0xf5, 0xff);
/// See [`BG_LIGHT`].
pub const BG_DARK: Color = Color(0x1e, 0x1e, 0x20, 0xff);

/// The window background for `theme`.
pub fn background_for(theme: Theme) -> Color {
    match theme {
        Theme::Dark => BG_DARK,
        _ => BG_LIGHT,
    }
}

/// Shows the picker at its saved or edge position and focuses it.
pub fn show<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let window = create(app)?;
    let config = load_picker();
    let state = app.state::<PickerState>();
    state.with(|s| s.shown(Instant::now(), config.close_on_blur));

    let custom = match config.placement {
        PickerPlacement::Custom => config.custom.clone(),
        PickerPlacement::Edge => None,
    };

    if state.kind().positions_itself() {
        place_tier1(app, &window, &config, custom.as_ref())?;
    } else {
        let (w, h) = floating_size(config.position, config.orientation);
        window.set_size(LogicalSize::new(w, h))?;
    }

    // The native background matches the page's, so there is no flash before it paints.
    let theme = window.theme().unwrap_or(Theme::Light);
    window.set_background_color(Some(background_for(theme)))?;

    window.show()?;
    window.unminimize()?;
    window.set_focus()?;

    if !state.kind().positions_itself() && state.kind() != PlacerKind::Noop {
        let kind = state.kind();
        let window = window.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let state = window.state::<PickerState>();
            settle(&state, placer_for(kind, &window).as_ref(), custom);
        });
    }
    Ok(window)
}

/// Tier 1: size the strip for the monitor (the saved one, else the one under the cursor),
/// then move it to the saved spot or the edge.
fn place_tier1<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
    config: &PickerConfig,
    custom: Option<&PickerCustom>,
) -> tauri::Result<()> {
    let monitors = app.available_monitors()?;
    let saved = custom.and_then(|c| {
        monitors
            .iter()
            .find(|m| m.name().is_some_and(|n| *n == c.monitor))
    });
    let under_cursor = app.cursor_position().ok().and_then(|p| {
        monitors
            .iter()
            .find(|m| tauri_monitor(m).rect.contains(p.x as i32, p.y as i32))
    });
    let primary = app.primary_monitor()?;
    let Some(monitor) = saved
        .or(under_cursor)
        .or(primary.as_ref())
        .or(monitors.first())
    else {
        tracing::warn!("no monitors reported; showing the picker where it is");
        return Ok(());
    };

    let area = monitor.work_area();
    let work_area = Rect::new(
        area.position.x,
        area.position.y,
        area.size.width,
        area.size.height,
    );
    let edge = edge_rect(
        config.position,
        config.orientation,
        work_area,
        monitor.scale_factor(),
    );
    window.set_size(PhysicalSize::new(edge.w, edge.h))?;

    let state = app.state::<PickerState>();
    if let Some(custom) = custom {
        match TauriPlacer::new(window.clone()).restore(custom) {
            Ok(Restore::Placed { x, y }) => {
                state.with(|s| s.placed(x, y));
                return Ok(());
            }
            Ok(Restore::Fallback(fallback)) => {
                tracing::info!(
                    ?fallback,
                    ?custom,
                    "saved picker position unusable; using the edge"
                );
            }
            Err(err) => tracing::warn!(error = %err, "cannot restore picker position"),
        }
    }
    window.set_position(PhysicalPosition::new(edge.x, edge.y))?;
    state.with(|s| s.placed(edge.x, edge.y));
    Ok(())
}

/// Wayland: once the compositor maps the window, restore the saved spot or note where the
/// compositor put it (the baseline a later drag is compared with).
fn settle(state: &PickerState, placer: &dyn Placer, mut custom: Option<PickerCustom>) {
    for _ in 0..SETTLE_TRIES {
        std::thread::sleep(SETTLE_STEP);
        if let Some(saved) = &custom {
            match placer.restore(saved) {
                Ok(Restore::Placed { x, y }) => {
                    state.with(|s| s.placed(x, y));
                    return;
                }
                Ok(Restore::Fallback(Fallback::NotMapped)) => continue,
                Ok(Restore::Fallback(fallback)) => {
                    tracing::info!(?fallback, ?saved, "saved picker position unusable");
                    custom = None;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "cannot restore picker position");
                    return;
                }
            }
        }
        match placer.capture() {
            Ok(Some(at)) => {
                state.with(|s| s.placed(at.x, at.y));
                return;
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(error = %err, "cannot read picker position");
                return;
            }
        }
    }
    tracing::debug!("picker never appeared in the compositor's window list");
}

/// Hides the picker, saving a dragged position first (compositors forget hidden windows).
pub async fn hide<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    let Some(window) = app.get_webview_window(PICKER_LABEL) else {
        return Ok(());
    };
    if !window.is_visible()? {
        return Ok(());
    }
    let kind = app.state::<PickerState>().kind();
    let probe = window.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = probe.state::<PickerState>();
        capture_and_save(&state, placer_for(kind, &probe).as_ref());
    })
    .await?;
    window.hide()?;
    app.state::<PickerState>().with(Session::hidden);
    Ok(())
}

// ── Commands ────────────────────────────────────────────────────────────────

/// `show_picker`.
#[tauri::command]
pub fn show_picker<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    show(&app)?;
    Ok(())
}

/// `hide_picker`: Escape, and blur when `close_on_blur`.
#[tauri::command]
pub async fn hide_picker<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    hide(app).await
}

/// `reset_picker_position`: `placement = "edge"`; a visible picker moves back to its edge.
#[tauri::command]
pub async fn reset_picker_position<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    tauri::async_runtime::spawn_blocking(save_edge).await??;
    tracing::info!("picker position reset to the edge");
    if let Some(window) = app.get_webview_window(PICKER_LABEL)
        && window.is_visible()?
    {
        show(&app)?;
    }
    Ok(())
}

// ── force_x11 ───────────────────────────────────────────────────────────────

/// `[picker] force_x11`: on native Wayland, re-executes this process with `GDK_BACKEND=x11`
/// (XWayland), so the picker can position itself. Returns only when no relaunch is needed or
/// the exec failed. Must run before Tauri starts. UNVERIFIED on real hardware (plan §H).
pub fn relaunch_under_x11_if_asked() {
    let force_x11 = load_picker().force_x11;
    if !picker_place::needs_x11_relaunch(std::env::consts::OS, force_x11, &|key| {
        std::env::var(key).ok()
    }) {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let Ok(exe) = std::env::current_exe() else {
            tracing::warn!("force_x11: cannot find the current executable");
            return;
        };
        tracing::info!("force_x11: relaunching under XWayland (GDK_BACKEND=x11)");
        let err = std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env("GDK_BACKEND", "x11")
            .env(picker_place::X11_RELAUNCH_MARKER, "1")
            .exec();
        tracing::warn!(error = %err, "force_x11: relaunch failed; staying on Wayland");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(x: i32, y: i32) -> PickerCustom {
        PickerCustom {
            x,
            y,
            monitor: "DP-1".into(),
        }
    }

    #[test]
    fn title_is_the_constant_compositor_rules_match() {
        assert_eq!(TITLE, "kakemono-picker");
        assert_eq!(TITLE, PICKER_LABEL);
    }

    #[test]
    fn nothing_retitles_the_picker_at_runtime() {
        let sources = [
            include_str!("picker.rs"),
            include_str!("picker_place.rs"),
            include_str!("windows.rs"),
            include_str!("lib.rs"),
            include_str!("tray.rs"),
            include_str!("commands.rs"),
        ];
        let needle = [".set", "_title("].concat();
        for source in sources {
            assert!(
                !source.contains(&needle),
                "window rules match the picker title"
            );
        }
        let caps = include_str!("../capabilities/default.json");
        assert!(
            !caps.contains("set-title"),
            "the frontend must not retitle windows"
        );
    }

    /// `#rrggbb` after `name:` in a CSS file.
    fn css_hex(css: &str, name: &str) -> Color {
        let at = css.find(&format!("{name}:")).expect(name) + name.len() + 1;
        let hex = css[at..]
            .trim_start()
            .strip_prefix('#')
            .expect("hex colour");
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        Color(byte(0), byte(2), byte(4), 0xff)
    }

    #[test]
    fn window_background_matches_the_page_tokens() {
        let tokens = include_str!("../../frontend/src/shared/tokens.css");
        assert_eq!(css_hex(tokens, "--bg-light"), BG_LIGHT);
        assert_eq!(css_hex(tokens, "--bg-dark"), BG_DARK);
        assert_eq!(background_for(Theme::Dark), BG_DARK);
        assert_eq!(background_for(Theme::Light), BG_LIGHT);
        // The inline <head> style paints the same colours before any CSS loads.
        let html = include_str!("../../frontend/picker.html");
        assert_eq!(css_hex(html, "background"), BG_LIGHT);
        let dark = &html[html.find("prefers-color-scheme: dark").unwrap()..];
        assert_eq!(css_hex(dark, "background"), BG_DARK);
        // The document title matches the window title the compositor rules key on.
        assert!(html.contains(&format!("<title>{TITLE}</title>")));
    }

    #[test]
    fn close_on_blur_false_keeps_the_picker_open() {
        let t0 = Instant::now();
        let mut s = Session::default();
        s.shown(t0, false);
        assert!(!s.hide_on_blur(t0 + Duration::from_secs(5)));
    }

    #[test]
    fn close_on_blur_hides_after_the_grace_period_only() {
        let t0 = Instant::now();
        let mut s = Session::default();
        s.shown(t0, true);
        assert!(!s.hide_on_blur(t0 + BLUR_GRACE - Duration::from_millis(1)));
        assert!(s.hide_on_blur(t0 + BLUR_GRACE));
        s.hidden();
        assert!(
            !s.hide_on_blur(t0 + Duration::from_secs(5)),
            "already hidden"
        );
    }

    #[test]
    fn a_drop_is_a_move_away_from_where_the_picker_was_put() {
        let mut s = Session::default();
        s.shown(Instant::now(), true);
        assert!(
            !s.is_drop(&custom(500, 900)),
            "unknown baseline never saves"
        );
        s.placed(0, 1280);
        assert!(!s.is_drop(&custom(0, 1280)));
        assert!(!s.is_drop(&custom(2, 1278)), "rounding noise");
        assert!(s.is_drop(&custom(300, 700)));
        s.shown(Instant::now(), true);
        assert!(
            !s.is_drop(&custom(300, 700)),
            "showing again resets the baseline"
        );
    }

    #[test]
    fn only_the_last_move_event_settles() {
        let mut s = Session::default();
        let first = s.bump_move();
        let second = s.bump_move();
        assert!(!s.move_settled(first));
        assert!(s.move_settled(second));
    }
}
