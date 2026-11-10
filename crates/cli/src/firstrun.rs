//! First-run desktop detection.
//!
//! At start the daemon calls [`ensure_desktop`]. On Linux it captures the session, resolves the
//! backend, and when detection ran (`backend = "auto"`, or `managed_by = "detect"` with a
//! drifted `detected_session`) it saves the result to `[desktop]` through
//! `kakemono_config::edit::set_desktop` (toml_edit: comments and layout are kept). The in-memory
//! config is updated too, so the platform resolves from the saved section.
//!
//! It never writes when:
//! - `managed_by = "user"` (the user owns `[desktop]`);
//! - `backend = "native"` (on Linux that means "detect every start");
//! - nothing usable was found (`backend` stays as it is, the setter reports the hint);
//! - a preset was detected but `set_command` is set: saving the preset would make that command
//!   override it, and dropping it would lose the user's command;
//! - the config is the legacy `wallpaperd` file (the Go app still owns it).
//!
//! [`plan`] is the pure decision, so it runs on every OS with a captured [`Env`].

use std::path::Path;

use chrono::{DateTime, SubsecRound as _, Utc};
use kakemono_config::{Config, ConfigError, DesktopConfig, ManagedBy, edit};
use kakemono_platform::linux::detect::{self, BackendKind, Env, Origin, Resolution};

/// Why [`plan`] keeps `[desktop]` as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Keep {
    /// `managed_by = "user"`.
    UserManaged,
    /// `backend = "native"`: detected on every start, never saved.
    Native,
    /// The configured backend applies to this session; nothing to detect.
    UpToDate,
    /// Detection found nothing usable; the hint says why.
    Unusable(String),
    /// A preset was detected, but `set_command` would override it once saved.
    SetCommandConflict(BackendKind),
    /// The config is the legacy `wallpaperd` file.
    Legacy,
}

impl std::fmt::Display for Keep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserManaged => f.write_str("[desktop] managed_by = \"user\""),
            Self::Native => {
                f.write_str("[desktop] backend = \"native\" is detected on every start")
            }
            Self::UpToDate => f.write_str("[desktop] already matches this session"),
            Self::Unusable(hint) => write!(f, "no usable backend detected: {hint}"),
            Self::SetCommandConflict(kind) => write!(
                f,
                "detected {kind}, but [desktop] set_command would override it; run `kakemono \
                 detect --write` to switch to {kind} (drops set_command)"
            ),
            Self::Legacy => f.write_str("the config is the legacy wallpaperd file"),
        }
    }
}

/// What [`ensure_desktop_with`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// `[desktop]` was saved with this backend for this session.
    Wrote {
        backend: BackendKind,
        session: String,
    },
    Kept(Keep),
}

/// The `[desktop]` section that saves `res`, starting from `current` (fit, theme and
/// `managed_by` are kept). `None` when `res` can't set a wallpaper.
///
/// A saved preset drops `set_command` (it would override the preset) and `persistent` (presets
/// know whether they are persistent). A saved `command` keeps both.
pub fn persisted_desktop(
    res: &Resolution,
    current: &DesktopConfig,
    now: DateTime<Utc>,
) -> Option<DesktopConfig> {
    if !res.is_usable() {
        return None;
    }
    let backend = res.backend.to_config()?;
    let command = res.backend == BackendKind::Command;
    Some(DesktopConfig {
        backend,
        set_command: if command {
            current.set_command.clone()
        } else {
            None
        },
        persistent: command && current.persistent,
        detected_session: Some(res.session_key.clone()),
        detected_at: Some(now.trunc_subsecs(0)),
        ..current.clone()
    })
}

