//! `StaticProvider`: a scripted [`Provider`] for engine and source tests (behind the `fake`
//! feature).

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use kakemono_config::schema::SourceType;

use crate::instance::{OnlineSource, SourceSet};
use crate::provider::{Provider, ProviderError, WallpaperResult};

impl SourceSet {
    /// A set of scripted sources, one per `(id, provider, query)`, in order. Each source's
    /// `kind` is the [`SourceType`] named by `provider.name()` (`unsupported` for other names).
    ///
    /// Callers coerce the provider: `("wh-1", Arc::clone(&p) as Arc<dyn Provider>, "forest")`.
    pub fn fake(sources: Vec<(&str, Arc<dyn Provider>, &str)>) -> Self {
        Self {
            online: sources
                .into_iter()
                .map(|(id, provider, query)| OnlineSource {
                    id: id.to_owned(),
                    kind: SourceType::from_config_str(provider.name()),
                    provider,
                    query: query.to_owned(),
                })
                .collect(),
        }
    }
}

/// A [`Provider`] that returns fixed results (or a fixed error) and records every query.
///
/// `fetch` and `check` give the same outcome but are recorded apart ([`StaticProvider::queries`],
/// [`StaticProvider::checks`]). [`StaticProvider::with_media`] makes it supply its own media
/// client and host list.
#[derive(Debug)]
pub struct StaticProvider {
    name: &'static str,
    outcome: Outcome,
    queries: Mutex<Vec<String>>,
    checks: Mutex<Vec<String>>,
    media: Option<(reqwest::Client, Vec<String>)>,
}

#[derive(Debug)]
enum Outcome {
    Results(Vec<WallpaperResult>),
    Error(String),
}

impl StaticProvider {
    /// A provider whose `fetch` always returns `results`.
    pub fn new(name: &'static str, results: Vec<WallpaperResult>) -> Self {
        Self {
            name,
            outcome: Outcome::Results(results),
            queries: Mutex::new(Vec::new()),
            checks: Mutex::new(Vec::new()),
            media: None,
        }
    }

    /// A provider whose `fetch` always fails with `ProviderError::Other(message)`.
    pub fn failing(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            outcome: Outcome::Error(message.into()),
            queries: Mutex::new(Vec::new()),
            checks: Mutex::new(Vec::new()),
            media: None,
        }
    }

    /// This provider, answering [`Provider::media_client`] with `client` and
    /// [`Provider::allowed_hosts`] with `hosts`.
    #[must_use]
    pub fn with_media(self, client: reqwest::Client, hosts: Vec<String>) -> Self {
        Self {
            media: Some((client, hosts)),
            ..self
        }
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

    /// Every query passed to `check`, in call order.
    pub fn checks(&self) -> Vec<String> {
        self.checks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn answer(&self) -> Result<Vec<WallpaperResult>, ProviderError> {
        match &self.outcome {
            Outcome::Results(r) => Ok(r.clone()),
            Outcome::Error(m) => Err(ProviderError::Other(m.clone())),
        }
    }
}

#[async_trait]
impl Provider for StaticProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        self.queries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(query.to_owned());
        self.answer()
    }

    async fn check(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        self.checks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(query.to_owned());
        self.answer()
    }

    fn allowed_hosts(&self) -> Vec<String> {
        match &self.media {
            Some((_, hosts)) => hosts.clone(),
            None => self.media_hosts().iter().map(|h| (*h).to_owned()).collect(),
        }
    }

    fn media_client(&self) -> Option<reqwest::Client> {
        self.media.as_ref().map(|(client, _)| client.clone())
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

        let bad = StaticProvider::failing("apod", "apod: boom");
        assert_eq!(bad.fetch("").await.unwrap_err().to_string(), "apod: boom");
        assert_eq!(bad.calls(), 1);
    }

    #[tokio::test]
    async fn static_provider_records_check_apart_from_fetch() {
        let p = StaticProvider::new("bing", vec![StaticProvider::result("bing", "https://i/1")]);
        let n = p.check("q").await.unwrap().len();
        assert_eq!((n, p.checks(), p.calls()), (1, vec!["q".to_owned()], 0));
    }

    #[test]
    fn static_provider_with_media_supplies_client_and_hosts() {
        let p = StaticProvider::new("bing", vec![]).with_media(
            crate::http::client_builder().build().unwrap(),
            vec!["cdn.test".into()],
        );
        assert_eq!(
            (p.media_client().is_some(), p.allowed_hosts()),
            (true, vec!["cdn.test".to_owned()])
        );
    }

    #[test]
    fn static_provider_without_media_has_the_defaults() {
        let p = StaticProvider::new("bing", vec![]);
        assert_eq!(
            (p.media_client().is_none(), p.allowed_hosts()),
            (true, Vec::<String>::new())
        );
    }

    #[tokio::test]
    async fn fake_source_set_keeps_order_kind_and_query() {
        let wh = Arc::new(StaticProvider::new(
            "wallhaven",
            vec![StaticProvider::result("wallhaven", "https://i/w.jpg")],
        ));
        let set = SourceSet::fake(vec![
            ("wh-1", Arc::clone(&wh) as Arc<dyn Provider>, "forest"),
            ("x-1", Arc::new(StaticProvider::new("custom", vec![])), ""),
        ]);
        let ids: Vec<&str> = set.online.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["wh-1", "x-1"]);
        let src = set.get("wh-1").unwrap();
        assert_eq!(src.kind, SourceType::Wallhaven);
        assert_eq!(set.get("x-1").unwrap().kind, SourceType::Unsupported);
        src.provider.fetch(&src.query).await.unwrap();
        assert_eq!(wh.queries(), ["forest"]);
    }
}
