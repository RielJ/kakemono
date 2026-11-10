//! The `Env` snapshot that [`resolve`](super::resolve::resolve) reads (plan §E "Inputs").
//!
//! Everything here except [`capture`], [`probe_dbus_names`] and [`capture_with_dbus`] is pure
//! or takes its inputs as arguments, so it compiles and is tested on every OS.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Environment variables the resolver reads.
pub const ENV_VARS: [&str; 10] = [
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "DESKTOP_SESSION",
    "XDG_SESSION_TYPE",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "SWAYSOCK",
    "NIRI_SOCKET",
    "KDE_FULL_SESSION",
];

/// Programs probed on `PATH`.
pub const BINARIES: [&str; 15] = [
    "gsettings",
    "xfconf-query",
    "awww",
    "swww",
    "hyprctl",
    "hyprpaper",
    "swaybg",
    "swaymsg",
    "wbg",
    "xwallpaper",
    "feh",
    "nitrogen",
    "hsetroot",
    "pcmanfm-qt",
    "pcmanfm",
];

/// Process names (`/proc/<pid>/comm`) worth knowing about. Each is at most 15 bytes, the kernel's
/// `comm` limit, so an exact match works.
pub const PROCESSES: [&str; 6] = [
    "awww-daemon",
    "swww-daemon",
    "hyprpaper",
    "swaybg",
    "wpaperd",
    "mpvpaper",
];

/// Session-bus names probed with `ListNames`.
pub const DBUS_NAMES: [&str; 4] = [
    "org.kde.plasmashell",
    "org.xfce.Xfconf",
    "org.freedesktop.portal.Desktop",
    "org.kde.StatusNotifierWatcher",
];

/// A snapshot of everything detection looks at. Serializable, so `doctor` can print it and tests
/// (and `detect --env-json`) can feed one in. Missing JSON fields default to empty.
///
/// Variables are stored raw (`None` when unset or empty); the resolver normalises case.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Env {
    pub xdg_current_desktop: Option<String>,
    pub xdg_session_desktop: Option<String>,
    pub desktop_session: Option<String>,
    pub xdg_session_type: Option<String>,
    pub wayland_display: Option<String>,
    pub display: Option<String>,
    pub hyprland_instance_signature: Option<String>,
    pub swaysock: Option<String>,
    pub niri_socket: Option<String>,
    pub kde_full_session: Option<String>,
    /// Names from [`BINARIES`] found on `PATH`.
    pub binaries: BTreeSet<String>,
    /// Names from [`PROCESSES`] that are running.
    pub processes: BTreeSet<String>,
    /// Names from [`DBUS_NAMES`] owned on the session bus. `None` = not probed (or no bus).
    pub dbus_names: Option<BTreeSet<String>>,
}

impl Env {
    /// Build the variable part from a lookup (`std::env::var` in production). Empty values count
    /// as unset. Binaries, processes and D-Bus names stay empty.
    pub fn from_vars(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut env = Env::default();
        for name in ENV_VARS {
            env.set_var(name, lookup(name));
        }
        env
    }

    /// Set one of [`ENV_VARS`] by name. Unknown names are ignored. Empty values become `None`.
    pub fn set_var(&mut self, name: &str, value: Option<String>) {
        let value = value.filter(|v| !v.trim().is_empty());
        if let Some(slot) = self.var_slot(name) {
            *slot = value;
        }
    }

    /// Builder form of [`set_var`](Self::set_var).
    #[must_use]
    pub fn with_var(mut self, name: &str, value: &str) -> Self {
        self.set_var(name, Some(value.to_owned()));
        self
    }

    /// Builder: mark binaries as installed.
    #[must_use]
    pub fn with_binaries(mut self, names: &[&str]) -> Self {
        self.binaries.extend(names.iter().map(|n| (*n).to_owned()));
        self
    }

    /// Builder: mark processes as running.
    #[must_use]
    pub fn with_processes(mut self, names: &[&str]) -> Self {
        self.processes.extend(names.iter().map(|n| (*n).to_owned()));
        self
    }

