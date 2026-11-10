//! `linux::backends::gsettings`: GNOME, Cinnamon and MATE through `gsettings`.
//! The module is plain argv code, so this runs on macOS and Linux CI. Process tests put a fake
//! `gsettings` (a `sh` script written by the test) first on a private `PATH`.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_platform::exec::{Output, RecordingRunner, TokioRunner};
use kakemono_platform::linux::backends::gsettings::{
    GSETTINGS_KINDS, GsettingsSetter, argvs, picture_options,
};
use kakemono_platform::linux::detect::BackendKind;
use kakemono_platform::{CommandStatus, Fit, SetError, WallpaperSetter};

const FITS: [Fit; 5] = [Fit::Fill, Fit::Fit, Fit::Center, Fit::Stretch, Fit::Tile];

#[test]
fn gsettings_argv_snapshot() {
    let image = Path::new("/home/u/Pictures/a b#1.jpg");
    let table: BTreeMap<&str, Vec<Vec<String>>> = GSETTINGS_KINDS
        .iter()
        .map(|kind| {
            (
                kind.as_str(),
                argvs(*kind, image, Fit::Fill).unwrap().unwrap(),
            )
        })
        .collect();
    insta::assert_json_snapshot!("gsettings_argvs", table);
}

#[test]
fn gsettings_picture_options_per_fit() {
    let options: Vec<&str> = FITS.iter().map(|fit| picture_options(*fit)).collect();
    assert_eq!(
        options,
        ["zoom", "scaled", "centered", "stretched", "wallpaper"]
    );
    let gnome = argvs(BackendKind::Gnome, Path::new("/w/x.png"), Fit::Tile)
        .unwrap()
        .unwrap();
    assert_eq!(
        gnome.last().unwrap(),
        &[
            "gsettings",
            "set",
            "org.gnome.desktop.background",
            "picture-options",
            "wallpaper"
        ]
    );
}

#[test]
fn gsettings_only_for_its_kinds_and_absolute_paths() {
    assert!(argvs(BackendKind::Kde, Path::new("/w/x.png"), Fit::Fill).is_none());
    assert!(GsettingsSetter::new(BackendKind::Sway, Arc::new(RecordingRunner::new())).is_none());
    let err = argvs(BackendKind::Mate, Path::new("x.png"), Fit::Fill)
        .unwrap()
        .unwrap_err();
    assert!(err.to_string().contains("absolute"), "{err}");
}

#[tokio::test]
async fn gsettings_setter_runs_every_step_in_order() {
    let runner = RecordingRunner::new();
    let setter = GsettingsSetter::new(BackendKind::Cinnamon, Arc::new(runner.clone()))
        .unwrap()
        .with_env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nowhere");
    setter
        .set(Path::new("/w/it's \"x\".png"), Fit::Center)
        .await
        .unwrap();
    assert_eq!(
        runner.argvs(),
        [
            [
                "gsettings",
                "set",
                "org.cinnamon.desktop.background",
                "picture-uri",
                "file:///w/it's%20%22x%22.png"
            ],
            [
                "gsettings",
                "set",
                "org.cinnamon.desktop.background",
                "picture-options",
                "centered"
            ]
        ]
    );
    assert_eq!(
        runner.invocations()[0].env,
        [(
            "DBUS_SESSION_BUS_ADDRESS".to_owned(),
            "unix:path=/nowhere".to_owned()
        )]
    );
    assert_eq!(setter.info().name, "cinnamon");
    assert_eq!(
        setter.info().detail,
        "gsettings set org.cinnamon.desktop.background"
    );
}

