//! Loading `config.toml` over the defaults, with `~` expansion (Go `config.Load`/`LoadOrDefault`).

use std::path::{Path, PathBuf};

use crate::defaults::{defaults_for_home, home_dir};
use crate::error::ConfigError;
use crate::schema::Config;

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
/// one unexpanded, which made a literal `~` directory), `favorites.dir`, `local.dirs`.
pub fn parse_str(text: &str, home: &Path) -> Result<Config, toml::de::Error> {
    // `#[serde(default)]` fills missing keys from `Default`, which reads the real home dir. Rebase
    // those defaults onto `home` by comparing against the real-home defaults first.
    let mut cfg: Config = toml::from_str(text)?;
    rebase_defaults(&mut cfg, home);
    expand_all(&mut cfg, home);
    Ok(cfg)
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
}

fn expand_all(cfg: &mut Config, home: &Path) {
    expand_in_place(&mut cfg.daemon.download_dir, home);
    expand_in_place(&mut cfg.daemon.trash_dir, home);
    if let Some(sock) = cfg.daemon.socket_path.as_mut() {
        expand_in_place(sock, home);
    }
    expand_in_place(&mut cfg.favorites.dir, home);
    for dir in &mut cfg.local.dirs {
        expand_in_place(dir, home);
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
