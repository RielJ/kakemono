//! Table test for the pure Linux resolver. Runs on every OS: no system access.

use kakemono_config::schema::{Backend, CommandSpec, DesktopConfig, ManagedBy};
use kakemono_platform::linux::detect::{BackendKind, Env, Origin, Resolution, resolve};
use serde::Serialize;

fn wayland(desktop: &str) -> Env {
    Env::default()
        .with_var("XDG_CURRENT_DESKTOP", desktop)
        .with_var("XDG_SESSION_TYPE", "wayland")
        .with_var("WAYLAND_DISPLAY", "wayland-0")
}

fn x11(desktop: &str) -> Env {
    Env::default()
        .with_var("XDG_CURRENT_DESKTOP", desktop)
        .with_var("XDG_SESSION_TYPE", "x11")
        .with_var("DISPLAY", ":0")
}

fn hyprland() -> Env {
    wayland("Hyprland").with_var("HYPRLAND_INSTANCE_SIGNATURE", "abc_123")
}

fn niri() -> Env {
    wayland("niri").with_var("NIRI_SOCKET", "/run/user/1000/niri.sock")
}

fn auto() -> DesktopConfig {
    DesktopConfig::default()
}

fn cmd(argv: &[&str]) -> Option<CommandSpec> {
    Some(CommandSpec::new(argv.iter().map(|s| (*s).to_owned()).collect()).unwrap())
}

struct Case {
    name: &'static str,
    env: Env,
    cfg: DesktopConfig,
    want: BackendKind,
}

fn case(name: &'static str, env: Env, cfg: DesktopConfig, want: BackendKind) -> Case {
    Case {
        name,
        env,
        cfg,
        want,
    }
}

