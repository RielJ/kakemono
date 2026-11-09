//! The pure resolver: `resolve(&Env, &DesktopConfig) -> Resolution`, implementing the plan §E
//! order. No system access, so it compiles and is table-tested on every OS.

use std::fmt;

use kakemono_config::schema::{Backend, DesktopConfig, ManagedBy};
use serde::{Deserialize, Serialize};

use super::env::Env;
use crate::api::UNCONFIGURED_HINT;

/// The backend a [`Resolution`] picked. Like [`Backend`] without `auto`/`native`, plus
/// `Unconfigured` (nothing usable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    /// `gsettings` (`org.gnome.desktop.background`). Task 31.
    Gnome,
    /// Plasma `evaluateScript` over D-Bus. Task 32.
    Kde,
    /// xfconf over D-Bus. Task 33.
    Xfce,
    /// `gsettings` (`org.cinnamon.desktop.background`). Task 31.
    Cinnamon,
    /// `gsettings` (`org.mate.background`). Task 31.
    Mate,
    /// Preset: `swaymsg output * bg …`.
    Sway,
    /// Preset: `hyprctl hyprpaper …`.
    Hyprpaper,
    /// Preset: `awww img …`.
    Awww,
    /// Preset: `swww img …`.
    Swww,
    /// Persistent preset: `swaybg`, supervised by kakemono.
    Swaybg,
    /// Persistent preset: `wbg`, supervised by kakemono.
    Wbg,
    /// Preset: `xwallpaper`.
    Xwallpaper,
    /// Preset: `feh --bg-…`.
    Feh,
    /// Preset: `nitrogen --set-… --save`.
    Nitrogen,
    /// Preset: `hsetroot`.
    Hsetroot,
    /// Preset: `pcmanfm --set-wallpaper`.
    Pcmanfm,
    /// Preset: `pcmanfm-qt --set-wallpaper`.
    PcmanfmQt,
    /// The user's `[desktop] set_command`. With no command it can't set anything; see `hint`.
    Command,
    /// Nothing usable was found. `set` fails with `hint`.
    Unconfigured,
}

