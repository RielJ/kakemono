//! Loading `config.toml` over the defaults, with `~` expansion (Go `config.Load`/`LoadOrDefault`).

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kakemono_fs::PathLock;
use toml_edit::DocumentMut;

use crate::defaults::{defaults_for_home, home_dir};
use crate::edit::write_atomic;
use crate::error::ConfigError;
use crate::migrate::{MigrationReport, migrate_document};
use crate::paths::{ConfigSource, Paths};
use crate::schema::Config;
use crate::sources::normalize_sources;

/// A loaded config and what loading had to fix or skip, e.g. a replaced source id or a
/// `[[sources]]` row with an unknown or missing `type`. Each warning is one line for a log or
/// `doctor`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
}

/// Reads and parses `path`. Missing keys take the defaults. Fails if the file does not exist.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    load_with_home(path, &home_dir())
}

/// Like [`load`], but returns the defaults when `path` does not exist.
pub fn load_or_default(path: &Path) -> Result<Config, ConfigError> {
    load_or_default_with_home(path, &home_dir())
}

/// [`load`] with an explicit home directory for `~` expansion and default paths.
pub fn load_with_home(path: &Path, home: &Path) -> Result<Config, ConfigError> {
    load_report_with_home(path, home).map(|l| l.config)
}

/// [`load_or_default`] with an explicit home directory.
pub fn load_or_default_with_home(path: &Path, home: &Path) -> Result<Config, ConfigError> {
    load_or_default_report_with_home(path, home).map(|l| l.config)
}

/// [`load_or_default`] with the load warnings ([`Loaded`]).
pub fn load_or_default_report(path: &Path) -> Result<Loaded, ConfigError> {
    load_or_default_report_with_home(path, &home_dir())
}

/// [`load_with_home`] with the load warnings ([`Loaded`]).
pub fn load_report_with_home(path: &Path, home: &Path) -> Result<Loaded, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|cause| ConfigError::Read {
        path: path.to_owned(),
        cause,
    })?;
    parse_report(&text, home).map_err(|cause| ConfigError::Parse {
        path: path.to_owned(),
        cause: Box::new(cause),
    })
}

/// [`load_or_default_with_home`] with the load warnings ([`Loaded`]). A missing file gives the
/// defaults and no warnings.
pub fn load_or_default_report_with_home(path: &Path, home: &Path) -> Result<Loaded, ConfigError> {
    match load_report_with_home(path, home) {
        Err(e) if e.is_not_found() => Ok(Loaded {
            config: defaults_for_home(home),
            warnings: Vec::new(),
        }),
        other => other,
    }
}

/// Parses TOML text into a [`Config`] and expands `~` against `home`. Pure. [`parse_report`]
/// without the warnings.
pub fn parse_str(text: &str, home: &Path) -> Result<Config, toml::de::Error> {
    parse_report(text, home).map(|l| l.config)
}

/// Parses TOML text into a [`Config`], expands `~` against `home`, and reports what it fixed or
/// skipped. Pure.
///
/// Expanded fields: `daemon.download_dir`, `daemon.trash_dir`, `daemon.socket_path` (Go left this
/// one unexpanded, which made a literal `~` directory), `daemon.manual_dir`, `favorites.dir`, and the
/// `location` of local `[[sources]]` rows (`folder`, `images`, `album-name`, `album-date`).
///
/// A legacy config (`[light]`/`[dark]`, provider `enabled` flags, …) is migrated in memory first
/// ([`migrate_document`]), so every reader sees `sources`. Source ids are then normalised
/// ([`normalize_sources`]): empty, invalid and duplicate ids get deterministic replacements, each
/// with a warning. A row whose `type` is missing or unknown loads as
/// [`SourceType::Unsupported`](crate::SourceType::Unsupported), which the engine skips, also with
/// a warning.
pub fn parse_report(text: &str, home: &Path) -> Result<Loaded, toml::de::Error> {
    let migrated = migrated_text(text, home);
    let text = migrated.as_deref().unwrap_or(text);
    let mut cfg: Config = toml::from_str(text)?;
    let mut warnings = normalize_sources(&mut cfg.sources);
    warnings.extend(type_warnings(text, &cfg));
    // `#[serde(default)]` fills missing keys from `Default`, which reads the real home dir. Rebase
    // those defaults onto `home` by comparing against the real-home defaults first.
    rebase_defaults(&mut cfg, home);
    expand_all(&mut cfg, home);
    Ok(Loaded {
        config: cfg,
        warnings,
    })
}

