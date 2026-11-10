//! Reading and saving `config.toml`, and the daemon reload after a save.

use super::{CmdResult, CommandError, blocking, load_config, locate_config};
use crate::daemon_link::DaemonLink;
use kakemono_config::Config;
use kakemono_config::paths::{ConfigSource, Paths};
use kakemono_protocol::types::Request;
use serde::Serialize;
use serde_json::Value as Json;
use std::path::Path;
use std::sync::Arc;
use tauri::State;
use ts_rs::TS;

// ── Config ──────────────────────────────────────────────────────────────────

/// `get_config` → the effective config (defaults filled in, `~` expanded, a legacy config
/// migrated to `sources`). The first call also migrates a legacy config file on disk.
#[tauri::command]
pub async fn get_config() -> CmdResult<Config> {
    blocking(|| {
        migrate_config_once();
        load_config()
    })
    .await
}

/// Runs [`migrate_config_file`] on the located config once per process, so its notes are
/// logged once and a failing write (a read-only dir) isn't retried on every `get_config`.
fn migrate_config_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| match locate_config() {
        Ok((paths, source)) => {
            migrate_config_file(&paths, &source);
        }
        Err(e) => tracing::warn!(error = %e, "could not locate the config to migrate it"),
    });
}

/// Rewrites a legacy config to `[[sources]]` on disk and logs each line at info level. The
/// wallpaperd file is migrated into the kakemono path (later saves go there, since
/// [`locate_config`] then finds it); a kakemono file is rewritten in place with a `.bak`.
/// Errors are logged, not returned: loading migrates in memory either way.
fn migrate_config_file(
    paths: &Paths,
    source: &ConfigSource,
) -> Option<kakemono_config::FileMigration> {
    match kakemono_config::migrate_config(paths, source) {
        Ok(done) => {
            for line in done
                .iter()
                .flat_map(kakemono_config::FileMigration::messages)
            {
                tracing::info!("{line}");
            }
            done
        }
        Err(e) => {
            tracing::warn!(
                path = %source.path().display(),
                error = %e,
                "could not migrate the config file; using the migrated config in memory"
            );
            None
        }
    }
}

/// Result of [`save_config`]. The file is written either way; `reloaded` says whether the
/// daemon picked it up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct SaveOutcome {
    /// The daemon answered IPC `reload`.
    pub reloaded: bool,
    /// Why the reload failed. `None` when it worked or the daemon isn't running.
    pub reload_error: Option<String>,
}

/// `save_config { patch }`: merges a partial config (only the changed keys; `null` removes a
/// key) into `config.toml` with the comment-preserving writer, then sends IPC `reload`.
#[tauri::command]
pub async fn save_config(link: State<'_, Arc<DaemonLink>>, patch: Json) -> CmdResult<SaveOutcome> {
    blocking(move || {
        let (_, source) = locate_config()?;
        write_patch(source.path(), &patch)
    })
    .await?;
    Ok(reload(&link).await)
}

/// Validates `patch` and applies it to `path`. Split out for tests.
pub fn write_patch(path: &Path, patch: &Json) -> CmdResult<()> {
    validate_patch(patch)?;
    kakemono_config::edit::apply_patch(path, patch)?;
    tracing::info!(config = %path.display(), "config saved");
    Ok(())
}

/// Checks what the schema can't: the patch is an object and a new `daemon.interval` is a
/// positive Go duration.
pub fn validate_patch(patch: &Json) -> CmdResult<()> {
    let Json::Object(obj) = patch else {
        return Err(CommandError::Invalid(
            "config patch must be a JSON object".into(),
        ));
    };
    if let Some(interval) = obj.get("daemon").and_then(|d| d.get("interval")) {
        let Some(text) = interval.as_str() else {
            return Err(CommandError::Invalid(
                "daemon.interval must be a string".into(),
            ));
        };
        let parsed = kakemono_config::parse_go(text)
            .map_err(|e| CommandError::Invalid(format!("daemon.interval: {e}")))?;
        if parsed.is_zero() {
            return Err(CommandError::Invalid(
                "daemon.interval must be greater than zero".into(),
            ));
        }
    }
    Ok(())
}