impl BackendKind {
    /// The config string, e.g. `"pcmanfm-qt"`. `Unconfigured` is `"unconfigured"`.
    pub fn as_str(self) -> &'static str {
        match self.to_config() {
            Some(backend) => backend.as_str(),
            None => "unconfigured",
        }
    }

    /// The pinned kind for a config value. `None` for `auto` and `native` (detect instead).
    pub fn from_config(backend: Backend) -> Option<Self> {
        Some(match backend {
            Backend::Auto | Backend::Native => return None,
            Backend::Gnome => Self::Gnome,
            Backend::Kde => Self::Kde,
            Backend::Xfce => Self::Xfce,
            Backend::Cinnamon => Self::Cinnamon,
            Backend::Mate => Self::Mate,
            Backend::Sway => Self::Sway,
            Backend::Hyprpaper => Self::Hyprpaper,
            Backend::Awww => Self::Awww,
            Backend::Swww => Self::Swww,
            Backend::Swaybg => Self::Swaybg,
            Backend::Wbg => Self::Wbg,
            Backend::Xwallpaper => Self::Xwallpaper,
            Backend::Feh => Self::Feh,
            Backend::Nitrogen => Self::Nitrogen,
            Backend::Hsetroot => Self::Hsetroot,
            Backend::Pcmanfm => Self::Pcmanfm,
            Backend::PcmanfmQt => Self::PcmanfmQt,
            Backend::Command => Self::Command,
        })
    }

    /// The config value to persist. `None` for `Unconfigured` (keep `auto`).
    pub fn to_config(self) -> Option<Backend> {
        Some(match self {
            Self::Gnome => Backend::Gnome,
            Self::Kde => Backend::Kde,
            Self::Xfce => Backend::Xfce,
            Self::Cinnamon => Backend::Cinnamon,
            Self::Mate => Backend::Mate,
            Self::Sway => Backend::Sway,
            Self::Hyprpaper => Backend::Hyprpaper,
            Self::Awww => Backend::Awww,
            Self::Swww => Backend::Swww,
            Self::Swaybg => Backend::Swaybg,
            Self::Wbg => Backend::Wbg,
            Self::Xwallpaper => Backend::Xwallpaper,
            Self::Feh => Backend::Feh,
            Self::Nitrogen => Backend::Nitrogen,
            Self::Hsetroot => Backend::Hsetroot,
            Self::Pcmanfm => Backend::Pcmanfm,
            Self::PcmanfmQt => Backend::PcmanfmQt,
            Self::Command => Backend::Command,
            Self::Unconfigured => return None,
        })
    }

    /// True for backends driven by an argv (a preset table entry or `set_command`), i.e. the ones
    /// whose `set_command` is shown and editable. False for the API backends and `Unconfigured`.
    pub fn uses_command(self) -> bool {
        !matches!(
            self,
            Self::Gnome | Self::Kde | Self::Xfce | Self::Cinnamon | Self::Mate | Self::Unconfigured
        )
    }

    /// True for setters that keep running (swaybg, wbg) and must be supervised.
    pub fn is_persistent(self) -> bool {
        matches!(self, Self::Swaybg | Self::Wbg)
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a [`Resolution`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// `[desktop] backend` was used as-is (user-pinned, or a saved detection for this session).
    Config,
    /// Detection ran (backend `auto`/`native`, or the session drifted).
    Detected,
}

/// The result of [`resolve`]. Serializable for `doctor`, `detect` and snapshots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub backend: BackendKind,
    /// An explicit argv template to run. `None` with a preset backend means "use the preset"
    /// (`linux::backends::presets::argv(backend, fit)`). Only ever `Some` for
    /// [`uses_command`](BackendKind::uses_command) backends, and only from `[desktop]
    /// set_command`.
    pub set_command: Option<Vec<String>>,
    /// The command keeps running and must be supervised (swaybg, wbg, or `persistent = true`).
    pub persistent: bool,
    pub origin: Origin,
    /// The current session, e.g. `"hyprland:wayland"`. Persist as `detected_session`.
    pub session_key: String,
    /// What to tell the user when this resolution can't set a wallpaper on its own
    /// (`Unconfigured`, or `Command` with no `set_command`). Use as `SetError::Unconfigured`.
    pub hint: Option<String>,
    /// Why, in order, for `doctor` and logs.
    pub reasons: Vec<String>,
}

impl Resolution {
    /// True if this can set a wallpaper (not `Unconfigured`, and `Command` has an argv).
    pub fn is_usable(&self) -> bool {
        match self.backend {
            BackendKind::Unconfigured => false,
            BackendKind::Command => self.set_command.is_some(),
            _ => true,
        }
    }
}

/// How the graphical session presents itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionType {
    Wayland,
    X11,
    Tty,
    Unknown,
}

impl SessionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wayland => "wayland",
            Self::X11 => "x11",
            Self::Tty => "tty",
            Self::Unknown => "unknown",
        }
    }
}

/// A compositor identified by its IPC socket variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Compositor {
    Hyprland,
    Sway,
    Niri,
}

impl Compositor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hyprland => "hyprland",
            Self::Sway => "sway",
            Self::Niri => "niri",
        }
    }

    fn var(self) -> &'static str {
        match self {
            Self::Hyprland => "HYPRLAND_INSTANCE_SIGNATURE",
            Self::Sway => "SWAYSOCK",
            Self::Niri => "NIRI_SOCKET",
        }
    }
}

/// The session facts the resolver and the session key use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Lowercased desktop names: `XDG_CURRENT_DESKTOP` split on `:`, else `XDG_SESSION_DESKTOP`,
    /// else the last path segment of `DESKTOP_SESSION`.
    pub desktops: Vec<String>,
    /// The variable `desktops` came from, for reasons.
    pub desktop_source: Option<String>,
    pub compositor: Option<Compositor>,
    pub session_type: SessionType,
}