/// One warning per `[[sources]]` row of `text` that loaded as `unsupported`, naming the row by
/// its (normalised) id in `cfg`. `text` already parsed as `cfg`, so its rows line up with
/// `cfg.sources`.
fn type_warnings(text: &str, cfg: &Config) -> Vec<String> {
    if !cfg
        .sources
        .iter()
        .any(|s| s.kind == crate::SourceType::Unsupported)
    {
        return Vec::new();
    }
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return Vec::new();
    };
    let raw_types = crate::edit::raw_source_types(&doc);
    cfg.sources
        .iter()
        .zip(raw_types)
        .filter(|(src, _)| src.kind == crate::SourceType::Unsupported)
        .map(
            |(src, raw)| match raw.as_ref().and_then(toml_edit::Item::as_str) {
                None if raw.is_none() => format!("source {:?}: missing type; skipped", src.id),
                None => format!("source {:?}: type is not a string; skipped", src.id),
                Some(kind) => format!("source {:?}: unknown type {kind:?}; skipped", src.id),
            },
        )
        .collect()
}

/// `text` migrated to `[[sources]]`, or `None` when it isn't legacy (or isn't valid TOML, which
/// `toml::from_str` then reports).
fn migrated_text(text: &str, home: &Path) -> Option<String> {
    let mut doc: DocumentMut = text.parse().ok()?;
    migrate_document(&mut doc, home)?;
    Some(doc.to_string())
}

// ── Migrating the file on disk ──────────────────────────────────────────────

/// A migration that [`migrate_file`] or [`migrate_config`] wrote to disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMigration {
    pub report: MigrationReport,
    /// The rewritten file.
    pub path: PathBuf,
    /// `config.toml.bak` (or `config.toml.bak.<unix-ts>`), holding the original bytes.
    pub backup: PathBuf,
}

impl FileMigration {
    /// The lines to log at info level, once: what was written where, then the report's notes.
    pub fn messages(&self) -> Vec<String> {
        let head = format!(
            "migrated {} to [[sources]]; the old file is kept as {}",
            self.path.display(),
            self.backup.display()
        );
        std::iter::once(head)
            .chain(self.report.notes.iter().cloned())
            .collect()
    }
}

/// Locks `path` against other writers in this process: [`patch`](crate::edit::patch) and the other
/// migrations (the UI's commands run on a blocking pool). Between processes, the `.bak` claim and
/// the identical (deterministic) output keep it safe.
fn migrate_lock(path: &Path) -> Result<PathLock, ConfigError> {
    PathLock::acquire(path).map_err(|cause| ConfigError::Read {
        path: path.to_owned(),
        cause,
    })
}

/// The file's bytes and its migrated text, or `None` when it is missing, not valid TOML, or has
/// no legacy keys.
fn read_and_migrate(
    path: &Path,
    home: &Path,
) -> Result<Option<(Vec<u8>, DocumentMut, MigrationReport)>, ConfigError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                cause,
            });
        }
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };
    let Ok(mut doc) = text.parse::<DocumentMut>() else {
        return Ok(None);
    };
    let Some(report) = migrate_document(&mut doc, home) else {
        return Ok(None);
    };
    Ok(Some((bytes, doc, report)))
}

/// Rewrites a legacy `path` to `[[sources]]` in place, once. Returns `Ok(None)` and touches
/// nothing when the file is missing, isn't valid TOML or has no legacy keys.
///
/// The original bytes are first kept in `config.toml.bak`, claimed with an atomic create-new. If
/// a `.bak` already holds the same bytes (another process got there first, or an earlier run
/// stopped before the rewrite), it counts as done; a `.bak` with other content is left alone and
/// `config.toml.bak.<unix-ts>` is used. The file is only rewritten (with [`write_atomic`]) after
/// the backup exists. On error the file is unchanged; [`parse_str`] still migrates in memory.
pub fn migrate_file(path: &Path, home: &Path) -> Result<Option<FileMigration>, ConfigError> {
    let _lock = migrate_lock(path)?;
    let Some((original, doc, report)) = read_and_migrate(path, home)? else {
        return Ok(None);
    };
    let backup = keep_backup(path, &original)?;
    // Another process may have rewritten the file while we claimed the backup.
    if std::fs::read(path).ok().as_deref() != Some(original.as_slice()) {
        return Ok(None);
    }
    write_atomic(path, doc.to_string().as_bytes())?;
    Ok(Some(FileMigration {
        report,
        path: path.to_owned(),
        backup,
    }))
}

/// Migrates the config `source` resolved to in place ([`migrate_file`]).
/// [`ConfigSource::Missing`] is a no-op.
pub fn migrate_config(
    paths: &Paths,
    source: &ConfigSource,
) -> Result<Option<FileMigration>, ConfigError> {
    match source {
        ConfigSource::Explicit(p) | ConfigSource::Primary(p) => migrate_file(p, &paths.home),
        ConfigSource::Missing(_) => Ok(None),
    }
}

