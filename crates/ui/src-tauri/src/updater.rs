//! In-app updates through `tauri-plugin-updater`.
//!
//! Each GitHub release carries `latest.json`: the version and, per platform, a bundle URL and its
//! minisign signature. `tauri.conf.json` points the plugin at
//! `releases/latest/download/latest.json` and holds the public key, so only bundles signed with
//! the project key install. Drafts are not "latest", so users see a release once it is
//! published.
//!
//! - `check_update` → [`UpdateInfo`], or `null` when this is the newest version. The [`Update`]
//!   is kept for `install_update`.
//! - `install_update` downloads and verifies the bundle, stops the daemon, installs, and
//!   restarts the app; the new app starts the new daemon at startup. The daemon must stop
//!   first: it runs the bundled `kakemono` binary, which the install replaces (Windows cannot
//!   replace a running `.exe` at all), and a daemon left running would keep the old code.
//! - `open_update_page` opens the release page, for the release notes and for the manual
//!   download when this copy cannot update itself (a dev build or an unknown bundle).
//!
//! A package manager's copy (Scoop, the AUR) must not replace itself behind the package
//! manager's back. Its package writes the command that updates it to an [`UPDATE_COMMAND_FILE`]
//! (see [`package_update_command`]), and the banner shows that command instead of Update.

use std::fs::{OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kakemono_protocol::types::Request;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;
use tokio::time::Instant;
use ts_rs::TS;

use crate::daemon_link::{DaemonLink, LinkError};
use crate::error::{ErrorKind, Kinded, serialize_as_wire_error};

/// The project's releases on GitHub.
pub const RELEASES_URL: &str = "https://github.com/RielJ/kakemono/releases";

/// How long `install_update` waits for the daemon to exit after `quit`.
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the lock is tried while waiting.
const LOCK_POLL: Duration = Duration::from_millis(100);
/// How long a check or download may stall before it fails.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(60);

/// How this copy of the app gets a new version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum InstallMode {
    /// The updater replaces this bundle and restarts it.
    InApp,
    /// The user downloads the new version from the release page.
    Download,
    /// A package manager installed this copy; [`UpdateInfo::command`] updates it.
    Managed,
}

/// The file a package writes the command that updates it to, e.g. `scoop update kakemono`.
pub const UPDATE_COMMAND_FILE: &str = "update-command";

/// The command from the package's [`UPDATE_COMMAND_FILE`]: beside `exe` (Scoop keeps the whole
/// app in one folder) or in `<exe dir>/../share/kakemono` (`/usr/share/kakemono` for a
/// `/usr/bin` install). Its first line, trimmed; `None` when there is no such file or the line
/// is empty.
pub fn package_update_command(exe: &Path) -> Option<String> {
    let dir = exe.parent()?;
    let candidates = [
        dir.join(UPDATE_COMMAND_FILE),
        dir.join("../share/kakemono").join(UPDATE_COMMAND_FILE),
    ];
    candidates.iter().find_map(|path| {
        let text = std::fs::read_to_string(path).ok()?;
        let line = text.lines().next()?.trim();
        (!line.is_empty()).then(|| line.to_owned())
    })
}

/// This running copy's [`package_update_command`].
fn this_package_update_command() -> Option<String> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| package_update_command(&exe))
}

/// [`InstallMode`] for the bundle this binary came in: a package manager's copy first
/// (`managed`), then every bundle the updater can replace, and the release page otherwise.
/// `None` is a binary run outside a bundle (a dev build): the updater would unpack the bundle
/// beside it.
pub fn install_mode(bundle: Option<BundleType>, managed: bool) -> InstallMode {
    if managed {
        return InstallMode::Managed;
    }
    match bundle {
        Some(
            BundleType::App
            | BundleType::Dmg
            | BundleType::AppImage
            | BundleType::Deb
            | BundleType::Rpm
            | BundleType::Msi
            | BundleType::Nsis,
        ) => InstallMode::InApp,
        None => InstallMode::Download,
    }
}

/// A newer version than this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
pub struct UpdateInfo {
    /// The new version, e.g. `0.3.0`.
    pub version: String,
    /// This app's version.
    pub current: String,
    pub install: InstallMode,
    /// The package manager's update command when `install` is `managed`.
    pub command: Option<String>,
}

/// `install_update`'s progress, sent on its channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum UpdateProgress {
    /// Bytes received so far, and the size when the server sent one.
    Downloading {
        #[ts(type = "number")]
        downloaded: u64,
        #[ts(type = "number | null")]
        total: Option<u64>,
    },
    /// Downloaded and verified; stopping the daemon and installing.
    Installing,
}

