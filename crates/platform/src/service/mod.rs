//! TODO(task 39): `service install|uninstall|status` for the daemon (LaunchAgent, systemd user unit, HKCU Run).

pub mod launchd;
pub mod systemd;
#[cfg(target_os = "windows")]
pub mod windows;
