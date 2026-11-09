//! Where the daemon listens (plan §I).
//!
//! Precedence: the explicit `socket_override` (CLI `--socket` or config `socket_path`), then
//! the `KAKEMONO_SOCKET` environment variable, then the platform default:
//!
//! | OS | Default |
//! |---|---|
//! | macOS | `~/.config/kakemono/kakemono.sock` |
//! | Linux and other Unix | `$XDG_RUNTIME_DIR/kakemono/kakemono.sock`, else `~/.config/kakemono/kakemono.sock` |
//! | Windows | `\\.\pipe\kakemono-<user>` |
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
const SOCKET_FILE: &str = "kakemono.sock";
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
    /// No home directory to build the default path from.
    #[error(
        "cannot determine the home directory for the default socket path; \
         set socket_path in config.toml or {SOCKET_ENV}"
    )]
    NoHome,
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
        let listener = ListenerOptions::new().name(self.name()?).create_tokio()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(listener)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path.display().fmt(f)
    }
}

/// Resolves the endpoint: `socket_override`, then `$KAKEMONO_SOCKET`, then the platform
/// default (module docs). Reads the environment only; touches no files.
pub fn default_endpoint(socket_override: Option<&Path>) -> Result<Endpoint, EndpointError> {
    resolve(
        Os::current(),
        socket_override,
        |k| std::env::var_os(k),
        std::env::home_dir(),
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
    home: Option<PathBuf>,
) -> Result<Endpoint, EndpointError> {
    let nonempty = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(p) = socket_override.filter(|p| !p.as_os_str().is_empty()) {
        return Endpoint::for_os(os, p.to_path_buf());
    }
    if let Some(p) = nonempty(SOCKET_ENV) {
        return Endpoint::for_os(os, PathBuf::from(p));
    }
    let home_default = || {
        home.filter(|h| !h.as_os_str().is_empty())
            .map(|h| h.join(".config").join(APP_DIR).join(SOCKET_FILE))
            .ok_or(EndpointError::NoHome)
    };
    let path = match os {
        Os::Windows => {
            let user = nonempty("USERNAME")
                .map(|u| u.to_string_lossy().replace(['\\', '/'], "_"))
                .unwrap_or_else(|| "default".to_owned());
            PathBuf::from(format!("{PIPE_PREFIX}{APP_DIR}-{user}"))
        }
        Os::Mac => home_default()?,
        Os::Linux | Os::OtherUnix => match nonempty("XDG_RUNTIME_DIR").map(PathBuf::from) {
            Some(dir) if dir.is_absolute() => dir.join(APP_DIR).join(SOCKET_FILE),
            _ => home_default()?,
        },
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
        home: Option<&str>,
    ) -> Result<PathBuf, EndpointError> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        resolve(
            os,
            over.map(Path::new),
            |k| vars.get(k).cloned(),
            home.map(PathBuf::from),
        )
        .map(|e| e.path)
    }

    #[test]
    fn macos_default_is_under_home_config() {
        assert_eq!(
            run(
                Os::Mac,
                None,
                &[("XDG_RUNTIME_DIR", "/run/user/1")],
                Some("/Users/a")
            ),
            Ok(PathBuf::from("/Users/a/.config/kakemono/kakemono.sock"))
        );
    }

    #[test]
    fn linux_prefers_xdg_runtime_dir() {
        assert_eq!(
            run(
                Os::Linux,
                None,
                &[("XDG_RUNTIME_DIR", "/run/user/1000")],
                Some("/home/a")
            ),
            Ok(PathBuf::from("/run/user/1000/kakemono/kakemono.sock"))
        );
    }

    #[test]
    fn linux_falls_back_to_home_when_xdg_runtime_dir_unset_empty_or_relative() {
        let want = Ok(PathBuf::from("/home/a/.config/kakemono/kakemono.sock"));
        for vars in [
            &[][..],
            &[("XDG_RUNTIME_DIR", "")][..],
            &[("XDG_RUNTIME_DIR", "run")][..],
        ] {
            assert_eq!(
                run(Os::Linux, None, vars, Some("/home/a")),
                want,
                "{vars:?}"
            );
            assert_eq!(
                run(Os::OtherUnix, None, vars, Some("/home/a")),
                want,
                "{vars:?}"
            );
        }
    }

    #[test]
    fn windows_pipe_uses_username() {
        assert_eq!(
            run(Os::Windows, None, &[("USERNAME", "Ana")], None),
            Ok(PathBuf::from(r"\\.\pipe\kakemono-Ana"))
        );
        assert_eq!(
            run(Os::Windows, None, &[("USERNAME", r"DOM\ana")], None),
            Ok(PathBuf::from(r"\\.\pipe\kakemono-DOM_ana"))
        );
        assert_eq!(
            run(Os::Windows, None, &[], None),
            Ok(PathBuf::from(r"\\.\pipe\kakemono-default"))
        );
    }

    #[test]
    fn precedence_override_then_env_then_default() {
        let env = [("KAKEMONO_SOCKET", "/tmp/env.sock")];
        assert_eq!(
            run(Os::Mac, Some("/tmp/flag.sock"), &env, Some("/Users/a")),
            Ok(PathBuf::from("/tmp/flag.sock"))
        );
        assert_eq!(
            run(Os::Mac, None, &env, Some("/Users/a")),
            Ok(PathBuf::from("/tmp/env.sock"))
        );
        assert_eq!(
            run(
                Os::Mac,
                Some(""),
                &[("KAKEMONO_SOCKET", "")],
                Some("/Users/a")
            ),
            Ok(PathBuf::from("/Users/a/.config/kakemono/kakemono.sock")),
            "empty values are ignored"
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
    fn missing_home_is_a_typed_error() {
        assert_eq!(run(Os::Mac, None, &[], None), Err(EndpointError::NoHome));
        assert_eq!(
            run(Os::Linux, None, &[], Some("")),
            Err(EndpointError::NoHome)
        );
    }

    #[test]
    fn macos_long_home_exceeds_sun_path_with_typed_error() {
        // "/.config/kakemono/kakemono.sock" is 31 bytes; 103 is the macOS maximum.
        let suffix = "/.config/kakemono/kakemono.sock".len();
        let fits = format!("/{}", "h".repeat(103 - suffix - 1));
        let path = run(Os::Mac, None, &[], Some(&fits)).expect("103 bytes fits");
        assert_eq!(path.as_os_str().len(), 103);

        let long = format!("/{}", "h".repeat(103 - suffix));
        let err = run(Os::Mac, None, &[], Some(&long)).expect_err("104 bytes is too long");
        let want_path = PathBuf::from(format!("{long}/.config/kakemono/kakemono.sock"));
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
}
