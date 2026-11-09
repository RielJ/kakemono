//! A reader that closes stdout early (`kakemono doctor | head`) must not produce a
//! "Broken pipe" error. The binary exits quietly with `STDOUT_CLOSED_EXIT` (141). Other errors
//! still print and exit 1.
//!
//! Each test starts the binary with stdout on a pipe whose read end is already closed, so the
//! first write fails with EPIPE. Rust ignores SIGPIPE, so the write returns an error instead of
//! killing the process.

#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output, Stdio};

use kakemono_cli::cli::STDOUT_CLOSED_EXIT;
use tempfile::TempDir;

/// Runs `kakemono <args>` in an isolated HOME with the read end of stdout closed before the
/// child starts.
fn run_with_closed_stdout(root: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kakemono"))
        .args(args)
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .env("XDG_CACHE_HOME", root.join("xdg-cache"))
        .env("KAKEMONO_CONFIG", root.join("config.toml"))
        .env_remove("KAKEMONO_SOCKET")
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kakemono");
    // Close the only read end: every write to the child's stdout now fails with EPIPE.
    drop(child.stdout.take());
    child.wait_with_output().expect("wait for kakemono")
}

fn assert_quiet_exit(args: &[&str]) {
    let dir = TempDir::new().unwrap();
    let out = run_with_closed_stdout(dir.path(), args);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(i32::from(STDOUT_CLOSED_EXIT)),
        "kakemono {args:?}: status {:?}, stderr: {stderr}",
        out.status
    );
    assert!(
        stderr.is_empty(),
        "kakemono {args:?} wrote to stderr: {stderr}"
    );
}

#[test]
fn doctor_exits_quietly_when_stdout_is_closed() {
    assert_quiet_exit(&["doctor"]);
}

#[test]
fn config_path_exits_quietly_when_stdout_is_closed() {
    assert_quiet_exit(&["config", "path"]);
}

#[test]
fn completions_exit_quietly_when_stdout_is_closed() {
    // clap_complete panics on a failed write; the script is rendered to memory first.
    assert_quiet_exit(&["completions", "zsh"]);
}

#[test]
fn other_errors_still_print_when_stdout_is_closed() {
    // No daemon is running, so `status` fails before it writes anything. That error must not
    // be hidden as a closed stdout.
    let dir = TempDir::new().unwrap();
    let out = run_with_closed_stdout(dir.path(), &["status"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(!stderr.is_empty(), "the daemon error was not printed");
    assert!(!stderr.contains("Broken pipe"), "stderr: {stderr}");
}

/// A write error other than EPIPE is still reported. Linux only: `/dev/full` fails every write
/// with ENOSPC.
#[cfg(target_os = "linux")]
#[test]
fn a_full_disk_is_still_reported() {
    let dir = TempDir::new().unwrap();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kakemono"))
        .args(["config", "path"])
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join("xdg-config"))
        .env("KAKEMONO_CONFIG", dir.path().join("config.toml"))
        .stdout(full)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.contains("No space left"), "stderr: {stderr}");
}
