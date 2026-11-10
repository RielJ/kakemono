//! Crate error type. Messages keep Go's `config: …` wording where Go had one.
//!
//! A variant's message ends with its cause (`cause`), so the cause is not also exposed as
//! [`std::error::Error::source`]: `{e:#}` (anyhow) would print it twice. Callers that show
//! `e.to_string()`, such as the UI, still get the whole message.

use std::io;
use std::path::PathBuf;

/// Errors from loading, initialising and editing `config.toml`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read (including "not found" from [`crate::load::load`]).
    #[error("config: cannot read {}: {cause}", path.display())]
    Read { path: PathBuf, cause: io::Error },
    /// The file is not valid TOML or does not match the schema.
    #[error("config: cannot parse {}: {cause}", path.display())]
    Parse {
        path: PathBuf,
        cause: Box<toml::de::Error>,
    },
    /// `init` refuses to overwrite an existing file (Go's message).
    #[error("config: {} already exists; remove it first to re-initialise", path.display())]
    AlreadyExists { path: PathBuf },
    /// The parent directory could not be created.
    #[error("config: cannot create config dir: {cause}")]
    CreateDir { path: PathBuf, cause: io::Error },
    /// The file could not be written.
    #[error("config: cannot write {}: {cause}", path.display())]
    Write { path: PathBuf, cause: io::Error },
    /// The existing file is not valid TOML, so it can't be edited without losing content.
    #[error("config: cannot edit {}: {cause}", path.display())]
    EditParse {
        path: PathBuf,
        cause: Box<toml_edit::TomlError>,
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
        matches!(self, ConfigError::Read { cause, .. } if cause.kind() == io::ErrorKind::NotFound)
    }
}
