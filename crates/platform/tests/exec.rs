//! `exec` tests: argv templating survives hostile paths, timeouts kill, stderr is captured.
//! Process tests use `tests/bin/record-argv` and `sh`, so they are Unix-only.

use std::path::Path;
use std::time::Duration;

use kakemono_platform::exec::{
    CommandRunner, Invocation, Output, RecordingRunner, RenderError, Vars, validate,
};
use kakemono_platform::{CommandStatus, Fit, Theme};

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|t| (*t).to_owned()).collect()
}

#[test]
fn exec_validate_accepts_known_and_literal_braces() {
    validate(&s(&["feh", "--bg-{fit}", "{path}", "{uri}", "{theme}"])).unwrap();
    // `{}` and `{print $1}` are not placeholders.
    validate(&s(&["awk", "{print $1}", "{}"])).unwrap();
}

#[test]
fn exec_validate_rejects_unknown_and_empty() {
    assert_eq!(
        validate(&s(&["feh", "--x={file}"])),
        Err(RenderError::UnknownPlaceholder {
            token: "--x={file}".into(),
            name: "file".into()
        })
    );
    assert_eq!(validate(&[]), Err(RenderError::EmptyCommand));
    let msg = validate(&s(&["x", "{PATH}"])).unwrap_err().to_string();
    assert!(msg.contains("{PATH}"), "{msg}");
}

#[test]
fn exec_vars_require_absolute_paths() {
    assert_eq!(
        Vars::new(Path::new("-rf.jpg"), Fit::Fill, Theme::Dark),
        Err(RenderError::RelativePath("-rf.jpg".into()))
    );
}

#[cfg(unix)]
#[test]
fn exec_render_substitutes_per_token() {
    use kakemono_platform::exec::render;

    let vars = Vars::new(Path::new("/w/a b#1.jpg"), Fit::Center, Theme::Dark).unwrap();
    assert_eq!(vars.uri(), "file:///w/a%20b%231.jpg");
    let argv = render(
        &s(&[
            "set",
            "{path}",
            "--uri={uri}",
            "{fit}-{theme}",
            "{path}{path}",
            "{nope}",
            "$HOME",
        ]),
        &vars,
    );
    assert_eq!(
        argv,
        s(&[
            "set",
            "/w/a b#1.jpg",
            "--uri=file:///w/a%20b%231.jpg",
            "center-dark",
            "/w/a b#1.jpg/w/a b#1.jpg",
            "{nope}",
            "$HOME",
        ])
    );
}

#[cfg(unix)]
mod process {
    use super::*;
    use kakemono_platform::SetError;
    use kakemono_platform::exec::{TokioRunner, render, run_checked};
    use std::path::PathBuf;
    use std::time::Instant;

    fn record_argv() -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/bin/record-argv")
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn read_recorded(path: &Path) -> Vec<String> {
        let bytes = std::fs::read(path).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let mut args: Vec<String> = text.split('\0').map(str::to_owned).collect();
        assert_eq!(args.pop().as_deref(), Some(""), "NUL-terminated");
        args
    }

    #[tokio::test]
    async fn exec_hostile_paths_stay_one_argv_element() {
        let dir = tempfile::tempdir().unwrap();
        let canary = dir.path().join("pwned");
        let nasty_dir = dir.path().join("-rf dir");
        std::fs::create_dir(&nasty_dir).unwrap();
        let names = [
            "with space.jpg".to_owned(),
            "it's \"quoted\".jpg".to_owned(),
            format!("$(touch {}).jpg", canary.display()).replace('/', "_"),
            "a; touch pwned; b.jpg".to_owned(),
            "`id`.jpg".to_owned(),
            "-leading-dash.jpg".to_owned(),
            "*glob?.jpg".to_owned(),
            "\nnewline.jpg".to_owned(),
        ];
        let out = dir.path().join("argv.bin");
        for name in names {
            let image: PathBuf = nasty_dir.join(&name);
            let vars = Vars::new(&image, Fit::Fill, Theme::Light).unwrap();
            let spec = s(&[
                &record_argv(),
                "--bg-{fit}",
                "{path}",
                "file={uri}",
                "-{path}",
            ]);
            validate(&spec).unwrap();
            let argv = render(&spec, &vars);
            let inv = Invocation::new(argv)
                .env("RECORD_ARGV_OUT", out.to_str().unwrap())
                .timeout(Duration::from_secs(10));
            run_checked(&TokioRunner, &inv).await.unwrap();

            let recorded = read_recorded(&out);
            let path = image.to_str().unwrap();
            assert_eq!(
                recorded,
                vec![
                    "--bg-fill".to_owned(),
                    path.to_owned(),
                    format!("file={}", vars.uri()),
                    format!("-{path}"),
                ],
                "name {name:?}"
            );
        }
        assert!(!canary.exists(), "a path was evaluated by a shell");
        assert!(!dir.path().join("pwned").exists());
        assert!(!nasty_dir.join("pwned").exists());
    }

    #[tokio::test]
    async fn exec_timeout_kills_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!("echo $$ > '{}'; exec sleep 30", pidfile.display());
        let inv = Invocation::new(s(&["sh", "-c", &script])).timeout(Duration::from_millis(300));

