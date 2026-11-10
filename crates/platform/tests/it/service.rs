//! `service`: the generated launchd plist / systemd unit / Windows Run value, and
//! the install / uninstall / status command sequences against `RecordingRunner` and
//! `MemoryRunKey`. Every OS's backend runs on every host: definitions go to a temp dir and no
//! real `launchctl`, `systemctl` or registry is touched.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_platform::CommandStatus;
use kakemono_platform::exec::{Output, RecordingRunner};
use kakemono_platform::service::windows::{MemoryRunKey, RunKey};
use kakemono_platform::service::{
    Os, Running, ServiceEnv, ServiceError, ServiceManager, launchd, systemd, windows,
};

fn env(os: Os, root: &Path) -> ServiceEnv {
    let exe = match os {
        Os::Windows => PathBuf::from(r"C:\Program Files\kakemono\kakemono.exe"),
        _ => PathBuf::from("/opt/kake mono/bin/kakemono"),
    };
    ServiceEnv {
        os,
        home: root.join("home"),
        config_home: root.join("home/.config"),
        log_dir: root.join("home/Library/Logs/kakemono"),
        exe,
        uid: 501,
    }
}

fn manager(os: Os, root: &Path) -> (ServiceManager, RecordingRunner, MemoryRunKey) {
    let runner = RecordingRunner::new();
    let key = MemoryRunKey::new();
    let manager = ServiceManager::with_parts(
        env(os, root),
        Arc::new(runner.clone()),
        Arc::new(key.clone()),
    );
    (manager, runner, key)
}

fn out(code: i32, stdout: &str) -> Output {
    Output {
        status: CommandStatus::Exited(code),
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

/// Argvs with the temp root replaced by `<tmp>`, for stable snapshots.
fn argvs(runner: &RecordingRunner, root: &Path) -> Vec<Vec<String>> {
    let root = root.to_string_lossy();
    runner
        .argvs()
        .into_iter()
        .map(|argv| {
            argv.into_iter()
                .map(|a| a.replace(&*root, "<tmp>"))
                .collect()
        })
        .collect()
}

fn fixed_env(os: Os) -> ServiceEnv {
    env(os, Path::new("/Users/u"))
}

// ---- definitions ----

#[test]
fn launchd_plist_snapshot() {
    insta::assert_snapshot!("launchd_plist", launchd::plist(&fixed_env(Os::MacOs)));
}

#[test]
fn launchd_plist_sets_a_fixed_path_with_homebrew() {
    // launchd's default PATH (/usr/bin:/bin:/usr/sbin:/sbin) misses Homebrew, so hooks like
    // `sketchybar` failed. The value is fixed, never the installing shell's PATH.
    assert_eq!(
        launchd::DAEMON_PATH,
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    );
    let plist = launchd::plist(&fixed_env(Os::MacOs));
    assert!(plist.contains(&format!(
        "<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>PATH</key>\n\t\t<string>{}</string>",
        launchd::DAEMON_PATH
    )));
}

#[test]
fn launchd_plist_escapes_xml() {
    let mut env = fixed_env(Os::MacOs);
    env.exe = PathBuf::from("/Apps/R&D <x>/kakemono");
    let plist = launchd::plist(&env);
    assert!(plist.contains("<string>/Apps/R&amp;D &lt;x&gt;/kakemono</string>"));
}

#[test]
fn systemd_unit_snapshot() {
    insta::assert_snapshot!(
        "systemd_unit",
        systemd::unit(Path::new("/home/u/.local/bin/kakemono"))
    );
}

#[test]
fn systemd_exec_quote() {
    assert_eq!(
        systemd::exec_quote("/usr/bin/kakemono"),
        "/usr/bin/kakemono"
    );
    assert_eq!(
        systemd::exec_quote("/opt/a b/kakemono"),
        "\"/opt/a b/kakemono\""
    );
    assert_eq!(systemd::exec_quote("/x/100%/$y"), "/x/100%%/$$y");
    assert_eq!(systemd::exec_quote(r#"/a "b"\c"#), r#""/a \"b\"\\c""#);
}

#[test]
fn compositor_snippets_snapshot() {
    insta::assert_snapshot!(
        "compositor_snippets",
        systemd::compositor_snippets(Path::new("/home/u/.local/bin/kakemono"))
    );
}

#[test]
fn windows_run_command_snapshot() {
    insta::assert_snapshot!(
        "windows_run_command",
        windows::run_command(Path::new(r"C:\Program Files\kakemono\kakemono.exe"))
    );
}

#[test]
fn definition_paths() {
    let mac = fixed_env(Os::MacOs);
    assert_eq!(
        mac.definition_path().unwrap(),
        Path::new("/Users/u/home/Library/LaunchAgents/dev.rielj.kakemono.daemon.plist")
    );
    let linux = fixed_env(Os::Linux);
    assert_eq!(
        linux.definition_path().unwrap(),
        Path::new("/Users/u/home/.config/systemd/user/kakemono.service")
    );
    let win = fixed_env(Os::Windows);
    assert_eq!(win.definition_path(), None);
    assert_eq!(
        win.location(),
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\kakemono-daemon"
    );
}

#[test]
fn labels_differ_from_the_ui_autostart() {
    // tauri-plugin-autostart registers the UI as `kakemono` (LaunchAgent label, Run value).
    assert_ne!(launchd::LABEL, "kakemono");
    assert_ne!(windows::RUN_VALUE_NAME, "kakemono");
}

// ---- launchctl print parsing ----

const PRINT_RUNNING: &str = "gui/501/dev.rielj.kakemono.daemon = {
\tactive count = 1
\tpath = /Users/u/Library/LaunchAgents/dev.rielj.kakemono.daemon.plist
\ttype = LaunchAgent
\tstate = running

\tprogram = /opt/kakemono
\targuments = {
\t\t/opt/kakemono
\t\tdaemon
\t\t-f
\t}

\tpid = 4242
\tendpoint = {
\t\tstate = active
\t}
}
";

const PRINT_STOPPED: &str = "gui/501/dev.rielj.kakemono.daemon = {
\tactive count = 0
\tstate = not running
\tlast exit code = 1
}
";

#[test]
fn parses_launchctl_print() {
    assert_eq!(
        launchd::parse_print(PRINT_RUNNING),
        launchd::PrintInfo {
            state: Some("running".into()),
            pid: Some(4242)
        }
    );
    assert_eq!(
        launchd::parse_print(PRINT_STOPPED),
        launchd::PrintInfo {
            state: Some("not running".into()),
            pid: None
        }
    );
}

// ---- macOS ----

#[tokio::test]
async fn launchd_install_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::MacOs, tmp.path());
    runner.push_output(out(113, "")); // print: not loaded
    let installed = manager.install().await.unwrap();

    let path = launchd::plist_path(&tmp.path().join("home"));
    assert_eq!(installed.path.as_deref(), Some(path.as_path()));
    assert!(installed.started);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        launchd::plist(manager.env())
    );
    assert!(tmp.path().join("home/Library/Logs/kakemono").is_dir());
    insta::assert_json_snapshot!("launchd_install_fresh_argvs", argvs(&runner, tmp.path()));
}

