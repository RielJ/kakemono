//! Shared test helper: replays a Go-captured `tests/fixtures/<provider>/<case>.case.json`
//! against a wiremock server and checks a provider's outcome and requests against it.
//!
//! Schema of the case files: `tests/fixtures/README.md`. Usage: `.pi/contracts/sources.md`.
//!
//! ```ignore
//! mod common;
//! use common::Case;
//!
//! async fn run(name: &str) {
//!     let case = Case::load("bing", name);
//!     let replay = case.serve().await;
//!     let bing = Bing::with_base_url(case.config(), replay.client(), replay.base_url());
//!     replay.verify(bing.fetch(&case.query).await).await;
//! }
//!
//! #[tokio::test]
//! async fn parses_json() { run("parses_json").await }
//! ```
//!
//! How the mock stands in for production hosts:
//! - Each route answers on its production host (`www.bing.com`, `wallhaven.cc`, ...):
//!   [`Replay::client`] resolves those hosts to the mock server, and routes only match when the
//!   `Host` header is the production host. So the provider must be built with
//!   [`Replay::base_url`] (`http://<host>`), not `server.uri()`.
//! - The mock has no TLS, so URLs use `http://`. When comparing, `https://<case host>` in the
//!   expected text is rewritten to `http://<case host>`.
//! - `{{HOST}}` in a body file becomes the request's `Host` header, so links in served HTML
//!   (`http://wallhaven.cc/w/abc123`) point back at the mock and match the Go output.
//! - [`Case::serve`] pauses the tokio clock, so retry back-off (429/503) costs no wall time.
//!   Use plain `#[tokio::test]` (current-thread), NOT `start_paused = true` (pausing twice
//!   panics). The replay client has no timeout, because a pending timeout timer would fire as
//!   soon as the paused runtime waits on the socket.
//! - User-Agent: the Go fixtures record `wallpaperd/1.0` or `Go-http-client/1.1`. Those are not
//!   checked. Instead every received request must carry `http::USER_AGENT` (`kakemono/<ver>`),
//!   which catches providers that override the shared client's UA.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};

use kakemono_sources::WallpaperResult;
use kakemono_sources::http;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use wiremock::{Match, Mock, MockServer, Request, Respond, ResponseTemplate};

/// `tests/fixtures` of this crate.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// One parsed `*.case.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    /// Not in the file: `<provider>/<case>`, for messages.
    #[serde(skip)]
    pub id: String,
    /// Not in the file: the provider's fixture dir (body files live here).
    #[serde(skip)]
    pub dir: PathBuf,
    pub go_test: String,
    pub provider: String,
    /// The provider's config section with TOML key names. See [`Case::config`].
    pub config: serde_json::Value,
    /// Argument to `fetch`.
    pub query: String,
    pub routes: Vec<Route>,
    /// Requests Go made, sorted by method + URL.
    pub requests: Vec<RecordedRequest>,
    /// Go shuffled the results; compare sorted by `source_url`.
    #[serde(default)]
    pub sort_results: bool,
    pub expected: Expected,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Route {
    pub method: String,
    pub host: String,
    pub path: String,
    pub status: u16,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body_file: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecordedRequest {
    pub method: String,
    pub url: String,
    /// Go's UA. Ignored (see module docs).
    pub user_agent: String,
}

/// Go's outcome: `{"results": [...]}` or `{"error": "..."}`.
#[derive(Debug, Clone, Deserialize)]
pub struct Expected {
    #[serde(default)]
    pub results: Option<Vec<WallpaperResult>>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Case {
    /// Loads `tests/fixtures/<provider>/<name>.case.json`. Panics with the path on any error.
    pub fn load(provider: &str, name: &str) -> Case {
        let dir = fixtures_dir().join(provider);
        let path = dir.join(format!("{name}.case.json"));
        let text =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        Case::parse(&format!("{provider}/{name}"), dir, &text)
    }

    /// Parses case JSON held in memory; `dir` holds its body files. For self-tests and
    /// hand-written cases.
    pub fn parse(id: &str, dir: PathBuf, json: &str) -> Case {
        let mut case: Case =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("parse {id}: {e}"));
        case.id = id.to_owned();
        case.dir = dir;
        case
    }

    /// Names of every `*.case.json` for `provider`, sorted (for "every case passes" loops).
    pub fn names(provider: &str) -> Vec<String> {
        let dir = fixtures_dir().join(provider);
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                name.strip_suffix(".case.json").map(str::to_owned)
            })
            .collect();
        names.sort();
        names
    }

    /// The case's `config` deserialized into `T` (e.g. the config crate's provider section, or
    /// a local struct). Panics on mismatch.
    pub fn config<T: DeserializeOwned>(&self) -> T {
        serde_json::from_value(self.config.clone())
            .unwrap_or_else(|e| panic!("{}: config does not fit the target type: {e}", self.id))
    }

    /// Distinct production hosts the routes answer on, sorted.
    pub fn hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self.routes.iter().map(|r| r.host.clone()).collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }

    /// Bytes of a body file in this case's dir.
    pub fn body(&self, file: &str) -> Vec<u8> {
        let path = self.dir.join(file);
        fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Starts a wiremock server serving every route, then pauses the tokio clock.
    pub async fn serve(&self) -> Replay {
        let server = MockServer::start().await;
        for route in &self.routes {
            let body = route
                .body_file
                .as_deref()
                .map(|f| self.body(f))
                .unwrap_or_default();
            Mock::given(MethodPathHost {
                method: route.method.to_ascii_uppercase(),
                path: route.path.clone(),
                host: route.host.to_ascii_lowercase(),
            })
            .respond_with(FixtureResponse {
                status: route.status,
                content_type: route.content_type.clone(),
                headers: route.headers.clone(),
                body,
            })
            .named(format!("{} {} {}", route.method, route.host, route.path))
            .mount(&server)
            .await;
        }
        tokio::time::pause();
        Replay {
            case: self.clone(),
            server,
        }
    }
}

