//! Tauri commands for the settings window's Desktop panel (plan Task 42).
//!
//! - `desktop_backend` → the daemon's `doctor` reply: the backend it is using right now.
//! - `test_desktop` → IPC `reload`, then `doctor` with `apply`: the daemon re-sets the current
//!   wallpaper with its own setter. A failed re-set is not an error here; it comes back in
//!   [`DoctorReport::apply`] with the daemon's error text.
//! - `detect_desktop { write }` → runs `kakemono detect` (or `detect --write`) and returns its
//!   report text. The CLI is the only place detection runs; the UI depends on neither the
//!   platform crate nor its detection code.
//!
//! Pinning (`managed_by = "user"`) needs no command here: the panel sends a `save_config` patch,
//! which goes through the comment-preserving writer.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use kakemono_protocol::types::Request;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::CommandError;
use crate::daemon_link::{DaemonLink, LinkError};
use crate::lifecycle::{self, CliCall, CliRunner, LifecycleError, ProcessRunner};

/// The backend the daemon reports (`doctor` data `backend`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendReport {
    /// Short id, e.g. `macos`, `gnome`, `awww`, `command`, `unconfigured`.
    pub name: String,
    /// Human-readable detail. For command backends: the argv joined for display, with
    /// ` (kept running)` appended for persistent setters. For `unconfigured`: the hint.
    pub detail: String,
}

/// The outcome of `doctor` with `apply` (`doctor` data `apply`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApplyReport {
    pub ok: bool,
    /// The image the daemon re-set.
    pub path: Option<String>,
    /// Why the re-set failed, as the daemon worded it.
    pub error: Option<String>,
}

/// The parts of the daemon's `doctor` data the Desktop panel shows. Other fields (`env`) are
/// ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DoctorReport {
    pub version: String,
    pub pid: u32,
    pub backend: BackendReport,
    pub theme: String,
    /// Present only after `test_desktop`.
    pub apply: Option<ApplyReport>,
}

type CmdResult<T> = Result<T, CommandError>;

/// IPC `doctor`, decoded into a [`DoctorReport`].
pub async fn doctor_report(link: &DaemonLink, apply: bool) -> CmdResult<DoctorReport> {
    let response = link.call(Request::Doctor { apply }).await?;
    let report = response
        .data_as::<DoctorReport>()
        .map_err(|source| LinkError::Decode {
            what: "doctor",
            source,
        })?;
    Ok(report.unwrap_or_default())
}

/// IPC `reload` (so the daemon reads the saved `[desktop]`), then `doctor` with `apply`. A failed
/// reload stops here with the daemon's error; a failed re-set is in the report's `apply`.
pub async fn test_apply(link: &DaemonLink) -> CmdResult<DoctorReport> {
    link.call(Request::Reload).await?;
    doctor_report(link, true).await
}

/// `desktop_backend` → the daemon's [`DoctorReport`] (no `apply`).
#[tauri::command]
pub async fn desktop_backend(link: State<'_, Arc<DaemonLink>>) -> CmdResult<DoctorReport> {
    doctor_report(&link, false).await
}

/// `test_desktop` → [`test_apply`].
#[tauri::command]
pub async fn test_desktop(link: State<'_, Arc<DaemonLink>>) -> CmdResult<DoctorReport> {
    test_apply(&link).await
}

/// `kakemono detect` (`write`: `detect --write`). Returns the CLI's report text. A non-zero exit
/// (for example `--write` on a pinned or legacy config) fails with its stderr.
pub async fn detect(
    runner: &impl CliRunner,
    cli: Option<&Path>,
    write: bool,
) -> Result<String, LifecycleError> {
    let cli = cli.ok_or(LifecycleError::CliNotFound)?;
    let mut args: Vec<OsString> = vec!["detect".into()];
    if write {
        args.push("--write".into());
    }
    let action = if write { "detect --write" } else { "detect" };
    let call = CliCall {
        program: cli.to_owned(),
        args,
    };
    let output = runner
        .run(call)
        .await
        .map_err(|source| LifecycleError::Io {
            action: action.to_owned(),
            source,
        })?;
    if !output.success {
        let message = match output.stderr.trim() {
            "" => "it exited with an error and printed nothing".to_owned(),
            text => text.to_owned(),
        };
        return Err(LifecycleError::CliFailed {
            action: action.to_owned(),
            message,
        });
    }
    Ok(output.stdout)
}

