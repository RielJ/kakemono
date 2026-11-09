//! Crate error type. Messages keep Go's `config: …` wording where Go had one.

use std::io;
use std::path::PathBuf;

/// Errors from loading, initialising and editing `config.toml`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read (including "not found" from [`crate::load::load`]).
    #[error("config: cannot read {}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    /// The file is not valid TOML or does not match the schema.
    #[error("config: cannot parse {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    /// `init` refuses to overwrite an existing file (Go's message).
    #[error("config: {} already exists; remove it first to re-initialise", path.display())]
    AlreadyExists { path: PathBuf },
    /// The parent directory could not be created.
    #[error("config: cannot create config dir: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    /// The file could not be written.
    #[error("config: cannot write {}: {source}", path.display())]
    Write { path: PathBuf, source: io::Error },
    /// The existing file is not valid TOML, so it can't be edited without losing content.
    #[error("config: cannot edit {}: {source}", path.display())]
    EditParse {
        path: PathBuf,
        source: Box<toml_edit::TomlError>,
    },
    /// A patch was rejected (bad shape, or the result would not load).
    #[error("config: invalid patch for {}: {reason}", path.display())]
    InvalidPatch { path: PathBuf, reason: String },
    /// No home directory, so default paths can't be computed.
    #[error("config: cannot locate home directory")]
    NoHome,
}

impl ConfigError {
    /// True when the underlying error is "file not found".
    pub fn is_not_found(&self) -> bool {
        matches!(self, ConfigError::Read { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}
