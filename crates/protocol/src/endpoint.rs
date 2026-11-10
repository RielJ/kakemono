//! Where the daemon listens.
//!
//! Precedence: the explicit `socket_override` (CLI `--socket` or config `socket_path`), then
//! the `KAKEMONO_SOCKET` environment variable, then the platform default:
//!
//! | OS | Default |
//! |---|---|
//! | Unix | `<runtime_dir>/kakemono.sock` |
//! | Windows | `\\.\pipe\kakemono-<user>` |
//!
//! The caller passes `runtime_dir`, which is `kakemono_config::paths::Paths::runtime_dir`: the
//! folder that also holds the daemon lock. That type owns the XDG and home rules, so the socket
//! and the lock can't end up in different folders.
//!
//! [`default_endpoint`] has no filesystem side effects. The server creates the directory
//! (mode 0700) when it binds, through [`Endpoint::prepare`] or [`Endpoint::bind`].

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use interprocess::local_socket::{GenericFilePath, ListenerOptions, Name, ToFsName};

/// Environment variable that overrides the default endpoint.
pub const SOCKET_ENV: &str = "KAKEMONO_SOCKET";

const APP_DIR: &str = "kakemono";
/// File name of the default Unix socket inside the runtime directory.
pub const SOCKET_FILE: &str = "kakemono.sock";
const PIPE_PREFIX: &str = r"\\.\pipe\";

/// A local-socket endpoint: a Unix socket path, or a Windows named-pipe path
/// (`\\.\pipe\...`). Construction validates the path for the current OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    path: PathBuf,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EndpointError {
    /// The Unix socket path does not fit in `sockaddr_un.sun_path`.
    #[error(
        "socket path is {len} bytes, over the {max}-byte limit for Unix sockets on this OS: {}; \
         set socket_path in config.toml or {SOCKET_ENV} to a shorter path",
        path.display()
    )]
    PathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },
    /// Windows endpoints must be named pipes.
    #[error("socket path must be a named pipe (\\\\.\\pipe\\<name>) on Windows: {}", .0.display())]
    NotAPipe(PathBuf),
    /// No runtime directory to build the default Unix socket path from.
    #[error(
        "cannot determine the runtime directory for the default socket path; \
         set socket_path in config.toml or {SOCKET_ENV}"
    )]
    NoRuntimeDir,
}

/// A socket path that [`Endpoint::new`] accepts on this OS, for tests: `dir/name` on Unix, and a
/// named pipe on Windows, made unique by the last component of `dir` (a temp dir), so parallel
/// tests don't share a pipe.
#[cfg(any(test, feature = "test-support"))]
pub fn test_socket_path(dir: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        let tag = dir.file_name().unwrap_or_default().to_string_lossy();
        PathBuf::from(format!("{PIPE_PREFIX}kakemono-test-{tag}-{name}"))
    } else {
        dir.join(name)
    }
}

impl Endpoint {
    /// Wraps `path` after checking it is usable on this OS: a Unix socket path short enough
    /// for `sun_path`, or a `\\.\pipe\` path on Windows.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, EndpointError> {
        Self::for_os(Os::current(), path.into())
    }

    fn for_os(os: Os, path: PathBuf) -> Result<Self, EndpointError> {
        match os {
            Os::Windows => {
                if !path.to_string_lossy().starts_with(PIPE_PREFIX) {
                    return Err(EndpointError::NotAPipe(path));
                }
            }
            Os::Mac | Os::Linux | Os::OtherUnix => {
                let len = path.as_os_str().len();
                let max = os.max_socket_path_bytes();
                if len > max {
                    return Err(EndpointError::PathTooLong { path, len, max });
                }
            }
        }
        Ok(Self { path })
    }

    /// The socket path or pipe path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The interprocess name for connecting or listening.
    pub fn name(&self) -> io::Result<Name<'_>> {
        self.path.as_os_str().to_fs_name::<GenericFilePath>()
    }

    /// Creates the socket's missing parent directories with mode 0700 (Unix). Existing
    /// directories keep their permissions. No-op on Windows.
    pub fn prepare(&self) -> io::Result<()> {
        #[cfg(unix)]
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        Ok(())
    }

    /// [`prepare`](Self::prepare), listen, then (Unix) set the socket file to 0600.
    ///
    /// Does not remove a stale socket: the daemon does that only while holding its
    /// instance lock. Needs a running tokio runtime.
    pub fn bind(&self) -> io::Result<interprocess::local_socket::tokio::Listener> {
        self.prepare()?;
        let listener = self.listen_private()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(listener)
    }
}

