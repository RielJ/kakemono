//! Open an http(s) URL in the browser, open a folder, or reveal a file in the file manager
//! (plan §G "URL open / reveal").
//!
//! Every launch is an argv run through a [`CommandRunner`] (never a shell string):
//!
//! | OS | URL | folder | reveal |
//! |---|---|---|---|
//! | macOS | `open <url>` | `open <dir>` | `open -R <file>` |
//! | Windows | `rundll32.exe url.dll,FileProtocolHandler <url>` (ShellExecute) | `explorer.exe <dir>` | `explorer.exe /select, <file>` |
//! | Linux | `xdg-open <url>` | `xdg-open <dir>` | D-Bus `org.freedesktop.FileManager1.ShowItems`, else `xdg-open <parent dir>` |
//!
//! - Only `http` and `https` URLs are opened ([`checked_url`]); the URL is passed re-serialized
//!   by the `url` crate, so it always starts with `http` and can't be read as a flag.
//! - Paths must be absolute (so they can't be read as a flag), UTF-8 and exist.
//! - `explorer.exe` exits 1 even on success, so its exit code is ignored.
//! - `xdg-open` is started detached: in its generic mode it runs the handler in the foreground
//!   and only returns when the browser or file manager exits.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use kakemono_config::paths::Os;
use url::Url;

use crate::api::SetError;
use crate::exec::{CommandRunner, Invocation, TokioRunner};

/// The freedesktop file-manager D-Bus service (and interface) used to reveal files on Linux.
pub const FILE_MANAGER1: &str = "org.freedesktop.FileManager1";
/// Its object path.
pub const FILE_MANAGER1_PATH: &str = "/org/freedesktop/FileManager1";
/// How long the `ShowItems` call may take before falling back to `xdg-open`.
pub const SHOW_ITEMS_TIMEOUT: Duration = Duration::from_secs(5);

/// Why an open or reveal failed.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("invalid URL {url:?}: {source}")]
    InvalidUrl {
        url: String,
        #[source]
        source: url::ParseError,
    },
    #[error("refusing to open {url:?}: only http and https URLs are allowed")]
    UnsupportedScheme { url: String },
    #[error("path must be absolute: {}", .0.display())]
    RelativePath(PathBuf),
    #[error("path is not valid UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),
    #[error("no such file or directory: {}", .0.display())]
    NotFound(PathBuf),
    /// The launcher failed to start or exited non-zero (always `SetError::Command`).
    #[error(transparent)]
    Command(#[from] SetError),
}

/// Parses `url` and accepts only `http` and `https`.
pub fn checked_url(url: &str) -> Result<Url, OpenError> {
    let parsed = Url::parse(url).map_err(|source| OpenError::InvalidUrl {
        url: url.to_owned(),
        source,
    })?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        _ => Err(OpenError::UnsupportedScheme {
            url: url.to_owned(),
        }),
    }
}

/// `path` as a `&str` after checking it is absolute, UTF-8 and exists.
fn checked_path(path: &Path) -> Result<&str, OpenError> {
    if !path.is_absolute() {
        return Err(OpenError::RelativePath(path.to_path_buf()));
    }
    let text = path
        .to_str()
        .ok_or_else(|| OpenError::NonUtf8Path(path.to_path_buf()))?;
    if !path.exists() {
        return Err(OpenError::NotFound(path.to_path_buf()));
    }
    Ok(text)
}

/// One launcher command and how to run it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// Run to completion; a non-zero exit is an error.
    Run(Vec<String>),
    /// Run to completion and ignore the exit code (`explorer.exe` exits 1 on success).
    RunIgnoreExit(Vec<String>),
    /// Start it and don't wait (`xdg-open`).
    Detach(Vec<String>),
}

impl Launch {
    pub fn argv(&self) -> &[String] {
        match self {
            Launch::Run(argv) | Launch::RunIgnoreExit(argv) | Launch::Detach(argv) => argv,
        }
    }
}

impl fmt::Display for Launch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = match self {
            Launch::Run(_) => "run",
            Launch::RunIgnoreExit(_) => "run (exit ignored)",
            Launch::Detach(_) => "detach",
        };
        write!(f, "{mode} {:?}", self.argv())
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