/// A running replay of one [`Case`].
pub struct Replay {
    pub case: Case,
    pub server: MockServer,
}

impl Replay {
    /// A client like `http::client()` (same UA) that sends every case host to the mock server.
    /// No timeout (paused clock), no proxy, no connection pool.
    pub fn client(&self) -> reqwest::Client {
        let addr = *self.server.address();
        let mut builder = http::client_builder().no_proxy().pool_max_idle_per_host(0);
        for host in self.case.hosts() {
            builder = builder.resolve(&host, addr);
        }
        builder.build().expect("build replay client")
    }

    /// `http://<host>` for the case's only host. Panics if the case has several hosts.
    pub fn base_url(&self) -> String {
        let hosts = self.case.hosts();
        assert_eq!(
            hosts.len(),
            1,
            "{}: several hosts {hosts:?}; use base_url_for",
            self.case.id
        );
        format!("http://{}", hosts[0])
    }

    /// `http://<host>` for one of the case's hosts.
    pub fn base_url_for(&self, host: &str) -> String {
        assert!(
            self.case.hosts().iter().any(|h| h == host),
            "{}: {host} is not a route host",
            self.case.id
        );
        format!("http://{host}")
    }

    /// [`Replay::assert_outcome`] then [`Replay::assert_requests`].
    pub async fn verify<E: Display>(&self, outcome: Result<Vec<WallpaperResult>, E>) {
        self.assert_outcome(outcome);
        self.assert_requests().await;
    }

    /// Checks `outcome` against `expected`: equal results (sorted by `source_url` when
    /// `sort_results`), or an error whose `Display` equals Go's text. Both sides are passed
    /// through [`Replay::normalize`].
    pub fn assert_outcome<E: Display>(&self, outcome: Result<Vec<WallpaperResult>, E>) {
        let id = &self.case.id;
        match (&self.case.expected.error, outcome) {
            (Some(want), Err(got)) => {
                assert_eq!(
                    self.normalize(&got.to_string()),
                    self.normalize(want),
                    "{id}: error text"
                );
            }
            (Some(want), Ok(got)) => panic!("{id}: expected error {want:?}, got Ok({got:#?})"),
            (None, Err(got)) => panic!("{id}: expected results, got error: {got}"),
            (None, Ok(mut got)) => {
                let mut want = self.case.expected.results.clone().unwrap_or_default();
                if self.case.sort_results {
                    got.sort_by(|a, b| a.source_url.cmp(&b.source_url));
                    want.sort_by(|a, b| a.source_url.cmp(&b.source_url));
                }
                let norm = |r: Vec<WallpaperResult>| -> Vec<WallpaperResult> {
                    r.into_iter()
                        .map(|r| WallpaperResult {
                            source_url: self.normalize(&r.source_url),
                            image_url: self.normalize(&r.image_url),
                            title: r.title,
                            author: r.author,
                            source: r.source,
                        })
                        .collect()
                };
                assert_eq!(norm(got), norm(want), "{id}: results");
            }
        }
    }