impl Endpoint {
    /// Listens on the socket, which (Unix) has no group or other permissions from the moment it
    /// exists. Where the OS can set a socket's mode before `bind` (Linux, the BSDs) that sets
    /// 0600; elsewhere (macOS) the socket is created under umask 077, restored right after.
    #[cfg(unix)]
    fn listen_private(&self) -> io::Result<interprocess::local_socket::tokio::Listener> {
        use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
        match ListenerOptions::new()
            .name(self.name()?)
            .mode(0o600)
            .create_tokio()
        {
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {}
            other => return other,
        }
        let _umask = Umask::set(0o077);
        ListenerOptions::new().name(self.name()?).create_tokio()
    }

    /// Listens on the named pipe. Its default ACL already limits it to this user.
    #[cfg(not(unix))]
    fn listen_private(&self) -> io::Result<interprocess::local_socket::tokio::Listener> {
        ListenerOptions::new().name(self.name()?).create_tokio()
    }
}

/// Sets the process umask and restores the previous one on drop. The umask is process-wide, so
/// a file another thread creates meanwhile gets at most stricter permissions.
#[cfg(unix)]
struct Umask(nix::sys::stat::Mode);

#[cfg(unix)]
impl Umask {
    fn set(bits: nix::libc::mode_t) -> Self {
        Self(nix::sys::stat::umask(
            nix::sys::stat::Mode::from_bits_truncate(bits),
        ))
    }
}

#[cfg(unix)]
impl Drop for Umask {
    fn drop(&mut self) {
        nix::sys::stat::umask(self.0);
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path.display().fmt(f)
    }
}

/// Resolves the endpoint: `socket_override`, then `$KAKEMONO_SOCKET`, then the platform
/// default (module docs). `runtime_dir` is `Paths::runtime_dir` from `kakemono-config`, or
/// `None` when it can't be determined; it is only used for the Unix default. Reads the
/// environment only; touches no files.
///
/// # Errors
///
/// [`EndpointError::NoRuntimeDir`] when the Unix default is needed and `runtime_dir` is `None`,
/// and the [`Endpoint::new`] errors for the chosen path.
pub fn default_endpoint(
    socket_override: Option<&Path>,
    runtime_dir: Option<&Path>,
) -> Result<Endpoint, EndpointError> {
    resolve(
        Os::current(),
        socket_override,
        |k| std::env::var_os(k),
        runtime_dir,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Mac,
    Linux,
    OtherUnix,
    Windows,
}

impl Os {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(any(target_os = "linux", target_os = "android")) {
            Self::Linux
        } else {
            Self::OtherUnix
        }
    }

    /// `sizeof(sun_path)` minus the NUL terminator: 104 on macOS and the BSDs, 108 on
    /// Linux.
    fn max_socket_path_bytes(self) -> usize {
        match self {
            Self::Linux => 107,
            Self::Mac | Self::OtherUnix | Self::Windows => 103,
        }
    }
}

