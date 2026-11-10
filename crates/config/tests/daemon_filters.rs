//! The `[daemon]` download keys and the `[filters]` table (Variety-style sources, Task 2).

use std::path::{Path, PathBuf};
use std::time::Duration;

use kakemono_config::{BrightnessFilter, Config, FiltersConfig, parse_str};

fn home() -> &'static Path {
    Path::new("/home/u")
}

#[test]
fn new_daemon_keys_have_the_spec_defaults() {
    let d = parse_str("", home()).expect("parse").daemon;
    assert!(!d.change_on_start);
    assert!(d.internet);
    assert!(d.download_enabled);
    assert_eq!(d.download_interval, "10m");
    assert!(d.download_limit_mb_enabled);
    assert_eq!(d.download_limit_mb, 500);
    assert_eq!(d.manual_dir, None);
}

#[test]
fn download_interval_duration_parses_go_durations() {
    let d = parse_str("", home()).expect("parse").daemon;
    assert_eq!(d.download_interval_duration(), Ok(Duration::from_secs(600)));
    let d = parse_str("[daemon]\ndownload_interval = \"1h30m\"\n", home())
        .expect("parse")
        .daemon;
    assert_eq!(
        d.download_interval_duration(),
        Ok(Duration::from_secs(90 * 60))
    );
}

#[test]
fn manual_dir_resolves_under_download_dir_when_unset() {
    let d = parse_str("[daemon]\ndownload_dir = \"/data/dl\"\n", home())
        .expect("parse")
        .daemon;
    assert_eq!(d.manual_dir_resolved(), PathBuf::from("/data/dl/manual"));
}

#[test]
fn manual_dir_expands_tilde_when_set() {
    let d = parse_str("[daemon]\nmanual_dir = \"~/Pictures/Mine\"\n", home())
        .expect("parse")
        .daemon;
    assert_eq!(d.manual_dir, Some(PathBuf::from("/home/u/Pictures/Mine")));
    assert_eq!(
        d.manual_dir_resolved(),
        PathBuf::from("/home/u/Pictures/Mine")
    );
}

#[test]
fn filters_default_to_off() {
    let f = parse_str("", home()).expect("parse").filters;
    assert_eq!(
        f,
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
    );
    assert_eq!(f, FiltersConfig::default());
}

#[test]
fn brightness_system_round_trips() {
    let cfg = parse_str("[filters]\nbrightness = \"system\"\n", home()).expect("parse");
    assert_eq!(cfg.filters.brightness, BrightnessFilter::System);
    let text = toml::to_string(&cfg).expect("serialise");
    assert!(text.contains("brightness = \"system\""), "{text}");
    let again = parse_str(&text, home()).expect("reparse");
    assert_eq!(again.filters, cfg.filters);
}

#[test]
fn brightness_strings_are_lowercase() {
    for (s, want) in [
        ("off", BrightnessFilter::Off),
        ("dark", BrightnessFilter::Dark),
        ("light", BrightnessFilter::Light),
        ("system", BrightnessFilter::System),
    ] {
        let cfg = parse_str(&format!("[filters]\nbrightness = \"{s}\"\n"), home()).expect(s);
        assert_eq!(cfg.filters.brightness, want);
        let json = serde_json::to_value(&cfg).expect("json");
        assert_eq!(json["filters"]["brightness"], serde_json::Value::from(s));
    }
}

#[test]
fn partial_filters_table_keeps_other_defaults() {
    let f = parse_str("[filters]\nlandscape = true\nmin_rating = 5\n", home())
        .expect("parse")
        .filters;
    assert!(f.landscape);
    assert_eq!(f.min_rating, 5);
    assert_eq!(f.min_size_percent, 80);
    assert_eq!(f.color, "#3b82f6");
}

#[test]
fn default_config_round_trips_through_toml() {
    let cfg = parse_str("", home()).expect("parse");
    let text = toml::to_string(&cfg).expect("serialise");
    let again: Config = parse_str(&text, home()).expect("reparse");
    assert_eq!(again, cfg);
}