/// Turns download chunks into [`UpdateProgress::Downloading`] reports: one per whole percent,
/// or one per MiB when the size is unknown, so the channel is not sent a message per chunk.
#[derive(Debug, Default)]
pub struct DownloadMeter {
    downloaded: u64,
    reported: Option<u64>,
}

impl DownloadMeter {
    /// Adds a chunk of `len` bytes; returns a report when the percent (or MiB) changed.
    pub fn chunk(&mut self, len: usize, total: Option<u64>) -> Option<UpdateProgress> {
        self.downloaded = self
            .downloaded
            .saturating_add(u64::try_from(len).unwrap_or(u64::MAX));
        let step = match total {
            Some(total) if total >= 100 => total / 100,
            Some(_) => 1,
            None => 1 << 20,
        };
        let bucket = self.downloaded / step;
        if self.reported == Some(bucket) {
            return None;
        }
        self.reported = Some(bucket);
        Some(UpdateProgress::Downloading {
            downloaded: self.downloaded,
            total,
        })
    }
}

/// The release page for `version` (`0.3.0` → `…/releases/tag/v0.3.0`), or the latest release.
pub fn release_page(version: Option<&str>) -> String {
    match version {
        Some(version) => format!("{RELEASES_URL}/tag/v{version}"),
        None => format!("{RELEASES_URL}/latest"),
    }
}

/// The update found by the last `check_update`, for `install_update` and `open_update_page`.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

/// A failed update command.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("update: {0}")]
    Updater(#[from] tauri_plugin_updater::Error),
    #[error("no update to install; check for updates again")]
    NoUpdate,
    #[error(
        "this copy of kakemono cannot update itself; download the new version from the release page"
    )]
    NotInApp,
    #[error("stopping the daemon: {0}")]
    Link(#[from] LinkError),
    #[error("the daemon did not stop within {} s", DAEMON_STOP_TIMEOUT.as_secs())]
    DaemonStillRunning,
    #[error("daemon lock {path}: {source}")]
    Lock {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{0}")]
    Opener(String),
    #[error("the install task failed: {0}")]
    Join(String),
}

impl Kinded for UpdateError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Updater(_) | Self::NoUpdate | Self::NotInApp | Self::DaemonStillRunning => {
                ErrorKind::Update
            }
            Self::Link(err) => err.kind(),
            Self::Lock { .. } => ErrorKind::Io,
            Self::Opener(_) => ErrorKind::Opener,
            Self::Join(_) => ErrorKind::Internal,
        }
    }
}

serialize_as_wire_error!(UpdateError);

type UpdateResult<T> = Result<T, UpdateError>;

fn updater<R: Runtime>(app: &AppHandle<R>) -> UpdateResult<tauri_plugin_updater::Updater> {
    Ok(app.updater_builder().timeout(NETWORK_TIMEOUT).build()?)
}

/// `check_update` → the newer version, or `null`.
#[tauri::command]
pub async fn check_update<R: Runtime>(
    app: AppHandle<R>,
    pending: State<'_, PendingUpdate>,
) -> UpdateResult<Option<UpdateInfo>> {
    tracing::debug!("checking for an update");
    // The settings window stays quiet about a failed check, so the log is the only trace of it.
    let update = async { Ok::<_, UpdateError>(updater(&app)?.check().await?) }
        .await
        .inspect_err(|err| tracing::warn!(%err, "update check failed"))?;
    let command = this_package_update_command();
    let mode = install_mode(tauri::utils::platform::bundle_type(), command.is_some());
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current: u.current_version.clone(),
        install: mode,
        command,
    });
    *pending.0.lock().await = update;
    Ok(info)
}

/// `install_update { progress }`: downloads, verifies and installs the update found by the last
/// `check_update`, then restarts the app. Only returns on failure; the daemon is started again
/// if it was stopped.
#[tauri::command]
pub async fn install_update<R: Runtime>(
    app: AppHandle<R>,
    pending: State<'_, PendingUpdate>,
    link: State<'_, Arc<DaemonLink>>,
    progress: Channel<UpdateProgress>,
) -> UpdateResult<()> {
    let managed = this_package_update_command().is_some();
    if install_mode(tauri::utils::platform::bundle_type(), managed) != InstallMode::InApp {
        return Err(UpdateError::NotInApp);
    }
    let update = pending
        .0
        .lock()
        .await
        .clone()
        .ok_or(UpdateError::NoUpdate)?;

    let mut meter = DownloadMeter::default();
    let bytes = update
        .download(
            |len, total| {
                if let Some(report) = meter.chunk(len, total) {
                    let _ = progress.send(report);
                }
            },
            || {},
        )
        .await?;
    let _ = progress.send(UpdateProgress::Installing);

    let link = link.inner().clone();
    let stopped = stop_daemon(&link).await?;
    // Blocking: unpacks the bundle, or waits for pkexec on deb/rpm. On Windows it starts the
    // installer and exits this process.
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes))
        .await
        .map_err(|e| UpdateError::Join(e.to_string()))
        .and_then(|r| r.map_err(UpdateError::from));
    match installed {
        Ok(()) => {
            tracing::info!("update installed; restarting");
            app.restart()
        }
        Err(err) => {
            tracing::warn!(error = %err, "update failed");
            if stopped {
                crate::lifecycle::spawn_at_startup(link);
            }
            Err(err)
        }
    }
}

