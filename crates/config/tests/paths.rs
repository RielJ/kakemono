//! Path layout, `init`, and the legacy `wallpaperd` config fallback (Task 6).

use std::path::{Path, PathBuf};

use kakemono_config::paths::{
    BaseDirs, ConfigSource, Os, Paths, TEMPLATE, copy_legacy_config, init,
};
use kakemono_config::{ConfigError, defaults_for_home, load_with_home};

fn xdg_base(home: &Path) -> BaseDirs {
    BaseDirs {
        home: home.to_owned(),
        config_home: home.join(".config"),
        cache_home: home.join(".cache"),
        state_home: Some(home.join(".local/state")),
        runtime_home: None,
        config_override: None,
    }
}

#[test]
fn macos_layout() {
    let home = PathBuf::from("/Users/a");
    let p = Paths::from_base(&xdg_base(&home), Os::MacOs);
    assert_eq!(p.config_file, home.join(".config/kakemono/config.toml"));
    assert_eq!(p.state_file, home.join(".config/kakemono/state.json"));
    assert_eq!(p.runtime_dir, home.join(".config/kakemono"));
    assert_eq!(p.log_dir, home.join(".local/state/kakemono"));
    assert_eq!(p.cache_dir, home.join(".cache/kakemono"));
    assert_eq!(
        p.legacy_config_file,
        home.join(".config/wallpaperd/config.toml")
    );
    assert_eq!(
        p.legacy_state_file,
        home.join(".config/wallpaperd/state.json")
    );
}

#[test]
fn linux_layout_honours_xdg() {
    let home = PathBuf::from("/home/a");
    let base = BaseDirs {
        config_home: PathBuf::from("/xdg/config"),
        runtime_home: Some(PathBuf::from("/run/user/1000")),
        ..xdg_base(&home)
    };
    let p = Paths::from_base(&base, Os::Linux);
    assert_eq!(
        p.config_file,
        PathBuf::from("/xdg/config/kakemono/config.toml")
    );
    assert_eq!(p.runtime_dir, PathBuf::from("/run/user/1000/kakemono"));
    // Go's DefaultConfigPath honoured XDG_CONFIG_HOME too.
    assert_eq!(
        p.legacy_config_file,
        PathBuf::from("/xdg/config/wallpaperd/config.toml")
    );
    // Go's state path did not.
    assert_eq!(
        p.legacy_state_file,
        home.join(".config/wallpaperd/state.json")
    );

    let no_runtime = Paths::from_base(
        &BaseDirs {
            runtime_home: None,
            ..base
        },
        Os::Linux,
    );
    assert_eq!(
        no_runtime.runtime_dir,
        PathBuf::from("/xdg/config/kakemono")
    );
}

#[test]
fn windows_layout() {
    let home = PathBuf::from(r"C:\Users\a");
    let base = BaseDirs {
        home: home.clone(),
        config_home: home.join(r"AppData\Roaming"),
        cache_home: home.join(r"AppData\Local"),
        state_home: None,
        runtime_home: None,
        config_override: None,
    };
    let p = Paths::from_base(&base, Os::Windows);
    assert_eq!(
        p.config_file,
        home.join(r"AppData\Roaming")
            .join("kakemono")
            .join("config.toml")
    );
    let local = home.join(r"AppData\Local").join("kakemono");
    assert_eq!(p.runtime_dir, local.join("run"));
    assert_eq!(p.log_dir, local.join("logs"));
    assert_eq!(p.cache_dir, local.join("cache"));
}

