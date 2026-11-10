//! macOS: a per-user LaunchAgent, `~/Library/LaunchAgents/dev.rielj.kakemono.daemon.plist`,
//! bootstrapped into the `gui/<uid>` domain with `launchctl`.
//!
//! The label is `dev.rielj.kakemono.daemon`, not the app identifier `dev.rielj.kakemono` and not
//! the UI's `kakemono` LaunchAgent (tauri-plugin-autostart), so the two never replace each other.
//! Go's README plist (`com.rielj.wallpaperd`) had `RunAtLoad`, `KeepAlive = true` and logs in
//! `/tmp`; here `KeepAlive` restarts only after a failed exit (like systemd's `on-failure`) and
//! stdout/stderr go to `<log_dir>/launchd.log`. `PATH` is set to [`DAEMON_PATH`] so hooks find
//! Homebrew tools.

use std::path::{Path, PathBuf};

use super::{
    Installed, Running, ServiceEnv, ServiceError, ServiceStatus, Uninstalled, create_dir,
    remove_file, run_any, run_checked, write_file, xml_escape,
};
use crate::exec::{CommandRunner, argv};

/// The launchd label (and plist file stem) of the daemon service.
pub const LABEL: &str = "dev.rielj.kakemono.daemon";
/// The file launchd writes the daemon's stdout and stderr to, inside `log_dir`. The daemon logs
/// to stderr under `-f`, so this is its log.
pub const LOG_FILE_NAME: &str = "launchd.log";

/// The daemon's `PATH`. launchd starts jobs with `/usr/bin:/bin:/usr/sbin:/sbin`, so hooks such
/// as `on_change = ["sketchybar", ...]` would not find Homebrew tools. Fixed (Apple silicon and
/// Intel Homebrew first, then the system dirs), never the installing shell's `PATH`, so the
/// plist is the same on every install.
pub const DAEMON_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// `<home>/Library/LaunchAgents/dev.rielj.kakemono.daemon.plist`.
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// `<log_dir>/launchd.log`.
pub fn log_path(log_dir: &Path) -> PathBuf {
    log_dir.join(LOG_FILE_NAME)
}

/// The LaunchAgent plist: runs `<exe> daemon -f` at login with `PATH` = [`DAEMON_PATH`],
/// restarts it after a crash.
pub fn plist(env: &ServiceEnv) -> String {
    let exe = xml_escape(&env.exe.to_string_lossy());
    let log = xml_escape(&log_path(&env.log_dir).to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
		<string>daemon</string>
		<string>-f</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>{DAEMON_PATH}</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#
    )
}

/// What `launchctl print gui/<uid>/<label>` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintInfo {
    /// The top-level `state = ...` (`running`, `not running`, ...).
    pub state: Option<String>,
    pub pid: Option<u32>,
}

/// Parses the top-level `state` and `pid` (one tab of indentation; nested blocks have more).
pub fn parse_print(stdout: &str) -> PrintInfo {
    let mut info = PrintInfo {
        state: None,
        pid: None,
    };
    for line in stdout.lines() {
        let Some(line) = line.strip_prefix('\t') else {
            continue;
        };
        if line.starts_with('\t') {
            continue;
        }
        if let Some(state) = line.strip_prefix("state = ")
            && info.state.is_none()
        {
            info.state = Some(state.trim().to_owned());
        } else if let Some(pid) = line.strip_prefix("pid = ")
            && info.pid.is_none()
        {
            info.pid = pid.trim().parse().ok();
        }
    }
    info
}

fn service_target(uid: u32) -> String {
    format!("gui/{uid}/{LABEL}")
}

/// `Some` if the service is loaded in `gui/<uid>`, `None` if not. `Err` only when `launchctl`
/// can't be started.
async fn print(
    runner: &dyn CommandRunner,
    uid: u32,
) -> Result<Option<PrintInfo>, crate::exec::RunError> {
    let output = run_any(runner, argv(&["launchctl", "print", &service_target(uid)])).await?;
    Ok(output.status.success().then(|| parse_print(&output.stdout)))
}

pub(super) async fn install(
    env: &ServiceEnv,
    runner: &dyn CommandRunner,
) -> Result<Installed, ServiceError> {
    let path = plist_path(&env.home);
    create_dir(&env.log_dir)?;
    write_file(&path, &plist(env))?;

    let uid = env.uid;
    let target = service_target(uid);
    // A loaded job keeps its old definition until it is booted out.
    if print(runner, uid).await?.is_some() {
        run_checked(runner, argv(&["launchctl", "bootout", &target])).await?;
    }
    // Clears a `launchctl disable` override left by the user, which would block bootstrap.
    run_checked(runner, argv(&["launchctl", "enable", &target])).await?;
    let domain = format!("gui/{uid}");
    let plist_arg = path.to_string_lossy();
    run_checked(
        runner,
        argv(&["launchctl", "bootstrap", &domain, &plist_arg]),
    )
    .await?;

    Ok(Installed {
        location: path.display().to_string(),
        path: Some(path),
        command: format!("{} daemon -f", env.exe.display()),
        started: true,
    })
}

pub(super) async fn uninstall(
    env: &ServiceEnv,
    runner: &dyn CommandRunner,
) -> Result<Uninstalled, ServiceError> {
    let path = plist_path(&env.home);
    let uid = env.uid;
    // Boot out even when the file is gone: a loaded job outlives its plist.
    let loaded = print(runner, uid).await?.is_some();
    if loaded {
        run_checked(
            runner,
            argv(&["launchctl", "bootout", &service_target(uid)]),
        )
        .await?;
    }
    let removed_file = remove_file(&path)?;
    Ok(Uninstalled {
        removed: loaded || removed_file,
        location: path.display().to_string(),
    })
}

pub(super) async fn status(
    env: &ServiceEnv,
    runner: &dyn CommandRunner,
) -> Result<ServiceStatus, ServiceError> {
    let path = plist_path(&env.home);
    let installed = path.is_file();
    let uid = env.uid;
    let running = match print(runner, uid).await {
        Ok(Some(PrintInfo { state, pid })) if state.as_deref() == Some("running") => {
            Running::Yes { pid }
        }
        Ok(Some(PrintInfo { state, .. })) => Running::No { state },
        Ok(None) => Running::No {
            state: installed.then(|| "not loaded".to_owned()),
        },
        Err(_) => Running::Unknown,
    };
    Ok(ServiceStatus {
        installed,
        running,
        location: path.display().to_string(),
        path: Some(path),
    })
}