fn cases() -> Vec<Case> {
    use BackendKind as K;
    vec![
        case(
            "gnome_wayland",
            wayland("GNOME").with_binaries(&["gsettings"]),
            auto(),
            K::Gnome,
        ),
        case(
            "ubuntu_gnome",
            wayland("ubuntu:GNOME").with_binaries(&["gsettings"]),
            auto(),
            K::Gnome,
        ),
        case(
            "pop_gnome_x11",
            x11("pop:GNOME").with_binaries(&["gsettings", "feh"]),
            auto(),
            K::Gnome,
        ),
        case(
            "budgie",
            x11("Budgie:GNOME").with_binaries(&["gsettings"]),
            auto(),
            K::Gnome,
        ),
        case("kde_x11", x11("KDE"), auto(), K::Kde),
        case(
            "kde_wayland",
            Env {
                dbus_names: Some(["org.kde.plasmashell".to_owned()].into()),
                ..wayland("KDE").with_var("KDE_FULL_SESSION", "true")
            },
            auto(),
            K::Kde,
        ),
        case(
            "kde_full_session_only",
            Env::default()
                .with_var("KDE_FULL_SESSION", "true")
                .with_var("DISPLAY", ":0"),
            auto(),
            K::Kde,
        ),
        case(
            "xfce_without_xfconf_on_bus",
            Env {
                dbus_names: Some(Default::default()),
                ..x11("XFCE").with_binaries(&["xfconf-query"])
            },
            auto(),
            K::Xfce,
        ),
        case(
            "x_cinnamon",
            x11("X-Cinnamon").with_binaries(&["gsettings"]),
            auto(),
            K::Cinnamon,
        ),
        case("mate_missing_gsettings", x11("MATE"), auto(), K::Mate),
        case(
            "lxqt",
            x11("LXQt").with_binaries(&["pcmanfm-qt", "feh"]),
            auto(),
            K::PcmanfmQt,
        ),
        case(
            "lxde",
            x11("LXDE").with_binaries(&["pcmanfm"]),
            auto(),
            K::Pcmanfm,
        ),
        case(
            "session_desktop_fallback",
            Env::default()
                .with_var("DESKTOP_SESSION", "/usr/share/xsessions/plasma")
                .with_var("DISPLAY", ":0"),
            auto(),
            K::Kde,
        ),
        case("cosmic", wayland("COSMIC"), auto(), K::Command),
        case(
            "cosmic_with_set_command",
            wayland("COSMIC"),
            DesktopConfig {
                set_command: cmd(&["cosmic-bg-set", "{path}"]),
                ..auto()
            },
            K::Command,
        ),
        case(
            "hyprland_running_hyprpaper",
            hyprland()
                .with_binaries(&["hyprctl", "hyprpaper", "awww"])
                .with_processes(&["hyprpaper"]),
            auto(),
            K::Hyprpaper,
        ),
        case(
            "hyprland_awww_only",
            hyprland().with_binaries(&["hyprctl", "awww", "swaybg"]),
            auto(),
            K::Awww,
        ),
        case(
            "hyprland_swww_only",
            hyprland().with_binaries(&["hyprctl", "swww"]),
            auto(),
            K::Swww,
        ),
        case(
            "hyprland_running_swww_daemon_beats_hyprpaper",
            hyprland()
                .with_binaries(&["hyprpaper", "swww"])
                .with_processes(&["swww-daemon"]),
            auto(),
            K::Swww,
        ),
        case(
            "hyprland_nothing_installed",
            hyprland().with_binaries(&["hyprctl"]),
            auto(),
            K::Unconfigured,
        ),
        case(
            "sway",
            wayland("sway")
                .with_var("SWAYSOCK", "/run/user/1000/sway-ipc.sock")
                .with_binaries(&["swaymsg", "swaybg"]),
            auto(),
            K::Sway,
        ),
        case(
            "niri_swaybg_only",
            niri().with_binaries(&["swaybg"]),
            auto(),
            K::Swaybg,
        ),
        case(
            "niri_wbg_only",
            niri().with_binaries(&["wbg"]),
            auto(),
            K::Wbg,
        ),
        case("niri_nothing", niri(), auto(), K::Unconfigured),
        case(
            "niri_running_awww_daemon",
            niri()
                .with_binaries(&["awww", "swaybg"])
                .with_processes(&["awww-daemon"]),
            auto(),
            K::Awww,
        ),
        case(
            "wpaperd_running",
            niri()
                .with_binaries(&["swaybg"])
                .with_processes(&["wpaperd"]),
            auto(),
            K::Command,
        ),
        case(
            "river_tty_session_type",
            Env::default()
                .with_var("XDG_CURRENT_DESKTOP", "river")
                .with_var("XDG_SESSION_TYPE", "tty")
                .with_var("WAYLAND_DISPLAY", "wayland-1")
                .with_binaries(&["swww", "swaybg"]),
            auto(),
            K::Swww,
        ),
        case(
            "i3_feh",
            x11("i3").with_binaries(&["feh", "nitrogen"]),
            auto(),
            K::Feh,
        ),
        case(
            "i3_xwallpaper",
            x11("i3").with_binaries(&["feh", "xwallpaper"]),
            auto(),
            K::Xwallpaper,
        ),
        case(
            "openbox_hsetroot",
            Env::default()
                .with_var("DISPLAY", ":0")
                .with_binaries(&["hsetroot"]),
            auto(),
            K::Hsetroot,
        ),
        case("x11_nothing_installed", x11("i3"), auto(), K::Unconfigured),
        case(
            "pinned_user",
            wayland("GNOME").with_binaries(&["gsettings"]),
            DesktopConfig {
                backend: Backend::Swaybg,
                managed_by: ManagedBy::User,
                detected_session: Some("hyprland:wayland".into()),
                ..auto()
            },
            K::Swaybg,
        ),
        case(
            "pinned_hand_written_no_detected_session",
            x11("i3").with_binaries(&["xwallpaper"]),
            DesktopConfig {
                backend: Backend::Feh,
                set_command: cmd(&["feh", "--no-fehbg", "--bg-center", "{path}"]),
                ..auto()
            },
            K::Feh,
        ),
        case(
            "pinned_command_persistent",
            niri(),
            DesktopConfig {
                backend: Backend::Command,
                set_command: cmd(&["mpvpaper", "*", "{path}"]),
                persistent: true,
                managed_by: ManagedBy::User,
                ..auto()
            },
            K::Command,
        ),
        case(
            "pinned_command_without_set_command",
            niri(),
            DesktopConfig {
                backend: Backend::Command,
                managed_by: ManagedBy::User,
                ..auto()
            },
            K::Command,
        ),
        case(
            "pinned_api_backend_ignores_set_command",
            wayland("GNOME"),
            DesktopConfig {
                backend: Backend::Gnome,
                set_command: cmd(&["true"]),
                managed_by: ManagedBy::User,
                ..auto()
            },
            K::Gnome,
        ),
        case(
            "saved_detection_same_session",
            hyprland().with_binaries(&["hyprpaper", "awww"]),
            DesktopConfig {
                backend: Backend::Awww,
                set_command: cmd(&["awww", "img", "{path}"]),
                detected_session: Some("hyprland:wayland".into()),
                ..auto()
            },
            K::Awww,
        ),
        case(
            "session_drift",
            wayland("GNOME").with_binaries(&["gsettings", "awww"]),
            DesktopConfig {
                backend: Backend::Awww,
                set_command: cmd(&["awww", "img", "{path}"]),
                detected_session: Some("hyprland:wayland".into()),
                ..auto()
            },
            K::Gnome,
        ),
        case(
            "session_drift_to_x11_preset_drops_old_command",
            x11("i3").with_binaries(&["feh"]),
            DesktopConfig {
                backend: Backend::Awww,
                set_command: cmd(&["awww", "img", "{path}"]),
                detected_session: Some("hyprland:wayland".into()),
                ..auto()
            },
            K::Feh,
        ),
        case(
            "native_means_detect",
            x11("XFCE"),
            DesktopConfig {
                backend: Backend::Native,
                ..auto()
            },
            K::Xfce,
        ),
        case("empty_environment", Env::default(), auto(), K::Unconfigured),
    ]
}