#[tokio::test]
async fn launchd_install_reloads_a_loaded_job() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::MacOs, tmp.path());
    runner.push_output(out(0, PRINT_RUNNING));
    manager.install().await.unwrap();
    insta::assert_json_snapshot!("launchd_install_reload_argvs", argvs(&runner, tmp.path()));
}

#[tokio::test]
async fn launchd_install_reports_bootstrap_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::MacOs, tmp.path());
    runner.push_output(out(113, ""));
    runner.push_output(Output::success()); // enable
    runner.push_output(Output::failure(
        5,
        "Bootstrap failed: 5: Input/output error\n",
    ));
    let err = manager.install().await.unwrap_err();
    assert!(matches!(err, ServiceError::Command { .. }), "{err:?}");
    let message = err.to_string();
    assert!(
        message.starts_with("`launchctl bootstrap gui/501 "),
        "{message}"
    );
    assert!(
        message.ends_with("(exit code 5): Bootstrap failed: 5: Input/output error"),
        "{message}"
    );
}

#[tokio::test]
async fn launchd_uninstall_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::MacOs, tmp.path());
    let path = launchd::plist_path(&tmp.path().join("home"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "x").unwrap();

    runner.push_output(out(0, PRINT_RUNNING));
    let first = manager.uninstall().await.unwrap();
    assert!(first.removed);
    assert!(!path.exists());

    runner.push_output(out(113, "Could not find service"));
    let second = manager.uninstall().await.unwrap();
    assert!(!second.removed);
    insta::assert_json_snapshot!("launchd_uninstall_twice_argvs", argvs(&runner, tmp.path()));
}

#[tokio::test]
async fn launchd_status_distinguishes_states() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::MacOs, tmp.path());

    // Not installed.
    runner.push_output(out(113, ""));
    let status = manager.status().await.unwrap();
    assert!(!status.installed);
    assert_eq!(status.running, Running::No { state: None });

    // Installed and running.
    let path = launchd::plist_path(&tmp.path().join("home"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "x").unwrap();
    runner.push_output(out(0, PRINT_RUNNING));
    let status = manager.status().await.unwrap();
    assert!(status.installed);
    assert_eq!(status.running, Running::Yes { pid: Some(4242) });
    assert_eq!(status.path.as_deref(), Some(path.as_path()));

    // Installed, loaded, stopped.
    runner.push_output(out(0, PRINT_STOPPED));
    let status = manager.status().await.unwrap();
    assert_eq!(
        status.running,
        Running::No {
            state: Some("not running".into())
        }
    );

    // Installed, not loaded.
    runner.push_output(out(113, ""));
    let status = manager.status().await.unwrap();
    assert_eq!(
        status.running,
        Running::No {
            state: Some("not loaded".into())
        }
    );

    // launchctl missing.
    runner.push_spawn_error("No such file or directory");
    let status = manager.status().await.unwrap();
    assert_eq!(status.running, Running::Unknown);

    // Status never changes anything: only print.
    for argv in runner.argvs() {
        assert!(argv[..2] == ["launchctl", "print"], "{argv:?}");
    }
}