/// The launcher that opens `url` (already checked by [`checked_url`]).
pub fn url_launch(os: Os, url: &Url) -> Launch {
    match os {
        Os::MacOs => Launch::Run(argv(&["open", url.as_str()])),
        // url.dll's FileProtocolHandler hands the URL to ShellExecute. explorer.exe would split
        // the URL at commas.
        Os::Windows => Launch::Run(argv(&[
            "rundll32.exe",
            "url.dll,FileProtocolHandler",
            url.as_str(),
        ])),
        Os::Linux => Launch::Detach(argv(&["xdg-open", url.as_str()])),
    }
}

/// The launcher that opens the folder `dir` (absolute).
pub fn folder_launch(os: Os, dir: &str) -> Launch {
    match os {
        Os::MacOs => Launch::Run(argv(&["open", dir])),
        Os::Windows => Launch::RunIgnoreExit(argv(&["explorer.exe", dir])),
        Os::Linux => Launch::Detach(argv(&["xdg-open", dir])),
    }
}

/// The launcher that shows the file `file` (absolute) selected in the file manager. On Linux
/// this is the fallback after `ShowItems`: open the parent folder.
pub fn reveal_launch(os: Os, file: &str) -> Launch {
    match os {
        Os::MacOs => Launch::Run(argv(&["open", "-R", file])),
        // `/select,` and the path as separate arguments: Rust quotes a path with spaces as a
        // whole, which explorer.exe accepts after `/select, `.
        Os::Windows => Launch::RunIgnoreExit(argv(&["explorer.exe", "/select,", file])),
        Os::Linux => {
            let parent = Path::new(file)
                .parent()
                .and_then(Path::to_str)
                .filter(|p| !p.is_empty())
                .unwrap_or(file);
            Launch::Detach(argv(&["xdg-open", parent]))
        }
    }
}

/// The `org.freedesktop.FileManager1` service (Linux). A seam so tests never reach a real bus.
#[async_trait::async_trait]
pub trait FileManager: Send + Sync + 'static {
    /// `ShowItems(uris, "")`: open the folders holding `uris` with those items selected.
    async fn show_items(&self, uris: &[String]) -> Result<(), String>;
}

/// No file-manager service (macOS, Windows): `show_items` always fails.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFileManager;

#[async_trait::async_trait]
impl FileManager for NoFileManager {
    async fn show_items(&self, _uris: &[String]) -> Result<(), String> {
        Err(format!("{FILE_MANAGER1} is not available on this OS"))
    }
}

#[cfg(target_os = "linux")]
pub use dbus::DbusFileManager;

#[cfg(target_os = "linux")]
mod dbus {
    use super::{FILE_MANAGER1, FILE_MANAGER1_PATH, FileManager};

    /// `ShowItems` over D-Bus: the session bus (connected on first use), or a given connection.
    #[derive(Debug, Default)]
    pub struct DbusFileManager {
        conn: tokio::sync::OnceCell<zbus::Connection>,
    }

    impl DbusFileManager {
        /// Connects to the session bus on the first call.
        pub fn session() -> Self {
            Self::default()
        }

        /// Uses `conn` (a peer-to-peer connection in tests).
        pub fn new(conn: zbus::Connection) -> Self {
            Self {
                conn: tokio::sync::OnceCell::new_with(Some(conn)),
            }
        }
    }

    #[async_trait::async_trait]
    impl FileManager for DbusFileManager {
        async fn show_items(&self, uris: &[String]) -> Result<(), String> {
            let conn = self
                .conn
                .get_or_try_init(zbus::Connection::session)
                .await
                .map_err(|err| format!("session bus: {err}"))?;
            conn.call_method(
                Some(FILE_MANAGER1),
                FILE_MANAGER1_PATH,
                Some(FILE_MANAGER1),
                "ShowItems",
                &(uris, ""),
            )
            .await
            .map_err(|err| format!("{FILE_MANAGER1}.ShowItems: {err}"))?;
            Ok(())
        }
    }
}

/// The file manager for this OS: D-Bus on Linux, none elsewhere.
pub fn default_file_manager() -> Arc<dyn FileManager> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(DbusFileManager::session())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Arc::new(NoFileManager)
    }
}

