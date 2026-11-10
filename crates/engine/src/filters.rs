//! Soft filters (spec items 19–24, "When possible, use images that…"). Pure: no I/O.
//!
//! [`Filters::compile`] turns `[filters]` into a checker once per config load. An invalid regex
//! or colour becomes a warning and that filter is skipped, so a typo never blocks every image.
//! [`Filters::check`] says which enabled filters an image fails; [`Filters::score`] counts the
//! passes so the picker can fall back to the best image when none passes them all.
//!
//! Thresholds (named consts below):
//! - [`DARK_MAX`] and [`LIGHT_MIN`]: the spec's "dark below 0.35, light above 0.6", applied to the
//!   mean Rec. 709 relative luminance in [`ImageFacts::luminance`].
//! - [`COLOR_MAX_DELTA_E`]: CIE76 distance in CIE Lab (D65) between the chosen colour and a
//!   palette colour.

use std::path::Path;

use kakemono_config::{BrightnessFilter, FiltersConfig};
use kakemono_platform::api::Theme;
use regex::Regex;

use crate::facts::ImageFacts;

/// Luminance below this is dark (spec item 21).
pub const DARK_MAX: f32 = 0.35;
/// Luminance above this is light (spec item 21).
pub const LIGHT_MIN: f32 = 0.60;
/// Largest CIE76 ΔE between the chosen colour and a palette colour that still counts as a match.
pub const COLOR_MAX_DELTA_E: f32 = 30.0;

/// Filter names used in [`Filters::check`] results and logs. They match the config keys.
pub const LANDSCAPE: &str = "landscape";
pub const MIN_SIZE: &str = "min_size";
pub const BRIGHTNESS: &str = "brightness";
pub const COLOR: &str = "color";
pub const RATING: &str = "rating";
pub const NAME_REGEX: &str = "name_regex";

/// What a filter needs to know about the machine, not the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterCtx {
    /// Size of the largest display in physical pixels.
    pub screen: (u32, u32),
    /// Current system appearance, for [`BrightnessFilter::System`].
    pub theme: Theme,
}

/// Compiled `[filters]`. Only the enabled, valid filters are kept.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    landscape: bool,
    /// Minimum size as a percentage of each screen side.
    min_size_percent: Option<u32>,
    brightness: BrightnessFilter,
    /// The chosen colour in CIE Lab.
    color: Option<[f32; 3]>,
    /// Minimum rating, 1–5.
    min_rating: Option<u8>,
    name_regex: Option<Regex>,
}

impl Filters {
    /// Compiles `cfg`. Returns the filters and one warning per enabled filter that was skipped
    /// because its setting is invalid (a bad `name_regex` or a `color` that isn't `#rrggbb`).
    /// `min_rating` is clamped to 1–5.
    pub fn compile(cfg: &FiltersConfig) -> (Filters, Vec<String>) {
        let mut warnings = Vec::new();
        let color = if cfg.color_enabled {
            match parse_hex_color(&cfg.color) {
                Some(rgb) => Some(srgb_to_lab(rgb)),
                None => {
                    warnings.push(format!(
                        "filters.color {:?} is not a #rrggbb colour; the color filter is off",
                        cfg.color
                    ));
                    None
                }
            }
        } else {
            None
        };
        let name_regex = if cfg.name_regex_enabled {
            match Regex::new(&cfg.name_regex) {
                Ok(re) => Some(re),
                Err(err) => {
                    warnings.push(format!(
                        "filters.name_regex {:?} is invalid; the name_regex filter is off: {err}",
                        cfg.name_regex
                    ));
                    None
                }
            }
        } else {
            None
        };
        let filters = Filters {
            landscape: cfg.landscape,
            min_size_percent: cfg.min_size_enabled.then_some(cfg.min_size_percent),
            brightness: cfg.brightness,
            color,
            min_rating: cfg.rating_enabled.then_some(cfg.min_rating.clamp(1, 5)),
            name_regex,
        };
        (filters, warnings)
    }

    /// Whether no filter is enabled, so every image passes.
    pub fn is_empty(&self) -> bool {
        !self.landscape
            && self.min_size_percent.is_none()
            && self.brightness == BrightnessFilter::Off
            && self.color.is_none()
            && self.min_rating.is_none()
            && self.name_regex.is_none()
    }