        let started = Instant::now();
        let output = TokioRunner.run(&inv).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            output.status,
            CommandStatus::TimedOut(Duration::from_millis(300))
        );

        let pid = std::fs::read_to_string(&pidfile).unwrap();
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "pid {} still running after timeout", pid.trim());

        let err = output.check("sh").unwrap_err();
        assert_eq!(err.to_string(), "sh: timed out after 0.3s");
    }

    #[tokio::test]
    async fn exec_stderr_is_captured_into_set_error() {
        let argv0 = record_argv();
        let inv = Invocation::new(vec![argv0.clone(), "x".into()])
            .env("RECORD_ARGV_STDERR", "  swww: no daemon running  ")
            .env("RECORD_ARGV_EXIT", "3");
        let err = run_checked(&TokioRunner, &inv).await.unwrap_err();
        match &err {
            SetError::Command {
                argv0: got,
                status,
                stderr,
            } => {
                assert_eq!(got, &argv0);
                assert_eq!(*status, CommandStatus::Exited(3));
                assert_eq!(stderr, "swww: no daemon running");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            err.to_string(),
            format!("{argv0}: exit code 3: swww: no daemon running")
        );
    }

    #[tokio::test]
    async fn exec_success_captures_stdout_and_stderr() {
        let inv = Invocation::new(s(&["sh", "-c", "echo out; echo err >&2"]));
        let output = run_checked(&TokioRunner, &inv).await.unwrap();
        assert_eq!(output.stdout, "out\n");
        assert_eq!(output.stderr, "err\n");
        assert!(output.status.success());
    }

    #[tokio::test]
    async fn exec_missing_binary_fails_to_start() {
        let inv = Invocation::new(s(&["/nonexistent/kakemono-setter", "x"]));
        let err = run_checked(&TokioRunner, &inv).await.unwrap_err();
        assert!(
            matches!(
                err,
                SetError::Command {
                    status: CommandStatus::FailedToStart,
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .starts_with("/nonexistent/kakemono-setter: failed to start: "),
            "{err}"
        );
    }

    #[tokio::test]
    async fn exec_empty_argv_is_unconfigured() {
        let err = run_checked(&TokioRunner, &Invocation::new(Vec::new()))
            .await
            .unwrap_err();
        assert!(matches!(err, SetError::Unconfigured { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn exec_clear_env_passes_only_given_vars() {
        let inv = Invocation::new(s(&["/usr/bin/env"]))
            .clear_env()
            .envs([("KAKEMONO_TEST", "1")]);
        let output = run_checked(&TokioRunner, &inv).await.unwrap();
        assert_eq!(output.stdout, "KAKEMONO_TEST=1\n");
    }

    #[tokio::test]
    async fn exec_large_output_is_capped_without_deadlock() {
        let inv = Invocation::new(s(&["sh", "-c", "yes | head -c 3000000"]));
        let output = run_checked(&TokioRunner, &inv).await.unwrap();
        assert_eq!(output.stdout.len(), 1024 * 1024);
    }

    #[tokio::test]
    async fn exec_background_grandchild_does_not_block() {
        // The grandchild keeps stdout open; `run` must still return soon after `sh` exits.
        let inv = Invocation::new(s(&["sh", "-c", "sleep 5 & echo hi"]));
        let started = Instant::now();
        let output = run_checked(&TokioRunner, &inv).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(output.stdout, "hi\n");
    }

    #[tokio::test]
    async fn exec_spawn_detached_returns_pid() {
        let pid = TokioRunner
            .spawn_detached(&s(&["sh", "-c", "exit 0"]))
            .unwrap();
        assert!(pid > 0);
        assert!(TokioRunner.spawn_detached(&[]).is_err());
        assert!(
            TokioRunner
                .spawn_detached(&s(&["/nonexistent/kakemono-bg"]))
                .is_err()
        );
    }
}

#[tokio::test(start_paused = true)]
async fn exec_recording_runner_times_out_on_paused_clock() {
    let runner = RecordingRunner::new();
    runner.set_delay(Duration::from_secs(60));
    let inv = Invocation::new(s(&["swww", "img", "/a.jpg"])).timeout(Duration::from_secs(15));
    let started = tokio::time::Instant::now();
    let output = runner.run(&inv).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(15));
    assert_eq!(
        output.status,
        CommandStatus::TimedOut(Duration::from_secs(15))
    );
    assert_eq!(runner.argvs(), vec![s(&["swww", "img", "/a.jpg"])]);
    assert_eq!(runner.in_flight(), 0);
}

#[tokio::test]
async fn exec_recording_runner_tracks_concurrency() {
    let runner = RecordingRunner::new();
    runner.set_delay(Duration::from_millis(50));
    runner.push_output(Output::failure(1, "x"));
    let inv = Invocation::new(s(&["a"]));
    let (a, b) = tokio::join!(runner.run(&inv), runner.run(&inv));
    assert_eq!(a.unwrap().status, CommandStatus::Exited(1));
    assert!(b.unwrap().status.success());
    assert_eq!(runner.max_in_flight(), 2);
    assert_eq!(runner.in_flight(), 0);
}
