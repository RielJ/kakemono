//! Task 37b over IPC: `reload` with a changed `[desktop]` rebuilds the daemon's setter, so the
//! next `doctor --apply` runs the new command and reports the new backend; a `[desktop]` that
//! cannot be used makes `reload` fail and keeps the old setter. A real server on a tempdir
//! socket over `ArgvSetter` (records argvs, runs nothing). Never touches the real config.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_cli::daemon::lock;
use kakemono_cli::daemon::server::{self, DoctorData, Server};
use kakemono_config::Config;
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine};
use kakemono_platform::api::{Platform, Theme};
use kakemono_platform::fake::{ArgvSetter, ManualSignals};
use kakemono_protocol::client::Client;
use kakemono_protocol::endpoint::{Endpoint, test_socket_path};
use kakemono_protocol::types::{Request, Response};
use kakemono_sources::SourceSet;
use tokio_util::sync::CancellationToken;

use crate::support::real_dir;

fn ok_data<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    assert!(response.ok, "{response:?}");
    response.data_as::<T>().unwrap().expect("data")
}

/// The test's dirs; `[desktop]` (and everything else) from `config.toml`.
fn load(config_path: &Path, root: &Path) -> Result<Config, kakemono_config::ConfigError> {
    let mut cfg = kakemono_config::load_with_home(config_path, root)?;
    cfg.daemon.download_dir = root.join("downloads");
    cfg.daemon.trash_dir = root.join("trash");
    cfg.favorites.dir = root.join("favorites");
    cfg.daemon.socket_path = Some(test_socket_path(root, "k.sock"));
    Ok(cfg)
}

fn write_desktop(config_path: &Path, set_command: &str) {
    std::fs::write(
        config_path,
        format!("[desktop]\nbackend = \"command\"\nset_command = {set_command}\n"),
    )
    .unwrap();
}

#[tokio::test]
async fn reload_rebuilds_the_setter_that_doctor_apply_uses() {
    let dir = tempfile::tempdir().unwrap();
    let root: PathBuf = real_dir(dir.path());
    let config_path = root.join("config.toml");
    write_desktop(&config_path, r#"["old-setter", "{path}"]"#);
    let cfg = load(&config_path, &root).unwrap();

    let setter = ArgvSetter::from_desktop(&cfg.desktop).unwrap();
    let platform = Platform {
        setter: Arc::new(setter.clone()),
        signals: Arc::new(ManualSignals::new(Theme::Light)),
    };
    let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
        .with_sources(SourceSet::default());
    let engine = Engine::new(cfg, State::new(), root.join("state.json"), deps).unwrap();
    let image = root.join("a.jpg");
    std::fs::write(&image, b"fake image data").unwrap();
    engine.set_file(&image).await.unwrap();

    let lock = lock::acquire(&root.join("run")).unwrap();
    let endpoint = Endpoint::new(test_socket_path(&root, "k.sock")).unwrap();
    let listener = server::bind(&endpoint, &lock).await.unwrap();
    let srv = Server::new(engine.clone(), {
        let (config_path, root) = (config_path.clone(), root.clone());
        move || load(&config_path, &root)
    });
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(srv.serve(listener, shutdown.clone()));
    let mut c = Client::connect(&endpoint).await.unwrap();
    let image = image.to_str().unwrap().to_owned();

    // Unchanged `[desktop]`: nothing is rebuilt.
    assert!(c.request(Request::Reload).await.unwrap().ok);
    assert_eq!(setter.rebuilds(), 0);

    // Settings "Test": save, reload, doctor --apply. No daemon restart.
    write_desktop(&config_path, r#"["new-setter", "--file", "{path}"]"#);
    let r = c.request(Request::Reload).await.unwrap();
    assert!(r.ok, "{r:?}");
    let doctor: DoctorData = ok_data(&c.request(Request::Doctor { apply: true }).await.unwrap());
    assert!(doctor.apply.unwrap().ok);
    assert_eq!(doctor.backend.detail, "new-setter --file {path}");
    assert_eq!(
        setter.runs().last().unwrap(),
        &["new-setter".to_owned(), "--file".to_owned(), image.clone()]
    );

    // A `[desktop]` that cannot be used: reload fails, the setter and config stay.
    std::fs::write(&config_path, "[desktop]\nbackend = \"command\"\n").unwrap();
    let r = c.request(Request::Reload).await.unwrap();
    assert!(!r.ok);
    assert_eq!(
        r.error,
        "reload: [desktop]: `[desktop] set_command` is empty"
    );
    let doctor: DoctorData = ok_data(&c.request(Request::Doctor { apply: true }).await.unwrap());
    assert_eq!(doctor.backend.detail, "new-setter --file {path}");
    assert!(doctor.apply.unwrap().ok);
    assert_eq!(setter.rebuilds(), 1);
    assert!(engine.config().desktop.set_command.is_some());

    shutdown.cancel();
    task.await.unwrap();
    drop(lock);
}
