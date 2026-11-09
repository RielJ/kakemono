//! Path layout, `init`, and the legacy `wallpaperd` config fallback (Task 6).

use std::path::{Path, PathBuf};

use wallpaperd_config::paths::{
    BaseDirs, ConfigSource, Os, Paths, TEMPLATE, copy_legacy_config, init,
};
use wallpaperd_config::{ConfigError, defaults_for_home, load_with_home};

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
    assert_eq!(cfg.daemon.cache_size, 50);

    // Only the Go file exists: read it, report Legacy, move nothing.
    std::fs::create_dir_all(p.legacy_config_file.parent().unwrap()).unwrap();
    std::fs::write(&p.legacy_config_file, "[daemon]\ncache_size = 7\n").unwrap();
    let (cfg, src) = p.load_config(None).unwrap();
    assert_eq!(src, ConfigSource::Legacy(p.legacy_config_file.clone()));
    assert!(src.is_legacy());
    assert_eq!(src.path(), p.legacy_config_file);
    assert_eq!(cfg.daemon.cache_size, 7);
    assert!(p.legacy_config_file.exists());
    assert!(!p.config_file.exists());

    // The user agrees to copy: the new file wins, the legacy file stays.
    copy_legacy_config(&p.legacy_config_file, &p.config_file).unwrap();
    assert!(p.legacy_config_file.exists());
    let (cfg, src) = p.load_config(None).unwrap();
    assert_eq!(src, ConfigSource::Primary(p.config_file.clone()));
    assert_eq!(cfg.daemon.cache_size, 7);

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
    assert_eq!(cfg.daemon.cache_size, 50);
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
    assert_eq!(cfg, go_cfg);
    // The new sections are commented out, so they keep their defaults.
    let defaults = defaults_for_home(dir.path());
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
