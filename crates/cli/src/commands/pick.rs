//! `kakemono pick` (plan Task 46): open the picker in the UI app. Also the shared launcher that
//! `kakemono settings` uses ([`open`]).
//!
//! Both commands find `kakemono-ui` ([`locate`]) and start it with `--picker` or `--settings`,
//! detached from this terminal, then exit. When the UI already runs, its single-instance plugin
//! makes the new process forward its argv to the running one and exit, so the page opens there.
//! `--config` and `--socket` reach a freshly started UI as `KAKEMONO_CONFIG` / `KAKEMONO_SOCKET`
//! (a running UI keeps its own), as Go's `settings` did.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Context as _;
use kakemono_protocol::endpoint::SOCKET_ENV;

use crate::cli::Context;
use crate::commands::process::{self, Detach};

/// Overrides where `kakemono-ui` is. When set it is the only place looked at.
pub const UI_ENV: &str = "KAKEMONO_UI";
/// The config path variable the UI reads.
pub use kakemono_config::paths::CONFIG_ENV;

/// The window to open. The UI's `page_from_args` reads the same flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Picker,
    Settings,
}

impl Page {
    pub fn flag(self) -> &'static str {
        match self {
            Page::Picker => "--picker",
            Page::Settings => "--settings",
        }
    }
}

/// The UI binary's file name on this OS.
pub fn ui_file_name() -> String {
    format!("kakemono-ui{}", std::env::consts::EXE_SUFFIX)
}

/// Where [`locate`] looks, captured from the process ([`Search::from_environment`]) or built by
/// tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    /// `$KAKEMONO_UI`, if set and not empty.
    pub ui_env: Option<PathBuf>,
    /// The directory of this executable, then of its resolved path when a symlink led here.
    pub exe_dirs: Vec<PathBuf>,
    /// `Contents/MacOS` of the installed `kakemono.app` bundles (macOS only).
    pub bundle_dirs: Vec<PathBuf>,
    /// `$PATH`.
    pub path_env: Option<OsString>,
}

impl Search {
    pub fn from_environment() -> Self {
        let mut exe_dirs = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            push_parent(&mut exe_dirs, &exe);
            if let Ok(real) = exe.canonicalize() {
                push_parent(&mut exe_dirs, &real);
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self {
            ui_env: std::env::var_os(UI_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            exe_dirs,
            bundle_dirs: bundle_dirs(home.as_deref()),
            path_env: std::env::var_os("PATH"),
        }
    }
}

fn push_parent(dirs: &mut Vec<PathBuf>, exe: &Path) {
    if let Some(dir) = exe.parent()
        && !dirs.iter().any(|d| d == dir)
    {
        dirs.push(dir.to_owned());
    }
}

/// `/Applications/kakemono.app/Contents/MacOS`, then the same under `~/Applications`.
#[cfg(target_os = "macos")]
fn bundle_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let inner = Path::new("kakemono.app/Contents/MacOS");
    let mut dirs = vec![Path::new("/Applications").join(inner)];
    if let Some(home) = home {
        dirs.push(home.join("Applications").join(inner));
    }
    dirs
}

#[cfg(not(target_os = "macos"))]
fn bundle_dirs(_home: Option<&Path>) -> Vec<PathBuf> {
    Vec::new()
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LocateError {
    #[error(
        "{UI_ENV} is set to {}, which is not a file; unset it or point it at the kakemono-ui binary",
        .0.display()
    )]
    EnvNotAFile(PathBuf),
    #[error(
        "cannot find kakemono-ui (the kakemono tray, picker and settings app).\nLooked in:\n{}\n\
         Install the kakemono app, put kakemono-ui on your PATH, or set {UI_ENV} to its path.",
        looked_in(.looked)
    )]
    NotFound { looked: Vec<PathBuf> },
}

fn looked_in(dirs: &[PathBuf]) -> String {
    let mut lines: Vec<String> = dirs.iter().map(|d| format!("  {}", d.display())).collect();
    lines.push("  every directory on PATH".to_owned());
    lines.join("\n")
}

/// Finds `kakemono-ui`: `$KAKEMONO_UI` alone when set; otherwise beside this executable, in the
/// installed `kakemono.app` bundle, then on `PATH`. `is_file` is injected so tests need no
/// real binaries.
pub fn locate(search: &Search, is_file: &dyn Fn(&Path) -> bool) -> Result<PathBuf, LocateError> {
    if let Some(path) = &search.ui_env {
        return if is_file(path) {
            Ok(path.clone())
        } else {
            Err(LocateError::EnvNotAFile(path.clone()))
        };
    }
    let name = ui_file_name();
    let fixed: Vec<PathBuf> = search
        .exe_dirs
        .iter()
        .chain(&search.bundle_dirs)
        .cloned()
        .collect();
    let on_path = search
        .path_env
        .iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| !dir.as_os_str().is_empty());
    fixed
        .iter()
        .cloned()
        .chain(on_path)
        .map(|dir| dir.join(&name))
        .find(|candidate| is_file(candidate))
        .ok_or(LocateError::NotFound { looked: fixed })
}

