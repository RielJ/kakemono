//! Preset argv table and `CommandSetter`. `linux::backends` builds on every Unix,
//! so this runs on macOS and Linux CI. Process tests use `tests/bin/record-argv` and `sh`.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_config::schema::{CommandSpec, DesktopConfig};
use kakemono_platform::exec::{Output, RecordingRunner, TokioRunner};
use kakemono_platform::linux::backends::command::{CommandSetter, Template};
use kakemono_platform::linux::backends::presets::{PRESET_KINDS, argv, command_for};
use kakemono_platform::linux::detect::{BackendKind, Env, resolve};
use kakemono_platform::{CommandStatus, Fit, SetError, Theme, WallpaperSetter};

const FITS: [Fit; 5] = [Fit::Fill, Fit::Fit, Fit::Center, Fit::Stretch, Fit::Tile];

#[test]
fn presets_snapshot() {
    let table: BTreeMap<String, BTreeMap<&str, Vec<String>>> = PRESET_KINDS
        .iter()
        .map(|kind| {
            let per_fit = FITS
                .iter()
                .map(|fit| (fit.as_str(), argv(*kind, *fit).unwrap()))
                .collect();
            (kind.as_str().to_owned(), per_fit)
        })
        .collect();
    insta::assert_json_snapshot!("presets", table);
}

#[test]
fn presets_render_hostile_path_as_one_token() {
    let setter =
        CommandSetter::preset(BackendKind::Sway, Arc::new(RecordingRunner::new())).unwrap();
    let argv = setter
        .render(Path::new("/w/a b; rm -rf ~ $(x).jpg"), Fit::Center)
        .unwrap();
    assert_eq!(
        argv,
        [
            "swaymsg",
            "output",
            "*",
            "bg",
            "/w/a b; rm -rf ~ $(x).jpg",
            "center"
        ]
    );
    let err = setter.render(Path::new("-rf.jpg"), Fit::Fill).unwrap_err();
    assert!(err.to_string().contains("absolute"), "{err}");
}

#[tokio::test]
async fn presets_command_setter_runs_through_runner() {
    let runner = RecordingRunner::new();
    let setter = CommandSetter::preset(BackendKind::Feh, Arc::new(runner.clone()))
        .unwrap()
        .with_env("DISPLAY", ":7");
    setter.set(Path::new("/w/x.png"), Fit::Fit).await.unwrap();
    let calls = runner.invocations();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].argv, ["feh", "--bg-max", "/w/x.png"]);
    assert_eq!(calls[0].env, [("DISPLAY".to_owned(), ":7".to_owned())]);
    assert_eq!(setter.info().name, "feh");
    assert_eq!(setter.info().detail, "feh --bg-fill {path}");
}

#[tokio::test]
async fn presets_failure_becomes_set_error_command() {
    let runner = RecordingRunner::new();
    runner.push_output(Output::failure(3, "  awww: no daemon running \n"));
    let setter = CommandSetter::preset(BackendKind::Awww, Arc::new(runner)).unwrap();
    match setter.set(Path::new("/w/x.png"), Fit::Fill).await {
        Err(SetError::Command {
            argv0,
            status,
            stderr,
        }) => {
            assert_eq!(argv0, "awww");
            assert_eq!(status, CommandStatus::Exited(3));
            assert_eq!(stderr, "awww: no daemon running");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn presets_set_command_overrides_preset_with_theme() {
    let cfg = DesktopConfig {
        backend: kakemono_config::schema::Backend::Command,
        set_command: Some(
            CommandSpec::new(
                ["my-setter", "--theme={theme}", "{uri}", "{fit}"]
                    .map(str::to_owned)
                    .to_vec(),
            )
            .unwrap(),
        ),
        managed_by: kakemono_config::schema::ManagedBy::User,
        ..DesktopConfig::default()
    };
    let res = resolve(&Env::default(), &cfg);
    let runner = RecordingRunner::new();
    let setter = CommandSetter::for_resolution(&res, Arc::new(runner.clone()))
        .unwrap()
        .unwrap()
        .with_theme(Arc::new(|| Theme::Dark));
    setter
        .set(Path::new("/w/a b.jpg"), Fit::Tile)
        .await
        .unwrap();
    assert_eq!(
        runner.argvs(),
        [["my-setter", "--theme=dark", "file:///w/a%20b.jpg", "tile"]]
    );
    assert_eq!(
        command_for(&res, Fit::Fill).unwrap()[0],
        "my-setter",
        "command_for prefers set_command"
    );
}

#[test]
fn presets_for_resolution_none_for_api_and_unconfigured() {
    let gnome = resolve(
        &Env::default().with_var("XDG_CURRENT_DESKTOP", "GNOME"),
        &DesktopConfig::default(),
    );
    assert_eq!(gnome.backend, BackendKind::Gnome);
    assert!(Template::for_resolution(&gnome).is_none());
    assert!(command_for(&gnome, Fit::Fill).is_none());
    let none = resolve(&Env::default(), &DesktopConfig::default());
    assert!(
        CommandSetter::for_resolution(&none, Arc::new(RecordingRunner::new()))
            .unwrap()
            .is_none()
    );
    assert!(CommandSetter::preset(BackendKind::Kde, Arc::new(RecordingRunner::new())).is_err());
}

fn record_argv() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bin/record-argv")
}

fn read_nul_args(path: &Path) -> Vec<String> {
    std::fs::read(path)
        .unwrap()
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8(s.to_vec()).unwrap())
        .collect()
}

/// `record-argv` symlinked as `awww` on a private `PATH`: the real runner resolves the preset's
/// program through `PATH` and passes the hostile path as one argument.
#[tokio::test]
async fn presets_awww_on_path_integration() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink(record_argv(), bin.join("awww")).unwrap();
    let out = dir.path().join("argv.out");

    let res = resolve(
        &Env::default()
            .with_var("NIRI_SOCKET", "/tmp/niri.sock")
            .with_var("WAYLAND_DISPLAY", "wayland-1")
            .with_binaries(&["awww"]),
        &DesktopConfig::default(),
    );
    assert_eq!(res.backend, BackendKind::Awww);
    // `sh` (the script's interpreter) must still be found, so keep /bin and /usr/bin.
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let setter = CommandSetter::for_resolution(&res, Arc::new(TokioRunner))
        .unwrap()
        .unwrap()
        .with_env("PATH", path)
        .with_env("RECORD_ARGV_OUT", out.to_str().unwrap());

    let image = dir.path().join("it's a \"wall\" $(paper).jpg");
    setter.set(&image, Fit::Fill).await.unwrap();
    assert_eq!(
        read_nul_args(&out),
        [
            "img",
            image.to_str().unwrap(),
            "--resize",
            "crop",
            "--transition-type",
            "fade"
        ]
    );

    // A failing run surfaces exit code and stderr.
    let failing = setter
        .clone()
        .with_env("RECORD_ARGV_EXIT", "4")
        .with_env("RECORD_ARGV_STDERR", "awww: daemon not running");
    match failing.set(&image, Fit::Fill).await {
        Err(SetError::Command {
            argv0,
            status,
            stderr,
        }) => {
            assert_eq!(argv0, "awww");
            assert_eq!(status, CommandStatus::Exited(4));
            assert_eq!(stderr, "awww: daemon not running");
        }
        other => panic!("{other:?}"),
    }
}
