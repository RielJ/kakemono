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
