//! Path layout, `init`, and config lookup (Task 6).

use std::path::{Path, PathBuf};

use kakemono_config::paths::{BaseDirs, ConfigSource, Os, Paths, TEMPLATE, init};
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
fn locate_reads_the_kakemono_file_when_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let p = Paths::from_base(&xdg_base(home), Os::current());

    // Nothing exists: defaults, and the new path is where `init` would write.
    let (cfg, src) = p.load_config(None).unwrap();
    assert_eq!(src, ConfigSource::Missing(p.config_file.clone()));
    assert_eq!(cfg.daemon.interval, "30m");

    std::fs::create_dir_all(&p.config_dir).unwrap();
    std::fs::write(&p.config_file, "[daemon]\ninterval = \"7m\"\n").unwrap();
    let (cfg, src) = p.load_config(None).unwrap();
    assert_eq!(src, ConfigSource::Primary(p.config_file.clone()));
    assert_eq!(cfg.daemon.interval, "7m");
}

#[test]
fn explicit_and_env_override_win() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let p = Paths::from_base(&xdg_base(home), Os::current());
    std::fs::create_dir_all(&p.config_dir).unwrap();
    std::fs::write(&p.config_file, "").unwrap();

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
    // Except the socket: Go's template pins `socket_path`; ours leaves it commented out so the
    // platform default applies.
    assert!(go_cfg.daemon.socket_path.is_some());
    assert_eq!(cfg.daemon.socket_path, None);
    go_cfg.daemon.socket_path = None;
    // And the sources: Go's template is a legacy file and migrates to one Bing row; ours has no
    // legacy keys and no `[[sources]]`, so it starts with the default rows.
    let defaults = defaults_for_home(dir.path());
    assert_eq!(go_cfg.sources.len(), 1);
    assert_eq!(cfg.sources, defaults.sources);
    go_cfg.sources.clone_from(&cfg.sources);
    // Rotation: the Go template turned it on outright; ours leaves it at the default, off.
    assert!(!cfg.daemon.rotation_enabled);
    assert!(go_cfg.daemon.rotation_enabled);
    go_cfg.daemon.rotation_enabled = false;
    // Appearance: both stay at the built-in light theme.
    assert_eq!(cfg.ui.appearance, kakemono_config::Appearance::Light);
    assert_eq!(go_cfg.ui.appearance, kakemono_config::Appearance::Light);
    // And the one new-install value: ours shows the Welcome window, where the built-in
    // defaults (and Go) do not.
    assert!(cfg.ui.show_welcome);
    assert!(!go_cfg.ui.show_welcome);
    go_cfg.ui.show_welcome = true;
    assert_eq!(cfg, go_cfg);
    // The new sections are commented out, so they keep their defaults.
    assert_eq!(cfg.desktop, defaults.desktop);
    assert_eq!(cfg.hooks, defaults.hooks);
}

#[test]
fn template_uses_new_name() {
    assert!(TEMPLATE.contains("kakemono"));
}

// ─── socket_path rule (Task 23) ─────────────────────────────────────────────────────────────────

mod socket_override {
    use std::path::{Path, PathBuf};

    use kakemono_config::{Config, parse_str};

    fn with_socket(text: &str) -> Config {
        parse_str(
            &format!("[daemon]\nsocket_path = {text:?}\n"),
            &PathBuf::from("/Users/a"),
        )
        .unwrap()
    }

    #[test]
    fn unset_socket_path_has_no_override() {
        assert_eq!(Config::default().socket_override(), None);
    }

    #[test]
    fn empty_socket_path_has_no_override() {
        assert_eq!(with_socket("").socket_override(), None);
    }

    #[test]
    fn configured_socket_path_is_the_override() {
        let cfg = with_socket("/tmp/custom.sock");
        assert_eq!(cfg.socket_override(), Some(Path::new("/tmp/custom.sock")));
    }

    #[test]
    fn tilde_socket_path_is_expanded() {
        let cfg = with_socket("~/k.sock");
        assert_eq!(
            cfg.socket_override(),
            Some(PathBuf::from("/Users/a").join("k.sock").as_path())
        );
    }
}