/// The first-run decision for `cfg` in the session `env` describes.
pub fn plan(env: &Env, cfg: &DesktopConfig, now: DateTime<Utc>) -> Result<DesktopConfig, Keep> {
    if cfg.managed_by == ManagedBy::User {
        return Err(Keep::UserManaged);
    }
    if cfg.backend == kakemono_config::Backend::Native {
        return Err(Keep::Native);
    }
    let res = detect::resolve(env, cfg);
    if res.origin == Origin::Config {
        return Err(Keep::UpToDate);
    }
    let unusable = || {
        Keep::Unusable(
            res.hint
                .clone()
                .unwrap_or_else(|| format!("{} cannot set a wallpaper", res.backend)),
        )
    };
    let desktop = persisted_desktop(&res, cfg, now).ok_or_else(unusable)?;
    if res.backend != BackendKind::Command && cfg.set_command.is_some() {
        return Err(Keep::SetCommandConflict(res.backend));
    }
    Ok(desktop)
}

/// [`plan`], then save the result to `config_path` and into `cfg`. A missing file gets the
/// commented template first. `cfg` is unchanged unless the write succeeded.
pub fn ensure_desktop_with(
    env: &Env,
    config_path: &Path,
    legacy: bool,
    cfg: &mut Config,
    now: DateTime<Utc>,
) -> Result<Outcome, ConfigError> {
    let desktop = match plan(env, &cfg.desktop, now) {
        Ok(desktop) => desktop,
        Err(keep) => return Ok(Outcome::Kept(keep)),
    };
    if legacy {
        return Ok(Outcome::Kept(Keep::Legacy));
    }
    write_desktop(config_path, &desktop)?;
    let outcome = Outcome::Wrote {
        backend: BackendKind::from_config(desktop.backend).unwrap_or(BackendKind::Unconfigured),
        session: desktop.detected_session.clone().unwrap_or_default(),
    };
    cfg.desktop = desktop;
    Ok(outcome)
}

/// Saves `desktop` as `[desktop]` in `path`, creating the file from the template if needed.
pub fn write_desktop(path: &Path, desktop: &DesktopConfig) -> Result<(), ConfigError> {
    if !path.exists() {
        kakemono_config::paths::init(path)?;
    }
    edit::set_desktop(path, desktop)
}

/// Called by the daemon at start, before the platform is built. Linux only; elsewhere the
/// native backend needs nothing. Failures are logged; the daemon starts either way.
pub fn ensure_desktop(config_path: &Path, legacy: bool, cfg: &mut Config) {
    #[cfg(target_os = "linux")]
    {
        let env = capture_env();
        match ensure_desktop_with(&env, config_path, legacy, cfg, Utc::now()) {
            Ok(Outcome::Wrote { backend, session }) => tracing::info!(
                path = %config_path.display(),
                "detected the {backend} backend for {session}; saved it to [desktop]"
            ),
            Ok(Outcome::Kept(Keep::UpToDate)) => {}
            Ok(Outcome::Kept(keep)) => tracing::info!("[desktop] not saved: {keep}"),
            Err(e) => tracing::warn!(error = %e, "could not save the detected backend"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config_path, legacy, cfg);
    }
}

/// The session as the daemon sees it, D-Bus names included.
#[cfg(target_os = "linux")]
pub fn capture_env() -> Env {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt.block_on(detect::env::capture_with_dbus()),
        Err(e) => {
            tracing::warn!(error = %e, "no runtime for the D-Bus probe; detecting without it");
            detect::env::capture()
        }
    }
}

#[cfg(test)]
mod tests {
    use kakemono_config::{Backend, CommandSpec};

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-03-04T05:06:07.5Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn hyprland() -> Env {
        Env::default()
            .with_var("XDG_CURRENT_DESKTOP", "Hyprland")
            .with_var("XDG_SESSION_TYPE", "wayland")
            .with_var("WAYLAND_DISPLAY", "wayland-1")
            .with_var("HYPRLAND_INSTANCE_SIGNATURE", "abc")
            .with_binaries(&["hyprctl", "hyprpaper"])
    }

    fn sway() -> Env {
        Env::default()
            .with_var("XDG_CURRENT_DESKTOP", "sway")
            .with_var("XDG_SESSION_TYPE", "wayland")
            .with_var("WAYLAND_DISPLAY", "wayland-1")
            .with_var("SWAYSOCK", "/run/sway.sock")
            .with_binaries(&["swaymsg"])
    }