/// `open_update_page`: the page of the update found by the last `check_update`, else the
/// latest release.
#[tauri::command]
pub async fn open_update_page<R: Runtime>(
    app: AppHandle<R>,
    pending: State<'_, PendingUpdate>,
) -> UpdateResult<()> {
    let version = pending.0.lock().await.as_ref().map(|u| u.version.clone());
    app.opener()
        .open_url(release_page(version.as_deref()), None::<&str>)
        .map_err(|e| UpdateError::Opener(e.to_string()))
}

/// Asks the daemon to quit and waits for its process to end. `Ok(false)`: it was not running.
async fn stop_daemon(link: &DaemonLink) -> UpdateResult<bool> {
    match link.call_alone(Request::Quit).await {
        Ok(_) => {}
        Err(err) if err.is_not_running() => return Ok(false),
        Err(err) => return Err(err.into()),
    }
    let paths = kakemono_config::paths::Paths::detect().map_err(|e| UpdateError::Lock {
        path: kakemono_config::paths::DAEMON_LOCK_FILE_NAME.to_owned(),
        source: io::Error::other(e.to_string()),
    })?;
    let lock = paths
        .runtime_dir
        .join(kakemono_config::paths::DAEMON_LOCK_FILE_NAME);
    wait_until_unlocked(&lock, DAEMON_STOP_TIMEOUT).await?;
    // The lock goes when the daemon's process exits, but Windows may hold the `.exe` open a
    // moment longer.
    #[cfg(windows)]
    tokio::time::sleep(Duration::from_millis(500)).await;
    tracing::info!("daemon stopped for the update");
    Ok(true)
}

/// Waits until nobody holds the daemon lock at `path` (a missing file is free), polling every
/// [`LOCK_POLL`]. The daemon holds it for its whole life, so a free lock means it has exited.
pub async fn wait_until_unlocked(path: &Path, timeout: Duration) -> UpdateResult<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if lock_is_free(path).map_err(|source| UpdateError::Lock {
            path: path.display().to_string(),
            source,
        })? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(UpdateError::DaemonStillRunning);
        }
        tokio::time::sleep(LOCK_POLL).await;
    }
}