/// `detect_desktop { write }` → [`detect`] with the real CLI.
#[tauri::command]
pub async fn detect_desktop(write: bool) -> Result<String, LifecycleError> {
    let cli = lifecycle::find_cli();
    detect(&ProcessRunner, cli.as_deref(), write).await
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::sync::Mutex;

    use super::*;
    use crate::lifecycle::CliOutput;

    /// Answers each call with the next scripted reply and records the argv.
    #[derive(Default)]
    struct FakeRunner {
        replies: Mutex<VecDeque<io::Result<CliOutput>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn with(reply: io::Result<CliOutput>) -> Self {
            Self {
                replies: Mutex::new(VecDeque::from([reply])),
                calls: Mutex::default(),
            }
        }
    }

    impl CliRunner for FakeRunner {
        async fn run(&self, call: CliCall) -> io::Result<CliOutput> {
            let mut argv = vec![call.program.display().to_string()];
            argv.extend(call.args.iter().map(|a| a.to_string_lossy().into_owned()));
            self.calls.lock().expect("calls").push(argv.join(" "));
            self.replies
                .lock()
                .expect("replies")
                .pop_front()
                .expect("an unexpected CLI call")
        }
    }

    fn out(success: bool, stdout: &str, stderr: &str) -> io::Result<CliOutput> {
        Ok(CliOutput {
            success,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        })
    }

    const CLI: &str = "/opt/k/kakemono";

    #[tokio::test]
    async fn detect_returns_the_report_text() {
        let report =
            "session:   hyprland:wayland\ndetected:  awww\nreasons:\n  - awww is running\n";
        let runner = FakeRunner::with(out(true, report, ""));
        let text = detect(&runner, Some(Path::new(CLI)), false)
            .await
            .expect("detect");
        assert_eq!(text, report);
        assert_eq!(*runner.calls.lock().unwrap(), vec![format!("{CLI} detect")]);
    }

    #[tokio::test]
    async fn detect_write_passes_the_flag_and_shows_stderr_on_refusal() {
        let refusal = "[desktop] is managed by you (managed_by = \"user\"); use --force";
        let runner = FakeRunner::with(out(false, "", &format!("{refusal}\n")));
        let err = detect(&runner, Some(Path::new(CLI)), true)
            .await
            .expect_err("refused");
        assert_eq!(err.kind(), "cli_failed");
        assert_eq!(
            err.to_string(),
            format!("`kakemono detect --write` failed: {refusal}")
        );
        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec![format!("{CLI} detect --write")]
        );
    }

    #[tokio::test]
    async fn detect_without_a_cli_runs_nothing() {
        let runner = FakeRunner::default();
        let err = detect(&runner, None, false).await.expect_err("no cli");
        assert_eq!(err.kind(), "cli_not_found");
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn doctor_data_decodes_and_ignores_env() {
        let data = r#"{"version":"2.0.0","pid":42,"backend":{"name":"command","detail":"setbg {path}"},
            "theme":"dark","env":{"session_type":"wayland"},
            "apply":{"ok":false,"error":"setbg exited with status 1: no such file"}}"#;
        let report: DoctorReport = serde_json::from_str(data).expect("decode");
        assert_eq!(report.backend.name, "command");
        assert_eq!(report.backend.detail, "setbg {path}");
        assert_eq!(
            report.apply,
            Some(ApplyReport {
                ok: false,
                path: None,
                error: Some("setbg exited with status 1: no such file".into()),
            })
        );
        let json = serde_json::to_value(&report).expect("encode");
        assert_eq!(
            json["apply"]["error"],
            "setbg exited with status 1: no such file"
        );
        assert!(json.get("env").is_none());
    }
}

