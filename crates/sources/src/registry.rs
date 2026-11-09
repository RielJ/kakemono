//! The provider registry built from config (port of Go `providers.Registry`).
//!
//! [`Registry::candidates`] replaces Go's `Random`: instead of one provider it returns every
//! eligible provider in random order, so the engine can fall back to the next one when a fetch
//! fails. The first element has the same distribution as Go's `Random` pick.

use std::fmt;
use std::sync::Arc;

use kakemono_config::Config;
use rand::seq::SliceRandom;
use reqwest::Client;

use crate::apod::Apod;
use crate::bing::Bing;
use crate::local::Local;
use crate::provider::Provider;
use crate::reddit::Reddit;
use crate::unsplash::Unsplash;
use crate::wallhaven::Wallhaven;

/// Providers keyed by [`Provider::name`], in registration order.
#[derive(Clone, Default)]
pub struct Registry {
    providers: Vec<Arc<dyn Provider>>,
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.providers.iter().map(|p| (p.name(), p.enabled())))
            .finish()
    }
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// All six providers, configured from `cfg`, sharing `http` (build it with
    /// [`crate::http::client`]). Registration order matches Go's daemon: bing, wallhaven, reddit,
    /// unsplash, apod, local. Disabled providers are registered too; [`Registry::candidates`]
    /// skips them.
    pub fn from_config(cfg: &Config, http: Client) -> Self {
        Self::new()
            .with(Arc::new(Bing::new(cfg.bing.clone(), http.clone())))
            .with(Arc::new(Wallhaven::new(
                cfg.wallhaven.clone(),
                http.clone(),
            )))
            .with(Arc::new(Reddit::new(cfg.reddit.clone(), http.clone())))
            .with(Arc::new(Unsplash::new(cfg.unsplash.clone(), http.clone())))
            .with(Arc::new(Apod::new(cfg.apod.clone(), http)))
            .with(Arc::new(Local::new(cfg.local.clone())))
    }

    /// Adds `provider` under its name. A provider with the same name is replaced in place
    /// (Go replaced the map entry).
    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        match self
            .providers
            .iter_mut()
            .find(|p| p.name() == provider.name())
        {
            Some(slot) => *slot = provider,
            None => self.providers.push(provider),
        }
    }

    /// [`Registry::register`] in builder form.
    #[must_use]
    pub fn with(mut self, provider: Arc<dyn Provider>) -> Self {
        self.register(provider);
        self
    }

    /// The provider registered under `name` (exact match), enabled or not.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.iter().find(|p| p.name() == name).cloned()
    }

    /// Every registered name, in registration order.
    pub fn names(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    /// Every enabled provider, in registration order.
    pub fn enabled(&self) -> Vec<Arc<dyn Provider>> {
        self.providers
            .iter()
            .filter(|p| p.enabled())
            .cloned()
            .collect()
    }

    /// The providers to try for a theme whose source list is `sources`, best first, using the
    /// thread RNG. See [`Registry::candidates_with_rng`].
    pub fn candidates(&self, sources: &[String]) -> Vec<Arc<dyn Provider>> {
        self.candidates_with_rng(sources, &mut rand::rng())
    }

    /// The providers to try for `sources`, in random order (Go `Random` semantics):
    ///
    /// - `sources` is shuffled; each name that is registered and enabled is kept once, at its
    ///   first position. Unknown names and disabled providers are skipped. A name listed twice is
    ///   twice as likely to come first, like Go's pick from the shuffled list.
    /// - If that leaves nothing (no names, or none enabled), every enabled provider is returned,
    ///   shuffled (Go's "any enabled provider" fallback).
    /// - The named list never grows with unlisted providers: a theme that names only `apod`
    ///   doesn't fall back to `bing` when apod fails.
    ///
    /// Empty only when no provider is enabled.
    pub fn candidates_with_rng<R: rand::Rng + ?Sized>(
        &self,
        sources: &[String],
        rng: &mut R,
    ) -> Vec<Arc<dyn Provider>> {
        let mut names: Vec<&str> = sources.iter().map(String::as_str).collect();
        names.shuffle(rng);

        let mut named: Vec<Arc<dyn Provider>> = Vec::with_capacity(names.len());
        for name in names {
            if named.iter().any(|p| p.name() == name) {
                continue;
            }
            match self.get(name) {
                Some(p) if p.enabled() => named.push(p),
                Some(_) => tracing::debug!(source = name, "source is disabled; skipped"),
                None => tracing::debug!(source = name, "unknown source; skipped"),
            }
        }
        if !named.is_empty() {
            return named;
        }

        let mut all = self.enabled();
        if !sources.is_empty() && !all.is_empty() {
            tracing::debug!(
                ?sources,
                "no listed source is enabled; using any enabled provider"
            );
        }
        all.shuffle(rng);
        all
    }

    /// One provider for `sources`: the first of [`Registry::candidates`] (Go `Random`).
    pub fn random(&self, sources: &[String]) -> Option<Arc<dyn Provider>> {
        self.candidates(sources).into_iter().next()
    }
}
