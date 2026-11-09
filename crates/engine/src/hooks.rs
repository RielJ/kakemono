//! `[hooks] on_change` (plan §I).
//!
//! After every wallpaper change the engine runs the user's `on_change` argv:
//! - No shell. The argv is rendered with [`kakemono_platform::exec::render`], so `{path}`,
//!   `{uri}`, `{fit}` and `{theme}` work as in `set_command` and a value never splits a token.
//! - The environment is cleared. Only [`PASS_THROUGH_ENV`] is copied from the daemon, plus
//!   `KAKEMONO_PATH`, `KAKEMONO_SOURCE`, `KAKEMONO_TITLE`, `KAKEMONO_AUTHOR`,
//!   `KAKEMONO_SOURCE_URL` and `KAKEMONO_THEME`.
//! - Fire-and-forget with a [`HOOK_TIMEOUT`]: the wallpaper change never waits for the hook,
//!   and a hook that fails, times out or can't start is only logged at `warn`.
//! - At most [`MAX_CONCURRENT_HOOKS`] run at once. A change that finds every slot taken (its
//!   previous runs are still going) drops its run and logs at `warn`.
//!
//! Go had no hooks (the README's sketchybar trigger was never implemented), so there is no Go
//! behaviour to match.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kakemono_platform::api::{Fit, SetError, Theme};
use kakemono_platform::exec::{self, CommandRunner, Invocation, RenderError, RunError, Vars};
use kakemono_protocol::types::WallpaperInfo;
use tokio::sync::Semaphore;

use crate::engine::Engine;

/// How long a hook may run before it is killed.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// How many hooks may run at once.
pub const MAX_CONCURRENT_HOOKS: usize = 4;

/// Variables copied from the daemon's environment (when set and UTF-8). Everything else is
/// cleared.
#[cfg(not(windows))]
pub const PASS_THROUGH_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// Variables copied from the daemon's environment (when set and UTF-8). Everything else is
/// cleared.
#[cfg(windows)]
pub const PASS_THROUGH_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "SystemRoot",
    "USERPROFILE",
];

/// Why a hook could not run, or failed.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    /// The wallpaper path could not be made absolute (no current directory).
    #[error("hook: resolve path {path:?}: {source}")]
    Path {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// Bad argv template (unknown placeholder) or a path the placeholders can't hold.
    #[error("hook: {0}")]
    Render(#[from] RenderError),
    /// The command could not start.
    #[error("hook: {0}")]
    Run(#[from] RunError),
    /// The command ran and failed or timed out.
    #[error("hook: {0}")]
    Failed(#[from] SetError),
}

/// Per-engine hook state: the concurrency limit. Lives on `Inner`.
pub(crate) struct HookSlots(Arc<Semaphore>);

impl Default for HookSlots {
    fn default() -> Self {
        Self(Arc::new(Semaphore::new(MAX_CONCURRENT_HOOKS)))
    }
}

/// Builds the hook invocation for `info`: rendered argv, cleared env with the pass-through
/// variables (looked up with `parent_env`) and the `KAKEMONO_*` variables, [`HOOK_TIMEOUT`].
///
/// A relative `info.path` (`set` stores the path as given) is made absolute against the current
/// directory, for both `{path}` and `KAKEMONO_PATH`.
pub fn invocation(
    spec: &[String],
    info: &WallpaperInfo,
    theme: Theme,
    fit: Fit,
    parent_env: impl Fn(&str) -> Option<String>,
) -> Result<Invocation, HookError> {
    exec::validate(spec)?;
    let path = std::path::absolute(Path::new(&info.path)).map_err(|source| HookError::Path {
        path: info.path.clone(),
        source,
    })?;
    let vars = Vars::new(&path, fit, theme)?;
    let argv = exec::render(spec, &vars);
    let passed = PASS_THROUGH_ENV
        .iter()
        .filter_map(|&name| parent_env(name).map(|value| (name, value)));
    Ok(Invocation::new(argv)
        .clear_env()
        .envs(passed)
        .env("KAKEMONO_PATH", vars.path())
        .env("KAKEMONO_SOURCE", info.source.as_str())
        .env("KAKEMONO_TITLE", info.title.as_str())
        .env("KAKEMONO_AUTHOR", info.author.as_str())
        .env("KAKEMONO_SOURCE_URL", info.source_url.as_str())
        .env("KAKEMONO_THEME", theme.as_str())
        .timeout(HOOK_TIMEOUT))
}

/// Runs `invocation` to completion (or its timeout) and logs the outcome: `debug` on success,
/// `warn` on any failure. Never returns an error; nothing waits on a hook.
pub async fn fire(invocation: Invocation, runner: &dyn CommandRunner) {
    let argv0 = invocation.argv0().to_owned();
    let result = match runner.run(&invocation).await {
        Ok(output) => output.check(&argv0).map(drop).map_err(HookError::from),
        Err(e) => Err(HookError::from(e)),
    };
    match result {
        Ok(()) => tracing::debug!(argv0 = %argv0, "on_change hook finished"),
        Err(e) => tracing::warn!(argv0 = %argv0, error = %e, "on_change hook failed"),
    }
}

/// Called by the engine after every wallpaper change, with the op lock still held. Never
/// blocks or awaits: builds the invocation, takes a slot and spawns [`fire`].
pub(crate) fn on_change(engine: &Engine, info: &WallpaperInfo, theme: Theme) {
    let cfg = engine.config();
    let Some(spec) = cfg.hooks.on_change.as_ref() else {
        return;
    };
    let invocation = match invocation(spec.argv(), info, theme, cfg.desktop.fit, |name| {
        std::env::var(name).ok()
    }) {
        Ok(invocation) => invocation,
        Err(e) => {
            tracing::warn!(error = %e, "on_change hook not run");
            return;
        }
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("on_change hook not run: no tokio runtime");
        return;
    };
    let Ok(permit) = Arc::clone(&engine.inner.hooks.0).try_acquire_owned() else {
        tracing::warn!(
            argv0 = %invocation.argv0(),
            limit = MAX_CONCURRENT_HOOKS,
            "on_change hook dropped: previous runs still going"
        );
        return;
    };
    let runner = Arc::clone(&engine.inner.deps.runner);
    runtime.spawn(async move {
        let _permit = permit;
        fire(invocation, runner.as_ref()).await;
    });
}
