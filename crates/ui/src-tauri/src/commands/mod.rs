//! Tauri commands for the settings window and the picker.
//!
//! Every command is a thin wrapper: daemon calls go through the shared [`DaemonLink`](crate::daemon_link::DaemonLink), config
//! reads and writes through `kakemono-config` (writes use its `toml_edit` writer, so comments
//! and formatting survive). The file-system helpers are plain functions so tests can run them
//! against temp dirs.
//!
//! Errors reach the frontend as `{ kind, message }` ([`CommandError`]).
//!
//! - `config`: `get_config`, `save_config` and the reload after a save.
//! - `control`: status, next/prev, pause/resume, favorite, trash, `set_file`.
//! - `platform`: `reveal`, `open_folder`, `app_info`.
//! - `listing`: the picker tabs (`tab_thumbnails`), the Settings card's `current_thumbnail`,
//!   and the folders they read.

use crate::daemon_link::LinkError;
use crate::error::{ErrorKind, Kinded, serialize_as_wire_error};
use kakemono_config::paths::{ConfigSource, Paths};
use kakemono_config::{Config, ConfigError};

mod config;
mod control;
mod listing;
mod platform;

pub use config::*;
pub use control::*;
pub use listing::*;
pub use platform::*;

/// A failed command, serialised for the frontend as `{ "kind": "...", "message": "..." }`.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Link(#[from] LinkError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Bad input from the frontend (empty path, invalid interval, non-object patch).
    #[error("{0}")]
    Invalid(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Opener(String),
    /// Copying a picker file into the favourites dir failed.
    #[error("copying to favorites: {0}: {reason}", reason = .0.io_error())]
    Favorite(#[from] kakemono_fs::CopyError),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

impl CommandError {
    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl Kinded for CommandError {
    /// `daemon_not_running` lets the UI show "Daemon not running" instead of an error.
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Link(err) => err.kind(),
            Self::Config(_) => ErrorKind::Config,
            Self::Invalid(_) => ErrorKind::Invalid,
            Self::Io { .. } | Self::Favorite(_) => ErrorKind::Io,
            Self::Opener(_) => ErrorKind::Opener,
            Self::Tauri(_) => ErrorKind::Internal,
        }
    }
}

serialize_as_wire_error!(CommandError);

pub(crate) type CmdResult<T> = Result<T, CommandError>;

/// Where the config lives right now: `$KAKEMONO_CONFIG`, the kakemono file, or (missing) the
/// kakemono path. Edits target the same file.
pub(crate) fn locate_config() -> CmdResult<(Paths, ConfigSource)> {
    let paths = Paths::detect()?;
    let source = paths.locate_config(None);
    Ok((paths, source))
}

pub(crate) fn load_config() -> CmdResult<Config> {
    let (_, source) = locate_config()?;
    Ok(kakemono_config::load_or_default(source.path())?)
}

/// Runs blocking file work off the async runtime. A failed join becomes `E`'s `tauri::Error`.
pub(crate) async fn blocking<T, E, F>(f: F) -> Result<T, E>
where
    T: Send + 'static,
    E: From<tauri::Error> + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn errors_serialise_with_a_kind() {
        let err = CommandError::Invalid("nope".into());
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({ "kind": "invalid", "message": "nope" })
        );
    }
}
