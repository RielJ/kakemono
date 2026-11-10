//! `doctor [--apply]`: diagnose the desktop backend and the daemon (plan §E, Task 37).
//!
//! Local checks run in this shell. On Linux (or with the hidden `--env-json`) they cover the
//! session env, the probes (`PATH`, processes, D-Bus names), the tray watcher, the resolution
//! with its reasons, a rendered argv, and picker window rules for the compositor. On macOS and
//! Windows they show the native backend.
//!
//! The Sources section reads `[[sources]]` from the config: enabled rows by type, the Internet
//! access switch, and the download folder's size against `download_limit_mb` (measured the way
//! the downloader's trim counts it). It warns when nothing is enabled, when Internet access is off
//! and every enabled source is online, and when an enabled local source's location is missing.
//!
//! Then the daemon's own report (IPC `doctor`): its backend, theme, and on Linux its env. Shell
//! variables the daemon lacks get a warning, since a systemd user unit often starts without them.
//!
//! `--apply` asks the daemon to re-set the current wallpaper through its own setter. With no
//! daemon it fails (exit 1); there is no local fallback, so nothing else ever drives the setter or
//! its `setter.pid`.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::bail;
use kakemono_config::{Config, ConfigError, Loaded, SourceType, expand_tilde};
use kakemono_engine::files::{Protect, counted_bytes};
use kakemono_platform::api::Theme;
use kakemono_platform::linux::detect::env::{BINARIES, DBUS_NAMES, ENV_VARS, PROCESSES};
use kakemono_platform::linux::detect::{self, Env, Resolution, Session};
use kakemono_protocol::client::{Client, ClientError};
use kakemono_protocol::types::{Request, StatusData};

use super::detect::{describe_config, describe_in_use, native_backend, os_name, session_env};
use crate::cli::Context;
use crate::daemon::server::DoctorData;

/// The error of `doctor --apply` without a daemon.
pub const NOT_RUNNING: &str = "daemon is not running; start it with `kakemono daemon`";

/// The bus name a tray icon needs (a StatusNotifier host).
const TRAY_WATCHER: &str = "org.kde.StatusNotifierWatcher";

/// The image used for the rendered argv when the daemon has no current wallpaper.
const EXAMPLE_IMAGE: &str = "/path/to/wallpaper.jpg";

/// What the daemon said, or why it said nothing.
enum DaemonReport {
    NotRunning,
    Unreachable(String),
    Running {
        doctor: Box<DoctorData>,
        status: Option<Box<StatusData>>,
    },
}

pub async fn run(
    ctx: &Context,
    out: &mut dyn Write,
    apply: bool,
    env_json: Option<&Path>,
) -> anyhow::Result<()> {
    let (paths, _) = ctx.config_source()?;
    let loaded = paths.load_config_report(ctx.global.config.as_deref());
    let env = session_env(env_json).await?;
    let daemon = query_daemon(ctx, apply).await;

    writeln!(
        out,
        "kakemono {} ({})",
        env!("CARGO_PKG_VERSION"),
        os_name()
    )?;
    let cfg = write_config(out, &paths, ctx, &loaded)?;
    match &env {
        Some(env) => write_linux(out, env, &cfg, &daemon)?,
        None => {
            let native = native_backend();
            writeln!(out)?;
            writeln!(out, "Backend")?;
            writeln!(out, "  backend:    {} (native)", native.name)?;
            writeln!(out, "  detail:     {}", native.detail)?;
        }
    }
    let download_bytes = measure_downloads(&cfg).await;
    let home = kakemono_config::defaults::home_dir();
    write_sources(
        out,
        &sources_report(&cfg, &home, &|p| p.exists(), download_bytes),
    )?;
    write_daemon(out, &daemon, env.as_ref())?;

    match &daemon {
        DaemonReport::Running { doctor, .. } => match &doctor.apply {
            Some(outcome) if !outcome.ok => bail!(
                "doctor --apply: {}",
                outcome
                    .error
                    .as_deref()
                    .unwrap_or("the wallpaper was not set")
            ),
            _ => Ok(()),
        },
        DaemonReport::NotRunning if apply => bail!(NOT_RUNNING),
        DaemonReport::Unreachable(err) if apply => bail!("{err}"),
        _ => Ok(()),
    }
}

