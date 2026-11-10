//! The preset argv table: one entry per command-driven backend.
//!
//! Each entry is an argv *template* for [`exec::render`](crate::exec::render): `{path}` is
//! substituted per token at set time, while the fit flag is chosen here from [`Fit`]. Nothing is
//! run through a shell, so `*` in the sway entry is a literal argument.

use crate::api::Fit;
use crate::linux::detect::{BackendKind, Resolution};

/// Every backend that has a preset, in table order (for `doctor` and snapshots).
pub const PRESET_KINDS: [BackendKind; 12] = [
    BackendKind::Sway,
    BackendKind::Hyprpaper,
    BackendKind::Awww,
    BackendKind::Swww,
    BackendKind::Swaybg,
    BackendKind::Wbg,
    BackendKind::Xwallpaper,
    BackendKind::Feh,
    BackendKind::Nitrogen,
    BackendKind::Hsetroot,
    BackendKind::Pcmanfm,
    BackendKind::PcmanfmQt,
];

fn v(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

/// The preset argv template for `kind` at `fit`, or `None` if `kind` has no preset (API
/// backends, `Command`, `Unconfigured`).
pub fn argv(kind: BackendKind, fit: Fit) -> Option<Vec<String>> {
    use BackendKind as K;
    Some(match kind {
        // swaymsg output * bg <file> <mode>. Sway supervises its own swaybg.
        K::Sway => v(&["swaymsg", "output", "*", "bg", "{path}", sway_mode(fit)]),
        // UNVERIFIED against hyprpaper 0.8.x: `hyprctl hyprpaper wallpaper "<monitor>,<path>"`,
        // empty monitor = all, `contain:`/`tile:` prefixes for fit. One entry: fix it here.
        K::Hyprpaper => v(&["hyprctl", "hyprpaper", "wallpaper", hyprpaper_target(fit)]),
        K::Awww => swww_like("awww", fit),
        K::Swww => swww_like("swww", fit),
        // Persistent: supervised by `persistent::Supervisor`.
        K::Swaybg => v(&["swaybg", "-i", "{path}", "-m", sway_mode(fit)]),
        // wbg has no scaling options; it always fills.
        K::Wbg => v(&["wbg", "{path}"]),
        K::Xwallpaper => v(&["xwallpaper", xwallpaper_flag(fit), "{path}"]),
        K::Feh => v(&["feh", feh_flag(fit), "{path}"]),
        K::Nitrogen => v(&["nitrogen", nitrogen_flag(fit), "--save", "{path}"]),
        K::Hsetroot => v(&["hsetroot", hsetroot_flag(fit), "{path}"]),
        K::Pcmanfm => pcmanfm_like("pcmanfm", pcmanfm_mode(fit)),
        K::PcmanfmQt => pcmanfm_like("pcmanfm-qt", pcmanfm_qt_mode(fit)),
        K::Gnome | K::Kde | K::Xfce | K::Cinnamon | K::Mate | K::Command | K::Unconfigured => {
            return None;
        }
    })
}

/// The argv template a resolution runs: its explicit `set_command`, else the preset for its
/// backend. `None` for API backends, `Unconfigured`, and `Command` without a `set_command`.
pub fn command_for(resolution: &Resolution, fit: Fit) -> Option<Vec<String>> {
    if !resolution.backend.uses_command() {
        return None;
    }
    resolution
        .set_command
        .clone()
        .or_else(|| argv(resolution.backend, fit))
}

fn swww_like(bin: &str, fit: Fit) -> Vec<String> {
    let resize = match fit {
        Fit::Fill => "crop",
        Fit::Fit => "fit",
        Fit::Stretch => "stretch",
        Fit::Center | Fit::Tile => "no",
    };
    v(&[
        bin,
        "img",
        "{path}",
        "--resize",
        resize,
        "--transition-type",
        "fade",
    ])
}

fn pcmanfm_like(bin: &str, mode: &str) -> Vec<String> {
    vec![
        bin.to_owned(),
        "--set-wallpaper".to_owned(),
        "{path}".to_owned(),
        format!("--wallpaper-mode={mode}"),
    ]
}

fn sway_mode(fit: Fit) -> &'static str {
    match fit {
        Fit::Fill => "fill",
        Fit::Fit => "fit",
        Fit::Center => "center",
        Fit::Stretch => "stretch",
        Fit::Tile => "tile",
    }
}