/// One UI start: the program, its arguments and the variables set on top of the inherited
/// environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(&'static str, PathBuf)>,
}

impl Launch {
    /// `kakemono-ui --picker|--settings`, with `--config` / `--socket` (made absolute) passed
    /// on as `KAKEMONO_CONFIG` / `KAKEMONO_SOCKET`.
    pub fn new(ctx: &Context, ui: PathBuf, page: Page) -> anyhow::Result<Self> {
        let mut env = Vec::new();
        if let Some(config) = ctx
            .global
            .config
            .as_deref()
            .filter(|p| !p.as_os_str().is_empty())
        {
            env.push((CONFIG_ENV, process::absolute(config)?));
        }
        if let Some(socket) = ctx
            .global
            .socket
            .as_deref()
            .filter(|p| !p.as_os_str().is_empty())
        {
            env.push((SOCKET_ENV, process::absolute(socket)?));
        }
        Ok(Self {
            program: ui,
            args: vec![page.flag().into()],
            env,
        })
    }
}

/// Starts a [`Launch`]. [`Detached`] in production; tests record instead.
pub trait Launcher {
    fn launch(&self, launch: &Launch) -> io::Result<()>;
}

/// Spawns the UI with stdio closed, in its own process group (Unix) or detached from the
/// console (Windows), and does not wait: the UI outlives this command.
#[derive(Clone, Copy, Debug, Default)]
pub struct Detached;

impl Launcher for Detached {
    fn launch(&self, launch: &Launch) -> io::Result<()> {
        let mut cmd = Command::new(&launch.program);
        cmd.args(&launch.args)
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        process::detach(&mut cmd, Detach::Group);
        // Dropping the handle neither kills nor waits; the child is reparented on exit.
        cmd.spawn().map(drop)
    }
}

/// Opens `page` in the UI found through `search`, started by `launcher`. Prints nothing on
/// success.
pub fn open(
    ctx: &Context,
    page: Page,
    search: &Search,
    is_file: &dyn Fn(&Path) -> bool,
    launcher: &dyn Launcher,
) -> anyhow::Result<()> {
    let ui = locate(search, is_file)?;
    let launch = Launch::new(ctx, ui, page)?;
    launcher
        .launch(&launch)
        .with_context(|| format!("starting {}", launch.program.display()))
}

