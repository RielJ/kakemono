//! `service install|uninstall|status [--json]`: the login service that starts the daemon
//! (plan Task 39; backends in `kakemono_platform::service`).
//!
//! Exit codes: 0 on success (`uninstall` also when nothing is installed), 1 on failure with the
//! message on stderr. `status --json` prints exactly
//! `{"installed":bool,"running":bool,"path":string|null}`.

use std::io::Write;

use kakemono_platform::service::{
    Installed, Os, Running, ServiceManager, ServiceStatus, Uninstalled, launchd, systemd,
};
use serde::Serialize;

use crate::cli::{Context, ServiceAction};
use crate::commands;

pub fn run(ctx: &Context, out: &mut dyn Write, action: ServiceAction) -> anyhow::Result<()> {
    let manager = ServiceManager::system()?;
    let runtime = crate::cli::runtime()?;
    let probe = async || ctx.client().await.is_ok();
    let next = runtime.block_on(run_with(&manager, out, action, probe))?;
    // `daemon` builds its own runtime, so it runs after ours is done.
    drop(runtime);
    if next == Next::StartDaemon
        && let Err(e) = commands::daemon::run(ctx, out, false, false)
    {
        eprintln!(
            "warning: the login entry is installed, but starting the daemon now failed: {e:#}"
        );
    }
    Ok(())
}

/// What `run` does after the service operation.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Done,
    /// The service manager didn't start the daemon (Windows) and none answers: start one.
    StartDaemon,
}

/// The testable part. `daemon_answers` probes the daemon's socket.
pub async fn run_with(
    manager: &ServiceManager,
    out: &mut dyn Write,
    action: ServiceAction,
    daemon_answers: impl AsyncFn() -> bool,
) -> anyhow::Result<Next> {
    match action {
        ServiceAction::Install => install(manager, out, daemon_answers).await,
        ServiceAction::Uninstall => {
            let uninstalled = manager.uninstall().await?;
            write_uninstalled(out, manager.env().os, &uninstalled)?;
            Ok(Next::Done)
        }
        ServiceAction::Status { json } => {
            let status = manager.status().await?;
            let running = match &status.running {
                Running::Yes { .. } => true,
                Running::No { .. } => false,
                Running::Unknown => daemon_answers().await,
            };
            if json {
                write_status_json(out, &status, running)?;
            } else {
                write_status(out, &status, running)?;
            }
            Ok(Next::Done)
        }
    }
}

async fn install(
    manager: &ServiceManager,
    out: &mut dyn Write,
    daemon_answers: impl AsyncFn() -> bool,
) -> anyhow::Result<Next> {
    let os = manager.env().os;
    // A daemon the service didn't start holds the lock, so the service's daemon would exit and
    // be restarted until that one stops. Re-installing over a running service is fine: the
    // reload stops the old one first.
    let before = manager.status().await?;
    let foreign_daemon = !matches!(before.running, Running::Yes { .. }) && daemon_answers().await;

    let installed = manager.install().await?;
    write_installed(out, manager, &installed)?;
    if foreign_daemon {
        if installed.started {
            writeln!(
                out,
                "Note: a daemon started outside the service is already running. The service's \
                 daemon exits (and is retried) until that one stops; stop it or log out and back in."
            )?;
        }
        return Ok(Next::Done);
    }
    Ok(if !installed.started && os == Os::Windows {
        Next::StartDaemon
    } else {
        Next::Done
    })
}

fn write_installed(
    out: &mut dyn Write,
    manager: &ServiceManager,
    installed: &Installed,
) -> anyhow::Result<()> {
    let env = manager.env();
    match env.os {
        Os::MacOs => {
            writeln!(out, "Installed the login service: {}", installed.location)?;
            writeln!(out, "Runs: {}", installed.command)?;
            writeln!(
                out,
                "Started it with launchd (label {}, log: {}).",
                launchd::LABEL,
                launchd::log_path(&env.log_dir).display()
            )?;
        }
        Os::Linux => {
            writeln!(out, "Installed the login service: {}", installed.location)?;
            writeln!(out, "Runs: {}", installed.command)?;
            writeln!(
                out,
                "Enabled and started it (log: journalctl --user -u {}).",
                systemd::UNIT_NAME
            )?;
            writeln!(out)?;
            write!(out, "{}", systemd::compositor_snippets(&env.exe))?;
        }
        Os::Windows => {
            writeln!(out, "Installed the login entry: {}", installed.location)?;
            writeln!(out, "Runs at login: {}", installed.command)?;
        }
    }
    Ok(())
}

