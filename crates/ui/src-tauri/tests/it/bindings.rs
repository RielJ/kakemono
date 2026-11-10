//! TypeScript bindings for the frontend, generated with ts-rs from the Rust types.
//!
//! The generated files are committed under `crates/ui/frontend/src/shared/bindings/` so the
//! frontend typechecks without a Rust toolchain. This test regenerates them into a temp dir
//! and fails if the committed copy differs. To update after changing a type:
//!
//! ```sh
//! KAKEMONO_UPDATE_BINDINGS=1 cargo test -p kakemono-ui --test bindings
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ts_rs::TS;

const UPDATE_ENV: &str = "KAKEMONO_UPDATE_BINDINGS";

fn committed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../frontend/src/shared/bindings")
}

/// Every root type the frontend uses. Dependencies are exported with them.
fn export_all(dir: &Path) {
    let cfg = ts_rs::Config::new().with_out_dir(dir);
    kakemono_config::Config::export_all(&cfg).expect("export Config");
    kakemono_protocol::types::StatusData::export_all(&cfg).expect("export StatusData");
    kakemono_protocol::types::WallpaperInfo::export_all(&cfg).expect("export WallpaperInfo");
    kakemono_protocol::types::Event::export_all(&cfg).expect("export Event");
    kakemono_protocol::types::Response::export_all(&cfg).expect("export Response");
    kakemono_protocol::types::Command::export_all(&cfg).expect("export Command");
    kakemono_protocol::types::CheckSourceData::export_all(&cfg).expect("export CheckSourceData");
    kakemono_protocol::types::DownloadNowData::export_all(&cfg).expect("export DownloadNowData");
}

/// `relative path -> contents` for every `.ts` file under `dir` (ts-rs puts some
/// dependencies in subdirectories, e.g. `serde_json/JsonValue.ts`).
fn read_ts_files(dir: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    collect_ts_files(dir, dir, &mut files);
    files
}

fn collect_ts_files(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_ts_files(root, &path, files);
        } else if path.extension().is_some_and(|ext| ext == "ts") {
            let name = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            let text =
                fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            files.insert(name, text);
        }
    }
}

#[test]
fn committed_bindings_match_the_rust_types() {
    let fresh_dir = tempfile::tempdir().expect("tempdir");
    export_all(fresh_dir.path());
    let fresh = read_ts_files(fresh_dir.path());
    assert!(
        fresh.contains_key("Config.ts"),
        "export produced {:?}",
        fresh.keys()
    );

    let committed_dir = committed_dir();
    if std::env::var_os(UPDATE_ENV).is_some() {
        fs::create_dir_all(&committed_dir).expect("create bindings dir");
        for stale in read_ts_files(&committed_dir)
            .keys()
            .filter(|n| !fresh.contains_key(*n))
        {
            fs::remove_file(committed_dir.join(stale)).expect("remove stale binding");
        }
        for (name, text) in &fresh {
            let path = committed_dir.join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("create binding dir");
            fs::write(path, text).expect("write binding");
        }
        return;
    }

    let committed = read_ts_files(&committed_dir);
    let names = |m: &BTreeMap<String, String>| m.keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        names(&committed),
        names(&fresh),
        "binding files differ; run `{UPDATE_ENV}=1 cargo test -p kakemono-ui --test bindings`"
    );
    for (name, text) in &fresh {
        assert_eq!(
            committed.get(name),
            Some(text),
            "{name} is stale; run `{UPDATE_ENV}=1 cargo test -p kakemono-ui --test bindings`"
        );
    }
}