/// `test_apply` against a scripted daemon on a temp socket (no real daemon).
#[cfg(all(test, unix))]
mod socket_tests {
    use interprocess::local_socket::tokio::prelude::*;
    use kakemono_protocol::endpoint::Endpoint;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::*;

    /// Serves one connection: answers each request line with the next reply. Returns the
    /// request lines it read.
    fn serve(
        listener: LocalSocketListener,
        replies: Vec<&'static str>,
    ) -> tokio::task::JoinHandle<Vec<String>> {
        tokio::spawn(async move {
            let conn = listener.accept().await.expect("accept");
            let mut reader = BufReader::new(conn);
            let mut seen = Vec::new();
            for reply in replies {
                let mut line = String::new();
                if reader.read_line(&mut line).await.expect("read") == 0 {
                    break;
                }
                seen.push(line.trim_end().to_owned());
                reader
                    .get_mut()
                    .write_all(reply.as_bytes())
                    .await
                    .expect("write");
            }
            seen
        })
    }

    async fn daemon(
        replies: Vec<&'static str>,
    ) -> (
        tempfile::TempDir,
        DaemonLink,
        tokio::task::JoinHandle<Vec<String>>,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoint = Endpoint::new(dir.path().join("run/k.sock")).expect("endpoint");
        let listener = endpoint.bind().expect("bind");
        let server = serve(listener, replies);
        (dir, DaemonLink::new(endpoint), server)
    }

    #[tokio::test]
    async fn test_reloads_then_applies_and_returns_the_daemon_error_text() {
        let (_dir, link, server) = daemon(vec![
            "{\"ok\":true}\n",
            "{\"ok\":true,\"data\":{\"version\":\"2\",\"pid\":7,\"backend\":{\"name\":\"command\",\"detail\":\"setbg {path}\"},\"theme\":\"light\",\"apply\":{\"ok\":false,\"error\":\"setbg: exit status 2\"}}}\n",
        ])
        .await;
        let report = test_apply(&link).await.expect("test");
        let apply = report.apply.expect("apply");
        assert!(!apply.ok);
        assert_eq!(apply.error.as_deref(), Some("setbg: exit status 2"));
        let seen = server.await.expect("server");
        assert_eq!(seen.len(), 2);
        assert!(seen[0].contains("\"reload\""), "{seen:?}");
        assert!(
            seen[1].contains("\"doctor\"") && seen[1].contains("\"apply\":true"),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_reload_stops_before_apply() {
        let (_dir, link, server) =
            daemon(vec!["{\"ok\":false,\"error\":\"reload: bad config\"}\n"]).await;
        let err = test_apply(&link).await.expect_err("reload fails");
        assert_eq!(err.kind(), "daemon");
        assert_eq!(err.to_string(), "reload: bad config");
        assert_eq!(server.await.expect("server").len(), 1);
    }

    #[tokio::test]
    async fn desktop_backend_asks_doctor_without_apply() {
        let (_dir, link, server) = daemon(vec![
            "{\"ok\":true,\"data\":{\"version\":\"2\",\"pid\":7,\"backend\":{\"name\":\"gnome\",\"detail\":\"gsettings set org.gnome.desktop.background\"},\"theme\":\"dark\"}}\n",
        ])
        .await;
        let report = doctor_report(&link, false).await.expect("doctor");
        assert_eq!(report.backend.name, "gnome");
        assert_eq!(report.apply, None);
        let seen = server.await.expect("server");
        assert!(
            seen[0].contains("\"doctor\"") && !seen[0].contains("\"apply\":true"),
            "{seen:?}"
        );
    }
}
