//! Helpers for the processes the CLI starts: the background daemon (`daemon`) and the UI
//! (`pick`, `settings`).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;

/// `path` made absolute against the current dir, for a child that may run elsewhere.
pub fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))
}

/// Windows process creation flags (`winbase.h`).
#[cfg(windows)]
mod flags {
    pub const DETACHED_PROCESS: u32 = 0x0000_0008;
    pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
}

/// How far a child is cut off from this terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detach {
    /// A new session (`setsid`) on Unix, so closing the terminal (SIGHUP) does not reach it;
    /// no console window on Windows. For the daemon.
    Session,
    /// A new process group on Unix, so Ctrl-C in this terminal does not reach it. For the UI.
    Group,
}

/// Starts `cmd`'s child detached from this terminal, as `how` says.
#[cfg(unix)]
pub fn detach(cmd: &mut Command, how: Detach) {
    use std::os::unix::process::CommandExt as _;
    match how {
        Detach::Session => {
            // SAFETY: `setsid` is async-signal-safe and touches no memory of the parent.
            unsafe {
                cmd.pre_exec(|| {
                    nix::unistd::setsid()
                        .map(drop)
                        .map_err(std::io::Error::from)
                });
            }
        }
        Detach::Group => {
            cmd.process_group(0);
        }
    }
}

/// Starts `cmd`'s child detached from this console, as `how` says.
#[cfg(windows)]
pub fn detach(cmd: &mut Command, how: Detach) {
    use flags::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS};
    use std::os::windows::process::CommandExt as _;
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    cmd.creation_flags(match how {
        Detach::Session => base | CREATE_NO_WINDOW,
        Detach::Group => base,
    });
}