/// IPC `reload`. A stopped daemon is not an error: it reads the file when it starts.
pub async fn reload(link: &DaemonLink) -> SaveOutcome {
    match link.call(Request::Reload).await {
        Ok(_) => SaveOutcome {
            reloaded: true,
            reload_error: None,
        },
        Err(err) if err.is_not_running() => SaveOutcome {
            reloaded: false,
            reload_error: None,
        },
        Err(err) => {
            tracing::warn!(error = %err, "config saved but the daemon did not reload it");
            SaveOutcome {
                reloaded: false,
                reload_error: Some(err.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorKind, Kinded};
    use serde_json::json;

    const COMMENTED: &str = "\
# my kakemono config
[daemon]
interval = \"30m\" # rotate every half hour
cache_size = 50

# sources
[bing]
enabled = true
";

    #[test]
    fn patch_keeps_comments_and_untouched_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, COMMENTED).unwrap();

        write_patch(&path, &json!({ "daemon": { "interval": "1h30m" } })).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, COMMENTED.replace("\"30m\"", "\"1h30m\""));
    }

    #[test]
    fn null_removes_a_key_and_nested_objects_merge() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[daemon]\nsocket_path = \"/tmp/x.sock\"\n").unwrap();

        write_patch(
            &path,
            &json!({ "daemon": { "socket_path": null }, "picker": { "position": "top" } }),
        )
        .unwrap();

        let cfg = kakemono_config::load(&path).unwrap();
        assert_eq!(cfg.daemon.socket_path, None);
        assert_eq!(cfg.picker.position, kakemono_config::PickerPosition::Top);
    }

    #[test]
    fn bad_intervals_and_shapes_are_rejected_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, COMMENTED).unwrap();

        for patch in [
            json!({ "daemon": { "interval": "1d" } }),
            json!({ "daemon": { "interval": "0s" } }),
            json!({ "daemon": { "interval": 30 } }),
            json!(["not", "an", "object"]),
            // Not cache_size: COMMENTED is a legacy config, and loading migrates that key away.
            json!({ "daemon": { "download_limit_mb": "lots" } }),
        ] {
            let err = write_patch(&path, &patch).expect_err(&patch.to_string());
            assert!(
                matches!(err.kind(), ErrorKind::Invalid | ErrorKind::Config),
                "{err}"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
    }

    fn temp_paths(root: &Path) -> Paths {
        let base = kakemono_config::paths::BaseDirs {
            home: root.to_owned(),
            config_home: root.join("cfg"),
            cache_home: root.join("cache"),
            state_home: Some(root.join("state")),
            runtime_home: Some(root.join("run")),
            config_override: None,
        };
        Paths::from_base(&base, kakemono_config::paths::Os::Linux)
    }

    #[test]
    fn migrating_the_wallpaperd_config_moves_saves_to_the_kakemono_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = temp_paths(dir.path());
        let legacy = "[light]\nsources = [\"bing\"]\n[dark]\nsources = [\"bing\"]\n";
        std::fs::create_dir_all(paths.legacy_config_file.parent().unwrap()).unwrap();
        std::fs::write(&paths.legacy_config_file, legacy).unwrap();
        let source = paths.locate_config(None);
        assert!(source.is_legacy());

        assert!(migrate_config_file(&paths, &source).is_some());
        // Only once: the next call finds the kakemono file and does nothing.
        assert!(migrate_config_file(&paths, &paths.locate_config(None)).is_none());

        // `save_config` and `app_info` locate the config again on each call: the kakemono file.
        assert_eq!(
            paths.locate_config(None),
            ConfigSource::Primary(paths.config_file.clone())
        );
        assert_eq!(
            std::fs::read_to_string(&paths.legacy_config_file).unwrap(),
            legacy
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_migration_is_logged_not_returned() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let paths = temp_paths(dir.path());
        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        let file = ro.join("config.toml");
        std::fs::write(&file, "[light]\nsources = [\"bing\"]\n").unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let done = migrate_config_file(&paths, &ConfigSource::Explicit(file.clone()));
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(done.is_none());
        assert_eq!(
            kakemono_config::load_or_default(&file)
                .unwrap()
                .sources
                .len(),
            1
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reload_reports_a_stopped_daemon_as_not_reloaded_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint =
            kakemono_protocol::endpoint::Endpoint::new(dir.path().join("k.sock")).unwrap();
        let link = DaemonLink::new(endpoint);
        assert_eq!(
            reload(&link).await,
            SaveOutcome {
                reloaded: false,
                reload_error: None
            }
        );
    }
}