    /// Checks every enabled filter. `Err` lists the names of the failed ones, in a fixed order:
    /// [`LANDSCAPE`], [`MIN_SIZE`], [`BRIGHTNESS`], [`COLOR`], [`RATING`], [`NAME_REGEX`].
    pub fn check(
        &self,
        path: &Path,
        facts: &ImageFacts,
        ctx: &FilterCtx,
    ) -> Result<(), Vec<&'static str>> {
        let failed: Vec<&'static str> = self
            .results(path, facts, ctx)
            .filter(|(_, pass)| !pass)
            .map(|(name, _)| name)
            .collect();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(failed)
        }
    }

    /// Number of enabled filters the image passes.
    pub fn score(&self, path: &Path, facts: &ImageFacts, ctx: &FilterCtx) -> usize {
        self.results(path, facts, ctx)
            .filter(|(_, pass)| *pass)
            .count()
    }

    /// `(name, passed)` for each enabled filter, in the order documented on [`Filters::check`].
    fn results<'a>(
        &'a self,
        path: &'a Path,
        facts: &'a ImageFacts,
        ctx: &'a FilterCtx,
    ) -> impl Iterator<Item = (&'static str, bool)> + 'a {
        let landscape = self
            .landscape
            .then_some((LANDSCAPE, facts.width > facts.height));
        let min_size = self.min_size_percent.map(|pct| {
            let big_enough = |side: u32, screen: u32| {
                u64::from(side) * 100 >= u64::from(screen) * u64::from(pct)
            };
            let pass =
                big_enough(facts.width, ctx.screen.0) && big_enough(facts.height, ctx.screen.1);
            (MIN_SIZE, pass)
        });
        let want_dark = match self.brightness {
            BrightnessFilter::Off => None,
            BrightnessFilter::Dark => Some(true),
            BrightnessFilter::Light => Some(false),
            BrightnessFilter::System => Some(ctx.theme == Theme::Dark),
        };
        let brightness = want_dark.map(|dark| {
            let pass = if dark {
                facts.luminance < DARK_MAX
            } else {
                facts.luminance > LIGHT_MIN
            };
            (BRIGHTNESS, pass)
        });
        let color = self.color.map(|target| {
            let pass = facts
                .palette
                .iter()
                .any(|(rgb, _)| delta_e76(target, srgb_to_lab(*rgb)) <= COLOR_MAX_DELTA_E);
            (COLOR, pass)
        });
        let rating = self
            .min_rating
            .map(|min| (RATING, facts.rating.is_some_and(|r| r >= min)));
        let name_regex = self.name_regex.as_ref().map(|re| {
            let pass = path
                .file_name()
                .is_some_and(|name| re.is_match(&name.to_string_lossy()));
            (NAME_REGEX, pass)
        });
        [landscape, min_size, brightness, color, rating, name_regex]
            .into_iter()
            .flatten()
    }
}