impl Session {
    pub fn of(env: &Env) -> Self {
        let split = |value: &str| -> Vec<String> {
            value
                .split(':')
                .map(|part| part.trim().to_lowercase())
                .filter(|part| !part.is_empty())
                .collect()
        };
        let mut desktops = Vec::new();
        let mut desktop_source = None;
        for var in [
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_DESKTOP",
            "DESKTOP_SESSION",
        ] {
            let Some(value) = env.var(var) else { continue };
            let value = if var == "DESKTOP_SESSION" {
                value.rsplit('/').next().unwrap_or(value)
            } else {
                value
            };
            desktops = split(value);
            if !desktops.is_empty() {
                desktop_source = Some(var.to_owned());
                break;
            }
        }

        let compositor = [Compositor::Hyprland, Compositor::Sway, Compositor::Niri]
            .into_iter()
            .find(|c| env.var(c.var()).is_some());

        let declared = env.xdg_session_type.as_deref().map(str::to_lowercase);
        let session_type = match declared.as_deref() {
            Some("wayland") => SessionType::Wayland,
            Some("x11") => SessionType::X11,
            // A compositor started from a tty often keeps `XDG_SESSION_TYPE=tty`; trust the
            // display variables over it.
            _ if env.wayland_display.is_some() => SessionType::Wayland,
            _ if env.display.is_some() => SessionType::X11,
            Some("tty") => SessionType::Tty,
            _ => SessionType::Unknown,
        };

        Self {
            desktops,
            desktop_source,
            compositor,
            session_type,
        }
    }

    /// `"<desktop>:<type>"`, e.g. `"hyprland:wayland"`, `"ubuntu+gnome:wayland"`, `"i3:x11"`.
    /// The desktop part is the desktop names joined with `+`, else the compositor, else
    /// `"unknown"`.
    pub fn key(&self) -> String {
        let desktop = if self.desktops.is_empty() {
            self.compositor
                .map_or_else(|| "unknown".to_owned(), |c| c.as_str().to_owned())
        } else {
            self.desktops.join("+")
        };
        format!("{desktop}:{}", self.session_type.as_str())
    }
}

/// The current session key for `env` (see [`Session::key`]).
pub fn session_key(env: &Env) -> String {
    Session::of(env).key()
}

const COSMIC_HINT: &str = "COSMIC has no wallpaper command kakemono can drive yet. Set \
     `[desktop] set_command` to a command that sets the wallpaper, or run `kakemono doctor`.";
const WPAPERD_HINT: &str = "wpaperd is running and manages the wallpaper itself (control it with \
     `wpaperctl`). Stop wpaperd so kakemono can pick a backend, or set `[desktop] set_command`.";
const COMMAND_HINT: &str =
    "`[desktop] backend = \"command\"` needs `[desktop] set_command`. Run `kakemono doctor`.";

/// Resolve the backend (plan §E order, first match wins):
/// 1. a pinned `[desktop] backend` (anything but `auto`/`native`), unless `managed_by =
///    "detect"` and the session drifted from `detected_session`;
/// 2. a full desktop environment;
/// 3. a running wallpaper daemon;
/// 4. the compositor (Hyprland, sway, niri or another Wayland session);
/// 5. an X11 setter;
/// 6. `Unconfigured`.
pub fn resolve(env: &Env, cfg: &DesktopConfig) -> Resolution {
    let session = Session::of(env);
    let session_key = session.key();
    let mut reasons = Vec::new();

    if let Some(kind) = BackendKind::from_config(cfg.backend) {
        match (cfg.managed_by, cfg.detected_session.as_deref()) {
            (ManagedBy::User, _) => {
                reasons.push(format!(
                    "backend = \"{kind}\" is pinned (managed_by = \"user\")"
                ));
                return from_config(kind, cfg, session_key, reasons);
            }
            (ManagedBy::Detect, None) => {
                reasons.push(format!(
                    "backend = \"{kind}\" is set in the config (no detected_session)"
                ));
                return from_config(kind, cfg, session_key, reasons);
            }
            (ManagedBy::Detect, Some(saved)) if saved == session_key => {
                reasons.push(format!(
                    "backend = \"{kind}\" was detected for this session ({session_key})"
                ));
                return from_config(kind, cfg, session_key, reasons);
            }
            (ManagedBy::Detect, Some(saved)) => reasons.push(format!(
                "session drift: detected_session = \"{saved}\" but this session is \
                 \"{session_key}\"; re-detecting"
            )),
        }
    } else if cfg.backend == Backend::Native {
        reasons.push("backend = \"native\" means \"detect\" on Linux".to_owned());
    } else {
        reasons.push("backend = \"auto\"; detecting".to_owned());
    }

    let (kind, hint) = detect(env, &session, &mut reasons);
    finish_detected(kind, hint, cfg, session_key, reasons)
}

