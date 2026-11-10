//! GNOME, Cinnamon and MATE: set the wallpaper with the `gsettings` command.
//!
//! Each `set` runs one `gsettings set <schema> <key> <value>` per key, in order, with no shell:
//!
//! | Backend | Keys |
//! |---|---|
//! | `gnome` | `org.gnome.desktop.background` `picture-uri`, `picture-uri-dark` ({uri}), `picture-options` |
//! | `cinnamon` | `org.cinnamon.desktop.background` `picture-uri` ({uri}), `picture-options` |
//! | `mate` | `org.mate.background` `picture-filename` ({path}), `picture-options` |
//!
//! GNOME older than 42 has no `picture-uri-dark` key. That one step may fail with "No such key"
//! and `set` still succeeds. Plain argv code, so it compiles (and is tested) on every Unix.

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::api::{BackendInfo, Fit, SetError, Theme, WallpaperSetter};
use crate::exec::{CommandRunner, Invocation, RunOptions, Vars};
use crate::linux::backends::command::render_error;
use crate::linux::detect::resolve::BackendKind;

/// The command every step runs.
pub const GSETTINGS: &str = "gsettings";

/// GNOME's background schema.
pub const GNOME_SCHEMA: &str = "org.gnome.desktop.background";
/// Cinnamon's background schema.
pub const CINNAMON_SCHEMA: &str = "org.cinnamon.desktop.background";
/// MATE's background schema. UNVERIFIED against a real MATE: `picture-options` is assumed to
/// live in this schema next to `picture-filename`.
pub const MATE_SCHEMA: &str = "org.mate.background";

/// The GNOME 42+ key that GNOME 41 and older lack.
const OPTIONAL_KEY: &str = "picture-uri-dark";

/// The backends this module handles.
pub const GSETTINGS_KINDS: [BackendKind; 3] =
    [BackendKind::Gnome, BackendKind::Cinnamon, BackendKind::Mate];

/// The `picture-options` value for `fit`. The three schemas share the enum.
pub fn picture_options(fit: Fit) -> &'static str {
    match fit {
        Fit::Fill => "zoom",
        Fit::Fit => "scaled",
        Fit::Center => "centered",
        Fit::Stretch => "stretched",
        Fit::Tile => "wallpaper",
    }
}

/// The schema `kind` writes to, or `None` if `kind` is not a gsettings backend.
pub fn schema(kind: BackendKind) -> Option<&'static str> {
    match kind {
        BackendKind::Gnome => Some(GNOME_SCHEMA),
        BackendKind::Cinnamon => Some(CINNAMON_SCHEMA),
        BackendKind::Mate => Some(MATE_SCHEMA),
        _ => None,
    }
}

/// Every `gsettings set` argv for one `set`, in run order. `None` if `kind` is not a gsettings
/// backend. Fails for a relative or non-UTF-8 path.
pub fn argvs(
    kind: BackendKind,
    image: &Path,
    fit: Fit,
) -> Option<Result<Vec<Vec<String>>, SetError>> {
    let schema = schema(kind)?;
    let vars = match Vars::new(image, fit, Theme::Light) {
        Ok(vars) => vars,
        Err(err) => return Some(Err(render_error(err))),
    };
    let image_keys: &[(&str, &str)] = match kind {
        BackendKind::Gnome => &[("picture-uri", vars.uri()), (OPTIONAL_KEY, vars.uri())],
        BackendKind::Cinnamon => &[("picture-uri", vars.uri())],
        _ => &[("picture-filename", vars.path())],
    };
    let set = |key: &str, value: &str| {
        [GSETTINGS, "set", schema, key, value]
            .map(str::to_owned)
            .to_vec()
    };
    let mut out: Vec<Vec<String>> = image_keys.iter().map(|(k, v)| set(k, v)).collect();
    out.push(set("picture-options", picture_options(fit)));
    Some(Ok(out))
}

/// True if `argv` sets a key that may be missing, and `stderr` says it is.
fn missing_optional_key(argv: &[String], stderr: &str) -> bool {
    argv.get(3).is_some_and(|key| key == OPTIONAL_KEY) && stderr.contains("No such key")
}

/// A [`WallpaperSetter`] for `gnome`, `cinnamon` and `mate`.
#[derive(Clone)]
pub struct GsettingsSetter {
    kind: BackendKind,
    runner: Arc<dyn CommandRunner>,
    run: RunOptions,
}

impl fmt::Debug for GsettingsSetter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GsettingsSetter")
            .field("kind", &self.kind)
            .field("run", &self.run)
            .finish_non_exhaustive()
    }
}

impl GsettingsSetter {
    /// The setter for `kind`, or `None` if `kind` is not `gnome`, `cinnamon` or `mate`.
    pub fn new(kind: BackendKind, runner: Arc<dyn CommandRunner>) -> Option<Self> {
        schema(kind)?;
        Some(Self {
            kind,
            runner,
            run: RunOptions::default(),
        })
    }

    /// Set a variable on every run (e.g. `PATH` in tests).
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.run = self.run.with_env(key, value);
        self
    }

    /// Per-command timeout (default [`crate::exec::DEFAULT_TIMEOUT`]).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.run = self.run.with_timeout(timeout);
        self
    }

    pub fn kind(&self) -> BackendKind {
        self.kind
    }

    /// The argvs `set(image, fit)` would run (for `doctor`'s dry run).
    pub fn render(&self, image: &Path, fit: Fit) -> Result<Vec<Vec<String>>, SetError> {
        argvs(self.kind, image, fit).unwrap_or_else(|| Ok(Vec::new()))
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for GsettingsSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        for argv in self.render(image, fit)? {
            let invocation = self.run.apply(Invocation::new(argv));
            tracing::debug!(backend = %self.kind, argv = ?invocation.argv, "running gsettings");
            let output = self.runner.run(&invocation).await?;
            if !output.status.success() && missing_optional_key(&invocation.argv, &output.stderr) {
                tracing::debug!("no {OPTIONAL_KEY} key (GNOME < 42); skipped");
                continue;
            }
            output.check(GSETTINGS)?;
        }
        Ok(())
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: self.kind.as_str().to_owned(),
            detail: format!("gsettings set {}", schema(self.kind).unwrap_or_default()),
        }
    }
}