/// Parses `#rrggbb` (either case, surrounding whitespace ignored).
fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// sRGB (8-bit, gamma-encoded) to CIE Lab with the D65 white point.
fn srgb_to_lab(rgb: [u8; 3]) -> [f32; 3] {
    let lin = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    // Linear sRGB to XYZ (D65), then normalised by the D65 white.
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f32| {
        const EPS: f32 = 216.0 / 24389.0;
        const KAPPA: f32 = 24389.0 / 27.0;
        if t > EPS {
            t.cbrt()
        } else {
            (KAPPA * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE76 colour difference: Euclidean distance in Lab.
fn delta_e76(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = |i: usize| a[i] - b[i];
    (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (u32, u32) = (2560, 1440);

    fn off() -> FiltersConfig {
        FiltersConfig {
            landscape: false,
            min_size_enabled: false,
            min_size_percent: 80,
            brightness: BrightnessFilter::Off,
            color_enabled: false,
            color: "#3b82f6".to_owned(),
            rating_enabled: false,
            min_rating: 4,
            name_regex_enabled: false,
            name_regex: String::new(),
        }
    }

    fn facts() -> ImageFacts {
        ImageFacts {
            width: 1920,
            height: 1080,
            luminance: 0.5,
            palette: vec![([128, 128, 128], 1.0)],
            rating: None,
        }
    }

    fn ctx(theme: Theme) -> FilterCtx {
        FilterCtx {
            screen: SCREEN,
            theme,
        }
    }

    fn compile_ok(cfg: &FiltersConfig) -> Filters {
        let (filters, warnings) = Filters::compile(cfg);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        filters
    }

    /// Runs `cases` of (facts, theme, expected pass) against one config.
    fn table(cfg: FiltersConfig, name: &'static str, cases: &[(ImageFacts, Theme, bool)]) {
        let filters = compile_ok(&cfg);
        let path = Path::new("/w/img.jpg");
        for (i, (f, theme, pass)) in cases.iter().enumerate() {
            let got = filters.check(path, f, &ctx(*theme));
            let want = if *pass { Ok(()) } else { Err(vec![name]) };
            assert_eq!(got, want, "case {i}: {f:?} theme {theme:?}");
            assert_eq!(
                filters.score(path, f, &ctx(*theme)),
                usize::from(*pass),
                "case {i}"
            );
        }
    }

    #[test]
    fn filters_off_pass_everything_with_score_zero() {
        let filters = compile_ok(&off());
        let f = ImageFacts {
            width: 1,
            height: 10,
            luminance: 1.0,
            palette: vec![],
            rating: None,
        };
        assert_eq!(filters.check(Path::new("x"), &f, &ctx(Theme::Dark)), Ok(()));
        assert_eq!(filters.score(Path::new("x"), &f, &ctx(Theme::Dark)), 0);
        assert!(filters.is_empty());
        let on = FiltersConfig {
            rating_enabled: true,
            ..off()
        };
        assert!(!Filters::compile(&on).0.is_empty());
    }

    #[test]
    fn filters_landscape() {
        let cfg = FiltersConfig {
            landscape: true,
            ..off()
        };
        let sized = |width, height| ImageFacts {
            width,
            height,
            ..facts()
        };
        table(
            cfg,
            LANDSCAPE,
            &[
                (sized(1920, 1080), Theme::Light, true),
                (sized(1080, 1920), Theme::Light, false),
                (sized(1000, 1000), Theme::Light, false),
            ],
        );
    }

    #[test]
    fn filters_min_size_percent_of_screen() {
        let cfg = FiltersConfig {
            min_size_enabled: true,
            min_size_percent: 80,
            ..off()
        };
        let sized = |width, height| ImageFacts {
            width,
            height,
            ..facts()
        };
        table(
            cfg,
            MIN_SIZE,
            &[
                (sized(2048, 1152), Theme::Light, true),
                (sized(1920, 1080), Theme::Light, false),
                // Both sides must be big enough.
                (sized(4000, 1000), Theme::Light, false),
                (sized(2047, 1152), Theme::Light, false),
            ],
        );
    }

    #[test]
    fn filters_brightness_dark_light_and_system() {
        let lum = |luminance| ImageFacts {
            luminance,
            ..facts()
        };
        let dark = FiltersConfig {
            brightness: BrightnessFilter::Dark,
            ..off()
        };
        table(
            dark,
            BRIGHTNESS,
            &[
                (lum(0.2), Theme::Light, true),
                (lum(0.7), Theme::Light, false),
                (lum(DARK_MAX), Theme::Light, false),
            ],
        );
        let light = FiltersConfig {
            brightness: BrightnessFilter::Light,
            ..off()
        };
        table(
            light,
            BRIGHTNESS,
            &[
                (lum(0.7), Theme::Dark, true),
                (lum(0.2), Theme::Dark, false),
                (lum(LIGHT_MIN), Theme::Dark, false),
            ],
        );
        let system = FiltersConfig {
            brightness: BrightnessFilter::System,
            ..off()
        };
        table(
            system,
            BRIGHTNESS,
            &[
                (lum(0.2), Theme::Dark, true),
                (lum(0.7), Theme::Dark, false),
                (lum(0.7), Theme::Light, true),
                (lum(0.2), Theme::Light, false),
                // Mid-grey is neither dark nor light.
                (lum(0.5), Theme::Dark, false),
                (lum(0.5), Theme::Light, false),
            ],
        );
    }

    #[test]
    fn filters_color_matches_palette_with_cie76() {
        let cfg = FiltersConfig {
            color_enabled: true,
            color: "#ff0000".to_owned(),
            ..off()
        };
        let pal = |palette: Vec<([u8; 3], f32)>| ImageFacts { palette, ..facts() };
        table(
            cfg,
            COLOR,
            &[
                (pal(vec![([250, 10, 10], 0.3)]), Theme::Light, true),
                (pal(vec![([0, 0, 255], 1.0)]), Theme::Light, false),
                // Any palette entry may match, not only the largest.
                (
                    pal(vec![([0, 0, 255], 0.7), ([250, 10, 10], 0.3)]),
                    Theme::Light,
                    true,
                ),
                (pal(vec![]), Theme::Light, false),
            ],
        );
    }

    #[test]
    fn filters_color_accepts_upper_case_hex() {
        let cfg = FiltersConfig {
            color_enabled: true,
            color: "#FF0000".to_owned(),
            ..off()
        };
        let filters = compile_ok(&cfg);
        let f = ImageFacts {
            palette: vec![([250, 10, 10], 1.0)],
            ..facts()
        };
        assert_eq!(filters.score(Path::new("a.jpg"), &f, &ctx(Theme::Light)), 1);
    }

    #[test]
    fn filters_srgb_to_lab_known_values() {
        let near = |got: [f32; 3], want: [f32; 3]| {
            for (g, w) in got.iter().zip(want) {
                assert!((g - w).abs() < 0.1, "got {got:?}, want {want:?}");
            }
        };
        near(srgb_to_lab([255, 255, 255]), [100.0, 0.0, 0.0]);
        near(srgb_to_lab([0, 0, 0]), [0.0, 0.0, 0.0]);
        near(srgb_to_lab([255, 0, 0]), [53.24, 80.09, 67.20]);
        near(srgb_to_lab([0, 0, 255]), [32.30, 79.19, -107.86]);
        assert!(delta_e76(srgb_to_lab([255, 0, 0]), srgb_to_lab([250, 10, 10])) < 5.0);
        assert!(delta_e76(srgb_to_lab([255, 0, 0]), srgb_to_lab([0, 0, 255])) > COLOR_MAX_DELTA_E);
    }

    #[test]
    fn filters_rating_at_least() {
        let cfg = FiltersConfig {
            rating_enabled: true,
            min_rating: 4,
            ..off()
        };
        let rated = |rating| ImageFacts { rating, ..facts() };
        table(
            cfg,
            RATING,
            &[
                (rated(Some(5)), Theme::Light, true),
                (rated(Some(4)), Theme::Light, true),
                (rated(None), Theme::Light, false),
                (rated(Some(3)), Theme::Light, false),
            ],
        );
    }

    #[test]
    fn filters_name_regex_matches_file_name_only() {
        let cfg = FiltersConfig {
            name_regex_enabled: true,
            name_regex: "^wall".to_owned(),
            ..off()
        };
        let filters = compile_ok(&cfg);
        let c = ctx(Theme::Light);
        let f = facts();
        assert_eq!(
            filters.check(Path::new("/pics/wall-01.jpg"), &f, &c),
            Ok(())
        );
        assert_eq!(
            filters.check(Path::new("/wallpapers/sky.jpg"), &f, &c),
            Err(vec![NAME_REGEX])
        );
        assert_eq!(
            filters.check(Path::new("/pics/my-wall.jpg"), &f, &c),
            Err(vec![NAME_REGEX])
        );
    }

    #[test]
    fn filters_invalid_regex_warns_and_is_skipped() {
        let cfg = FiltersConfig {
            name_regex_enabled: true,
            name_regex: "(".to_owned(),
            landscape: true,
            ..off()
        };
        let (filters, warnings) = Filters::compile(&cfg);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("name_regex"), "{warnings:?}");
        let c = ctx(Theme::Light);
        // The regex filter is skipped; landscape still applies.
        assert_eq!(filters.check(Path::new("/x/zzz.jpg"), &facts(), &c), Ok(()));
        assert_eq!(filters.score(Path::new("/x/zzz.jpg"), &facts(), &c), 1);
    }

    #[test]
    fn filters_invalid_color_warns_and_is_skipped() {
        for bad in ["red", "#ff00", "#gg0000", "", "#ff00001"] {
            let cfg = FiltersConfig {
                color_enabled: true,
                color: bad.to_owned(),
                ..off()
            };
            let (filters, warnings) = Filters::compile(&cfg);
            assert_eq!(warnings.len(), 1, "{bad:?}: {warnings:?}");
            assert!(warnings[0].contains("color"), "{warnings:?}");
            let f = ImageFacts {
                palette: vec![],
                ..facts()
            };
            assert_eq!(
                filters.check(Path::new("a.jpg"), &f, &ctx(Theme::Light)),
                Ok(()),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn filters_disabled_invalid_settings_do_not_warn() {
        let cfg = FiltersConfig {
            name_regex: "(".to_owned(),
            color: "nope".to_owned(),
            ..off()
        };
        let (_, warnings) = Filters::compile(&cfg);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn filters_score_counts_passes_and_check_lists_failures() {
        let cfg = FiltersConfig {
            landscape: true,
            min_size_enabled: true,
            min_size_percent: 80,
            brightness: BrightnessFilter::Dark,
            color_enabled: true,
            color: "#ff0000".to_owned(),
            rating_enabled: true,
            min_rating: 4,
            name_regex_enabled: true,
            name_regex: "^wall".to_owned(),
        };
        let filters = compile_ok(&cfg);
        let c = ctx(Theme::Light);
        // Passes landscape, brightness and rating; fails min_size, color and name_regex.
        let f = ImageFacts {
            width: 1920,
            height: 1080,
            luminance: 0.1,
            palette: vec![([0, 0, 255], 1.0)],
            rating: Some(5),
        };
        let path = Path::new("/w/sky.jpg");
        assert_eq!(filters.score(path, &f, &c), 3);
        assert_eq!(
            filters.check(path, &f, &c),
            Err(vec![MIN_SIZE, COLOR, NAME_REGEX])
        );
        let all = ImageFacts {
            width: 2560,
            height: 1440,
            palette: vec![([255, 0, 0], 1.0)],
            ..f
        };
        assert_eq!(filters.score(Path::new("/w/wall.jpg"), &all, &c), 6);
        assert_eq!(filters.check(Path::new("/w/wall.jpg"), &all, &c), Ok(()));
    }
}
