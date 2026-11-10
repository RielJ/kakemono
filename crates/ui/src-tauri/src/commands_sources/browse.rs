//! The picker's Sources tab: remote images the daemon browses (`browse`, `browse_use`).
//!
//! - `browse_page { session?, page, source? }` → [`BrowseListing`]: IPC `browse` on a
//!   connection of its own ([`DaemonLink::call_alone`]). Each preview the daemon saved under
//!   its reported `preview_dir` is admitted for `thumb://` ([`ListKind::Previews`]); any other
//!   path gets no URL. The listing carries no remote URL.
//! - `browse_use { sourceId, remoteId, action }` → [`BrowseUseData`]: IPC `browse_use` with a
//!   [`BROWSE_USE_TIMEOUT`] deadline, since it downloads the full image. A timeout is a
//!   `daemon_timeout` error saying the download goes on in the background.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kakemono_protocol::client::ClientError;
use kakemono_protocol::types::{
    BrowseAction, BrowseData, BrowseItem, BrowseSourceStatus, BrowseUseData, Request,
};
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use super::{SourceError, SrcResult, call_for, decode_data};
use crate::daemon_link::{DaemonLink, LinkError};
use crate::thumbs::{ListKind, Thumbs};

/// How long `browse_use` waits for the daemon to download and apply the image.
pub const BROWSE_USE_TIMEOUT: Duration = Duration::from_secs(120);

/// Previews of one browse session that stay admitted, newest kept. Matches the daemon's
/// item index cap: an older item can't be used anyway.
pub const PREVIEW_ADMIT_CAP: usize = 2000;

/// What the UI says when `browse_use` outlives [`BROWSE_USE_TIMEOUT`].
pub const STILL_DOWNLOADING: &str = "Still downloading in the background…";

/// One remote image on a [`BrowseListing`] page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemoteThumb {
    pub source_id: String,
    pub kind: String,
    pub remote_id: String,
    pub title: String,
    pub author: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub nsfw: bool,
    /// `thumb://` URL of the local preview; `null` when the daemon has none or it is not
    /// inside the preview dir (the card shows a placeholder).
    pub image_url: Option<String>,
}

/// One page of the Sources tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowseListing {
    /// Send back with the next page.
    pub session: String,
    pub page: u32,
    /// The daemon dropped the asked-for session and started over at page 0.
    pub restarted: bool,
    /// No source has more images.
    pub done: bool,
    pub items: Vec<RemoteThumb>,
    pub sources: Vec<BrowseSourceStatus>,
}

/// The preview paths admitted for the current browse session, so scrolling back to an earlier
/// page still shows its thumbnails. Managed as `Arc<BrowsePreviews>`.
#[derive(Debug, Default)]
pub struct BrowsePreviews(Mutex<Admitted>);

#[derive(Debug, Default)]
struct Admitted {
    session: String,
    preview_dir: String,
    /// Oldest first, at most [`PREVIEW_ADMIT_CAP`].
    paths: VecDeque<String>,
}

impl Admitted {
    /// Starts over when the session or the preview dir changed.
    fn reset_unless(&mut self, session: &str, preview_dir: &str, restarted: bool) {
        if restarted || self.session != session || self.preview_dir != preview_dir {
            *self = Self {
                session: session.to_owned(),
                preview_dir: preview_dir.to_owned(),
                paths: VecDeque::new(),
            };
        }
    }

    /// Adds `items`' absolute preview paths not yet held, dropping the oldest past the cap.
    fn extend(&mut self, items: &[BrowseItem]) {
        let mut held: HashSet<String> = self.paths.iter().cloned().collect();
        for path in items.iter().filter_map(|item| item.preview.as_deref()) {
            if Path::new(path).is_absolute() && held.insert(path.to_owned()) {
                self.paths.push_back(path.to_owned());
            }
        }
        let excess = self.paths.len().saturating_sub(PREVIEW_ADMIT_CAP);
        self.paths.drain(..excess);
    }
}