// ---- Linux ----

#[tokio::test]
async fn systemd_install_writes_unit_and_enables() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::Linux, tmp.path());
    let installed = manager.install().await.unwrap();
    let path = systemd::unit_path(&tmp.path().join("home/.config"));
    assert_eq!(installed.path.as_deref(), Some(path.as_path()));
    assert_eq!(
        installed.command,
        "\"/opt/kake mono/bin/kakemono\" daemon -f"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        systemd::unit(Path::new("/opt/kake mono/bin/kakemono"))
    );
    insta::assert_json_snapshot!("systemd_install_argvs", argvs(&runner, tmp.path()));
}

#[tokio::test]
async fn systemd_install_reports_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::Linux, tmp.path());
    runner.push_output(Output::failure(
        1,
        "Failed to connect to bus: No medium found",
    ));
    let err = manager.install().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "`systemctl --user daemon-reload` failed (exit code 1): Failed to connect to bus: No medium found"
    );
}

/// L9: a non-UTF-8 exe would be written lossily, so the unit or plist would name a file that
/// does not exist. Install refuses before writing or running anything.
#[cfg(unix)]
#[tokio::test]
async fn install_refuses_a_non_utf8_exe() {
    use std::os::unix::ffi::OsStrExt;

    let tmp = tempfile::tempdir().unwrap();
    for os in [Os::Linux, Os::MacOs, Os::Windows] {
        let runner = RecordingRunner::new();
        let key = MemoryRunKey::new();
        let exe = PathBuf::from(std::ffi::OsStr::from_bytes(b"/opt/kake\xffmono/kakemono"));
        let manager = ServiceManager::with_parts(
            ServiceEnv {
                exe: exe.clone(),
                ..env(os, tmp.path())
            },
            Arc::new(runner.clone()),
            Arc::new(key.clone()),
        );
        let err = manager.install().await.unwrap_err();
        assert!(
            matches!(&err, ServiceError::NonUtf8Path(path) if *path == exe),
            "{os:?}: {err:?}"
        );
        assert!(runner.argvs().is_empty(), "{os:?}");
        assert_eq!(key.value(), None, "{os:?}");
        assert!(manager.env().definition_path().is_none_or(|p| !p.exists()));
    }
}

#[tokio::test]
async fn systemd_uninstall_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::Linux, tmp.path());
    manager.install().await.unwrap();
    let installs = runner.argvs().len();

    let first = manager.uninstall().await.unwrap();
    assert!(first.removed);
    assert!(!systemd::unit_path(&tmp.path().join("home/.config")).exists());
    let second = manager.uninstall().await.unwrap();
    assert!(!second.removed);

    let uninstall_argvs: Vec<_> = argvs(&runner, tmp.path()).split_off(installs);
    insta::assert_json_snapshot!("systemd_uninstall_twice_argvs", uninstall_argvs);
}

#[tokio::test]
async fn systemd_status_distinguishes_states() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, _) = manager(Os::Linux, tmp.path());

    runner.push_output(out(4, "inactive\n"));
    let status = manager.status().await.unwrap();
    assert!(!status.installed);
    assert_eq!(
        status.running,
        Running::No {
            state: Some("inactive".into())
        }
    );

    manager.install().await.unwrap();
    runner.push_output(out(0, "active\n"));
    let status = manager.status().await.unwrap();
    assert!(status.installed);
    assert_eq!(status.running, Running::Yes { pid: None });

    runner.push_output(out(3, "failed\n"));
    let status = manager.status().await.unwrap();
    assert_eq!(
        status.running,
        Running::No {
            state: Some("failed".into())
        }
    );

    runner.push_spawn_error("No such file or directory");
    let status = manager.status().await.unwrap();
    assert_eq!(status.running, Running::Unknown);
}

// ---- Windows (MemoryRunKey) ----

#[tokio::test]
async fn windows_install_uninstall_status() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, runner, key) = manager(Os::Windows, tmp.path());

    let status = manager.status().await.unwrap();
    assert!(!status.installed);
    assert_eq!(status.running, Running::Unknown);

    let installed = manager.install().await.unwrap();
    assert!(!installed.started);
    assert_eq!(installed.path, None);
    assert_eq!(
        key.value().as_deref(),
        Some(r#""C:\Program Files\kakemono\kakemono.exe" daemon"#)
    );
    assert!(manager.status().await.unwrap().installed);

    assert!(manager.uninstall().await.unwrap().removed);
    assert_eq!(key.value(), None);
    assert!(!manager.uninstall().await.unwrap().removed);
    assert!(!key.is_enabled().unwrap());

    assert!(runner.argvs().is_empty(), "Windows runs no commands");
}

#[tokio::test]
async fn no_run_key_off_windows_errors() {
    let key = windows::NoRunKey;
    assert!(key.is_enabled().is_err());
}
