//! Loading `config.toml` over the defaults, with `~` expansion (Go `config.Load`/`LoadOrDefault`).

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use toml_edit::DocumentMut;

use crate::defaults::{defaults_for_home, home_dir};
use crate::edit::write_atomic;
use crate::error::ConfigError;
use crate::migrate::{MigrationReport, migrate_document};
use crate::paths::{ConfigSource, Paths, legacy_default_socket};
use crate::schema::Config;
use crate::sources::normalize_sources;

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
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    parse_str(&text, home).map_err(|source| ConfigError::Parse {
        path: path.to_owned(),
        source: Box::new(source),
    })
}

/// [`load_or_default`] with an explicit home directory.
pub fn load_or_default_with_home(path: &Path, home: &Path) -> Result<Config, ConfigError> {
    match load_with_home(path, home) {
        Err(e) if e.is_not_found() => Ok(defaults_for_home(home)),
        other => other,
    }
}

/// Parses TOML text into a [`Config`] and expands `~` against `home`. Pure.
///
/// Expanded fields: `daemon.download_dir`, `daemon.trash_dir`, `daemon.socket_path` (Go left this
/// one unexpanded, which made a literal `~` directory), `daemon.manual_dir`, `favorites.dir`, and the
/// `location` of local `[[sources]]` rows (`folder`, `images`, `album-name`, `album-date`).
///
/// A legacy config (`[light]`/`[dark]`, provider `enabled` flags, …) is migrated in memory first
/// ([`migrate_document`]), so every reader sees `sources`. Source ids are then normalised
/// ([`normalize_sources`]): empty, invalid and duplicate ids get deterministic replacements.
pub fn parse_str(text: &str, home: &Path) -> Result<Config, toml::de::Error> {
    let migrated = migrated_text(text, home);
    // `#[serde(default)]` fills missing keys from `Default`, which reads the real home dir. Rebase
    // those defaults onto `home` by comparing against the real-home defaults first.
    let mut cfg: Config = toml::from_str(migrated.as_deref().unwrap_or(text))?;
    normalize_sources(&mut cfg.sources);
    rebase_defaults(&mut cfg, home);
    expand_all(&mut cfg, home);
    Ok(cfg)
}

/// `text` migrated to `[[sources]]`, or `None` when it isn't legacy (or isn't valid TOML, which
/// `toml::from_str` then reports).
fn migrated_text(text: &str, home: &Path) -> Option<String> {
    let mut doc: DocumentMut = text.parse().ok()?;
    migrate_document(&mut doc, home)?;
    Some(doc.to_string())
}

// ── Migrating the file on disk ──────────────────────────────────────────────

/// Where [`migrate_file`], [`migrate_legacy_file`] or [`migrate_config`] wrote the result and
/// where the original bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationTarget {
    /// `path` was rewritten; `backup` (`config.toml.bak` or `config.toml.bak.<unix-ts>`) holds
    /// the original bytes.
    InPlace { path: PathBuf, backup: PathBuf },
    /// The Go `wallpaperd` file `from` was migrated into the kakemono file `to`; `from` is left
    /// as is and serves as the backup.
    Legacy { from: PathBuf, to: PathBuf },
}

/// A migration that was written to disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMigration {
    pub report: MigrationReport,
    pub target: MigrationTarget,
}

impl FileMigration {
    /// The lines to log at info level, once: what was written where, then the report's notes.
    pub fn messages(&self) -> Vec<String> {
        let head = match &self.target {
            MigrationTarget::InPlace { path, backup } => format!(
                "migrated {} to [[sources]]; the old file is kept as {}",
                path.display(),
                backup.display()
            ),
            MigrationTarget::Legacy { from, to } => format!(
                "migrated {} to {}; the old file is left as is",
                from.display(),
                to.display()
            ),
        };
        std::iter::once(head)
            .chain(self.report.notes.iter().cloned())
            .collect()
    }
}

/// Serialises migrations within this process. [`write_atomic`] names its temp file by PID, so two
/// threads (the UI's commands run on a blocking pool) must not write the same file at once.
/// Between processes, the `.bak` claim and the identical (deterministic) output keep it safe.
static MIGRATE_LOCK: Mutex<()> = Mutex::new(());