async fn query_daemon(ctx: &Context, apply: bool) -> DaemonReport {
    let endpoint = match ctx.endpoint() {
        Ok(endpoint) => endpoint,
        Err(e) => return DaemonReport::Unreachable(format!("{e:#}")),
    };
    let mut client = match Client::connect(&endpoint).await {
        Ok(client) => client,
        Err(ClientError::DaemonNotRunning { .. }) => return DaemonReport::NotRunning,
        Err(e) => return DaemonReport::Unreachable(e.to_string()),
    };
    // Status first, so the rendered argv uses the wallpaper that was current before `apply`.
    // A transport failure poisons the connection, so it ends the report here.
    let status = match client.request(Request::Status).await {
        Ok(r) if r.ok => r.data_as::<StatusData>().ok().flatten(),
        Ok(_) => None,
        Err(e) => return DaemonReport::Unreachable(e.to_string()),
    };
    let doctor = match client.request(Request::Doctor { apply }).await {
        Ok(r) if r.ok => r.data_as::<DoctorData>(),
        Ok(r) => return DaemonReport::Unreachable(format!("daemon error: {}", r.error)),
        Err(e) => return DaemonReport::Unreachable(e.to_string()),
    };
    match doctor {
        Ok(Some(doctor)) => DaemonReport::Running {
            doctor: Box::new(doctor),
            status: status.map(Box::new),
        },
        Ok(None) => DaemonReport::Unreachable("daemon sent an empty doctor report".to_owned()),
        Err(e) => DaemonReport::Unreachable(format!(
            "daemon sent a doctor report this CLI can't read ({e}); is it another version?"
        )),
    }
}

/// The config line(s): the file, then each load warning (a replaced source id, a skipped
/// `[[sources]]` row). A config that fails to load is reported and the defaults are used.
fn write_config(
    out: &mut dyn Write,
    paths: &kakemono_config::paths::Paths,
    ctx: &Context,
    loaded: &Result<(Loaded, kakemono_config::paths::ConfigSource), ConfigError>,
) -> anyhow::Result<Config> {
    let source = paths.locate_config(ctx.global.config.as_deref());
    let note = if source.is_legacy() {
        " (legacy wallpaperd config)"
    } else if !source.path().exists() {
        " (not created yet; defaults apply)"
    } else {
        ""
    };
    writeln!(out, "config:       {}{note}", source.path().display())?;
    Ok(match loaded {
        Ok((loaded, _)) => {
            for warning in &loaded.warnings {
                writeln!(out, "  warning: {warning}")?;
            }
            loaded.config.clone()
        }
        Err(e) => {
            writeln!(out, "  error: {e}")?;
            writeln!(out, "  (the checks below use the defaults)")?;
            Config::default()
        }
    })
}