/// Turns a `browse` answer into the frontend's page: admits the session's previews under
/// `data.preview_dir` ([`ListKind::Previews`], replacing the last admission) and gives each
/// item the URL of its preview, or `None`. A relative `preview_dir` admits nothing.
pub fn to_listing(thumbs: &Thumbs, previews: &BrowsePreviews, data: BrowseData) -> BrowseListing {
    let urls: HashMap<String, String> = {
        let mut admitted = previews
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        admitted.reset_unless(&data.session, &data.preview_dir, data.restarted);
        admitted.extend(&data.items);
        let root = Path::new(&data.preview_dir);
        let roots: &[&Path] = if root.is_absolute() { &[root] } else { &[] };
        let paths: Vec<&str> = admitted.paths.iter().map(String::as_str).collect();
        let urls = thumbs.admit(ListKind::Previews, roots, &paths);
        let urls: HashMap<String, String> = paths
            .into_iter()
            .zip(urls)
            .filter(|(_, url)| !url.is_empty())
            .map(|(path, url)| (path.to_owned(), url))
            .collect();
        admitted.paths.retain(|path| urls.contains_key(path));
        urls
    };
    BrowseListing {
        items: data
            .items
            .into_iter()
            .map(|item| RemoteThumb {
                image_url: item.preview.as_ref().and_then(|p| urls.get(p).cloned()),
                source_id: item.source_id,
                kind: item.kind,
                remote_id: item.remote_id,
                title: item.title,
                author: item.author,
                width: item.width,
                height: item.height,
                nsfw: item.nsfw,
            })
            .collect(),
        session: data.session,
        page: data.page,
        restarted: data.restarted,
        done: data.done,
        sources: data.sources,
    }
}

/// `browse_page { session?, page, source? }` → [`BrowseListing`].
#[tauri::command]
pub async fn browse_page(
    link: State<'_, Arc<DaemonLink>>,
    thumbs: State<'_, Arc<Thumbs>>,
    previews: State<'_, Arc<BrowsePreviews>>,
    session: Option<String>,
    page: u32,
    source: Option<String>,
) -> SrcResult<BrowseListing> {
    let data = browse_on(&link, session, page, source).await?;
    let (thumbs, previews) = (Arc::clone(&thumbs), Arc::clone(&previews));
    super::blocking(move || Ok(to_listing(&thumbs, &previews, data))).await
}

/// IPC `browse` on a connection of its own, decoded.
pub async fn browse_on(
    link: &DaemonLink,
    session: Option<String>,
    page: u32,
    source: Option<String>,
) -> SrcResult<BrowseData> {
    let request = Request::Browse {
        session,
        page,
        source,
    };
    call_for(link, request, "browse").await
}

/// `browse_use { sourceId, remoteId, action }` → [`BrowseUseData`].
#[tauri::command]
pub async fn browse_use(
    link: State<'_, Arc<DaemonLink>>,
    source_id: String,
    remote_id: String,
    action: BrowseAction,
) -> SrcResult<BrowseUseData> {
    let request = Request::BrowseUse {
        source_id,
        remote_id,
        action,
    };
    browse_use_on(&link, request, BROWSE_USE_TIMEOUT).await
}

/// Sends `request` (a [`Request::BrowseUse`]) with `timeout`. A timeout becomes
/// [`SourceError::StillDownloading`].
pub async fn browse_use_on(
    link: &DaemonLink,
    request: Request,
    timeout: Duration,
) -> SrcResult<BrowseUseData> {
    let response = match link.call_alone_with_timeout(request, timeout).await {
        Err(LinkError::Client(ClientError::Timeout(_))) => {
            return Err(SourceError::StillDownloading);
        }
        other => other?,
    };
    decode_data(&response, "browse_use")
}

#[cfg(test)]
mod tests {
    use super::super::fake_daemon::{serve, temp_endpoint};
    use super::*;
    use crate::error::{ErrorKind, Kinded};
    use interprocess::local_socket::tokio::prelude::*;
    use kakemono_protocol::types::BrowseSourceState;
    use std::path::PathBuf;
    use tokio::io::{AsyncBufReadExt, BufReader};

    struct Fixture {
        _tmp: tempfile::TempDir,
        preview_dir: PathBuf,
        outside: PathBuf,
        thumbs: Thumbs,
        previews: BrowsePreviews,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let preview_dir = tmp.path().join("cache/previews");
        let outside = tmp.path().join("elsewhere");
        std::fs::create_dir_all(preview_dir.join("wh-1")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let thumbs = Thumbs::new(tmp.path().join("ui-cache/thumbs"));
        Fixture {
            _tmp: tmp,
            preview_dir,
            outside,
            thumbs,
            previews: BrowsePreviews::default(),
        }
    }

