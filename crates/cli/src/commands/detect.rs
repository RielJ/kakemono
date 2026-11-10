//! `detect [--write [--force]]`: what backend detection picks in this session.
//!
//! Detection ignores a pinned `[desktop] backend`, so the output answers "what would `auto`
//! choose here", and the `in use` line shows what the daemon actually resolves. `--write` saves
//! the detected backend to `[desktop]` (toml_edit; comments are kept). It refuses when
//! `managed_by = "user"` unless `--force` is given; `managed_by` itself is never changed.
//!
//! Linux only. Elsewhere it prints the native backend and saves nothing. The hidden
//! `--env-json <file>` replaces the live session capture with a saved [`Env`] (tests, bug
//! reports); it also makes the Linux resolver run on macOS and Windows.

use std::io::Write;
use std::path::Path;

use anyhow::{Context as _, bail};
use chrono::Utc;
use kakemono_config::paths::ConfigSource;
use kakemono_config::{Backend, DesktopConfig, ManagedBy};
use kakemono_platform::api::BackendInfo;
use kakemono_platform::linux::detect::{self, BackendKind, Env, Origin, Resolution};

use crate::cli::Context;
use crate::firstrun;

pub async fn run(
    ctx: &Context,
    out: &mut dyn Write,
    write: bool,
    force: bool,
    env_json: Option<&Path>,
) -> anyhow::Result<()> {
    let (paths, _) = ctx.config_source()?;
    let (cfg, source) = paths
        .load_config(ctx.global.config.as_deref())
        .context("loading config")?;
    let Some(env) = session_env(env_json).await? else {
        let native = native_backend();
        writeln!(out, "backend:   {} ({})", native.name, native.detail)?;
        writeln!(
            out,
            "Backend detection is only used on Linux; {} always uses its native backend.",
            os_name()
        )?;
        if write {
            writeln!(out, "Nothing to save.")?;
        }
        return Ok(());
    };

    let res = detect::resolve(&env, &detection_only(&cfg.desktop));
    let in_use = detect::resolve(&env, &cfg.desktop);
    write_report(out, &res, &in_use, &cfg.desktop)?;
    if write {
        save(out, &res, &cfg.desktop, &source, force)?;
    }
    Ok(())
}

/// `cfg` with the backend left to detection.
fn detection_only(cfg: &DesktopConfig) -> DesktopConfig {
    DesktopConfig {
        backend: Backend::Auto,
        managed_by: ManagedBy::Detect,
        detected_session: None,
        detected_at: None,
        ..cfg.clone()
    }
}

fn write_report(
    out: &mut dyn Write,
    res: &Resolution,
    in_use: &Resolution,
    cfg: &DesktopConfig,
) -> anyhow::Result<()> {
    writeln!(out, "session:   {}", res.session_key)?;
    writeln!(out, "detected:  {}", res.backend)?;
    if res.persistent {
        writeln!(out, "           (keeps running; kakemono supervises it)")?;
    }
    writeln!(out, "config:    {}", describe_config(cfg))?;
    writeln!(out, "in use:    {}", describe_in_use(in_use))?;
    writeln!(out, "reasons:")?;
    for reason in &res.reasons {
        writeln!(out, "  - {reason}")?;
    }
    if let Some(hint) = &res.hint {
        writeln!(out, "hint:      {hint}")?;
    }
    Ok(())
}

/// `[desktop]` in one line, e.g. `backend = "hyprpaper", managed_by = "detect", detected for
/// hyprland:wayland`.
pub(crate) fn describe_config(cfg: &DesktopConfig) -> String {
    let managed_by = match cfg.managed_by {
        ManagedBy::Detect => "detect",
        ManagedBy::User => "user",
    };
    let mut line = format!(
        "backend = \"{}\", managed_by = \"{managed_by}\"",
        cfg.backend.as_str()
    );
    if let Some(session) = &cfg.detected_session {
        line.push_str(&format!(", detected for {session}"));
    }
    line
}

/// The backend the daemon resolves, and whether it came from the config or from detection.
pub(crate) fn describe_in_use(res: &Resolution) -> String {
    let origin = match res.origin {
        Origin::Config => "from the config",
        Origin::Detected => "detected at start",
    };
    format!("{} ({origin})", res.backend)
}

fn save(
    out: &mut dyn Write,
    res: &Resolution,
    cfg: &DesktopConfig,
    source: &ConfigSource,
    force: bool,
) -> anyhow::Result<()> {
    let path = source.path();
    if source.is_legacy() {
        bail!(
            "{} is the legacy wallpaperd config; run `kakemono config init` first, then detect --write",
            path.display()
        );
    }
    let Some(desktop) = firstrun::persisted_desktop(res, cfg, Utc::now()) else {
        bail!(
            "nothing to save: {}",
            res.hint
                .as_deref()
                .unwrap_or("no usable backend was detected")
        );
    };
    if cfg.managed_by == ManagedBy::User && !force {
        bail!(
            "[desktop] in {} has managed_by = \"user\"; not overwriting it (use --force)",
            path.display()
        );
    }
    firstrun::write_desktop(path, &desktop)
        .with_context(|| format!("saving [desktop] to {}", path.display()))?;
    if cfg.set_command.is_some() && desktop.set_command.is_none() {
        writeln!(
            out,
            "Removed set_command: it would override the {} preset.",
            res.backend
        )?;
    }
    writeln!(
        out,
        "Saved backend = \"{}\" for {} to {}",
        res.backend,
        res.session_key,
        path.display()
    )?;
    if res.backend != BackendKind::Unconfigured {
        writeln!(out, "Restart the daemon to use it.")?;
    }
    Ok(())
}

/// The session to detect in: `--env-json`, else the live capture on Linux, else `None` (native
/// backend).
pub(crate) async fn session_env(env_json: Option<&Path>) -> anyhow::Result<Option<Env>> {
    if let Some(path) = env_json {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let env = serde_json::from_str(&text)
            .with_context(|| format!("parsing {} as a detection env", path.display()))?;
        return Ok(Some(env));
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Some(detect::env::capture_with_dbus().await))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

/// The native backend of this build (macOS and Windows; the daemon reports the live detail).
pub(crate) fn native_backend() -> BackendInfo {
    #[cfg(target_os = "macos")]
    {
        use kakemono_platform::WallpaperSetter as _;
        kakemono_platform::macos::setter::MacSetter::new().info()
    }
    // Windows. (Linux always has a session env, so it never asks.)
    #[cfg(not(target_os = "macos"))]
    {
        BackendInfo {
            name: "windows".to_owned(),
            detail: "IDesktopWallpaper".to_owned(),
        }
    }
}

pub(crate) fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    }
}