fn write_linux(
    out: &mut dyn Write,
    env: &Env,
    cfg: &Config,
    daemon: &DaemonReport,
) -> anyhow::Result<()> {
    writeln!(out)?;
    writeln!(out, "Session (this shell)")?;
    writeln!(out, "  session:    {}", detect::session_key(env))?;
    for name in ENV_VARS {
        writeln!(out, "  {name:<28} {}", env.var(name).unwrap_or("-"))?;
    }
    let found = |names: &[&str], has: &dyn Fn(&str) -> bool| -> String {
        let hits: Vec<&str> = names.iter().copied().filter(|n| has(n)).collect();
        if hits.is_empty() {
            "none".to_owned()
        } else {
            hits.join(", ")
        }
    };
    writeln!(
        out,
        "  on PATH:    {}",
        found(&BINARIES, &|n| env.has_binary(n))
    )?;
    writeln!(
        out,
        "  running:    {}",
        found(&PROCESSES, &|n| env.has_process(n))
    )?;
    let bus = match &env.dbus_names {
        Some(_) => found(&DBUS_NAMES, &|n| env.has_dbus_name(n) == Some(true)),
        None => "not probed (no session bus)".to_owned(),
    };
    writeln!(out, "  D-Bus:      {bus}")?;
    let tray = match env.has_dbus_name(TRAY_WATCHER) {
        Some(true) => format!("ok ({TRAY_WATCHER})"),
        Some(false) => format!(
            "missing: no {TRAY_WATCHER}, so the tray icon won't show (GNOME: install the \
             AppIndicator extension); use `kakemono settings` instead"
        ),
        None => "unknown (no session bus)".to_owned(),
    };
    writeln!(out, "  tray:       {tray}")?;

    let res = detect::resolve(env, &cfg.desktop);
    writeln!(out)?;
    writeln!(out, "Backend")?;
    writeln!(out, "  config:     {}", describe_config(&cfg.desktop))?;
    writeln!(out, "  backend:    {}", describe_in_use(&res))?;
    if res.persistent {
        writeln!(out, "  persistent: yes (kakemono supervises it)")?;
    }
    writeln!(out, "  reasons:")?;
    for reason in &res.reasons {
        writeln!(out, "    - {reason}")?;
    }
    if let Some(hint) = &res.hint {
        writeln!(out, "  hint:       {hint}")?;
    }
    let (image, theme) = match daemon {
        DaemonReport::Running { doctor, status } => (
            status
                .as_ref()
                .and_then(|s| s.current.as_ref())
                .map(|c| c.path.clone()),
            doctor.theme.parse().unwrap_or(Theme::Light),
        ),
        _ => (None, Theme::Light),
    };
    let image = image.unwrap_or_else(|| EXAMPLE_IMAGE.to_owned());
    match rendered(&res, Path::new(&image), cfg.desktop.fit, theme) {
        Rendered::Argvs(argvs) => {
            writeln!(out, "  would run:")?;
            for argv in argvs {
                writeln!(out, "    {}", shell_join(&argv))?;
            }
        }
        Rendered::Api(api) => writeln!(out, "  would run:  nothing; sets through {api}")?,
        Rendered::Error(e) => writeln!(out, "  would run:  error: {e}")?,
        Rendered::Nothing => writeln!(out, "  would run:  nothing (no backend)")?,
    }

    if let Some(rules) = window_rules(&Session::of(env)) {
        writeln!(out)?;
        writeln!(out, "Picker window rules (add to your compositor config)")?;
        for line in rules {
            writeln!(out, "  {line}")?;
        }
    }
    Ok(())
}

enum Rendered {
    Argvs(Vec<Vec<String>>),
    Api(&'static str),
    Error(String),
    Nothing,
}

/// The argv(s) one `set` of `image` would run, without running anything.
#[cfg(unix)]
fn rendered(res: &Resolution, image: &Path, fit: kakemono_platform::Fit, theme: Theme) -> Rendered {
    use kakemono_platform::linux::backends::{command::Template, gsettings};
    use kakemono_platform::linux::detect::BackendKind;
    match res.backend {
        BackendKind::Kde => return Rendered::Api("D-Bus (org.kde.plasmashell evaluateScript)"),
        BackendKind::Xfce => return Rendered::Api("D-Bus (xfconf)"),
        _ => {}
    }
    if let Some(argvs) = gsettings::argvs(res.backend, image, fit) {
        return match argvs {
            Ok(argvs) => Rendered::Argvs(argvs),
            Err(e) => Rendered::Error(e.to_string()),
        };
    }
    match Template::for_resolution(res) {
        Some(template) => match template.render(image, fit, theme) {
            Ok(argv) => Rendered::Argvs(vec![argv]),
            Err(e) => Rendered::Error(e.to_string()),
        },
        None => Rendered::Nothing,
    }
}

/// Windows has no Linux backends to render (only reached with `--env-json`).
#[cfg(not(unix))]
fn rendered(
    _res: &Resolution,
    _image: &Path,
    _fit: kakemono_platform::Fit,
    _theme: Theme,
) -> Rendered {
    Rendered::Nothing
}

/// One-line window rules for the picker (contract `ui.md`; the match key is the title).
fn window_rules(session: &Session) -> Option<Vec<&'static str>> {
    use kakemono_platform::linux::detect::resolve::Compositor;
    let desktops = &session.desktops;
    let compositor = session.compositor.or_else(|| {
        [Compositor::Hyprland, Compositor::Sway, Compositor::Niri]
            .into_iter()
            .find(|c| desktops.iter().any(|d| d == c.as_str()))
    })?;
    Some(match compositor {
        Compositor::Hyprland => vec![
            "# ~/.config/hypr/hyprland.conf",
            "windowrulev2 = float, title:^(kakemono-picker)$",
            "windowrulev2 = pin, title:^(kakemono-picker)$",
            "windowrulev2 = noborder, title:^(kakemono-picker)$",
        ],
        Compositor::Sway => vec![
            "# ~/.config/sway/config",
            "for_window [title=\"^kakemono-picker$\"] floating enable, sticky enable, border none, move position center",
        ],
        Compositor::Niri => vec![
            "// ~/.config/niri/config.kdl",
            "window-rule {",
            "    match title=\"^kakemono-picker$\"",
            "    open-floating true",
            "}",
        ],
    })
}