#[test]
fn locate_prefers_new_then_legacy() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let p = Paths::from_base(&xdg_base(home), Os::current());

    // Nothing exists: defaults, and the new path is where `init` would write.
    let (cfg, src) = p.load_config(None).unwrap();
    assert_eq!(src, ConfigSource::Missing(p.config_file.clone()));
    assert!(!src.is_legacy());
    assert_eq!(cfg.daemon.interval, "30m");

    // Only the Go file exists (a non-legacy key as the marker: cache_size is migrated away): read it, report Legacy, move nothing.
    std::fs::create_dir_all(p.legacy_config_file.parent().unwrap()).unwrap();
    std::fs::write(&p.legacy_config_file, "[daemon]\ninterval = \"7m\"\n").unwrap();
    let (cfg, src) = p.load_config(None).unwrap();
    assert_eq!(src, ConfigSource::Legacy(p.legacy_config_file.clone()));
    assert!(src.is_legacy());
    assert_eq!(src.path(), p.legacy_config_file);
    assert_eq!(cfg.daemon.interval, "7m");
    assert!(p.legacy_config_file.exists());
    assert!(!p.config_file.exists());

    // The user agrees to copy: the new file wins, the legacy file stays.
    copy_legacy_config(&p.legacy_config_file, &p.config_file).unwrap();
    assert!(p.legacy_config_file.exists());
    let (cfg, src) = p.load_config(None).unwrap();
    assert_eq!(src, ConfigSource::Primary(p.config_file.clone()));
    assert_eq!(cfg.daemon.interval, "7m");

    // Copying again refuses to overwrite.
    let err = copy_legacy_config(&p.legacy_config_file, &p.config_file).unwrap_err();
    assert!(matches!(err, ConfigError::AlreadyExists { .. }), "{err:?}");
}

#[test]
fn explicit_and_env_override_win() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let p = Paths::from_base(&xdg_base(home), Os::current());
    std::fs::create_dir_all(p.legacy_config_file.parent().unwrap()).unwrap();
    std::fs::write(&p.legacy_config_file, "").unwrap();

    let flag = home.join("flag.toml");
    assert_eq!(
        p.locate_config(Some(&flag)),
        ConfigSource::Explicit(flag.clone())
    );

    let env = home.join("env.toml");
    let with_env = Paths::from_base(
        &BaseDirs {
            config_override: Some(env.clone()),
            ..xdg_base(home)
        },
        Os::current(),
    );
    assert_eq!(with_env.locate_config(None), ConfigSource::Explicit(env));
    assert_eq!(
        with_env.locate_config(Some(&flag)),
        ConfigSource::Explicit(flag.clone())
    );

    // A missing explicit file yields defaults, as Go's LoadOrDefault did.
    let (cfg, _) = p.load_config(Some(&flag)).unwrap();
    assert_eq!(cfg.daemon.download_limit_mb, 500);
}

#[test]
fn init_writes_template_and_refuses_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kakemono").join("config.toml");
    init(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    let err = init(&path).unwrap_err();
    assert!(matches!(err, ConfigError::AlreadyExists { .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!(
            "config: {} already exists; remove it first to re-initialise",
            path.display()
        )
    );
}

#[test]
fn template_loads_like_go_template() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, TEMPLATE).unwrap();
    let cfg = load_with_home(&path, dir.path()).unwrap();
    // The template is Go's template plus commented-out sections, so it loads to the same values.
    let go = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go_default_template.toml");
    let mut go_cfg = load_with_home(&go, dir.path()).unwrap();
    // Except the socket: Go's template pins `~/.config/wallpaperd/daemon.sock`; ours leaves it
    // commented out so the platform default (under kakemono) applies.
    assert!(go_cfg.daemon.socket_path.is_some());
    assert_eq!(cfg.daemon.socket_path, None);
    go_cfg.daemon.socket_path = None;
    // And the sources: Go's template is a legacy file and migrates to one Bing row; ours has no
    // legacy keys and no `[[sources]]`, so it starts with the default rows.
    let defaults = defaults_for_home(dir.path());
    assert_eq!(go_cfg.sources.len(), 1);
    assert_eq!(cfg.sources, defaults.sources);
    go_cfg.sources.clone_from(&cfg.sources);
    assert_eq!(cfg, go_cfg);
    // The new sections are commented out, so they keep their defaults.
    assert_eq!(cfg.desktop, defaults.desktop);
    assert_eq!(cfg.hooks, defaults.hooks);
}

#[test]
fn template_uses_new_name() {
    assert!(TEMPLATE.contains("kakemono"));
    assert!(
        !TEMPLATE.to_lowercase().contains("wallpaperd"),
        "template still says wallpaperd"
    );
}

