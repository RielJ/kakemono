//! Linux: a systemd user unit, `$XDG_CONFIG_HOME/systemd/user/kakemono.service`, bound to
//! `graphical-session.target` (plan §F).
//!
//! Compositors that never start `graphical-session.target` (Hyprland without uwsm, plain sway or
//! niri sessions) won't start the unit, and a unit may miss `WAYLAND_DISPLAY` and friends unless
//! the compositor imports them. [`compositor_snippets`] gives the start-from-the-compositor
//! alternative that `service install` prints.

use std::path::{Path, PathBuf};

use super::{
    Installed, Running, ServiceEnv, ServiceError, ServiceStatus, Uninstalled, remove_file, run_any,
    run_checked, s, shell_quote, write_file,
};
use crate::exec::CommandRunner;

/// The unit name.
pub const UNIT_NAME: &str = "kakemono.service";

/// `<config_home>/systemd/user/kakemono.service`.
pub fn unit_path(config_home: &Path) -> PathBuf {
    config_home.join("systemd").join("user").join(UNIT_NAME)
}

/// The unit file. `ExecStart` is the absolute `exe` with `daemon -f`; logs go to the journal.
pub fn unit(exe: &Path) -> String {
    let exec = exec_quote(&exe.to_string_lossy());
    format!(
        "[Unit]
Description=kakemono wallpaper daemon
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=simple
ExecStart={exec} daemon -f
Restart=on-failure
RestartSec=5

[Install]
WantedBy=graphical-session.target
"
    )
}

/// Quotes one `ExecStart` word: `%` and `$` are always doubled (specifiers and variable
/// expansion apply inside quotes too); words with spaces, quotes or backslashes get C-style
/// double quotes.
pub fn exec_quote(word: &str) -> String {
    let escaped = word.replace('%', "%%").replace('$', "$$");
    let needs_quotes = escaped.is_empty()
        || escaped
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | ';'));
    if !needs_quotes {
        return escaped;
    }
    let mut out = String::with_capacity(escaped.len() + 2);
    out.push('"');
    for c in escaped.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What `service install` prints after enabling the unit: how to make the compositor start the
/// unit's target, or start the daemon itself.
pub fn compositor_snippets(exe: &Path) -> String {
    let exe_str = exe.to_string_lossy();
    let sh = shell_quote(&exe_str);
    let kdl = exe_str.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "The unit starts with graphical-session.target. If your compositor doesn't start that \
         target, or the daemon can't see your session, start the daemon from the compositor \
         config instead (it refuses to run twice, so this is safe alongside the unit):
  Hyprland (hyprland.conf):  exec-once = {sh} daemon
  sway (config):             exec {sh} daemon
  niri (config.kdl):         spawn-at-startup \"{kdl}\" \"daemon\"
To let the unit see the session, import it first, e.g. in Hyprland:
  exec-once = dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP HYPRLAND_INSTANCE_SIGNATURE
"
    )
}

fn systemctl<const N: usize>(args: [&str; N]) -> Vec<String> {
    let mut argv = s(["systemctl", "--user"]);
    argv.extend(args.iter().map(|a| (*a).to_owned()));
    argv
}

pub(super) async fn install(
    env: &ServiceEnv,
    runner: &dyn CommandRunner,
) -> Result<Installed, ServiceError> {
    let path = unit_path(&env.config_home);
    write_file(&path, &unit(&env.exe))?;
    run_checked(runner, systemctl(["daemon-reload"])).await?;
    run_checked(runner, systemctl(["enable", UNIT_NAME])).await?;
    // `restart` starts a stopped unit and picks up a rewritten one (`enable --now` would not).
    run_checked(runner, systemctl(["restart", UNIT_NAME])).await?;
    Ok(Installed {
        location: path.display().to_string(),
        path: Some(path),
        command: format!("{} daemon -f", exec_quote(&env.exe.to_string_lossy())),
        started: true,
    })
}

pub(super) async fn uninstall(
    env: &ServiceEnv,
    runner: &dyn CommandRunner,
) -> Result<Uninstalled, ServiceError> {
    let path = unit_path(&env.config_home);
    let location = path.display().to_string();
    if !path.is_file() {
        return Ok(Uninstalled {
            removed: false,
            location,
        });
    }
    // Stop and drop the wants symlink before the file goes; then forget the unit.
    run_checked(runner, systemctl(["disable", "--now", UNIT_NAME])).await?;
    remove_file(&path)?;
    run_checked(runner, systemctl(["daemon-reload"])).await?;
    Ok(Uninstalled {
        removed: true,
        location,
    })
}

pub(super) async fn status(
    env: &ServiceEnv,
    runner: &dyn CommandRunner,
) -> Result<ServiceStatus, ServiceError> {
    let path = unit_path(&env.config_home);
    let installed = path.is_file();
    // `is-active` prints the state and exits non-zero for anything but `active`.
    let running = match run_any(runner, systemctl(["is-active", UNIT_NAME])).await {
        Ok(output) => {
            let state = output.stdout.trim();
            if state == "active" {
                Running::Yes { pid: None }
            } else {
                Running::No {
                    state: (!state.is_empty()).then(|| state.to_owned()),
                }
            }
        }
        Err(_) => Running::Unknown,
    };
    Ok(ServiceStatus {
        installed,
        running,
        location: path.display().to_string(),
        path: Some(path),
    })
}