/// The download folder's size as the downloader's trim counts it, off the async workers.
/// `None` when it doesn't exist yet or can't be read.
async fn measure_downloads(cfg: &Config) -> Option<u64> {
    let dir = cfg.daemon.download_dir.clone();
    let protect = Protect::new().config(cfg);
    tokio::task::spawn_blocking(move || counted_bytes(&dir, &protect))
        .await
        .ok()
        .flatten()
}

/// One mebibyte: `download_limit_mb` is enforced in units of 1024 × 1024 bytes.
const MB: u64 = 1024 * 1024;

/// The Sources section as lines, without the heading. `home` expands a leading `~` in local
/// locations and `favorites.dir`; `exists` checks a local location; `download_bytes` is
/// [`measure_downloads`].
///
/// A missing local location is a warning, except the favorites folder: it is made on the first
/// favorite, so a fresh install lacks it and that gets an info line.
fn sources_report(
    cfg: &Config,
    home: &Path,
    exists: &dyn Fn(&Path) -> bool,
    download_bytes: Option<u64>,
) -> Vec<String> {
    let enabled: Vec<_> = cfg.sources.iter().filter(|s| s.enabled).collect();
    let mut by_type: Vec<(SourceType, usize)> = Vec::new();
    for row in &enabled {
        match by_type.iter_mut().find(|(kind, _)| *kind == row.kind) {
            Some((_, n)) => *n += 1,
            None => by_type.push((row.kind, 1)),
        }
    }
    let counts = by_type
        .iter()
        .map(|(kind, n)| format!("{n} {kind}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut lines = vec![if enabled.is_empty() {
        format!("enabled:    0 of {}", cfg.sources.len())
    } else {
        format!(
            "enabled:    {} of {} ({counts})",
            enabled.len(),
            cfg.sources.len()
        )
    }];
    let internet = cfg.daemon.internet;
    lines.push(format!(
        "internet:   {}",
        if internet { "on" } else { "off" }
    ));
    let dir = cfg.daemon.download_dir.display();
    let size = match download_bytes {
        #[allow(clippy::cast_precision_loss)] // A display value; MB precision is plenty.
        Some(bytes) => format!("{:.1} MB", bytes as f64 / MB as f64),
        None => "none yet".to_owned(),
    };
    lines.push(if cfg.daemon.download_limit_mb_enabled {
        format!(
            "downloads:  {size} of {} MB in {dir}",
            cfg.daemon.download_limit_mb
        )
    } else {
        format!("downloads:  {size} in {dir} (no limit)")
    });

    if enabled.is_empty() {
        lines.push("warning:    no enabled sources; nothing to pick from".to_owned());
    } else if !internet && enabled.iter().all(|s| s.kind.is_online()) {
        lines.push(
            "warning:    Internet access is off and every enabled source is online: nothing new \
             is downloaded, so changes only use images already on disk"
                .to_owned(),
        );
    }
    let expand = |p: &Path| expand_tilde(p, home).unwrap_or_else(|| p.to_owned());
    let favorites: PathBuf = expand(&cfg.favorites.dir);
    for row in enabled.iter().filter(|s| s.kind.is_local()) {
        let location = row.location.trim();
        if location.is_empty() {
            lines.push(format!("warning:    source {:?} has no location", row.id));
            continue;
        }
        let path = expand(Path::new(location));
        if exists(&path) {
            continue;
        }
        if path == favorites {
            lines.push(format!(
                "info:       source {:?}: favorites folder not created yet (made on first favorite)",
                row.id
            ));
        } else {
            lines.push(format!(
                "warning:    source {:?}: {location} does not exist",
                row.id
            ));
        }
    }
    lines
}

fn write_sources(out: &mut dyn Write, lines: &[String]) -> anyhow::Result<()> {
    writeln!(out)?;
    writeln!(out, "Sources")?;
    for line in lines {
        writeln!(out, "  {line}")?;
    }
    Ok(())
}