#[derive(Serialize)]
struct Row<'a> {
    name: &'a str,
    #[serde(flatten)]
    resolution: &'a Resolution,
}

#[test]
fn resolve_table() {
    let cases = cases();
    assert!(cases.len() >= 25, "plan asks for at least 25 scenarios");
    let mut rows = Vec::new();
    let mut resolutions = Vec::new();
    for c in &cases {
        let r = resolve(&c.env, &c.cfg);
        assert_eq!(r.backend, c.want, "{}: {:#?}", c.name, r.reasons);
        assert!(!r.reasons.is_empty(), "{}: no reasons", c.name);
        resolutions.push(r);
    }
    for (c, r) in cases.iter().zip(&resolutions) {
        rows.push(Row {
            name: c.name,
            resolution: r,
        });
    }
    insta::assert_json_snapshot!("scenarios", rows);
}

fn get(name: &str) -> Resolution {
    let c = cases().into_iter().find(|c| c.name == name).unwrap();
    resolve(&c.env, &c.cfg)
}

#[test]
fn resolve_session_keys() {
    assert_eq!(get("hyprland_awww_only").session_key, "hyprland:wayland");
    assert_eq!(get("ubuntu_gnome").session_key, "ubuntu+gnome:wayland");
    assert_eq!(get("i3_feh").session_key, "i3:x11");
    assert_eq!(get("empty_environment").session_key, "unknown:unknown");
}

#[test]
fn resolve_drift_redetects_and_drops_stale_command() {
    let r = get("session_drift");
    assert_eq!(r.origin, Origin::Detected);
    assert_eq!(r.set_command, None);
    assert!(r.reasons[0].contains("session drift"), "{:?}", r.reasons);

    let r = get("session_drift_to_x11_preset_drops_old_command");
    assert_eq!((r.backend, r.set_command), (BackendKind::Feh, None));

    let r = get("saved_detection_same_session");
    assert_eq!(r.origin, Origin::Config);
    assert_eq!(
        r.set_command.as_deref(),
        Some(&["awww".to_owned(), "img".into(), "{path}".into()][..])
    );
}

#[test]
fn resolve_pinned_command_and_persistence() {
    let r = get("pinned_command_persistent");
    assert!(r.persistent && r.is_usable() && r.hint.is_none());
    assert_eq!(r.origin, Origin::Config);

    let r = get("pinned_command_without_set_command");
    assert!(!r.is_usable());
    assert!(r.hint.unwrap().contains("set_command"));

    assert!(get("niri_swaybg_only").persistent);
    assert!(get("niri_wbg_only").persistent);
    assert!(!get("hyprland_awww_only").persistent);

    let r = get("pinned_api_backend_ignores_set_command");
    assert_eq!(r.set_command, None);
}

#[test]
fn resolve_hints_for_unusable_results() {
    let r = get("empty_environment");
    assert_eq!(
        r.hint.as_deref(),
        Some(kakemono_platform::UNCONFIGURED_HINT)
    );
    assert!(get("cosmic").hint.unwrap().contains("COSMIC"));
    assert!(get("wpaperd_running").hint.unwrap().contains("wpaperctl"));
    let r = get("cosmic_with_set_command");
    assert!(r.is_usable() && r.hint.is_none());
}

#[test]
fn resolve_env_json_input() {
    // `detect --env-json` feeds a captured snapshot back in.
    let env: Env = serde_json::from_str(
        r#"{"xdg_current_desktop":"sway","swaysock":"/tmp/s","wayland_display":"wayland-1"}"#,
    )
    .unwrap();
    assert_eq!(resolve(&env, &auto()).backend, BackendKind::Sway);
}