fn hyprpaper_target(fit: Fit) -> &'static str {
    match fit {
        Fit::Fit => ",contain:{path}",
        Fit::Tile => ",tile:{path}",
        Fit::Fill | Fit::Center | Fit::Stretch => ",{path}",
    }
}

fn xwallpaper_flag(fit: Fit) -> &'static str {
    match fit {
        Fit::Fill => "--zoom",
        Fit::Fit => "--maximize",
        Fit::Center => "--center",
        Fit::Stretch => "--stretch",
        Fit::Tile => "--tile",
    }
}

fn feh_flag(fit: Fit) -> &'static str {
    match fit {
        Fit::Fill => "--bg-fill",
        Fit::Fit => "--bg-max",
        Fit::Center => "--bg-center",
        Fit::Stretch => "--bg-scale",
        Fit::Tile => "--bg-tile",
    }
}

fn nitrogen_flag(fit: Fit) -> &'static str {
    match fit {
        Fit::Fill => "--set-zoom-fill",
        Fit::Fit => "--set-zoom",
        Fit::Center => "--set-centered",
        Fit::Stretch => "--set-scaled",
        Fit::Tile => "--set-tiled",
    }
}

fn hsetroot_flag(fit: Fit) -> &'static str {
    match fit {
        // `-fill` stretches in hsetroot; `-cover` (1.0.5+) is the crop-to-fill mode.
        Fit::Fill => "-cover",
        Fit::Fit => "-full",
        Fit::Center => "-center",
        Fit::Stretch => "-fill",
        Fit::Tile => "-tile",
    }
}

fn pcmanfm_mode(fit: Fit) -> &'static str {
    match fit {
        Fit::Fill => "crop",
        Fit::Fit => "fit",
        Fit::Center => "center",
        Fit::Stretch => "stretch",
        Fit::Tile => "tile",
    }
}

fn pcmanfm_qt_mode(fit: Fit) -> &'static str {
    match fit {
        Fit::Fill => "zoom",
        Fit::Fit => "fit",
        Fit::Center => "center",
        Fit::Stretch => "stretch",
        Fit::Tile => "tile",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::validate;

    const FITS: [Fit; 5] = [Fit::Fill, Fit::Fit, Fit::Center, Fit::Stretch, Fit::Tile];

    #[test]
    fn every_preset_validates_and_has_one_path() {
        for kind in PRESET_KINDS {
            for fit in FITS {
                let argv = argv(kind, fit).unwrap();
                validate(&argv).unwrap();
                assert_eq!(
                    argv.iter().filter(|t| t.contains("{path}")).count(),
                    1,
                    "{kind} {fit}: {argv:?}"
                );
                // The program is the binary detection probes for, except hyprpaper (driven via
                // hyprctl).
                let program = match kind {
                    BackendKind::Hyprpaper => "hyprctl",
                    _ => crate::linux::detect::resolve::binary_of(kind),
                };
                assert_eq!(argv[0], program);
            }
        }
    }

    #[test]
    fn api_backends_have_no_preset() {
        for kind in [
            BackendKind::Gnome,
            BackendKind::Kde,
            BackendKind::Xfce,
            BackendKind::Cinnamon,
            BackendKind::Mate,
            BackendKind::Command,
            BackendKind::Unconfigured,
        ] {
            assert_eq!(argv(kind, Fit::Fill), None);
        }
    }
}