fn write_uninstalled(out: &mut dyn Write, os: Os, uninstalled: &Uninstalled) -> anyhow::Result<()> {
    if !uninstalled.removed {
        writeln!(out, "The login service is not installed.")?;
        return Ok(());
    }
    writeln!(out, "Removed the login service: {}", uninstalled.location)?;
    if os == Os::Windows {
        writeln!(
            out,
            "A daemon that is running now keeps running until you log out."
        )?;
    }
    Ok(())
}

/// The `status --json` shape (contract: exactly these three keys).
#[derive(Debug, Serialize)]
struct StatusJson<'a> {
    installed: bool,
    running: bool,
    path: Option<&'a str>,
}

fn write_status_json(
    out: &mut dyn Write,
    status: &ServiceStatus,
    running: bool,
) -> anyhow::Result<()> {
    // Contract: `path` is the installed definition file, so null when not installed.
    let path = status
        .path
        .as_deref()
        .filter(|_| status.installed)
        .map(|p| p.to_string_lossy());
    let json = StatusJson {
        installed: status.installed,
        running,
        path: path.as_deref(),
    };
    serde_json::to_writer(&mut *out, &json)?;
    writeln!(out)?;
    Ok(())
}

fn write_status(out: &mut dyn Write, status: &ServiceStatus, running: bool) -> anyhow::Result<()> {
    if !status.installed {
        writeln!(out, "Service: not installed")?;
        if running {
            writeln!(out, "State:   running (a daemon the service didn't start)")?;
        }
        writeln!(out, "Install: kakemono service install")?;
        return Ok(());
    }
    writeln!(out, "Service: installed ({})", status.location)?;
    let state = match &status.running {
        Running::Yes { pid: Some(pid) } => format!("running (pid {pid})"),
        Running::Yes { pid: None } => "running".to_owned(),
        Running::No { state: Some(state) } => format!("not running ({state})"),
        Running::No { state: None } => "not running".to_owned(),
        Running::Unknown if running => "running (the daemon answers)".to_owned(),
        Running::Unknown => "not running (the daemon doesn't answer)".to_owned(),
    };
    writeln!(out, "State:   {state}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use clap::Parser as _;
    use kakemono_platform::CommandStatus;
    use kakemono_platform::exec::{Output, RecordingRunner};
    use kakemono_platform::service::ServiceEnv;
    use kakemono_platform::service::windows::MemoryRunKey;

    use super::*;
    use crate::cli::{Cli, Commands};

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        manager: ServiceManager,
        runner: RecordingRunner,
        key: MemoryRunKey,
    }

    fn fixture(os: Os) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let runner = RecordingRunner::new();
        let key = MemoryRunKey::new();
        let env = ServiceEnv {
            os,
            home: root.join("home"),
            config_home: root.join("home/.config"),
            log_dir: root.join("logs"),
            exe: PathBuf::from("/usr/local/bin/kakemono"),
        };
        let manager =
            ServiceManager::with_parts(env, Arc::new(runner.clone()), Arc::new(key.clone()));
        Fixture {
            _tmp: tmp,
            root,
            manager,
            runner,
            key,
        }
    }

    fn out(code: i32, stdout: &str) -> Output {
        Output {
            status: CommandStatus::Exited(code),
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    async fn call(f: &Fixture, action: ServiceAction, answers: bool) -> (Next, String) {
        let mut buf = Vec::new();
        let next = run_with(&f.manager, &mut buf, action, async || answers)
            .await
            .unwrap();
        let text = String::from_utf8(buf)
            .unwrap()
            .replace(&*f.root.to_string_lossy(), "<tmp>");
        (next, text)
    }

    const STATUS_JSON: ServiceAction = ServiceAction::Status { json: true };
    const STATUS: ServiceAction = ServiceAction::Status { json: false };

    #[test]
    fn parses_status_json_flag() {
        let cli = Cli::try_parse_from(["kakemono", "service", "status", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Service {
                action: ServiceAction::Status { json: true }
            }
        ));
        let cli = Cli::try_parse_from(["kakemono", "service", "install"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Service {
                action: ServiceAction::Install
            }
        ));
    }

    #[tokio::test]
    async fn status_json_contract() {
        // Not installed, not running.
        let f = fixture(Os::Linux);
        f.runner.push_output(out(4, "inactive\n"));
        let (_, not_installed) = call(&f, STATUS_JSON, false).await;

        // Installed and running.
        call(&f, ServiceAction::Install, false).await;
        f.runner.push_output(out(0, "active\n"));
        let (_, running) = call(&f, STATUS_JSON, false).await;

        // Installed, stopped.
        f.runner.push_output(out(3, "failed\n"));
        let (_, stopped) = call(&f, STATUS_JSON, false).await;

        // Windows: no file; running from the socket probe.
        let w = fixture(Os::Windows);
        call(&w, ServiceAction::Install, true).await;
        let (_, windows) = call(&w, STATUS_JSON, true).await;

        let value: serde_json::Value = serde_json::from_str(running.trim()).unwrap();
        insta::assert_snapshot!(
            "service_status_json",
            [not_installed, running, stopped, windows].concat()
        );
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["installed", "path", "running"]);
    }

    #[tokio::test]
    async fn status_human() {
        let f = fixture(Os::MacOs);
        f.runner.push_output(out(0, "501\n"));
        f.runner.push_output(out(113, ""));
        let (_, not_installed) = call(&f, STATUS, false).await;

        let path = launchd::plist_path(&f.root.join("home"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "x").unwrap();
        f.runner.push_output(out(0, "501\n"));
        f.runner
            .push_output(out(0, "x = {\n\tstate = running\n\tpid = 77\n}\n"));
        let (_, running) = call(&f, STATUS, false).await;

        f.runner.push_output(out(0, "501\n"));
        f.runner.push_output(out(113, ""));
        let (_, not_loaded) = call(&f, STATUS, false).await;

        insta::assert_snapshot!(
            "service_status_human",
            [not_installed, running, not_loaded].join("--\n")
        );
    }

    #[tokio::test]
    async fn install_output_linux_and_windows() {
        let f = fixture(Os::Linux);
        f.runner.push_output(out(3, "inactive\n")); // status before install
        let (next, linux) = call(&f, ServiceAction::Install, false).await;
        assert_eq!(next, Next::Done);

        let w = fixture(Os::Windows);
        let (next, windows) = call(&w, ServiceAction::Install, false).await;
        assert_eq!(next, Next::StartDaemon);
        assert!(w.key.value().is_some());
        // A daemon already answers: don't start another.
        let (next, _) = call(&w, ServiceAction::Install, true).await;
        assert_eq!(next, Next::Done);

        insta::assert_snapshot!("service_install_output", [linux, windows].join("--\n"));
    }

    #[tokio::test]
    async fn install_warns_about_a_foreign_daemon() {
        let f = fixture(Os::Linux);
        f.runner.push_output(out(3, "inactive\n"));
        let (_, text) = call(&f, ServiceAction::Install, true).await;
        assert!(
            text.contains("a daemon started outside the service"),
            "{text}"
        );

        // Re-install over the service's own running daemon: no warning.
        f.runner.push_output(out(0, "active\n"));
        let (_, text) = call(&f, ServiceAction::Install, true).await;
        assert!(!text.contains("outside the service"), "{text}");
    }

    #[tokio::test]
    async fn uninstall_is_idempotent() {
        let f = fixture(Os::Linux);
        call(&f, ServiceAction::Install, false).await;
        let (_, first) = call(&f, ServiceAction::Uninstall, false).await;
        let (_, second) = call(&f, ServiceAction::Uninstall, false).await;
        assert_eq!(
            first,
            "Removed the login service: <tmp>/home/.config/systemd/user/kakemono.service\n"
        );
        assert_eq!(second, "The login service is not installed.\n");
    }

    #[tokio::test]
    async fn failures_are_errors() {
        let f = fixture(Os::Linux);
        f.runner.push_output(out(3, "inactive\n"));
        f.runner
            .push_output(Output::failure(1, "Failed to connect to bus"));
        let mut buf = Vec::new();
        let err = run_with(&f.manager, &mut buf, ServiceAction::Install, async || false)
            .await
            .unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "`systemctl --user daemon-reload` failed (exit code 1): Failed to connect to bus"
        );
    }
}