/// Opens URLs and folders and reveals files for one OS through a [`CommandRunner`].
#[derive(Clone)]
pub struct Opener {
    os: Os,
    runner: Arc<dyn CommandRunner>,
    file_manager: Arc<dyn FileManager>,
}

impl fmt::Debug for Opener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opener")
            .field("os", &self.os)
            .finish_non_exhaustive()
    }
}

impl Opener {
    /// The real thing for this OS: [`TokioRunner`] and [`default_file_manager`].
    pub fn system() -> Self {
        Self::with_parts(Os::current(), Arc::new(TokioRunner), default_file_manager())
    }

    /// Explicit OS, runner and file manager (fakes in tests: nothing is launched).
    pub fn with_parts(
        os: Os,
        runner: Arc<dyn CommandRunner>,
        file_manager: Arc<dyn FileManager>,
    ) -> Self {
        Self {
            os,
            runner,
            file_manager,
        }
    }

    pub fn os(&self) -> Os {
        self.os
    }

    /// Opens an `http`/`https` URL in the default browser. Other schemes are refused.
    pub async fn open_url(&self, url: &str) -> Result<(), OpenError> {
        let url = checked_url(url)?;
        self.launch(url_launch(self.os, &url)).await
    }

    /// Opens the folder `dir` in the file manager.
    pub async fn open_folder(&self, dir: &Path) -> Result<(), OpenError> {
        let dir = checked_path(dir)?;
        self.launch(folder_launch(self.os, dir)).await
    }

    /// Shows `file` selected in the file manager. Linux: `ShowItems`, falling back to opening
    /// the parent folder when the call fails or takes longer than [`SHOW_ITEMS_TIMEOUT`].
    pub async fn reveal(&self, file: &Path) -> Result<(), OpenError> {
        let text = checked_path(file)?;
        if self.os == Os::Linux {
            match self.show_items(file).await {
                Ok(()) => return Ok(()),
                Err(reason) => {
                    tracing::debug!(%reason, "ShowItems failed; opening the parent folder");
                }
            }
        }
        self.launch(reveal_launch(self.os, text)).await
    }

    async fn show_items(&self, file: &Path) -> Result<(), String> {
        let uri = Url::from_file_path(file)
            .map_err(|()| format!("no file URI for {}", file.display()))?;
        let uris = [String::from(uri)];
        tokio::time::timeout(SHOW_ITEMS_TIMEOUT, self.file_manager.show_items(&uris))
            .await
            .map_err(|_| format!("timed out after {}s", SHOW_ITEMS_TIMEOUT.as_secs()))?
    }

    async fn launch(&self, launch: Launch) -> Result<(), OpenError> {
        tracing::debug!(%launch, "opening");
        match launch {
            Launch::Detach(argv) => {
                self.runner.spawn_detached(&argv).map_err(SetError::from)?;
            }
            Launch::Run(argv) => {
                let invocation = Invocation::new(argv);
                let output = self.runner.run(&invocation).await.map_err(SetError::from)?;
                output.check(invocation.argv0())?;
            }
            Launch::RunIgnoreExit(argv) => {
                let output = self
                    .runner
                    .run(&Invocation::new(argv))
                    .await
                    .map_err(SetError::from)?;
                tracing::debug!(status = %output.status, "launcher exited");
            }
        }
        Ok(())
    }
}

/// [`Opener::open_url`] on this OS.
pub async fn open_url(url: &str) -> Result<(), OpenError> {
    Opener::system().open_url(url).await
}

/// [`Opener::open_folder`] on this OS.
pub async fn open_folder(dir: &Path) -> Result<(), OpenError> {
    Opener::system().open_folder(dir).await
}

