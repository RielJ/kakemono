//! Windows: a value named `kakemono-daemon` under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (written with auto-launch), whose command
//! is `"<exe>" daemon`.
//!
//! Plain `daemon` (no `-f`) re-execs itself detached with `CREATE_NO_WINDOW` and logs to
//! `log_dir`; `-f` would keep a console window open for the whole session. The name differs from
//! the UI's own Run value (`kakemono`, tauri-plugin-autostart). There is no service manager, so
//! [`status`](super::ServiceManager::status) reports [`Running::Unknown`] and callers ask the
//! daemon's socket.
//!
//! The pure parts (the command line, the install/uninstall/status logic over a [`RunKey`])
//! compile and are tested on every OS; only [`HkcuRunKey`] is Windows-only.

use std::io;
use std::path::Path;
use std::sync::Arc;
#[cfg(any(test, feature = "fake"))]
use std::sync::{Mutex, PoisonError};

use super::{Installed, Running, ServiceEnv, ServiceError, ServiceStatus, Uninstalled};

/// The Run value name.
pub const RUN_VALUE_NAME: &str = "kakemono-daemon";
/// The registry key holding [`RUN_VALUE_NAME`].
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// `HKCU\...\Run\kakemono-daemon`, for messages.
pub fn run_value_location() -> String {
    format!(r"{RUN_KEY}\{RUN_VALUE_NAME}")
}

/// The program part of the Run value: the exe in double quotes (paths often contain spaces).
pub fn quoted_program(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// The arguments after the program.
pub const RUN_ARGS: [&str; 1] = ["daemon"];

/// The full Run value: `"<exe>" daemon`.
pub fn run_command(exe: &Path) -> String {
    format!("{} {}", quoted_program(exe), RUN_ARGS.join(" "))
}

/// The `kakemono-daemon` Run value. Object-safe so tests can use `MemoryRunKey`.
pub trait RunKey: Send + Sync + 'static {
    /// Set and not disabled in Task Manager's Startup tab.
    fn is_enabled(&self) -> io::Result<bool>;
    /// Writes `<program> <args...>` (and re-enables it in Task Manager).
    fn enable(&self, program: &str, args: &[&str]) -> io::Result<()>;
    /// Removes the value. Succeeds when it is missing.
    fn disable(&self) -> io::Result<()>;
}

/// In-memory [`RunKey`] for tests (feature `fake`). Clones share state.
#[cfg(any(test, feature = "fake"))]
#[derive(Clone, Debug, Default)]
pub struct MemoryRunKey {
    value: Arc<Mutex<Option<String>>>,
}

#[cfg(any(test, feature = "fake"))]
impl MemoryRunKey {
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored command line.
    pub fn value(&self) -> Option<String> {
        self.value
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(any(test, feature = "fake"))]
impl RunKey for MemoryRunKey {
    fn is_enabled(&self) -> io::Result<bool> {
        Ok(self.value().is_some())
    }

    fn enable(&self, program: &str, args: &[&str]) -> io::Result<()> {
        *self.value.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(format!("{program} {}", args.join(" ")));
        Ok(())
    }

    fn disable(&self) -> io::Result<()> {
        *self.value.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(())
    }
}

/// A [`RunKey`] that fails: the Windows registry on any other OS.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRunKey;

impl RunKey for NoRunKey {
    fn is_enabled(&self) -> io::Result<bool> {
        Err(unsupported())
    }

    fn enable(&self, _: &str, _: &[&str]) -> io::Result<()> {
        Err(unsupported())
    }

    fn disable(&self) -> io::Result<()> {
        Err(unsupported())
    }
}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the Windows registry is only available on Windows",
    )
}

/// The real HKCU Run value, via auto-launch (`WindowsEnableMode::CurrentUser`).
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Default)]
pub struct HkcuRunKey;

#[cfg(target_os = "windows")]
impl HkcuRunKey {
    fn launcher(program: &str, args: &[&str]) -> auto_launch::AutoLaunch {
        auto_launch::AutoLaunch::new(
            RUN_VALUE_NAME,
            program,
            auto_launch::WindowsEnableMode::CurrentUser,
            args,
        )
    }
}

#[cfg(target_os = "windows")]
impl RunKey for HkcuRunKey {
    fn is_enabled(&self) -> io::Result<bool> {
        Self::launcher("", &[])
            .is_enabled()
            .map_err(io::Error::other)
    }

    fn enable(&self, program: &str, args: &[&str]) -> io::Result<()> {
        Self::launcher(program, args)
            .enable()
            .map_err(io::Error::other)
    }

    fn disable(&self) -> io::Result<()> {
        Self::launcher("", &[]).disable().map_err(io::Error::other)
    }
}

/// [`HkcuRunKey`] on Windows, [`NoRunKey`] elsewhere.
pub fn system_run_key() -> Arc<dyn RunKey> {
    #[cfg(target_os = "windows")]
    return Arc::new(HkcuRunKey);
    #[cfg(not(target_os = "windows"))]
    return Arc::new(NoRunKey);
}

pub(super) fn install(env: &ServiceEnv, key: &dyn RunKey) -> Result<Installed, ServiceError> {
    key.enable(&quoted_program(&env.exe), &RUN_ARGS)
        .map_err(ServiceError::Registry)?;
    Ok(Installed {
        path: None,
        location: run_value_location(),
        command: run_command(&env.exe),
        started: false,
    })
}

pub(super) fn uninstall(key: &dyn RunKey) -> Result<Uninstalled, ServiceError> {
    let was_enabled = key.is_enabled().map_err(ServiceError::Registry)?;
    // Disable regardless: a value disabled in Task Manager reads as not enabled but still exists.
    key.disable().map_err(ServiceError::Registry)?;
    Ok(Uninstalled {
        removed: was_enabled,
        location: run_value_location(),
    })
}

pub(super) fn status(key: &dyn RunKey) -> Result<ServiceStatus, ServiceError> {
    Ok(ServiceStatus {
        installed: key.is_enabled().map_err(ServiceError::Registry)?,
        running: Running::Unknown,
        path: None,
        location: run_value_location(),
    })
}