    #[test]
    fn auto_is_saved_with_the_session() {
        let desktop = plan(&hyprland(), &DesktopConfig::default(), now()).unwrap();
        assert_eq!(desktop.backend, Backend::Hyprpaper);
        assert_eq!(desktop.managed_by, ManagedBy::Detect);
        assert_eq!(
            desktop.detected_session.as_deref(),
            Some("hyprland:wayland")
        );
        assert_eq!(
            desktop.detected_at.unwrap().to_rfc3339(),
            "2026-03-04T05:06:07+00:00"
        );
        // Saved: the same session now resolves from the config.
        assert_eq!(plan(&hyprland(), &desktop, now()), Err(Keep::UpToDate));
    }

    #[test]
    fn session_drift_re_detects_and_saves() {
        let saved = plan(&hyprland(), &DesktopConfig::default(), now()).unwrap();
        let desktop = plan(&sway(), &saved, now()).unwrap();
        assert_eq!(desktop.backend, Backend::Sway);
        assert_eq!(desktop.detected_session.as_deref(), Some("sway:wayland"));
    }

    #[test]
    fn user_native_unusable_and_set_command_conflicts_are_kept() {
        let user = DesktopConfig {
            managed_by: ManagedBy::User,
            ..DesktopConfig::default()
        };
        assert_eq!(plan(&hyprland(), &user, now()), Err(Keep::UserManaged));

        let native = DesktopConfig {
            backend: Backend::Native,
            ..DesktopConfig::default()
        };
        assert_eq!(plan(&hyprland(), &native, now()), Err(Keep::Native));

        let nothing = plan(&Env::default(), &DesktopConfig::default(), now());
        assert!(matches!(nothing, Err(Keep::Unusable(_))), "{nothing:?}");

        let with_command = DesktopConfig {
            set_command: Some(CommandSpec::new(vec!["my-setter".into(), "{path}".into()]).unwrap()),
            ..DesktopConfig::default()
        };
        assert_eq!(
            plan(&hyprland(), &with_command, now()),
            Err(Keep::SetCommandConflict(BackendKind::Hyprpaper))
        );
    }

    #[test]
    fn a_pinned_backend_without_detected_session_is_left_alone() {
        let pinned = DesktopConfig {
            backend: Backend::Feh,
            ..DesktopConfig::default()
        };
        assert_eq!(plan(&hyprland(), &pinned, now()), Err(Keep::UpToDate));
    }

    #[test]
    fn writes_the_section_and_keeps_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            "# my config\n[daemon]\ninterval = \"10m\" # keep me\n\n[desktop]\n# how to fit\nfit = \"fit\"\n",
        )
        .unwrap();
        let mut cfg = kakemono_config::load_or_default(&path).unwrap();
        let outcome = ensure_desktop_with(&hyprland(), &path, false, &mut cfg, now()).unwrap();
        assert_eq!(
            outcome,
            Outcome::Wrote {
                backend: BackendKind::Hyprpaper,
                session: "hyprland:wayland".into()
            }
        );
        let text = std::fs::read_to_string(&path).unwrap();
        for kept in [
            "# my config",
            "interval = \"10m\" # keep me",
            "# how to fit",
        ] {
            assert!(text.contains(kept), "{kept:?} lost:\n{text}");
        }
        let reloaded = kakemono_config::load_or_default(&path).unwrap();
        assert_eq!(reloaded.desktop, cfg.desktop);
        assert_eq!(reloaded.desktop.fit, kakemono_platform::Fit::Fit);

        // Second start in the same session: nothing to do, file untouched.
        let before = std::fs::read(&path).unwrap();
        let again = ensure_desktop_with(&hyprland(), &path, false, &mut cfg, now()).unwrap();
        assert_eq!(again, Outcome::Kept(Keep::UpToDate));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn legacy_and_failed_plans_leave_the_config_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let mut cfg = Config::default();
        let outcome = ensure_desktop_with(&hyprland(), &path, true, &mut cfg, now()).unwrap();
        assert_eq!(outcome, Outcome::Kept(Keep::Legacy));
        assert!(!path.exists());
        assert_eq!(cfg.desktop, DesktopConfig::default());
    }
}