    /// The value of one of [`ENV_VARS`] by name.
    pub fn var(&self, name: &str) -> Option<&str> {
        let value = match name {
            "XDG_CURRENT_DESKTOP" => &self.xdg_current_desktop,
            "XDG_SESSION_DESKTOP" => &self.xdg_session_desktop,
            "DESKTOP_SESSION" => &self.desktop_session,
            "XDG_SESSION_TYPE" => &self.xdg_session_type,
            "WAYLAND_DISPLAY" => &self.wayland_display,
            "DISPLAY" => &self.display,
            "HYPRLAND_INSTANCE_SIGNATURE" => &self.hyprland_instance_signature,
            "SWAYSOCK" => &self.swaysock,
            "NIRI_SOCKET" => &self.niri_socket,
            "KDE_FULL_SESSION" => &self.kde_full_session,
            _ => return None,
        };
        value.as_deref()
    }

    fn var_slot(&mut self, name: &str) -> Option<&mut Option<String>> {
        Some(match name {
            "XDG_CURRENT_DESKTOP" => &mut self.xdg_current_desktop,
            "XDG_SESSION_DESKTOP" => &mut self.xdg_session_desktop,
            "DESKTOP_SESSION" => &mut self.desktop_session,
            "XDG_SESSION_TYPE" => &mut self.xdg_session_type,
            "WAYLAND_DISPLAY" => &mut self.wayland_display,
            "DISPLAY" => &mut self.display,
            "HYPRLAND_INSTANCE_SIGNATURE" => &mut self.hyprland_instance_signature,
            "SWAYSOCK" => &mut self.swaysock,
            "NIRI_SOCKET" => &mut self.niri_socket,
            "KDE_FULL_SESSION" => &mut self.kde_full_session,
            _ => return None,
        })
    }

    pub fn has_binary(&self, name: &str) -> bool {
        self.binaries.contains(name)
    }

    pub fn has_process(&self, name: &str) -> bool {
        self.processes.contains(name)
    }

    /// `Some(true/false)` if D-Bus was probed, `None` if not.
    pub fn has_dbus_name(&self, name: &str) -> Option<bool> {
        self.dbus_names.as_ref().map(|names| names.contains(name))
    }
}

/// Build an `Env` from injected sources: a variable lookup, a `PATH` probe and a procfs root
/// (`/proc` in production). Does not probe D-Bus.
pub fn capture_with(
    lookup: impl Fn(&str) -> Option<String>,
    on_path: impl Fn(&str) -> bool,
    proc_root: &Path,
) -> Env {
    let mut env = Env::from_vars(lookup);
    env.binaries = BINARIES
        .iter()
        .filter(|name| on_path(name))
        .map(|name| (*name).to_owned())
        .collect();
    env.processes = running_processes(proc_root);
    env
}

/// Names from [`PROCESSES`] found in `<proc_root>/<pid>/comm`. Unreadable entries (processes that
/// exited mid-scan, permission errors, a missing procfs) are skipped.
pub fn running_processes(proc_root: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return BTreeSet::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("comm")).ok())
        .map(|comm| comm.trim_end_matches('\n').to_owned())
        .filter(|comm| PROCESSES.contains(&comm.as_str()))
        .collect()
}

/// Snapshot the real system: environment variables, `PATH` (via `which`) and `/proc`. Blocking
/// (filesystem reads); call it from `spawn_blocking` inside async code. Does not probe D-Bus.
#[cfg(target_os = "linux")]
pub fn capture() -> Env {
    capture_with(
        |name| std::env::var(name).ok(),
        |bin| which::which(bin).is_ok(),
        Path::new("/proc"),
    )
}

/// How long the D-Bus probe may take before it is abandoned.
#[cfg(target_os = "linux")]
pub const DBUS_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Names from [`DBUS_NAMES`] owned on the session bus, or `None` if there is no session bus or
/// the probe fails or times out.
#[cfg(target_os = "linux")]
pub async fn probe_dbus_names() -> Option<BTreeSet<String>> {
    probe_dbus_names_on(&tokio::sync::OnceCell::new()).await
}