/// Keeps `original` next to `path` as `<name>.bak`, else `<name>.bak.<unix-ts>`, else
/// `<name>.bak.<unix-ts>-<n>`, and returns the file used. Each candidate is claimed with an atomic
/// create-new; one that already holds `original` is reused, one with other bytes is skipped.
fn keep_backup(path: &Path, original: &[u8]) -> Result<PathBuf, ConfigError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let candidates = std::iter::once(format!("{name}.bak"))
        .chain(std::iter::once(format!("{name}.bak.{ts}")))
        .chain((1..100).map(|n| format!("{name}.bak.{ts}-{n}")));
    for candidate in candidates.map(|c| path.with_file_name(c)) {
        let write_err = |cause| ConfigError::Write {
            path: candidate.clone(),
            cause,
        };
        match open_new(&candidate) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(original).and_then(|()| f.sync_all()) {
                    drop(f);
                    let _ = std::fs::remove_file(&candidate);
                    return Err(write_err(e));
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if holds(&candidate, original) {
                    return Ok(candidate);
                }
            }
            Err(e) => return Err(write_err(e)),
        }
    }
    Err(ConfigError::Write {
        path: path.with_file_name(format!("{name}.bak")),
        cause: io::Error::new(io::ErrorKind::AlreadyExists, "no free backup name"),
    })
}

/// Whether `file` holds exactly `bytes`. A strict prefix may be another process still writing the
/// backup, so it is re-read for up to about a second.
fn holds(file: &Path, bytes: &[u8]) -> bool {
    for _ in 0..50 {
        match std::fs::read(file) {
            Ok(got) if got == bytes => return true,
            Ok(got) if bytes.starts_with(&got) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => return false,
        }
    }
    false
}

/// Opens `path` for writing with create-new semantics, mode 0600 on Unix (the config may hold
/// API keys).
fn open_new(path: &Path) -> io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Replaces default paths computed from the real home with the same paths under `home`.
fn rebase_defaults(cfg: &mut Config, home: &Path) {
    let real = home_dir();
    if real == home {
        return;
    }
    let from = crate::defaults::defaults_for_home(&real);
    let to = defaults_for_home(home);
    let swap = |field: &mut PathBuf, old: &PathBuf, new: &PathBuf| {
        if field == old {
            new.clone_into(field);
        }
    };
    swap(
        &mut cfg.daemon.download_dir,
        &from.daemon.download_dir,
        &to.daemon.download_dir,
    );
    swap(
        &mut cfg.daemon.trash_dir,
        &from.daemon.trash_dir,
        &to.daemon.trash_dir,
    );
    swap(
        &mut cfg.favorites.dir,
        &from.favorites.dir,
        &to.favorites.dir,
    );
    // The default favorites row points at the default favorites dir.
    let old_fav = from.favorites.dir.to_string_lossy();
    let new_fav = to.favorites.dir.to_string_lossy();
    for src in &mut cfg.sources {
        if src.kind.is_local() && src.location == old_fav {
            src.location = new_fav.clone().into_owned();
        }
    }
}

fn expand_all(cfg: &mut Config, home: &Path) {
    expand_in_place(&mut cfg.daemon.download_dir, home);
    expand_in_place(&mut cfg.daemon.trash_dir, home);
    if let Some(sock) = cfg.daemon.socket_path.as_mut() {
        expand_in_place(sock, home);
    }
    if let Some(manual) = cfg.daemon.manual_dir.as_mut() {
        expand_in_place(manual, home);
    }
    expand_in_place(&mut cfg.favorites.dir, home);
    for src in cfg.sources.iter_mut().filter(|s| s.kind.is_local()) {
        if let Some(expanded) = expand_tilde(Path::new(&src.location), home) {
            src.location = expanded.to_string_lossy().into_owned();
        }
    }
}

fn expand_in_place(p: &mut PathBuf, home: &Path) {
    if let Some(expanded) = expand_tilde(p, home) {
        *p = expanded;
    }
}

/// Expands a leading `~` or `~/…` (also `~\…` on Windows) to `home`. Returns `None` if `p` has no
/// leading tilde. `~user` is not supported, as in Go.
pub fn expand_tilde(p: &Path, home: &Path) -> Option<PathBuf> {
    let s = p.to_str()?;
    if s == "~" {
        return Some(home.to_owned());
    }
    let rest = s
        .strip_prefix("~/")
        .or_else(|| cfg!(windows).then(|| s.strip_prefix("~\\")).flatten())?;
    Some(home.join(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_cases() {
        let home = Path::new("/home/u");
        assert_eq!(
            expand_tilde(Path::new("~"), home),
            Some(PathBuf::from("/home/u"))
        );
        assert_eq!(
            expand_tilde(Path::new("~/a/b"), home),
            Some(PathBuf::from("/home/u/a/b"))
        );
        assert_eq!(expand_tilde(Path::new("/abs"), home), None);
        assert_eq!(expand_tilde(Path::new("~user/x"), home), None);
        assert_eq!(expand_tilde(Path::new("rel/~/x"), home), None);
    }
}