/// `kakemono pick`.
pub fn run(ctx: &Context, _out: &mut dyn Write) -> anyhow::Result<()> {
    open(
        ctx,
        Page::Picker,
        &Search::from_environment(),
        &Path::is_file,
        &Detached,
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;

    use super::*;
    use crate::cli::GlobalArgs;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<Launch>>);

    impl Launcher for Recorder {
        fn launch(&self, launch: &Launch) -> io::Result<()> {
            self.0.borrow_mut().push(launch.clone());
            Ok(())
        }
    }

    /// One line per launch: `KEY=value … program args…`, with `/` separators.
    fn argv(launch: &Launch) -> String {
        let mut parts: Vec<String> = launch
            .env
            .iter()
            .map(|(k, v)| format!("{k}={}", v.display()))
            .collect();
        parts.push(launch.program.display().to_string());
        parts.extend(launch.args.iter().map(|a| a.to_string_lossy().into_owned()));
        parts.join(" ").replace('\\', "/")
    }

    fn files(paths: &[&str]) -> impl Fn(&Path) -> bool + use<> {
        let set: HashSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    }

    fn search() -> Search {
        Search {
            ui_env: None,
            exe_dirs: vec!["/opt/k/bin".into(), "/real/k/bin".into()],
            bundle_dirs: vec!["/Applications/kakemono.app/Contents/MacOS".into()],
            path_env: Some(
                std::env::join_paths(["/usr/local/bin", "/usr/bin"]).expect("join PATH"),
            ),
        }
    }

    fn ui(dir: &str) -> String {
        format!("{dir}/{}", ui_file_name())
    }

    #[test]
    fn flags_match_the_ui() {
        assert_eq!(Page::Picker.flag(), "--picker");
        assert_eq!(Page::Settings.flag(), "--settings");
    }

    #[test]
    fn beside_the_exe_wins() {
        let fs = files(&[&ui("/real/k/bin"), &ui("/usr/bin"), &ui("/opt/k/bin")]);
        assert_eq!(locate(&search(), &fs), Ok(PathBuf::from(ui("/opt/k/bin"))));
    }

    #[test]
    fn resolved_exe_dir_then_bundle_then_path() {
        let s = search();
        let fs = files(&[&ui("/real/k/bin"), &ui("/usr/bin")]);
        assert_eq!(locate(&s, &fs), Ok(PathBuf::from(ui("/real/k/bin"))));

        let bundle = ui("/Applications/kakemono.app/Contents/MacOS");
        let fs = files(&[&bundle, &ui("/usr/bin")]);
        assert_eq!(locate(&s, &fs), Ok(PathBuf::from(bundle)));

        let fs = files(&[&ui("/usr/bin")]);
        assert_eq!(locate(&s, &fs), Ok(PathBuf::from(ui("/usr/bin"))));
    }

    #[test]
    fn env_override_is_the_only_place_looked() {
        let mut s = search();
        s.ui_env = Some("/dev/kakemono-ui".into());
        let fs = files(&["/dev/kakemono-ui", &ui("/opt/k/bin")]);
        assert_eq!(locate(&s, &fs), Ok(PathBuf::from("/dev/kakemono-ui")));

        let fs = files(&[&ui("/opt/k/bin")]);
        let err = locate(&s, &fs).expect_err("override points nowhere");
        assert_eq!(err, LocateError::EnvNotAFile("/dev/kakemono-ui".into()));
        assert_eq!(
            err.to_string(),
            "KAKEMONO_UI is set to /dev/kakemono-ui, which is not a file; unset it or point it at \
             the kakemono-ui binary"
        );
    }

    #[test]
    fn not_found_lists_the_places_and_the_hint() {
        let err = locate(&search(), &files(&[])).expect_err("nothing installed");
        insta::assert_snapshot!(err.to_string(), @r"
        cannot find kakemono-ui (the kakemono tray, picker and settings app).
        Looked in:
          /opt/k/bin
          /real/k/bin
          /Applications/kakemono.app/Contents/MacOS
          every directory on PATH
        Install the kakemono app, put kakemono-ui on your PATH, or set KAKEMONO_UI to its path.
        ");
    }

    #[test]
    fn empty_path_entries_are_skipped() {
        let s = Search {
            path_env: Some(std::env::join_paths(["", "/usr/bin"]).expect("join PATH")),
            ..Search::default()
        };
        // An empty entry would mean the cwd; never launch a `kakemono-ui` from there.
        let fs = files(&[&ui(""), &ui("/usr/bin")]);
        assert_eq!(locate(&s, &fs), Ok(PathBuf::from(ui("/usr/bin"))));
    }

    #[test]
    fn launches_the_picker_without_env_by_default() {
        let recorder = Recorder::default();
        let ctx = Context::default();
        open(
            &ctx,
            Page::Picker,
            &search(),
            &files(&[&ui("/usr/bin")]),
            &recorder,
        )
        .expect("open");
        let launches = recorder.0.into_inner();
        assert_eq!(launches.len(), 1);
        assert_eq!(argv(&launches[0]), format!("{} --picker", ui("/usr/bin")));
    }

    #[test]
    fn forwards_config_and_socket_as_absolute_env() {
        let recorder = Recorder::default();
        let ctx = Context::new(GlobalArgs {
            config: Some("/etc/k/config.toml".into()),
            socket: Some("/run/k.sock".into()),
        });
        open(
            &ctx,
            Page::Settings,
            &search(),
            &files(&[&ui("/opt/k/bin")]),
            &recorder,
        )
        .expect("open");
        let launches = recorder.0.into_inner();
        let expected = format!(
            "KAKEMONO_CONFIG={} KAKEMONO_SOCKET={} {} --settings",
            process::absolute(Path::new("/etc/k/config.toml"))
                .expect("abs")
                .display(),
            process::absolute(Path::new("/run/k.sock"))
                .expect("abs")
                .display(),
            ui("/opt/k/bin"),
        )
        .replace('\\', "/");
        assert_eq!(argv(&launches[0]), expected);
    }

    #[test]
    fn a_missing_ui_launches_nothing() {
        let recorder = Recorder::default();
        let err = open(
            &Context::default(),
            Page::Picker,
            &search(),
            &files(&[]),
            &recorder,
        )
        .expect_err("not found");
        assert!(err.to_string().contains("cannot find kakemono-ui"), "{err}");
        assert!(recorder.0.into_inner().is_empty());
    }

    #[test]
    fn a_launch_failure_names_the_program() {
        struct Failing;
        impl Launcher for Failing {
            fn launch(&self, _: &Launch) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            }
        }
        let err = open(
            &Context::default(),
            Page::Picker,
            &search(),
            &files(&[&ui("/usr/bin")]),
            &Failing,
        )
        .expect_err("spawn fails");
        assert_eq!(
            format!("{err:#}"),
            format!("starting {}: denied", ui("/usr/bin"))
        );
    }
}