/// [`probe_dbus_names`] over `bus`, which is connected to the session bus on first use and
/// kept, so a caller that probes on every re-detection reuses one connection.
#[cfg(target_os = "linux")]
pub async fn probe_dbus_names_on(
    bus: &tokio::sync::OnceCell<zbus::Connection>,
) -> Option<BTreeSet<String>> {
    let probe = async {
        let conn = bus.get_or_try_init(zbus::Connection::session).await?;
        let proxy = zbus::fdo::DBusProxy::new(conn).await?;
        let names = proxy.list_names().await?;
        Ok::<_, zbus::Error>(
            names
                .iter()
                .map(|name| name.as_str())
                .filter(|name| DBUS_NAMES.contains(name))
                .map(str::to_owned)
                .collect(),
        )
    };
    match tokio::time::timeout(DBUS_PROBE_TIMEOUT, probe).await {
        Ok(Ok(names)) => Some(names),
        Ok(Err(err)) => {
            tracing::debug!(error = %err, "D-Bus name probe failed");
            None
        }
        Err(_) => {
            tracing::debug!("D-Bus name probe timed out");
            None
        }
    }
}

/// [`capture`] (on a blocking thread) plus [`probe_dbus_names`]. Must run inside a tokio runtime.
#[cfg(target_os = "linux")]
pub async fn capture_with_dbus() -> Env {
    capture_with_dbus_on(&tokio::sync::OnceCell::new()).await
}

/// [`capture_with_dbus`], probing over `bus` (see [`probe_dbus_names_on`]).
#[cfg(target_os = "linux")]
pub async fn capture_with_dbus_on(bus: &tokio::sync::OnceCell<zbus::Connection>) -> Env {
    let mut env = match tokio::task::spawn_blocking(capture).await {
        Ok(env) => env,
        Err(err) => {
            tracing::warn!(error = %err, "environment capture task failed");
            Env::default()
        }
    };
    env.dbus_names = probe_dbus_names_on(bus).await;
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_vars_drops_empty_and_unknown() {
        let env = Env::from_vars(|name| match name {
            "XDG_CURRENT_DESKTOP" => Some("ubuntu:GNOME".into()),
            "DISPLAY" => Some(String::new()),
            "WAYLAND_DISPLAY" => Some("  ".into()),
            _ => None,
        });
        assert_eq!(env.var("XDG_CURRENT_DESKTOP"), Some("ubuntu:GNOME"));
        assert_eq!(env.display, None);
        assert_eq!(env.wayland_display, None);
        assert_eq!(env.var("HOME"), None);
    }

    #[test]
    fn every_env_var_has_a_slot() {
        let env = Env::from_vars(|name| Some(name.to_lowercase()));
        for name in ENV_VARS {
            assert_eq!(env.var(name), Some(name.to_lowercase().as_str()), "{name}");
        }
    }

    #[test]
    fn processes_fit_in_comm() {
        for name in PROCESSES {
            assert!(name.len() <= 15, "{name} is longer than TASK_COMM_LEN - 1");
        }
    }

    #[test]
    fn running_processes_reads_numeric_comm_entries() {
        let dir = tempfile::tempdir().unwrap();
        let write = |pid: &str, comm: &str| {
            let p = dir.path().join(pid);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("comm"), format!("{comm}\n")).unwrap();
        };
        write("12", "awww-daemon");
        write("13", "bash");
        write("14", "swaybg");
        write("self", "hyprpaper");
        std::fs::create_dir_all(dir.path().join("15")).unwrap(); // no comm file
        let found = running_processes(dir.path());
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            ["awww-daemon", "swaybg"]
        );
        assert!(running_processes(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn capture_with_uses_injected_sources() {
        let dir = tempfile::tempdir().unwrap();
        let env = capture_with(
            |name| (name == "SWAYSOCK").then(|| "/run/sway.sock".to_owned()),
            |bin| matches!(bin, "swaymsg" | "feh" | "not-probed"),
            dir.path(),
        );
        assert_eq!(env.swaysock.as_deref(), Some("/run/sway.sock"));
        assert_eq!(env.binaries.iter().collect::<Vec<_>>(), ["feh", "swaymsg"]);
        assert!(env.processes.is_empty());
        assert_eq!(env.dbus_names, None);
    }

    #[test]
    fn env_round_trips_through_partial_json() {
        let env: Env =
            serde_json::from_str(r#"{"xdg_current_desktop":"KDE","binaries":["feh"]}"#).unwrap();
        assert_eq!(env.xdg_current_desktop.as_deref(), Some("KDE"));
        assert!(env.has_binary("feh"));
        let back: Env = serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();
        assert_eq!(back, env);
    }
}
