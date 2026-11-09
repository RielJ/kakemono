//! `StaticProvider`: a scripted [`Provider`] for engine and registry tests (behind the `fake`
//! feature).

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use crate::provider::{Provider, ProviderError, WallpaperResult};

/// A [`Provider`] that returns fixed results (or a fixed error) and records every query.
#[derive(Debug)]
pub struct StaticProvider {
    name: &'static str,
    enabled: bool,
    outcome: Outcome,
    queries: Mutex<Vec<String>>,
}

#[derive(Debug)]
enum Outcome {
    Results(Vec<WallpaperResult>),
    Error(String),
}

impl StaticProvider {
    /// An enabled provider whose `fetch` always returns `results`.
    pub fn new(name: &'static str, results: Vec<WallpaperResult>) -> Self {
        Self {
            name,
            enabled: true,
            outcome: Outcome::Results(results),
            queries: Mutex::new(Vec::new()),
        }
    }

    /// An enabled provider whose `fetch` always fails with `ProviderError::Other(message)`.
    pub fn failing(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            enabled: true,
            outcome: Outcome::Error(message.into()),
            queries: Mutex::new(Vec::new()),
        }
    }

    /// Same provider with `enabled()` returning `enabled`.
    #[must_use]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// One result with `source = name` and the given image URL; other fields derived from it.
    pub fn result(name: &str, image_url: &str) -> WallpaperResult {
        WallpaperResult {
            source_url: format!("https://example.test/{name}"),
            image_url: image_url.to_owned(),
            title: format!("{name} wallpaper"),
            author: String::new(),
            source: name.to_owned(),
        }
    }

    /// Every query passed to `fetch`, in call order.
    pub fn queries(&self) -> Vec<String> {
        self.queries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Number of `fetch` calls.
    pub fn calls(&self) -> usize {
        self.queries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[async_trait]
impl Provider for StaticProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        self.queries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(query.to_owned());
        match &self.outcome {
            Outcome::Results(r) => Ok(r.clone()),
            Outcome::Error(m) => Err(ProviderError::Other(m.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_provider_scripts_results_and_errors() {
        let ok = StaticProvider::new(
            "bing",
            vec![StaticProvider::result("bing", "https://i/1.jpg")],
        );
        assert_eq!(ok.fetch("q").await.unwrap()[0].source, "bing");
        assert_eq!(ok.queries(), ["q"]);

        let bad = StaticProvider::failing("apod", "apod: boom").with_enabled(false);
        assert!(!bad.enabled());
        assert_eq!(bad.fetch("").await.unwrap_err().to_string(), "apod: boom");
        assert_eq!(bad.calls(), 1);
    }
}