fn from_config(
    kind: BackendKind,
    cfg: &DesktopConfig,
    session_key: String,
    mut reasons: Vec<String>,
) -> Resolution {
    let set_command = if kind.uses_command() {
        cfg.set_command.as_ref().map(|c| c.argv().to_vec())
    } else {
        if cfg.set_command.is_some() {
            reasons.push(format!("set_command is ignored by the {kind} backend"));
        }
        None
    };
    if set_command.is_some() && kind != BackendKind::Command {
        reasons.push("set_command overrides the preset argv".to_owned());
    }
    let hint =
        (kind == BackendKind::Command && set_command.is_none()).then(|| COMMAND_HINT.to_owned());
    let persistent = kind.is_persistent() || (kind.uses_command() && cfg.persistent);
    Resolution {
        backend: kind,
        set_command,
        persistent,
        origin: Origin::Config,
        session_key,
        hint,
        reasons,
    }
}

fn finish_detected(
    kind: BackendKind,
    hint: Option<&str>,
    cfg: &DesktopConfig,
    session_key: String,
    mut reasons: Vec<String>,
) -> Resolution {
    // Detected presets always use the preset argv: a saved `set_command` belongs to whatever was
    // configured before. Only a detected `command` backend borrows the user's `set_command`.
    let set_command = (kind == BackendKind::Command)
        .then(|| cfg.set_command.as_ref().map(|c| c.argv().to_vec()))
        .flatten();
    let mut persistent = kind.is_persistent();
    let hint = match (kind, &set_command) {
        (BackendKind::Command, Some(_)) => {
            reasons.push("using [desktop] set_command".to_owned());
            persistent = cfg.persistent;
            None
        }
        (BackendKind::Unconfigured, _) => Some(UNCONFIGURED_HINT.to_owned()),
        _ => hint.map(str::to_owned),
    };
    Resolution {
        backend: kind,
        set_command,
        persistent,
        origin: Origin::Detected,
        session_key,
        hint,
        reasons,
    }
}