fn write_daemon(
    out: &mut dyn Write,
    daemon: &DaemonReport,
    shell_env: Option<&Env>,
) -> anyhow::Result<()> {
    writeln!(out)?;
    writeln!(out, "Daemon")?;
    let (doctor, status) = match daemon {
        DaemonReport::NotRunning => {
            writeln!(out, "  running:    no (start it with `kakemono daemon`)")?;
            return Ok(());
        }
        DaemonReport::Unreachable(err) => {
            writeln!(out, "  running:    unknown: {err}")?;
            return Ok(());
        }
        DaemonReport::Running { doctor, status } => (doctor, status),
    };
    writeln!(
        out,
        "  running:    yes (pid {}, kakemono {})",
        doctor.pid, doctor.version
    )?;
    writeln!(
        out,
        "  backend:    {} ({})",
        doctor.backend.name, doctor.backend.detail
    )?;
    writeln!(out, "  theme:      {}", doctor.theme)?;
    if let Some(status) = status {
        let current = status.current.as_ref().map_or("none", |c| c.path.as_str());
        writeln!(out, "  current:    {current}")?;
        writeln!(
            out,
            "  rotation:   {}",
            if status.paused { "paused" } else { "running" }
        )?;
    }
    if let (Some(daemon_env), Some(shell_env)) = (&doctor.env, shell_env) {
        for warning in env_warnings(daemon_env, shell_env) {
            writeln!(out, "  warning:    {warning}")?;
        }
    }
    if let Some(apply) = &doctor.apply {
        match (&apply.path, &apply.error) {
            (Some(path), _) if apply.ok => writeln!(out, "  apply:      ok, re-set {path}")?,
            (_, error) => writeln!(
                out,
                "  apply:      failed: {}",
                error.as_deref().unwrap_or("unknown error")
            )?,
        }
    }
    Ok(())
}

/// Shell variables the daemon lacks, and a session mismatch. Under a systemd user unit the
/// compositor variables are often missing, so detection and setters see a different session.
fn env_warnings(daemon: &Env, shell: &Env) -> Vec<String> {
    let missing: Vec<&str> = ENV_VARS
        .iter()
        .copied()
        .filter(|name| shell.var(name).is_some() && daemon.var(name).is_none())
        .collect();
    let mut warnings = Vec::new();
    if !missing.is_empty() {
        let names = missing.join(" ");
        warnings.push(format!(
            "the daemon's environment lacks {} that this shell has. If it runs as a systemd user \
             service, run `systemctl --user import-environment {names}` (or `dbus-update-activation-environment \
             --systemd --all` from your compositor's startup), then restart it",
            missing.join(", ")
        ));
    }
    let (daemon_key, shell_key) = (detect::session_key(daemon), detect::session_key(shell));
    if daemon_key != shell_key {
        warnings.push(format!(
            "the daemon sees session {daemon_key}, this shell {shell_key}"
        ));
    }
    warnings
}