#[tokio::test]
async fn gsettings_failure_stops_and_reports() {
    let runner = RecordingRunner::new();
    runner.push_output(Output::failure(
        1,
        "Schema “org.mate.background” is not installed\n",
    ));
    let setter = GsettingsSetter::new(BackendKind::Mate, Arc::new(runner.clone())).unwrap();
    match setter.set(Path::new("/w/x.png"), Fit::Fill).await {
        Err(SetError::Command {
            argv0,
            status,
            stderr,
        }) => {
            assert_eq!(argv0, "gsettings");
            assert_eq!(status, CommandStatus::Exited(1));
            assert!(stderr.contains("not installed"), "{stderr}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(runner.argvs().len(), 1, "stops at the first failure");
}

/// GNOME < 42 has no `picture-uri-dark`; only that step may fail, and only with "No such key".
#[tokio::test]
async fn gsettings_gnome_tolerates_missing_dark_key_only() {
    let runner = RecordingRunner::new();
    runner.push_output(Output::success());
    runner.push_output(Output::failure(1, "No such key “picture-uri-dark”\n"));
    let setter = GsettingsSetter::new(BackendKind::Gnome, Arc::new(runner.clone())).unwrap();
    setter.set(Path::new("/w/x.png"), Fit::Fit).await.unwrap();
    assert_eq!(runner.argvs().len(), 3);

    let runner = RecordingRunner::new();
    runner.push_output(Output::failure(1, "No such key “picture-uri”\n"));
    let setter = GsettingsSetter::new(BackendKind::Gnome, Arc::new(runner.clone())).unwrap();
    assert!(setter.set(Path::new("/w/x.png"), Fit::Fit).await.is_err());
    assert_eq!(runner.argvs().len(), 1);
}

/// A fake `gsettings` that appends its argv (one line, NUL-separated args) to `$GSETTINGS_LOG`.
/// With `GSETTINGS_OLD=1` it fails `picture-uri-dark` like GNOME 41 does.
const FAKE_GSETTINGS: &str = r#"#!/bin/sh
for arg in "$@"; do printf '%s\0' "$arg" >> "$GSETTINGS_LOG"; done
printf '\n' >> "$GSETTINGS_LOG"
if [ "${GSETTINGS_OLD:-}" = 1 ] && [ "$4" = picture-uri-dark ]; then
  echo "No such key “picture-uri-dark”" >&2
  exit 1
fi
exit 0
"#;

fn fake_path(dir: &Path) -> String {
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let script = bin.join("gsettings");
    std::fs::write(&script, FAKE_GSETTINGS).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    // `sh` must still resolve, so keep /bin and /usr/bin after the fake.
    format!("{}:/usr/bin:/bin", bin.display())
}

fn read_log(path: &PathBuf) -> Vec<Vec<String>> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| {
            line.split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn gsettings_fake_binary_on_path_integration() {
    let dir = tempfile::tempdir().unwrap();
    let path = fake_path(dir.path());
    let log = dir.path().join("gsettings.log");
    let image = dir.path().join("it's a \"wall\" $(paper).jpg");

    let setter = GsettingsSetter::new(BackendKind::Gnome, Arc::new(TokioRunner))
        .unwrap()
        .with_env("PATH", &path)
        .with_env("GSETTINGS_LOG", log.to_str().unwrap());
    setter.set(&image, Fit::Stretch).await.unwrap();
    let expected = argvs(BackendKind::Gnome, &image, Fit::Stretch)
        .unwrap()
        .unwrap();
    let calls = read_log(&log);
    assert_eq!(calls.len(), 3);
    for (call, argv) in calls.iter().zip(&expected) {
        assert_eq!(call, &argv[1..], "args after argv[0]");
    }
    assert!(calls[0][3].starts_with("file:///"), "{:?}", calls[0]);

    // GNOME 41: the dark key fails, `set` still succeeds and picture-options still runs.
    std::fs::remove_file(&log).unwrap();
    setter
        .clone()
        .with_env("GSETTINGS_OLD", "1")
        .set(&image, Fit::Fill)
        .await
        .unwrap();
    let calls = read_log(&log);
    assert_eq!(calls.len(), 3);
    assert_eq!(
        calls[2],
        [
            "set",
            "org.gnome.desktop.background",
            "picture-options",
            "zoom"
        ]
    );

    // MATE writes a plain path, not a URI.
    std::fs::remove_file(&log).unwrap();
    GsettingsSetter::new(BackendKind::Mate, Arc::new(TokioRunner))
        .unwrap()
        .with_env("PATH", &path)
        .with_env("GSETTINGS_LOG", log.to_str().unwrap())
        .set(&image, Fit::Fill)
        .await
        .unwrap();
    assert_eq!(
        read_log(&log)[0],
        [
            "set",
            "org.mate.background",
            "picture-filename",
            image.to_str().unwrap()
        ]
    );
}