// ─── legacy socket rule (Task 23) ───────────────────────────────────────────────────────────────

mod effective_socket_path {
    use std::path::{Path, PathBuf};

    use kakemono_config::paths::{ConfigSource, SocketPath, legacy_default_socket};
    use kakemono_config::{Config, load_with_home, parse_str};

    fn home() -> PathBuf {
        PathBuf::from("/Users/a")
    }

    fn with_socket(text: &str) -> Config {
        parse_str(&format!("[daemon]\nsocket_path = {text:?}\n"), &home()).unwrap()
    }

    fn legacy() -> ConfigSource {
        ConfigSource::Legacy(home().join(".config/wallpaperd/config.toml"))
    }

    #[test]
    fn legacy_default_socket_is_gos_path() {
        assert_eq!(
            legacy_default_socket(&home()),
            PathBuf::from("/Users/a/.config/wallpaperd/daemon.sock")
        );
    }

    #[test]
    fn unset_socket_uses_default_for_every_source() {
        let cfg = Config::default();
        for source in [
            legacy(),
            ConfigSource::Primary(home().join(".config/kakemono/config.toml")),
            ConfigSource::Explicit(PathBuf::from("/tmp/c.toml")),
            ConfigSource::Missing(home().join(".config/kakemono/config.toml")),
        ] {
            let got = cfg.effective_socket_path(&source, &home());
            assert_eq!(got, SocketPath::Default, "{source:?}");
            assert_eq!(got.override_path(), None);
            assert_eq!(got.notice(), None);
        }
    }

    #[test]
    fn empty_socket_path_uses_default() {
        let cfg = with_socket("");
        assert_eq!(
            cfg.effective_socket_path(&legacy(), &home()),
            SocketPath::Default
        );
    }

    #[test]
    fn legacy_config_with_go_default_socket_is_ignored() {
        let cfg = with_socket("~/.config/wallpaperd/daemon.sock");
        let got = cfg.effective_socket_path(&legacy(), &home());
        let go = legacy_default_socket(&home());
        assert_eq!(got, SocketPath::IgnoredLegacy(&go));
        assert_eq!(got.override_path(), None);
        let notice = got.notice().unwrap();
        assert!(
            notice.contains("/Users/a/.config/wallpaperd/daemon.sock"),
            "{notice}"
        );
        assert!(notice.contains("legacy wallpaperd config"), "{notice}");
    }

    #[test]
    fn legacy_config_with_absolute_go_default_is_ignored() {
        let cfg = with_socket("/Users/a/.config/wallpaperd/daemon.sock");
        assert!(matches!(
            cfg.effective_socket_path(&legacy(), &home()),
            SocketPath::IgnoredLegacy(_)
        ));
    }

    #[test]
    fn legacy_config_with_custom_socket_is_honoured() {
        let cfg = with_socket("/tmp/custom.sock");
        let got = cfg.effective_socket_path(&legacy(), &home());
        assert_eq!(got, SocketPath::Configured(Path::new("/tmp/custom.sock")));
        assert_eq!(got.override_path(), Some(Path::new("/tmp/custom.sock")));
        assert_eq!(got.notice(), None);
    }

    #[test]
    fn go_default_socket_from_other_sources_is_honoured() {
        let cfg = with_socket("~/.config/wallpaperd/daemon.sock");
        let go = legacy_default_socket(&home());
        for source in [
            ConfigSource::Primary(home().join(".config/kakemono/config.toml")),
            ConfigSource::Explicit(PathBuf::from("/tmp/c.toml")),
        ] {
            assert_eq!(
                cfg.effective_socket_path(&source, &home()),
                SocketPath::Configured(&go),
                "{source:?}"
            );
        }
    }

    /// The real Go template, read through the legacy fallback, resolves to the kakemono default.
    #[test]
    fn go_template_through_legacy_fallback_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let go =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go_default_template.toml");
        let cfg = load_with_home(&go, dir.path()).unwrap();
        let source = ConfigSource::Legacy(go.clone());
        assert!(matches!(
            cfg.effective_socket_path(&source, dir.path()),
            SocketPath::IgnoredLegacy(_)
        ));
    }
}
