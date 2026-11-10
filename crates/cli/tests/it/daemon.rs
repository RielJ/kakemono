//! `kakemono daemon` wiring (plan Task 36): the real binary on the fake platform
//! (`KAKEMONO_FAKE_PLATFORM=1`) with HOME, XDG dirs, `KAKEMONO_CONFIG` and the socket inside a
//! temp dir. Never touches the real desktop, config or socket.
#![cfg(unix)]

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use kakemono_cli::daemon::{FAKE_PLATFORM_ENV, PID_FILE_NAME};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(15);

struct Env {
    _dir: TempDir,
    root: PathBuf,
    socket: PathBuf,
}

impl Env {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("config.toml"), config).unwrap();
        let socket = root.join("k.sock");
        Self {
            _dir: dir,
            root,
            socket,
        }
    }

    /// No rotation, so nothing reaches the network.
    fn quiet() -> Self {
        Self::new("[daemon]\nrotation_enabled = false\n")
    }

    fn cmd(&self) -> Command {
        let root = &self.root;
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("kakemono"));
        cmd.current_dir(root)
            .env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join("xdg-config"))
            .env("XDG_RUNTIME_DIR", root.join("xdg-run"))
            .env("XDG_STATE_HOME", root.join("xdg-state"))
            .env("XDG_CACHE_HOME", root.join("xdg-cache"))
            .env("KAKEMONO_CONFIG", root.join("config.toml"))
            .env(FAKE_PLATFORM_ENV, "1")
            .env_remove("KAKEMONO_SOCKET")
            .env_remove("KAKEMONO_LOG")
            .stdin(Stdio::null());
        cmd
    }

    fn daemon_args(&self, cmd: &mut Command) {
        cmd.arg("daemon").arg("--socket").arg(&self.socket);
    }

    fn status_ok(&self) -> bool {
        self.cmd()
            .arg("--socket")
            .arg(&self.socket)
            .arg("status")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    fn wait_ready(&self, child: &mut Child) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.status_ok() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("daemon exited early: {status}");
            }
            assert!(Instant::now() < deadline, "daemon never answered");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Every file under the root whose name satisfies `pred`.
    fn find(&self, pred: impl Fn(&str) -> bool) -> Vec<PathBuf> {
        fn walk(dir: &Path, pred: &dyn Fn(&str) -> bool, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, pred, out);
                } else if path.file_name().and_then(|n| n.to_str()).is_some_and(pred) {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &pred, &mut out);
        out
    }

    fn wait_gone(&self, path: &Path) {
        let deadline = Instant::now() + TIMEOUT;
        while path.exists() {
            assert!(
                Instant::now() < deadline,
                "{} never removed",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn wait_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("daemon did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn signal(pid: u32, sig: Signal) {
    kill(Pid::from_raw(i32::try_from(pid).unwrap()), sig).unwrap();
}

fn stderr_of(child: &mut Child) -> String {
    let mut s = String::new();
    child.stderr.take().unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn foreground_serves_until_sigterm_then_cleans_up() {
    let env = Env::quiet();
    let mut cmd = env.cmd();
    env.daemon_args(&mut cmd);
    let mut child = cmd
        .arg("--foreground")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    env.wait_ready(&mut child);

    let pid_files = env.find(|n| n == PID_FILE_NAME);
    assert_eq!(pid_files.len(), 1, "{pid_files:?}");
    let pid_file = pid_files[0].clone();
    assert_eq!(
        std::fs::read_to_string(&pid_file).unwrap(),
        child.id().to_string()
    );

    // A second daemon is refused by the lock and leaves the first one alone.
    let mut second = env.cmd();
    env.daemon_args(&mut second);
    let out = second.arg("-f").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("daemon is already running (pid {}", child.id())),
        "{err}"
    );
    assert!(env.status_ok(), "first daemon still serves");

    signal(child.id(), Signal::SIGTERM);
    let status = wait_exit(&mut child);
    let log = stderr_of(&mut child);
    assert!(status.success(), "{status}\n{log}");
    assert!(log.contains("daemon started"), "{log}");
    assert!(log.contains("daemon shutting down (SIGTERM)"), "{log}");
    assert!(log.contains("daemon stopped"), "{log}");
    assert!(log.contains("fake platform"), "{log}");
    assert!(!env.socket.exists(), "socket removed");
    assert!(!pid_file.exists(), "PID file removed");
    assert_eq!(
        env.find(|n| n == "state.json").len(),
        1,
        "state saved on shutdown"
    );
}

#[test]
fn foreground_stops_on_sigint() {
    let env = Env::quiet();
    let mut cmd = env.cmd();
    env.daemon_args(&mut cmd);
    let mut child = cmd
        .arg("-f")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    env.wait_ready(&mut child);
    signal(child.id(), Signal::SIGINT);
    let status = wait_exit(&mut child);
    let log = stderr_of(&mut child);
    assert!(status.success(), "{status}\n{log}");
    assert!(log.contains("daemon shutting down (SIGINT)"), "{log}");
}

#[test]
fn foreground_creates_a_missing_config() {
    let env = Env::quiet();
    std::fs::remove_file(env.root.join("config.toml")).unwrap();
    let mut cmd = env.cmd();
    env.daemon_args(&mut cmd);
    // The template enables rotation, but with no state the first tick is a full interval (30m)
    // away, so nothing reaches the network before SIGTERM.
    let mut child = cmd
        .arg("-f")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    env.wait_ready(&mut child);
    signal(child.id(), Signal::SIGTERM);
    wait_exit(&mut child);
    let log = stderr_of(&mut child);
    let config = env.root.join("config.toml");
    assert!(config.exists(), "{log}");
    assert!(
        log.contains(&format!("Created default config at {}", config.display())),
        "{log}"
    );
}

#[test]
fn foreground_rejects_a_broken_config() {
    let env = Env::new("[daemon\n");
    let mut cmd = env.cmd();
    env.daemon_args(&mut cmd);
    let out = cmd.arg("-f").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("loading config"), "{err}");
    assert!(env.find(|n| n == PID_FILE_NAME).is_empty());
}

#[test]
fn background_detaches_logs_to_file_and_stops_on_sigterm() {
    let env = Env::quiet();
    let mut cmd = env.cmd();
    env.daemon_args(&mut cmd);
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pid: u32 = stdout
        .strip_prefix("kakemono daemon started (pid ")
        .and_then(|rest| rest.split(')').next())
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(
        stdout.ends_with(&format!("Socket: {}\n", env.socket.display())),
        "{stdout}"
    );

    // The parent has exited; the daemon still answers.
    assert!(env.status_ok());
    let pid_file = env.find(|n| n == PID_FILE_NAME);
    assert_eq!(pid_file.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&pid_file[0]).unwrap(),
        pid.to_string()
    );

    // Already running: refused before spawning anything.
    let mut again = env.cmd();
    env.daemon_args(&mut again);
    let out = again.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("daemon is already running"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    signal(pid, Signal::SIGTERM);
    env.wait_gone(&pid_file[0]);
    env.wait_gone(&env.socket);

    let logs = env.find(|n| n.starts_with("kakemono.") && n.ends_with(".log"));
    assert_eq!(logs.len(), 1, "{logs:?}");
    let log = std::fs::read_to_string(&logs[0]).unwrap();
    assert!(log.contains("daemon started"), "{log}");
    assert!(log.contains("daemon stopped"), "{log}");
}

#[test]
fn background_reports_a_daemon_that_fails_at_start_up() {
    let env = Env::new("[daemon\n");
    let mut cmd = env.cmd();
    env.daemon_args(&mut cmd);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("the daemon exited during start-up"), "{err}");
    let logs = env.find(|n| n.starts_with("kakemono.") && n.ends_with(".log"));
    assert_eq!(logs.len(), 1, "{logs:?}");
    let log = std::fs::read_to_string(&logs[0]).unwrap();
    assert!(log.contains("loading config"), "{log}");
}