fn resolve(
    os: Os,
    socket_override: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
    runtime_dir: Option<&Path>,
) -> Result<Endpoint, EndpointError> {
    let nonempty = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(p) = socket_override.filter(|p| !p.as_os_str().is_empty()) {
        return Endpoint::for_os(os, p.to_path_buf());
    }
    if let Some(p) = nonempty(SOCKET_ENV) {
        return Endpoint::for_os(os, PathBuf::from(p));
    }
    let path = match os {
        Os::Windows => {
            let user = nonempty("USERNAME")
                .map(|u| u.to_string_lossy().replace(['\\', '/'], "_"))
                .unwrap_or_else(|| "default".to_owned());
            PathBuf::from(format!("{PIPE_PREFIX}{APP_DIR}-{user}"))
        }
        Os::Mac | Os::Linux | Os::OtherUnix => runtime_dir
            .filter(|d| !d.as_os_str().is_empty())
            .map(|d| d.join(SOCKET_FILE))
            .ok_or(EndpointError::NoRuntimeDir)?,
    };
    Endpoint::for_os(os, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn run(
        os: Os,
        over: Option<&str>,
        vars: &[(&str, &str)],
        runtime_dir: Option<&str>,
    ) -> Result<PathBuf, EndpointError> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        resolve(
            os,
            over.map(Path::new),
            |k| vars.get(k).cloned(),
            runtime_dir.map(Path::new),
        )
        .map(|e| e.path)
    }

    /// Every Unix uses the runtime dir it is given; the XDG and home rules live in
    /// `kakemono_config::paths::Paths`, not here.
    #[test]
    fn unix_default_is_in_the_runtime_dir() {
        for os in [Os::Mac, Os::Linux, Os::OtherUnix] {
            assert_eq!(
                run(
                    os,
                    None,
                    &[("XDG_RUNTIME_DIR", "/run/user/1"), ("HOME", "/Users/a")],
                    Some("/x/kakemono")
                ),
                Ok(PathBuf::from("/x/kakemono/kakemono.sock")),
                "{os:?}"
            );
        }
    }

    #[test]
    fn windows_pipe_uses_username_and_ignores_the_runtime_dir() {
        assert_eq!(
            run(
                Os::Windows,
                None,
                &[("USERNAME", "Ana")],
                Some(r"C:\Users\Ana\AppData\Local\kakemono\run")
            ),
            Ok(PathBuf::from(r"\\.\pipe\kakemono-Ana"))
        );
        assert_eq!(
            run(Os::Windows, None, &[], None),
            Ok(PathBuf::from(r"\\.\pipe\kakemono-default"))
        );
    }

    #[test]
    fn precedence_override_then_env_then_default() {
        let env = [("KAKEMONO_SOCKET", "/tmp/env.sock")];
        let rt = Some("/Users/a/.config/kakemono");
        assert_eq!(
            run(Os::Mac, Some("/tmp/flag.sock"), &env, rt),
            Ok(PathBuf::from("/tmp/flag.sock"))
        );
        assert_eq!(
            run(Os::Mac, None, &env, rt),
            Ok(PathBuf::from("/tmp/env.sock"))
        );
        assert_eq!(
            run(Os::Mac, Some(""), &[("KAKEMONO_SOCKET", "")], rt),
            Ok(PathBuf::from("/Users/a/.config/kakemono/kakemono.sock")),
            "empty values are ignored"
        );
    }

    #[test]
    fn an_override_needs_no_runtime_dir() {
        assert_eq!(
            run(Os::Linux, Some("/tmp/flag.sock"), &[], None),
            Ok(PathBuf::from("/tmp/flag.sock"))
        );
        assert_eq!(
            run(
                Os::Linux,
                None,
                &[("KAKEMONO_SOCKET", "/tmp/env.sock")],
                None
            ),
            Ok(PathBuf::from("/tmp/env.sock"))
        );
    }

    #[test]
    fn windows_rejects_non_pipe_override() {
        assert_eq!(
            run(Os::Windows, Some(r"C:\x.sock"), &[], None),
            Err(EndpointError::NotAPipe(PathBuf::from(r"C:\x.sock")))
        );
    }

    #[test]
    fn missing_runtime_dir_is_a_typed_error() {
        assert_eq!(
            run(Os::Mac, None, &[], None),
            Err(EndpointError::NoRuntimeDir)
        );
        assert_eq!(
            run(Os::Linux, None, &[], Some("")),
            Err(EndpointError::NoRuntimeDir)
        );
    }

    #[test]
    fn a_long_runtime_dir_exceeds_sun_path_with_typed_error() {
        // "/kakemono.sock" is 14 bytes; 103 is the macOS maximum.
        let suffix = "/kakemono.sock".len();
        let fits = format!("/{}", "h".repeat(103 - suffix - 1));
        let path = run(Os::Mac, None, &[], Some(&fits)).expect("103 bytes fits");
        assert_eq!(path.as_os_str().len(), 103);

        let long = format!("/{}", "h".repeat(103 - suffix));
        let err = run(Os::Mac, None, &[], Some(&long)).expect_err("104 bytes is too long");
        let want_path = PathBuf::from(format!("{long}/kakemono.sock"));
        assert_eq!(
            err,
            EndpointError::PathTooLong {
                path: want_path.clone(),
                len: 104,
                max: 103
            }
        );
        let msg = err.to_string();
        assert!(msg.contains(&*want_path.to_string_lossy()), "{msg}");
        assert!(msg.contains("KAKEMONO_SOCKET"), "{msg}");
    }

    #[test]
    fn linux_allows_up_to_107_bytes() {
        let over = format!("/{}", "x".repeat(106));
        assert!(run(Os::Linux, Some(&over), &[], None).is_ok());
        let over = format!("/{}", "x".repeat(107));
        assert!(matches!(
            run(Os::Linux, Some(&over), &[], None),
            Err(EndpointError::PathTooLong {
                len: 108,
                max: 107,
                ..
            })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn prepare_creates_missing_dirs_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ep = Endpoint::new(tmp.path().join("a/b/k.sock")).expect("endpoint");
        ep.prepare().expect("prepare");
        for d in ["a", "a/b"] {
            let mode = std::fs::metadata(tmp.path().join(d))
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{d}");
        }
    }

    /// The socket is private from the moment it exists, even under a permissive umask: no
    /// window where another user could connect before the chmod.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_socket_is_created_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ep = Endpoint::new(tmp.path().join("k.sock")).expect("endpoint");
        let old = nix::sys::stat::umask(nix::sys::stat::Mode::empty());
        let listener = ep.listen_private();
        nix::sys::stat::umask(old);
        let _listener = listener.expect("listen");
        let mode = std::fs::symlink_metadata(ep.path())
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
}