    /// Checks the requests the mock received against the ones Go made.
    ///
    /// Compared as sorted, de-duplicated sets of (method, host, path, sorted decoded query
    /// pairs): query-key order and repeats from 429/503 retries do not matter; scheme is
    /// ignored. Every received request must carry `http::USER_AGENT`.
    pub async fn assert_requests(&self) {
        let id = &self.case.id;
        let received = self
            .server
            .received_requests()
            .await
            .expect("wiremock request recording is on");

        for r in &received {
            let ua = r.headers.get("user-agent").and_then(|v| v.to_str().ok());
            assert_eq!(
                ua,
                Some(http::USER_AGENT),
                "{id}: User-Agent of {} {}",
                r.method,
                r.url
            );
        }

        let mut got: Vec<RequestKey> = received.iter().map(RequestKey::received).collect();
        let mut want: Vec<RequestKey> = self
            .case
            .requests
            .iter()
            .map(RequestKey::recorded)
            .collect();
        for v in [&mut got, &mut want] {
            v.sort();
            v.dedup();
        }
        assert_eq!(got, want, "{id}: requests (method, host, path, query)");
    }

    /// Rewrites `https://<case host>` to `http://<case host>` (the mock has no TLS).
    pub fn normalize(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for host in self.case.hosts() {
            out = out.replace(&format!("https://{host}"), &format!("http://{host}"));
        }
        out
    }
}

/// Request identity used by [`Replay::assert_requests`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestKey {
    pub method: String,
    pub host: String,
    pub path: String,
    pub query: Vec<(String, String)>,
}

impl RequestKey {
    fn recorded(r: &RecordedRequest) -> Self {
        let url = url::Url::parse(&r.url).unwrap_or_else(|e| panic!("recorded url {}: {e}", r.url));
        Self::from_parts(&r.method, url.host_str().unwrap_or_default(), &url)
    }

    fn received(r: &Request) -> Self {
        Self::from_parts(r.method.as_str(), &host_header(r), &r.url)
    }

    fn from_parts(method: &str, host: &str, url: &url::Url) -> Self {
        let mut query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        query.sort();
        Self {
            method: method.to_ascii_uppercase(),
            host: host.to_ascii_lowercase(),
            path: url.path().to_owned(),
            query,
        }
    }
}

/// The request's `Host` header without a port.
fn host_header(r: &Request) -> String {
    let raw = r
        .headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let host = match raw.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => raw,
    };
    host.to_ascii_lowercase()
}

/// Matches method + path + `Host` header (query ignored), as the README specifies.
struct MethodPathHost {
    method: String,
    path: String,
    host: String,
}

impl Match for MethodPathHost {
    fn matches(&self, r: &Request) -> bool {
        r.method.as_str() == self.method && r.url.path() == self.path && host_header(r) == self.host
    }
}

/// Serves a route, substituting `{{HOST}}` in the body with the request's `Host` header.
struct FixtureResponse {
    status: u16,
    content_type: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Respond for FixtureResponse {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let host = r
            .headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        let body = match std::str::from_utf8(&self.body) {
            Ok(text) if text.contains("{{HOST}}") => text.replace("{{HOST}}", host).into_bytes(),
            _ => self.body.clone(),
        };
        let mut t = ResponseTemplate::new(self.status);
        t = if self.content_type.is_empty() {
            t.set_body_bytes(body)
        } else {
            t.set_body_raw(body, &self.content_type)
        };
        for (k, v) in &self.headers {
            t = t.insert_header(k.as_str(), v.as_str());
        }
        t
    }
}
