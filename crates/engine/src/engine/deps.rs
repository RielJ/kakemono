//! [`Deps`]: what the engine needs from outside, with test seams.

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Local};
use kakemono_config::Config;
use kakemono_platform::api::Platform;
use kakemono_platform::exec::{CommandRunner, TokioRunner};
use kakemono_sources::SourceSet;
use reqwest::Client;

/// Wall-clock source. Used for `WallpaperInfo.set_at` and `state.last_changed`.
pub type Clock = Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>;

/// Builds the online sources for a config. Called by [`Engine::new`](crate::Engine::new) and on every
/// [`Engine::reload`](crate::Engine::reload), so `[[sources]]` edits and provider keys take effect on reload.
pub type SourcesFactory = Arc<dyn Fn(&Config, Client) -> SourceSet + Send + Sync>;

/// Everything the engine needs from outside. Cheap to clone.
#[derive(Clone)]
pub struct Deps {
    /// The wallpaper setter and system signals (`signals.theme()` picks the sources). The
    /// setter is the first one; `reload` may replace it ([`Engine::setter`](crate::Engine::setter)).
    pub platform: Platform,
    /// Shared HTTP client for providers and downloads (build with
    /// `kakemono_sources::http::client`).
    pub http: Client,
    /// Builds the online sources from config. Default: `SourceSet::from_config`.
    pub sources: SourcesFactory,
    /// Runs external commands (hooks, Task 22). Default: `TokioRunner`.
    pub runner: Arc<dyn CommandRunner>,
    /// Wall clock. Default: local time.
    pub now: Clock,
}

impl Deps {
    /// Production defaults: the real online sources, `TokioRunner`, local time.
    pub fn new(platform: Platform, http: Client) -> Self {
        Self {
            platform,
            http,
            sources: Arc::new(SourceSet::from_config),
            runner: Arc::new(TokioRunner),
            now: Arc::new(|| Local::now().fixed_offset()),
        }
    }

    /// Use `factory` to build the online sources (on construction and every reload).
    #[must_use]
    pub fn with_sources_factory(
        mut self,
        factory: impl Fn(&Config, Client) -> SourceSet + Send + Sync + 'static,
    ) -> Self {
        self.sources = Arc::new(factory);
        self
    }

    /// Use `sources` regardless of config (tests). Reloads keep it.
    #[must_use]
    pub fn with_sources(self, sources: SourceSet) -> Self {
        self.with_sources_factory(move |_, _| sources.clone())
    }

    /// Use `runner` for external commands.
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn CommandRunner>) -> Self {
        self.runner = runner;
        self
    }

    /// Use `now` as the wall clock.
    #[must_use]
    pub fn with_clock(
        mut self,
        now: impl Fn() -> DateTime<FixedOffset> + Send + Sync + 'static,
    ) -> Self {
        self.now = Arc::new(now);
        self
    }
}

impl fmt::Debug for Deps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Deps")
            .field("setter", &self.platform.setter.info())
            .finish_non_exhaustive()
    }
}