/// Takes and releases the lock at `path` without blocking.
fn lock_is_free(path: &Path) -> io::Result<bool> {
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(err),
    };
    match file.try_lock() {
        Ok(()) => {
            file.unlock()?;
            Ok(true)
        }
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_update_in_app_and_bare_binaries_link_to_the_release() {
        for bundle in [
            BundleType::App,
            BundleType::Dmg,
            BundleType::AppImage,
            BundleType::Deb,
            BundleType::Rpm,
            BundleType::Msi,
            BundleType::Nsis,
        ] {
            assert_eq!(
                install_mode(Some(bundle.clone()), false),
                InstallMode::InApp,
                "{bundle:?}"
            );
        }
        assert_eq!(install_mode(None, false), InstallMode::Download);
    }

    #[test]
    fn a_package_manager_wins_over_the_bundle() {
        assert_eq!(
            install_mode(Some(BundleType::Deb), true),
            InstallMode::Managed
        );
        assert_eq!(install_mode(None, true), InstallMode::Managed);
    }

    /// `<root>/bin/kakemono-ui`, as a package lays it out.
    fn exe_in(root: &Path) -> std::path::PathBuf {
        std::fs::create_dir_all(root.join("bin")).unwrap();
        root.join("bin").join("kakemono-ui")
    }

    #[test]
    fn reads_the_update_command_beside_the_binary() {
        let dir = tempfile::tempdir().unwrap();
        let exe = exe_in(dir.path());
        assert_eq!(package_update_command(&exe), None);
        std::fs::write(
            dir.path().join("bin").join(UPDATE_COMMAND_FILE),
            "scoop update kakemono\r\n",
        )
        .unwrap();
        assert_eq!(
            package_update_command(&exe).as_deref(),
            Some("scoop update kakemono")
        );
    }

    #[test]
    fn reads_the_update_command_from_share() {
        let dir = tempfile::tempdir().unwrap();
        let exe = exe_in(dir.path());
        let share = dir.path().join("share").join("kakemono");
        std::fs::create_dir_all(&share).unwrap();
        std::fs::write(
            share.join(UPDATE_COMMAND_FILE),
            "yay -Syu kakemono-bin\n# comment\n",
        )
        .unwrap();
        assert_eq!(
            package_update_command(&exe).as_deref(),
            Some("yay -Syu kakemono-bin")
        );
    }

    #[test]
    fn ignores_an_empty_update_command() {
        let dir = tempfile::tempdir().unwrap();
        let exe = exe_in(dir.path());
        std::fs::write(dir.path().join("bin").join(UPDATE_COMMAND_FILE), "  \n").unwrap();
        assert_eq!(package_update_command(&exe), None);
    }

    #[test]
    fn release_pages() {
        assert_eq!(
            release_page(Some("0.3.0")),
            "https://github.com/RielJ/kakemono/releases/tag/v0.3.0"
        );
        assert_eq!(
            release_page(None),
            "https://github.com/RielJ/kakemono/releases/latest"
        );
    }

    fn downloaded(report: Option<UpdateProgress>) -> Option<u64> {
        match report {
            Some(UpdateProgress::Downloading { downloaded, .. }) => Some(downloaded),
            _ => None,
        }
    }

    #[test]
    fn the_meter_reports_once_per_percent() {
        let mut meter = DownloadMeter::default();
        let total = Some(10_000);
        // First chunk always reports, so the bar appears at once.
        assert_eq!(downloaded(meter.chunk(10, total)), Some(10));
        assert_eq!(meter.chunk(80, total), None, "still under 1 %");
        assert_eq!(downloaded(meter.chunk(10, total)), Some(100), "1 %");
        assert_eq!(meter.chunk(99, total), None);
        assert_eq!(downloaded(meter.chunk(9_801, total)), Some(10_000));
    }

    #[test]
    fn the_meter_reports_per_mib_without_a_size() {
        let mut meter = DownloadMeter::default();
        assert!(meter.chunk(1, None).is_some());
        assert_eq!(meter.chunk((1 << 20) - 2, None), None);
        assert_eq!(downloaded(meter.chunk(1, None)), Some(1 << 20));
    }

    #[test]
    fn the_meter_handles_tiny_sizes() {
        let mut meter = DownloadMeter::default();
        assert!(meter.chunk(5, Some(10)).is_some());
        assert!(meter.chunk(5, Some(10)).is_some());
    }

    #[test]
    fn progress_on_the_wire() {
        let json = serde_json::to_value(UpdateProgress::Downloading {
            downloaded: 5,
            total: None,
        })
        .expect("json");
        assert_eq!(
            json,
            serde_json::json!({ "step": "downloading", "downloaded": 5, "total": null })
        );
        let json = serde_json::to_value(UpdateProgress::Installing).expect("json");
        assert_eq!(json, serde_json::json!({ "step": "installing" }));
    }

    #[test]
    fn errors_have_kinds() {
        assert_eq!(UpdateError::NoUpdate.kind(), ErrorKind::Update);
        assert_eq!(UpdateError::DaemonStillRunning.kind(), ErrorKind::Update);
        assert_eq!(UpdateError::Opener("x".into()).kind(), ErrorKind::Opener);
    }

    #[tokio::test]
    async fn a_missing_lock_file_is_free() {
        let dir = tempfile::tempdir().expect("tempdir");
        wait_until_unlocked(&dir.path().join("daemon.lock"), Duration::ZERO)
            .await
            .expect("free");
    }

    #[tokio::test]
    async fn waits_until_the_holder_lets_go() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("daemon.lock");
        let held = std::fs::File::create(&path).expect("create");
        held.lock().expect("lock");

        let err = wait_until_unlocked(&path, Duration::from_millis(150))
            .await
            .expect_err("held");
        assert!(matches!(err, UpdateError::DaemonStillRunning), "{err}");

        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        wait_until_unlocked(&path, Duration::from_secs(5))
            .await
            .expect("released");
        release.join().expect("thread");
        // The waiter let go of it too.
        let again = std::fs::File::open(&path).expect("open");
        again.try_lock().expect("free after the wait");
    }
}