/// Steps 2–6. Returns the kind and, for `command`, the hint to show without a `set_command`.
fn detect(
    env: &Env,
    session: &Session,
    reasons: &mut Vec<String>,
) -> (BackendKind, Option<&'static str>) {
    // 2. Full desktop environments.
    if let Some(found) = desktop_environment(env, session, reasons) {
        return found;
    }

    // 3. A wallpaper daemon the user already runs.
    let daemons: [(&str, BackendKind, Option<&'static str>); 4] = [
        ("awww-daemon", BackendKind::Awww, None),
        ("swww-daemon", BackendKind::Swww, None),
        ("hyprpaper", BackendKind::Hyprpaper, None),
        ("wpaperd", BackendKind::Command, Some(WPAPERD_HINT)),
    ];
    for (process, kind, hint) in daemons {
        if env.has_process(process) {
            reasons.push(format!("{process} is running → {kind}"));
            return (kind, hint);
        }
    }

    // 4. Compositor signature.
    match session.compositor {
        Some(Compositor::Hyprland) => {
            if let Some(kind) = first_installed(
                env,
                &[
                    BackendKind::Hyprpaper,
                    BackendKind::Awww,
                    BackendKind::Swww,
                    BackendKind::Swaybg,
                ],
            ) {
                reasons.push(format!(
                    "Hyprland (HYPRLAND_INSTANCE_SIGNATURE) with {} installed → {kind}",
                    binary_of(kind)
                ));
                return (kind, None);
            }
            reasons.push(
                "Hyprland, but none of hyprpaper, awww, swww or swaybg is installed".to_owned(),
            );
        }
        Some(Compositor::Sway) => {
            reasons.push("sway (SWAYSOCK) → sway (swaymsg output * bg)".to_owned());
            return (BackendKind::Sway, None);
        }
        Some(Compositor::Niri) | None => {}
    }
    if session.compositor.is_some() || session.session_type == SessionType::Wayland {
        let what = match session.compositor {
            Some(Compositor::Niri) => "niri (NIRI_SOCKET)",
            _ => "Wayland session",
        };
        if let Some(kind) = first_installed(
            env,
            &[
                BackendKind::Awww,
                BackendKind::Swww,
                BackendKind::Swaybg,
                BackendKind::Wbg,
            ],
        ) {
            reasons.push(format!(
                "{what} with {} installed → {kind}",
                binary_of(kind)
            ));
            return (kind, None);
        }
        reasons.push(format!(
            "{what}, but none of awww, swww, swaybg or wbg is installed"
        ));
        return (BackendKind::Unconfigured, None);
    }

    // 5. X11 without a known desktop environment.
    if session.session_type == SessionType::X11 {
        if let Some(kind) = first_installed(
            env,
            &[
                BackendKind::Xwallpaper,
                BackendKind::Feh,
                BackendKind::Nitrogen,
                BackendKind::Hsetroot,
            ],
        ) {
            reasons.push(format!(
                "X11 session with {} installed → {kind}",
                binary_of(kind)
            ));
            return (kind, None);
        }
        reasons.push(
            "X11 session, but none of xwallpaper, feh, nitrogen or hsetroot is installed"
                .to_owned(),
        );
        return (BackendKind::Unconfigured, None);
    }

    // 6. Nothing.
    reasons.push(
        "no graphical session found (no WAYLAND_DISPLAY, DISPLAY or XDG_SESSION_TYPE)".to_owned(),
    );
    (BackendKind::Unconfigured, None)
}

fn desktop_environment(
    env: &Env,
    session: &Session,
    reasons: &mut Vec<String>,
) -> Option<(BackendKind, Option<&'static str>)> {
    let source = session.desktop_source.as_deref().unwrap_or("desktop");
    for name in &session.desktops {
        if let Some((kind, hint)) = desktop_backend(name) {
            reasons.push(format!("{source} has \"{name}\" → {kind}"));
            check_desktop_tools(env, kind, reasons);
            return Some((kind, hint));
        }
    }
    if env
        .kde_full_session
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    {
        reasons.push("KDE_FULL_SESSION=true → kde".to_owned());
        check_desktop_tools(env, BackendKind::Kde, reasons);
        return Some((BackendKind::Kde, None));
    }
    if session.compositor.is_none() && env.has_dbus_name("org.kde.plasmashell") == Some(true) {
        reasons.push("org.kde.plasmashell is on the session bus → kde".to_owned());
        return Some((BackendKind::Kde, None));
    }
    None
}

/// The backend for one lowercased desktop name, if it is a full desktop environment.
pub fn desktop_backend(name: &str) -> Option<(BackendKind, Option<&'static str>)> {
    let kind = match name {
        "kde" | "plasma" | "plasmawayland" | "plasmax11" => BackendKind::Kde,
        "gnome" | "gnome-xorg" | "gnome-wayland" | "gnome-classic" | "gnome-flashback"
        | "ubuntu" | "ubuntu-wayland" | "ubuntu-xorg" | "pop" | "unity" | "budgie"
        | "budgie-desktop" => BackendKind::Gnome,
        "x-cinnamon" | "cinnamon" | "cinnamon2d" | "cinnamon-wayland" => BackendKind::Cinnamon,
        "mate" => BackendKind::Mate,
        "xfce" | "xfce4" => BackendKind::Xfce,
        "lxqt" => BackendKind::PcmanfmQt,
        "lxde" => BackendKind::Pcmanfm,
        "cosmic" => return Some((BackendKind::Command, Some(COSMIC_HINT))),
        _ => return None,
    };
    Some((kind, None))
}

/// Notes (not failures) when a desktop's tool or bus name is missing; `doctor` shows them.
fn check_desktop_tools(env: &Env, kind: BackendKind, reasons: &mut Vec<String>) {
    let binary = match kind {
        BackendKind::Gnome | BackendKind::Cinnamon | BackendKind::Mate => Some("gsettings"),
        BackendKind::Pcmanfm | BackendKind::PcmanfmQt => Some(binary_of(kind)),
        _ => None,
    };
    if let Some(binary) = binary
        && !env.has_binary(binary)
    {
        reasons.push(format!("warning: {binary} is not on PATH"));
    }
    let bus_name = match kind {
        BackendKind::Kde => Some("org.kde.plasmashell"),
        BackendKind::Xfce => Some("org.xfce.Xfconf"),
        _ => None,
    };
    if let Some(bus_name) = bus_name
        && env.has_dbus_name(bus_name) == Some(false)
    {
        reasons.push(format!("warning: {bus_name} is not on the session bus"));
    }
}

fn first_installed(env: &Env, kinds: &[BackendKind]) -> Option<BackendKind> {
    kinds
        .iter()
        .copied()
        .find(|kind| env.has_binary(binary_of(*kind)))
}

/// The program a preset backend needs on `PATH` (for API backends, their tool or name).
pub fn binary_of(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Gnome | BackendKind::Cinnamon | BackendKind::Mate => "gsettings",
        BackendKind::Kde => "plasmashell",
        BackendKind::Xfce => "xfconf-query",
        BackendKind::Sway => "swaymsg",
        BackendKind::Hyprpaper => "hyprpaper",
        BackendKind::Awww => "awww",
        BackendKind::Swww => "swww",
        BackendKind::Swaybg => "swaybg",
        BackendKind::Wbg => "wbg",
        BackendKind::Xwallpaper => "xwallpaper",
        BackendKind::Feh => "feh",
        BackendKind::Nitrogen => "nitrogen",
        BackendKind::Hsetroot => "hsetroot",
        BackendKind::Pcmanfm => "pcmanfm",
        BackendKind::PcmanfmQt => "pcmanfm-qt",
        BackendKind::Command | BackendKind::Unconfigured => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_round_trips_config() {
        for backend in Backend::ALL {
            match BackendKind::from_config(backend) {
                Some(kind) => {
                    assert_eq!(kind.to_config(), Some(backend));
                    assert_eq!(kind.as_str(), backend.as_str());
                }
                None => assert!(matches!(backend, Backend::Auto | Backend::Native)),
            }
        }
        assert_eq!(BackendKind::Unconfigured.to_config(), None);
        assert_eq!(BackendKind::Unconfigured.to_string(), "unconfigured");
    }

    #[test]
    fn session_type_prefers_display_vars_over_tty() {
        let env = Env::default()
            .with_var("XDG_SESSION_TYPE", "tty")
            .with_var("WAYLAND_DISPLAY", "wayland-1");
        assert_eq!(Session::of(&env).session_type, SessionType::Wayland);
        let env = Env::default().with_var("XDG_SESSION_TYPE", "tty");
        assert_eq!(Session::of(&env).session_type, SessionType::Tty);
    }

    #[test]
    fn session_key_shapes() {
        let env = Env::default()
            .with_var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")
            .with_var("XDG_SESSION_TYPE", "wayland");
        assert_eq!(session_key(&env), "ubuntu+gnome:wayland");
        let env = Env::default()
            .with_var("NIRI_SOCKET", "/run/niri.sock")
            .with_var("WAYLAND_DISPLAY", "wayland-1");
        assert_eq!(session_key(&env), "niri:wayland");
        let env = Env::default().with_var("DESKTOP_SESSION", "/usr/share/xsessions/plasma");
        assert_eq!(session_key(&env), "plasma:unknown");
        assert_eq!(session_key(&Env::default()), "unknown:unknown");
    }
}