/// `argv` as one line a POSIX shell would split back into the same words.
fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            let plain = !arg.is_empty()
                && arg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
            if plain {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_join_quotes_only_what_needs_it() {
        let argv: Vec<String> = ["swaymsg", "output", "*", "bg", "/a b/it's.jpg", "fill"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            shell_join(&argv),
            r"swaymsg output '*' bg '/a b/it'\''s.jpg' fill"
        );
    }

    fn row(
        id: &str,
        kind: SourceType,
        location: &str,
        enabled: bool,
    ) -> kakemono_config::SourceEntry {
        kakemono_config::SourceEntry {
            id: id.to_owned(),
            enabled,
            kind,
            location: location.to_owned(),
        }
    }

    /// A config with download_dir `/dl`, the given rows and Internet access.
    fn sources_cfg(rows: Vec<kakemono_config::SourceEntry>, internet: bool) -> Config {
        let mut cfg = Config::default();
        cfg.daemon.download_dir = "/dl".into();
        cfg.daemon.internet = internet;
        cfg.sources = rows;
        cfg
    }

    const TEST_HOME: &str = "/home/u";

    fn report(cfg: &Config, download_bytes: Option<u64>) -> String {
        let exists = |p: &Path| p.starts_with("/exists");
        let mut out = Vec::new();
        write_sources(
            &mut out,
            &sources_report(cfg, Path::new(TEST_HOME), &exists, download_bytes),
        )
        .unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn sources_counts_enabled_rows_by_type_and_measures_downloads() {
        let cfg = sources_cfg(
            vec![
                row("bing", SourceType::Bing, "", true),
                row("wh-1", SourceType::Wallhaven, "nature", true),
                row("wh-2", SourceType::Wallhaven, "space", true),
                row("us-1", SourceType::Unsplash, "sea", false),
                row("favorites", SourceType::Folder, "/exists/favorites", true),
            ],
            true,
        );
        insta::assert_snapshot!("doctor_sources_ok", report(&cfg, Some(150 * MB / 2)));
    }

    #[test]
    fn sources_download_limit_off_and_no_folder_yet() {
        let mut cfg = sources_cfg(vec![row("bing", SourceType::Bing, "", true)], true);
        cfg.daemon.download_limit_mb_enabled = false;
        insta::assert_snapshot!("doctor_sources_no_limit", report(&cfg, None));
    }

    #[test]
    fn sources_warns_when_nothing_is_enabled() {
        let cfg = sources_cfg(vec![row("bing", SourceType::Bing, "", false)], true);
        insta::assert_snapshot!("doctor_sources_none_enabled", report(&cfg, Some(0)));
        let empty = sources_cfg(Vec::new(), true);
        assert!(report(&empty, Some(0)).contains("no enabled sources; nothing to pick from"));
    }

    #[test]
    fn sources_warns_when_offline_and_every_enabled_source_is_online() {
        let online = sources_cfg(
            vec![
                row("bing", SourceType::Bing, "", true),
                row("dir-1", SourceType::Folder, "/exists/pics", false),
            ],
            false,
        );
        insta::assert_snapshot!("doctor_sources_offline", report(&online, Some(MB)));
        // A local source still gives new images: no warning.
        let mixed = sources_cfg(
            vec![
                row("bing", SourceType::Bing, "", true),
                row("dir-1", SourceType::Folder, "/exists/pics", true),
            ],
            false,
        );
        assert!(!report(&mixed, Some(MB)).contains("warning"));
    }

    #[test]
    fn sources_warns_about_missing_local_locations_by_row_id() {
        let cfg = sources_cfg(
            vec![
                row("dir-gone", SourceType::Folder, "/gone/pics", true),
                row("img-gone", SourceType::Images, "/gone/a.jpg", true),
                row("alb-ok", SourceType::AlbumName, "/exists/album", true),
                row("dir-blank", SourceType::Folder, "  ", true),
                row("dir-off", SourceType::Folder, "/gone/off", false),
                // favorites.dir is `~/fav`: equal after tilde expansion, so info, not a warning.
                row("favorites", SourceType::Folder, "/home/u/fav/", true),
                row("dir-tilde", SourceType::Folder, "~/gone", true),
            ],
            true,
        );
        let mut cfg = cfg;
        cfg.favorites.dir = "~/fav".into();
        insta::assert_snapshot!("doctor_sources_missing_local", report(&cfg, Some(0)));
    }

    #[test]
    fn sources_fresh_config_init_has_no_warnings() {
        // What `kakemono config init` writes, before anything is downloaded or favorited.
        let cfg =
            kakemono_config::parse_str(kakemono_config::paths::TEMPLATE, Path::new(TEST_HOME))
                .unwrap();
        let nothing_exists = |_: &Path| false;
        let mut out = Vec::new();
        write_sources(
            &mut out,
            &sources_report(&cfg, Path::new(TEST_HOME), &nothing_exists, None),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("warning"), "{text}");
        assert!(
            text.contains(
                "info:       source \"favorites\": favorites folder not created yet (made on first favorite)"
            ),
            "{text}"
        );
    }

    #[test]
    fn env_warnings_name_what_the_daemon_lacks() {
        let shell = Env::default()
            .with_var("XDG_CURRENT_DESKTOP", "Hyprland")
            .with_var("WAYLAND_DISPLAY", "wayland-1")
            .with_var("HYPRLAND_INSTANCE_SIGNATURE", "abc");
        let daemon = Env::default().with_var("XDG_CURRENT_DESKTOP", "Hyprland");
        let warnings = env_warnings(&daemon, &shell);
        assert_eq!(warnings.len(), 2, "{warnings:#?}");
        assert!(warnings[0].contains("lacks WAYLAND_DISPLAY, HYPRLAND_INSTANCE_SIGNATURE"));
        assert!(
            warnings[0].contains("import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE")
        );
        assert!(warnings[1].contains("hyprland:unknown, this shell hyprland:wayland"));
        assert!(env_warnings(&shell, &shell).is_empty());
    }
}