/// [`Opener::reveal`] on this OS.
pub async fn reveal(file: &Path) -> Result<(), OpenError> {
    Opener::system().reveal(file).await
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::sync::Mutex;

    use super::*;
    use crate::api::CommandStatus;
    use crate::exec::{Output, RecordingRunner};

    const OSES: [Os; 3] = [Os::MacOs, Os::Windows, Os::Linux];

    /// Records `ShowItems` calls; fails, succeeds or hangs as told.
    #[derive(Default)]
    struct FakeFileManager {
        calls: Mutex<Vec<Vec<String>>>,
        outcome: Option<Result<(), String>>,
    }

    impl FakeFileManager {
        fn ok() -> Arc<Self> {
            Arc::new(Self {
                outcome: Some(Ok(())),
                ..Self::default()
            })
        }
        fn failing() -> Arc<Self> {
            Arc::new(Self {
                outcome: Some(Err("org.freedesktop.DBus.Error.ServiceUnknown".to_owned())),
                ..Self::default()
            })
        }
        /// Never answers.
        fn hanging() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl FileManager for FakeFileManager {
        async fn show_items(&self, uris: &[String]) -> Result<(), String> {
            self.calls.lock().unwrap().push(uris.to_vec());
            match &self.outcome {
                Some(outcome) => outcome.clone(),
                None => std::future::pending().await,
            }
        }
    }

    fn fake_opener(os: Os, fm: Arc<FakeFileManager>) -> (Opener, RecordingRunner) {
        let runner = RecordingRunner::new();
        (Opener::with_parts(os, Arc::new(runner.clone()), fm), runner)
    }

    /// Every launch the runner saw, as text.
    fn launches(runner: &RecordingRunner) -> Vec<String> {
        let mut out: Vec<String> = runner
            .argvs()
            .into_iter()
            .map(|a| format!("run {a:?}"))
            .collect();
        out.extend(
            runner
                .detached()
                .into_iter()
                .map(|a| format!("detach {a:?}")),
        );
        out
    }

    #[test]
    fn launch_argvs_per_os() {
        let url = checked_url("https://example.com/a b?q=1,2&x=\"y\"").unwrap();
        let mut text = String::new();
        for os in OSES {
            let (dir, file) = match os {
                Os::Windows => (
                    r"C:\Users\Ann Lee\Pictures",
                    r"C:\Users\Ann Lee\Pictures\a b.jpg",
                ),
                _ => (
                    "/home/ann/Wall papers",
                    "/home/ann/Wall papers/a b;$(x).jpg",
                ),
            };
            writeln!(text, "{os:?}").unwrap();
            writeln!(text, "  url:    {}", url_launch(os, &url)).unwrap();
            writeln!(text, "  folder: {}", folder_launch(os, dir)).unwrap();
            writeln!(text, "  reveal: {}", reveal_launch(os, file)).unwrap();
        }
        insta::assert_snapshot!(text, @r#"
        MacOs
          url:    run ["open", "https://example.com/a%20b?q=1,2&x=%22y%22"]
          folder: run ["open", "/home/ann/Wall papers"]
          reveal: run ["open", "-R", "/home/ann/Wall papers/a b;$(x).jpg"]
        Windows
          url:    run ["rundll32.exe", "url.dll,FileProtocolHandler", "https://example.com/a%20b?q=1,2&x=%22y%22"]
          folder: run (exit ignored) ["explorer.exe", "C:\\Users\\Ann Lee\\Pictures"]
          reveal: run (exit ignored) ["explorer.exe", "/select,", "C:\\Users\\Ann Lee\\Pictures\\a b.jpg"]
        Linux
          url:    detach ["xdg-open", "https://example.com/a%20b?q=1,2&x=%22y%22"]
          folder: detach ["xdg-open", "/home/ann/Wall papers"]
          reveal: detach ["xdg-open", "/home/ann/Wall papers"]
        "#);
    }

    #[test]
    fn linux_reveal_of_root_opens_root() {
        assert_eq!(
            reveal_launch(Os::Linux, "/"),
            Launch::Detach(argv(&["xdg-open", "/"]))
        );
    }

    #[test]
    fn only_http_and_https_urls() {
        for ok in [
            "http://example.com",
            "https://example.com/x?y#z",
            "HTTPS://Example.com",
        ] {
            assert!(checked_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com",
            "mailto:a@example.com",
            "x-apple.systempreferences:com.apple.preference",
            "smb://host/share",
            "data:text/html,hi",
        ] {
            let err = checked_url(bad).unwrap_err();
            assert!(
                matches!(err, OpenError::UnsupportedScheme { .. }),
                "{bad}: {err}"
            );
        }
        for invalid in ["", "example.com", "-a", "/etc/passwd", "http://"] {
            assert!(
                matches!(checked_url(invalid), Err(OpenError::InvalidUrl { .. })),
                "{invalid}"
            );
        }
        assert_eq!(
            checked_url("file:///x").unwrap_err().to_string(),
            r#"refusing to open "file:///x": only http and https URLs are allowed"#
        );
    }

    #[tokio::test]
    async fn refused_urls_launch_nothing() {
        for os in OSES {
            let fm = FakeFileManager::ok();
            let (opener, runner) = fake_opener(os, fm);
            assert!(opener.open_url("file:///etc/passwd").await.is_err());
            assert!(opener.open_url("--help").await.is_err());
            assert!(launches(&runner).is_empty(), "{os:?}");
        }
    }

    #[tokio::test]
    async fn opener_uses_the_os_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a b.jpg");
        std::fs::write(&file, b"x").unwrap();
        let d = dir.path().to_str().unwrap();
        let f = file.to_str().unwrap();

        for os in OSES {
            let fm = FakeFileManager::failing();
            let (opener, runner) = fake_opener(os, Arc::clone(&fm));
            opener.open_url("https://example.com/").await.unwrap();
            opener.open_folder(dir.path()).await.unwrap();
            opener.reveal(&file).await.unwrap();
            let url = r#"["xdg-open", "https://example.com/"]"#;
            let want: Vec<String> = match os {
                Os::MacOs => vec![
                    r#"run ["open", "https://example.com/"]"#.to_owned(),
                    format!("run {:?}", ["open", d]),
                    format!("run {:?}", ["open", "-R", f]),
                ],
                Os::Windows => vec![
                    r#"run ["rundll32.exe", "url.dll,FileProtocolHandler", "https://example.com/"]"#
                        .to_owned(),
                    format!("run {:?}", ["explorer.exe", d]),
                    format!("run {:?}", ["explorer.exe", "/select,", f]),
                ],
                Os::Linux => vec![
                    format!("detach {url}"),
                    format!("detach {:?}", ["xdg-open", d]),
                    format!("detach {:?}", ["xdg-open", d]),
                ],
            };
            assert_eq!(launches(&runner), want, "{os:?}");
            let show_items_calls = fm.calls().len();
            assert_eq!(show_items_calls, usize::from(os == Os::Linux), "{os:?}");
        }
    }

    #[tokio::test]
    async fn linux_reveal_uses_show_items() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a b#1.jpg");
        std::fs::write(&file, b"x").unwrap();
        let fm = FakeFileManager::ok();
        let (opener, runner) = fake_opener(Os::Linux, Arc::clone(&fm));
        opener.reveal(&file).await.unwrap();
        let uri = String::from(Url::from_file_path(&file).unwrap());
        assert!(
            uri.starts_with("file:///") && uri.ends_with("/a%20b%231.jpg"),
            "{uri}"
        );
        assert_eq!(fm.calls(), [vec![uri]]);
        assert!(launches(&runner).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn linux_reveal_falls_back_when_show_items_hangs() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.jpg");
        std::fs::write(&file, b"x").unwrap();
        let fm = FakeFileManager::hanging();
        let (opener, runner) = fake_opener(Os::Linux, Arc::clone(&fm));
        opener.reveal(&file).await.unwrap();
        assert_eq!(fm.calls().len(), 1);
        assert_eq!(
            runner.detached(),
            [vec![
                "xdg-open".to_owned(),
                dir.path().to_str().unwrap().to_owned()
            ]]
        );
    }

    #[tokio::test]
    async fn bad_paths_launch_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.jpg");
        for os in OSES {
            let (opener, runner) = fake_opener(os, FakeFileManager::ok());
            let err = opener.reveal(&missing).await.unwrap_err();
            assert!(matches!(err, OpenError::NotFound(_)), "{err}");
            let err = opener.open_folder(Path::new("rel/dir")).await.unwrap_err();
            assert!(matches!(err, OpenError::RelativePath(_)), "{err}");
            let err = opener.reveal(Path::new("-R")).await.unwrap_err();
            assert!(matches!(err, OpenError::RelativePath(_)), "{err}");
            assert!(launches(&runner).is_empty(), "{os:?}");
        }
    }

    #[tokio::test]
    async fn launcher_failures() {
        let dir = tempfile::tempdir().unwrap();

        // macOS `open` exiting non-zero is an error with its stderr.
        let (opener, runner) = fake_opener(Os::MacOs, FakeFileManager::ok());
        runner.push_output(Output::failure(1, "The file does not exist.\n"));
        let err = opener.open_folder(dir.path()).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "open: exit code 1: The file does not exist."
        );

        // Not installed.
        runner.push_spawn_error("No such file or directory");
        let err = opener.open_url("https://example.com").await.unwrap_err();
        assert!(matches!(
            err,
            OpenError::Command(SetError::Command {
                status: CommandStatus::FailedToStart,
                ..
            })
        ));

        // explorer.exe exits 1 on success.
        let (opener, runner) = fake_opener(Os::Windows, FakeFileManager::ok());
        runner.push_output(Output::failure(1, ""));
        opener.open_folder(dir.path()).await.unwrap();
        // ... but failing to start is still an error.
        runner.push_spawn_error("not found");
        assert!(opener.open_folder(dir.path()).await.is_err());
    }
}

