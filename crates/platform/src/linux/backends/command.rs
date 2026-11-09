//! `CommandSetter`: renders an argv template through [`exec`](crate::exec) and runs it to
//! completion with a [`CommandRunner`]. Used for every preset backend except the persistent ones
//! (swaybg, wbg, `persistent = true`), which go through
//! [`persistent::PersistentSetter`](super::persistent::PersistentSetter).

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::api::{BackendInfo, Fit, SetError, Theme, WallpaperSetter};
use crate::exec::{self, CommandRunner, Invocation, RenderError, Vars};
use crate::linux::detect::{BackendKind, Resolution};

use super::presets;

/// Supplies `{theme}` at set time (e.g. `SystemSignals::theme`).
pub type ThemeSource = Arc<dyn Fn() -> Theme + Send + Sync>;

/// Where the argv template comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Template {
    /// The preset table entry for this kind; the fit flag follows the `fit` passed to `set`.
    Preset(BackendKind),
    /// A fixed template (`[desktop] set_command`).
    Fixed(Vec<String>),
}

impl Template {
    /// The template for `resolution`: its `set_command`, else its preset. `None` when there is
    /// neither (API backends, `Unconfigured`, `Command` without `set_command`).
    pub fn for_resolution(resolution: &Resolution) -> Option<Self> {
        if !resolution.backend.uses_command() {
            return None;
        }
        match &resolution.set_command {
            Some(argv) => Some(Self::Fixed(argv.clone())),
            None => presets::argv(resolution.backend, Fit::Fill)
                .map(|_| Self::Preset(resolution.backend)),
        }
    }

    /// Check the template: non-empty, known placeholders, and (for `Preset`) a preset exists.
    pub fn validate(&self) -> Result<(), RenderError> {
        match self {
            Self::Preset(kind) => {
                let argv = presets::argv(*kind, Fit::Fill).ok_or(RenderError::EmptyCommand)?;
                exec::validate(&argv)
            }
            Self::Fixed(argv) => exec::validate(argv),
        }
    }

    /// The argv template for `fit` (placeholders not yet substituted).
    pub fn template(&self, fit: Fit) -> Vec<String> {
        match self {
            Self::Preset(kind) => presets::argv(*kind, fit).unwrap_or_default(),
            Self::Fixed(argv) => argv.clone(),
        }
    }

    /// Substitute `{path}`, `{uri}`, `{fit}` and `{theme}`. Fails for a relative or non-UTF-8
    /// path, or an empty template.
    pub fn render(&self, image: &Path, fit: Fit, theme: Theme) -> Result<Vec<String>, SetError> {
        let template = self.template(fit);
        if template.is_empty() {
            return Err(render_error(RenderError::EmptyCommand));
        }
        let vars = Vars::new(image, fit, theme).map_err(render_error)?;
        Ok(exec::render(&template, &vars))
    }
}

/// A bad image path or template, as a `SetError`.
pub fn render_error(err: RenderError) -> SetError {
    match err {
        RenderError::EmptyCommand => SetError::Unconfigured {
            hint: "`[desktop] set_command` is empty".to_owned(),
        },
        other => SetError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            other.to_string(),
        )),
    }
}

/// Runs a rendered argv per `set` and waits for it (default timeout [`exec::DEFAULT_TIMEOUT`]).
#[derive(Clone)]
pub struct CommandSetter {
    name: String,
    template: Template,
    runner: Arc<dyn CommandRunner>,
    env: Vec<(String, String)>,
    timeout: Duration,
    theme: ThemeSource,
}

impl fmt::Debug for CommandSetter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandSetter")
            .field("name", &self.name)
            .field("template", &self.template)
            .field("env", &self.env)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl CommandSetter {
    /// A setter for `template`, reported as `name` in [`BackendInfo`]. Validates the template.
    pub fn new(
        name: impl Into<String>,
        template: Template,
        runner: Arc<dyn CommandRunner>,
    ) -> Result<Self, RenderError> {
        template.validate()?;
        Ok(Self {
            name: name.into(),
            template,
            runner,
            env: Vec::new(),
            timeout: exec::DEFAULT_TIMEOUT,
            theme: Arc::new(|| Theme::Light),
        })
    }

    /// The preset for `kind`. `Err(EmptyCommand)` if `kind` has no preset.
    pub fn preset(kind: BackendKind, runner: Arc<dyn CommandRunner>) -> Result<Self, RenderError> {
        Self::new(kind.as_str(), Template::Preset(kind), runner)
    }

    /// The setter for a resolution (its `set_command`, else its preset). `Ok(None)` when it has
    /// no command; `Err` when `set_command` is invalid.
    pub fn for_resolution(
        resolution: &Resolution,
        runner: Arc<dyn CommandRunner>,
    ) -> Result<Option<Self>, RenderError> {
        Template::for_resolution(resolution)
            .map(|template| Self::new(resolution.backend.as_str(), template, runner))
            .transpose()
    }

    /// Set a variable on every run (e.g. `PATH` in tests).
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Where `{theme}` comes from (default: always light).
    #[must_use]
    pub fn with_theme(mut self, theme: ThemeSource) -> Self {
        self.theme = theme;
        self
    }

    pub fn template(&self) -> &Template {
        &self.template
    }

    /// The exact argv `set(image, fit)` would run (for `doctor`'s dry run).
    pub fn render(&self, image: &Path, fit: Fit) -> Result<Vec<String>, SetError> {
        self.template.render(image, fit, (self.theme)())
    }
}

#[async_trait::async_trait]
impl WallpaperSetter for CommandSetter {
    async fn set(&self, image: &Path, fit: Fit) -> Result<(), SetError> {
        let argv = self.render(image, fit)?;
        let invocation = Invocation::new(argv)
            .envs(self.env.iter().cloned())
            .timeout(self.timeout);
        tracing::debug!(backend = %self.name, argv = ?invocation.argv, "running wallpaper command");
        exec::run_checked(self.runner.as_ref(), &invocation).await?;
        Ok(())
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: self.name.clone(),
            detail: shell_words_join(&self.template.template(Fit::Fill)),
        }
    }
}

/// Display-only argv join: tokens with spaces or quotes are single-quoted.
pub(crate) fn shell_words_join(argv: &[String]) -> String {
    argv.iter()
        .map(|t| {
            if !t.is_empty()
                && t.chars()
                    .all(|c| !c.is_whitespace() && !matches!(c, '\'' | '"' | '\\' | '$' | '`'))
            {
                t.clone()
            } else {
                format!("'{}'", t.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_quotes_only_when_needed() {
        let argv: Vec<String> = ["swaymsg", "output", "*", "bg", "{path}", "a b", "it's"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            shell_words_join(&argv),
            r"swaymsg output * bg {path} 'a b' 'it'\''s'"
        );
    }
}