    fn png(dir: &Path, name: &str) -> String {
        let path = dir.join(name);
        image::RgbImage::new(8, 8).save(&path).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn item(remote_id: &str, preview: Option<String>) -> BrowseItem {
        BrowseItem {
            source_id: "wh-1".into(),
            kind: "wallhaven".into(),
            remote_id: remote_id.into(),
            title: format!("title {remote_id}"),
            author: "someone".into(),
            width: Some(1920),
            height: Some(1080),
            nsfw: false,
            preview,
        }
    }

    fn data(f: &Fixture, session: &str, page: u32, items: Vec<BrowseItem>) -> BrowseData {
        BrowseData {
            session: session.into(),
            page,
            restarted: false,
            done: false,
            preview_dir: f.preview_dir.to_string_lossy().into_owned(),
            items,
            sources: vec![BrowseSourceStatus {
                source_id: "wh-1".into(),
                kind: "wallhaven".into(),
                state: BrowseSourceState::Active,
            }],
        }
    }

    fn listing(f: &Fixture, data: BrowseData) -> BrowseListing {
        to_listing(&f.thumbs, &f.previews, data)
    }

    #[test]
    fn to_listing_gives_previews_under_preview_dir_thumb_urls() {
        let f = fixture();
        let path = png(&f.preview_dir.join("wh-1"), "a.png");
        let got = listing(&f, data(&f, "s1", 0, vec![item("a", Some(path.clone()))]));
        let url = got.items[0].image_url.as_deref().unwrap_or_default();
        assert!(url.starts_with("thumb://localhost/"), "{url:?}");
        assert!(f.thumbs.is_admitted(Path::new(&path)));
    }

    #[test]
    fn to_listing_refuses_previews_outside_preview_dir() {
        let f = fixture();
        let outside = png(&f.outside, "stray.png");
        let dotdot = format!("{}/../../elsewhere/stray.png", f.preview_dir.display());
        let relative = "cache/previews/wh-1/rel.png".to_owned();
        let got = listing(
            &f,
            data(
                &f,
                "s1",
                0,
                vec![
                    item("out", Some(outside.clone())),
                    item("dotdot", Some(dotdot)),
                    item("rel", Some(relative)),
                    item("none", None),
                ],
            ),
        );
        assert_eq!(
            got.items
                .iter()
                .map(|t| t.image_url.clone())
                .collect::<Vec<_>>(),
            vec![None, None, None, None]
        );
        assert!(!f.thumbs.is_admitted(Path::new(&outside)));
    }

    #[test]
    fn to_listing_with_a_relative_preview_dir_admits_nothing() {
        let f = fixture();
        let path = png(&f.preview_dir.join("wh-1"), "a.png");
        let mut answer = data(&f, "s1", 0, vec![item("a", Some(path.clone()))]);
        answer.preview_dir = "cache/previews".into();
        let got = listing(&f, answer);
        assert_eq!(got.items[0].image_url, None);
    }

    #[test]
    fn to_listing_has_no_http_urls() {
        let f = fixture();
        let path = png(&f.preview_dir.join("wh-1"), "a.png");
        let got = listing(&f, data(&f, "s1", 0, vec![item("a", Some(path))]));
        let json = serde_json::to_string(&got).unwrap();
        assert!(!json.contains("http"), "{json}");
    }

    #[test]
    fn to_listing_keeps_earlier_pages_of_the_session_admitted() {
        let f = fixture();
        let first = png(&f.preview_dir.join("wh-1"), "a.png");
        let second = png(&f.preview_dir.join("wh-1"), "b.png");
        listing(&f, data(&f, "s1", 0, vec![item("a", Some(first.clone()))]));
        listing(&f, data(&f, "s1", 1, vec![item("b", Some(second))]));
        assert!(f.thumbs.is_admitted(Path::new(&first)));
    }

    #[test]
    fn to_listing_drops_previews_of_an_old_session() {
        let f = fixture();
        let first = png(&f.preview_dir.join("wh-1"), "a.png");
        let second = png(&f.preview_dir.join("wh-1"), "b.png");
        listing(&f, data(&f, "s1", 0, vec![item("a", Some(first.clone()))]));
        listing(&f, data(&f, "s2", 0, vec![item("b", Some(second))]));
        assert!(!f.thumbs.is_admitted(Path::new(&first)));
    }

    #[test]
    fn to_listing_drops_previews_when_the_daemon_restarted_the_session() {
        let f = fixture();
        let first = png(&f.preview_dir.join("wh-1"), "a.png");
        let second = png(&f.preview_dir.join("wh-1"), "b.png");
        listing(&f, data(&f, "s1", 3, vec![item("a", Some(first.clone()))]));
        let mut answer = data(&f, "s1", 0, vec![item("b", Some(second))]);
        answer.restarted = true;
        listing(&f, answer);
        assert!(!f.thumbs.is_admitted(Path::new(&first)));
    }

    #[test]
    fn to_listing_keeps_metadata_and_sources() {
        let f = fixture();
        let got = listing(&f, data(&f, "s1", 2, vec![item("a", None)]));
        assert_eq!(
            (
                got.session.as_str(),
                got.page,
                got.items[0].clone(),
                got.sources.len()
            ),
            (
                "s1",
                2,
                RemoteThumb {
                    source_id: "wh-1".into(),
                    kind: "wallhaven".into(),
                    remote_id: "a".into(),
                    title: "title a".into(),
                    author: "someone".into(),
                    width: Some(1920),
                    height: Some(1080),
                    nsfw: false,
                    image_url: None,
                },
                1
            )
        );
    }

    #[test]
    fn admitted_previews_stop_at_the_cap() {
        let mut admitted = Admitted::default();
        let items: Vec<BrowseItem> = (0..PREVIEW_ADMIT_CAP + 5)
            .map(|i| item(&i.to_string(), Some(format!("/p/{i}.png"))))
            .collect();
        admitted.extend(&items);
        assert_eq!(
            (admitted.paths.len(), admitted.paths.front().cloned()),
            (PREVIEW_ADMIT_CAP, Some("/p/5.png".to_owned()))
        );
    }

    // ── Against a fake daemon ──

    const BROWSE: &str = concat!(
        "{\"ok\":true,\"data\":{\"session\":\"s1\",\"page\":0,\"restarted\":false,",
        "\"done\":false,\"preview_dir\":\"/nowhere/previews\",\"items\":[],\"sources\":[]}}"
    );

    #[tokio::test]
    async fn browse_on_sends_browse_and_decodes_the_page() {
        let (_dir, endpoint) = temp_endpoint();
        let server = serve(&endpoint, vec![BROWSE.into()]);
        let link = DaemonLink::new(endpoint);
        let got = browse_on(&link, Some("s0".into()), 3, Some("wh-1".into()))
            .await
            .expect("page");
        assert_eq!(got.session, "s1");
        assert_eq!(
            server.await.unwrap(),
            vec![
                serde_json::json!({"cmd":"browse","args":{"session":"s0","page":3,"source":"wh-1"}})
            ]
        );
    }

    #[tokio::test]
    async fn browse_on_reports_the_daemons_refusal() {
        let (_dir, endpoint) = temp_endpoint();
        let _server = serve(
            &endpoint,
            vec!["{\"ok\":false,\"error\":\"Internet access is off\"}".into()],
        );
        let link = DaemonLink::new(endpoint);
        let err = browse_on(&link, None, 0, None).await.expect_err("refused");
        assert_eq!(
            (err.kind(), err.to_string()),
            (ErrorKind::Daemon, "Internet access is off".to_owned())
        );
    }

    #[tokio::test]
    async fn browse_without_a_daemon_is_unreachable() {
        let (_dir, endpoint) = temp_endpoint();
        let link = DaemonLink::new(endpoint);
        let err = browse_on(&link, None, 0, None)
            .await
            .expect_err("no daemon");
        assert_eq!(err.kind(), ErrorKind::DaemonUnreachable);
    }

    fn use_request() -> Request {
        Request::BrowseUse {
            source_id: "wh-1".into(),
            remote_id: "x8e1k2".into(),
            action: BrowseAction::Favorite,
        }
    }

    #[tokio::test]
    async fn browse_use_on_sends_browse_use_and_decodes_the_path() {
        let (_dir, endpoint) = temp_endpoint();
        let server = serve(
            &endpoint,
            vec!["{\"ok\":true,\"data\":{\"path\":\"/dl/wh-1/x.jpg\",\"wallpaper\":null}}".into()],
        );
        let link = DaemonLink::new(endpoint);
        let got = browse_use_on(&link, use_request(), BROWSE_USE_TIMEOUT)
            .await
            .expect("used");
        assert_eq!(got.path, "/dl/wh-1/x.jpg");
        assert_eq!(
            server.await.unwrap(),
            vec![serde_json::json!({"cmd":"browse_use","args":{
                "source_id":"wh-1","remote_id":"x8e1k2","action":"favorite"}})]
        );
    }

    #[tokio::test]
    async fn browse_use_timeout_says_still_downloading() {
        let (_dir, endpoint) = temp_endpoint();
        let listener = endpoint.bind().expect("bind");
        // Reads the request and never answers, like a daemon still downloading.
        let _server = tokio::spawn(async move {
            let conn = listener.accept().await.expect("accept");
            let mut reader = BufReader::new(conn);
            let mut line = String::new();
            let _ = reader.read_line(&mut line).await;
            std::future::pending::<()>().await;
        });
        let link = DaemonLink::new(endpoint);
        let err = browse_use_on(&link, use_request(), Duration::from_millis(100))
            .await
            .expect_err("timed out");
        assert_eq!(
            (err.kind(), err.to_string()),
            (ErrorKind::DaemonTimeout, STILL_DOWNLOADING.to_owned())
        );
    }
}