fn migrate_lock() -> MutexGuard<'static, ()> {
    MIGRATE_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
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
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
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
    let _guard = migrate_lock();
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
        target: MigrationTarget::InPlace {
            path: path.to_owned(),
            backup,
        },
    }))
}

/// Migrates the Go `wallpaperd` file `legacy` into the kakemono file `primary`, leaving `legacy`
/// byte-for-byte as is (it is the backup). `primary` is created atomically and never overwritten:
/// if it exists, or appears meanwhile, this returns `Ok(None)`. A `daemon.socket_path` equal to
/// the Go default socket is dropped, so the kakemono default socket keeps applying.
pub fn migrate_legacy_file(
    legacy: &Path,
    primary: &Path,
    home: &Path,
) -> Result<Option<FileMigration>, ConfigError> {
    let _guard = migrate_lock();
    if primary.exists() {
        return Ok(None);
    }
    let Some((_, mut doc, mut report)) = read_and_migrate(legacy, home)? else {
        return Ok(None);
    };
    if let Some(note) = drop_legacy_socket(&mut doc, home) {
        report.notes.push(note);
    }
    if !create_atomic(primary, doc.to_string().as_bytes())? {
        return Ok(None);
    }
    Ok(Some(FileMigration {
        report,
        target: MigrationTarget::Legacy {
            from: legacy.to_owned(),
            to: primary.to_owned(),
        },
    }))
}

/// Migrates the config `source` resolved to: [`ConfigSource::Explicit`] and
/// [`ConfigSource::Primary`] in place ([`migrate_file`]), [`ConfigSource::Legacy`] into
/// `paths.config_file` ([`migrate_legacy_file`]). [`ConfigSource::Missing`] is a no-op. Locate the
/// config again afterwards: a migrated legacy config now resolves to `Primary`.
pub fn migrate_config(
    paths: &Paths,
    source: &ConfigSource,
) -> Result<Option<FileMigration>, ConfigError> {
    match source {
        ConfigSource::Explicit(p) | ConfigSource::Primary(p) => migrate_file(p, &paths.home),
        ConfigSource::Legacy(p) => migrate_legacy_file(p, &paths.config_file, &paths.home),
        ConfigSource::Missing(_) => Ok(None),
    }
}

/// Removes `daemon.socket_path` when it is the Go daemon's socket (`~/.config/wallpaperd/…`), and
/// returns a note saying so.
fn drop_legacy_socket(doc: &mut DocumentMut, home: &Path) -> Option<String> {
    let daemon = doc.get_mut("daemon")?.as_table_like_mut()?;
    let socket = daemon.get("socket_path")?.as_str()?.to_owned();
    let expanded = expand_tilde(Path::new(&socket), home).unwrap_or_else(|| socket.clone().into());
    if expanded != legacy_default_socket(home) {
        return None;
    }
    daemon.remove("socket_path");
    Some(format!(
        "dropped daemon.socket_path {socket:?} (the wallpaperd socket); kakemono uses its own"
    ))
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
        let write_err = |source| ConfigError::Write {
            path: candidate.clone(),
            source,
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
        source: io::Error::new(io::ErrorKind::AlreadyExists, "no free backup name"),
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

/// Creates `path` with `bytes`, never replacing an existing file, and never showing a partial
/// file: the bytes go to a temp file first, which is then hard-linked into place (an atomic
/// create-new). Where hard links aren't supported, falls back to a create-new write. Returns
/// `false` when `path` already exists.
fn create_atomic(path: &Path, bytes: &[u8]) -> Result<bool, ConfigError> {
    let write_err = |source| ConfigError::Write {
        path: path.to_owned(),
        source,
    };
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|source| ConfigError::CreateDir {
        path: dir.to_owned(),
        source,
    })?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let tmp = dir.join(format!(".{name}.new.{}", std::process::id()));
    let written =
        open_new_replacing(&tmp).and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(write_err(e));
    }
    let linked = std::fs::hard_link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(_) => match open_new(path) {
            Ok(mut f) => f
                .write_all(bytes)
                .and_then(|()| f.sync_all())
                .map(|()| true)
                .map_err(write_err),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(write_err(e)),
        },
    }
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

/// [`open_new`], after removing a stale `path` (a temp file left by a crashed run of this PID).
fn open_new_replacing(path: &Path) -> io::Result<std::fs::File> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    open_new(path)
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
