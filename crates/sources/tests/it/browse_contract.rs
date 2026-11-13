//! The browse contract's default `Provider` methods: a provider that implements only `name` and
//! `fetch` still compiles and reports that it cannot browse.

use async_trait::async_trait;
use kakemono_sources::{
    BrowseSupport, ImageRef, Provider, ProviderError, RemoteItem, WallpaperResult,
};

/// Implements only the required methods, like every provider before browse existed.
struct Dummy;

#[async_trait]
impl Provider for Dummy {
    fn name(&self) -> &'static str {
        "dummy"
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        Ok(Vec::new())
    }
}

fn item(image: ImageRef) -> RemoteItem {
    RemoteItem {
        remote_id: "abc".into(),
        preview_url: "https://th.wallhaven.cc/small/ab/abc.jpg".into(),
        image,
        source_url: "https://wallhaven.cc/w/abc".into(),
        title: String::new(),
        author: String::new(),
        width: None,
        height: None,
        nsfw: false,
    }
}

#[tokio::test]
async fn browse_contract_default_browse_is_unsupported() {
    let err = Dummy.browse("", None).await.unwrap_err();
    assert!(matches!(err, ProviderError::Unsupported(_)), "{err:?}");
}

#[tokio::test]
async fn browse_contract_unsupported_error_names_the_provider() {
    let err = Dummy.browse("forest", None).await.unwrap_err();
    assert_eq!(err.to_string(), "dummy cannot browse");
}

#[test]
fn browse_contract_default_support_is_unsupported() {
    assert_eq!(Dummy.browse_support(), BrowseSupport::Unsupported);
}

#[test]
fn browse_contract_default_media_hosts_is_empty() {
    assert!(Dummy.media_hosts().is_empty());
}

#[tokio::test]
async fn browse_contract_resolve_image_returns_direct_url() {
    let url = "https://w.wallhaven.cc/full/ab/wallhaven-abc.jpg";
    let got = Dummy
        .resolve_image(&item(ImageRef::Direct(url.into())))
        .await
        .unwrap();
    assert_eq!(got, url);
}

#[tokio::test]
async fn browse_contract_resolve_image_rejects_page_by_default() {
    let err = Dummy
        .resolve_image(&item(ImageRef::Page("https://wallhaven.cc/w/abc".into())))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Unsupported(_)), "{err:?}");
}

/// `Provider` stays object-safe with the new methods: the engine holds `Arc<dyn Provider>`.
#[tokio::test]
async fn browse_contract_works_through_dyn_provider() {
    let provider: std::sync::Arc<dyn Provider> = std::sync::Arc::new(Dummy);
    assert!(provider.browse("", None).await.is_err());
}

#[test]
fn browse_contract_default_allowed_hosts_are_the_media_hosts() {
    let bing = kakemono_sources::bing::Bing::new(
        kakemono_config::schema::BingConfig::default(),
        kakemono_sources::http::client_builder().build().unwrap(),
    );
    let media: Vec<String> = bing.media_hosts().iter().map(|h| (*h).to_owned()).collect();
    assert!(!media.is_empty());
    assert_eq!(bing.allowed_hosts(), media);
}

#[test]
fn browse_contract_default_allowed_hosts_is_empty_without_media_hosts() {
    assert!(Dummy.allowed_hosts().is_empty());
}

#[test]
fn browse_contract_default_media_client_is_none() {
    let bing = kakemono_sources::bing::Bing::new(
        kakemono_config::schema::BingConfig::default(),
        kakemono_sources::http::client_builder().build().unwrap(),
    );
    assert!(bing.media_client().is_none());
}

/// Answers `fetch` with one result whose title is the query.
struct Echo;

#[async_trait]
impl Provider for Echo {
    fn name(&self) -> &'static str {
        "echo"
    }

    async fn fetch(&self, query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        Ok(vec![WallpaperResult {
            source_url: String::new(),
            image_url: String::new(),
            title: query.to_owned(),
            author: String::new(),
            source: "echo".into(),
        }])
    }
}

#[tokio::test]
async fn browse_contract_default_check_is_fetch() {
    let titles: Vec<String> = Echo
        .check("forest")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect();
    assert_eq!(titles, ["forest"]);
}