/// The real [`DbusFileManager`] against a fake `org.freedesktop.FileManager1` on a peer-to-peer
/// socket pair: no session bus, no file manager.
#[cfg(all(test, target_os = "linux"))]
mod dbus_tests {
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};

    use zbus::connection::Builder;

    use super::*;
    use crate::exec::RecordingRunner;

    /// `ShowItems` calls: the URIs and the startup id.
    type Calls = Arc<Mutex<Vec<(Vec<String>, String)>>>;

    #[derive(Clone, Default)]
    struct FakeFileManager1 {
        calls: Calls,
        fail: bool,
    }

    #[zbus::interface(name = "org.freedesktop.FileManager1")]
    impl FakeFileManager1 {
        fn show_items(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
            self.calls.lock().unwrap().push((uris, startup_id));
            if self.fail {
                return Err(zbus::fdo::Error::Failed("no window".to_owned()));
            }
            Ok(())
        }
    }

    async fn p2p(fm: FakeFileManager1) -> (zbus::Connection, zbus::Connection) {
        let (a, b) = UnixStream::pair().unwrap();
        let server = Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .p2p()
            .serve_at(FILE_MANAGER1_PATH, fm)
            .build();
        let client = Builder::unix_stream(b).p2p().build();
        let (server, client) = tokio::join!(server, client);
        (client.unwrap(), server.unwrap())
    }

    #[tokio::test]
    async fn reveal_calls_show_items_over_dbus() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a b.jpg");
        std::fs::write(&file, b"x").unwrap();
        let fake = FakeFileManager1::default();
        let (client, _server) = p2p(fake.clone()).await;
        let runner = RecordingRunner::new();
        let opener = Opener::with_parts(
            Os::Linux,
            Arc::new(runner.clone()),
            Arc::new(DbusFileManager::new(client)),
        );
        opener.reveal(&file).await.unwrap();
        let uri = String::from(Url::from_file_path(&file).unwrap());
        assert_eq!(*fake.calls.lock().unwrap(), [(vec![uri], String::new())]);
        assert!(runner.detached().is_empty() && runner.argvs().is_empty());
    }

    #[tokio::test]
    async fn dbus_error_falls_back_to_xdg_open_of_parent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.jpg");
        std::fs::write(&file, b"x").unwrap();
        let fake = FakeFileManager1 {
            fail: true,
            ..FakeFileManager1::default()
        };
        let (client, _server) = p2p(fake.clone()).await;
        let runner = RecordingRunner::new();
        let opener = Opener::with_parts(
            Os::Linux,
            Arc::new(runner.clone()),
            Arc::new(DbusFileManager::new(client)),
        );
        opener.reveal(&file).await.unwrap();
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
        assert_eq!(
            runner.detached(),
            [vec![
                "xdg-open".to_owned(),
                dir.path().to_str().unwrap().to_owned()
            ]]
        );
    }
}
