//! Fixtures shared by the engine integration tests: an engine over a temp dir with fake
//! platform parts and fake online sources, plus small file and HTTP helpers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use kakemono_config::Config;
use kakemono_config::schema::{SourceEntry, SourceType};
use kakemono_engine::state::State;
use kakemono_engine::{Deps, Engine};
use kakemono_platform::api::{Fit, Platform, Theme, WallpaperSetter};
use kakemono_platform::fake::{ManualSignals, RecordingSetter};
use kakemono_sources::fake::StaticProvider;
use kakemono_sources::{Provider, SourceSet};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The engine's fixed wall clock.
pub const NOW: &str = "2026-03-04T05:06:07.890+08:00";

/// Fake providers by row id; a row with no fake under its id gets the first fake whose
/// provider name is its type (a `check_source` row has no fixed id).
pub type Fakes = Vec<(&'static str, Arc<dyn Provider>)>;

pub struct Fixture {
    _dir: TempDir,
    pub root: PathBuf,
    pub setter: RecordingSetter,
    pub signals: ManualSignals,
    pub engine: Engine,
}

impl Fixture {
    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn manual(&self) -> PathBuf {
        self.root.join("manual")
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// An image file directly in the root (outside every engine folder).
    pub fn image(&self, name: &str) -> PathBuf {
        write(&self.root.join(name), b"fake image data")
    }
}

/// Builds the engine's online sources from the config (on start and every reload).
type SourcesFactory = Box<dyn Fn(&Config, reqwest::Client) -> SourceSet + Send + Sync>;

/// Builds a [`Fixture`]. Defaults: Internet off, no manual dir, `interval = "30m"`, light
/// theme, a [`RecordingSetter`] as the setter, no online sources.
pub struct Builder {
    internet: bool,
    manual_dir: bool,
    interval: &'static str,
    theme: Theme,
    setter: Option<Arc<dyn WallpaperSetter>>,
    sources: SourcesFactory,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            internet: false,
            manual_dir: false,
            interval: "30m",
            theme: Theme::Light,
            setter: None,
            sources: Box::new(|_, _| SourceSet::default()),
        }
    }

    pub fn internet(mut self, on: bool) -> Self {
        self.internet = on;
        self
    }

    /// Sets `[daemon] manual_dir` to `<root>/manual` ([`Fixture::manual`]).
    pub fn manual_dir(mut self) -> Self {
        self.manual_dir = true;
        self
    }

    pub fn interval(mut self, interval: &'static str) -> Self {
        self.interval = interval;
        self
    }

    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// The engine's setter; [`Fixture::setter`] then records nothing.
    pub fn setter(mut self, setter: Arc<dyn WallpaperSetter>) -> Self {
        self.setter = Some(setter);
        self
    }

    /// One fake per row, rebuilt on every reload ([`fake_set`]).
    pub fn fakes(self, fakes: Fakes) -> Self {
        self.sources_factory(move |cfg, _| fake_set(cfg, &fakes))
    }

    /// One fixed set, whatever the config says.
    pub fn sources(self, sources: SourceSet) -> Self {
        self.sources_factory(move |_, _| sources.clone())
    }

    pub fn sources_factory(
        mut self,
        factory: impl Fn(&Config, reqwest::Client) -> SourceSet + Send + Sync + 'static,
    ) -> Self {
        self.sources = Box::new(factory);
        self
    }

    /// The engine over the config, after `edit` (which gets the temp root) changed it.
    pub fn build(self, edit: impl FnOnce(&mut Config, &Path)) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut cfg = Config::default();
        cfg.daemon.download_dir = root.join("downloads");
        cfg.daemon.manual_dir = self.manual_dir.then(|| root.join("manual"));
        cfg.daemon.trash_dir = root.join("trash");
        cfg.favorites.dir = root.join("favorites");
        cfg.daemon.interval = self.interval.to_owned();
        cfg.daemon.internet = self.internet;
        cfg.daemon.download_limit_mb_enabled = false;
        cfg.desktop.fit = Fit::Center;
        cfg.sources = Vec::new();
        edit(&mut cfg, &root);

        let setter = RecordingSetter::new();
        let signals = ManualSignals::new(self.theme);
        let platform = Platform {
            setter: self.setter.unwrap_or_else(|| Arc::new(setter.clone())),
            signals: Arc::new(signals.clone()),
        };
        let now = now();
        let deps = Deps::new(platform, kakemono_sources::http::client().unwrap())
            .with_sources_factory(self.sources)
            .with_clock(move || now);
        let engine = Engine::new(cfg, State::new(), root.join("state.json"), deps).unwrap();
        Fixture {
            _dir: dir,
            root,
            setter,
            signals,
            engine,
        }
    }
}

/// [`NOW`], parsed.
pub fn now() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(NOW).unwrap()
}

/// The `SourceSet` for `cfg`: one fake per enabled row (see [`Fakes`]).
pub fn fake_set(cfg: &Config, fakes: &Fakes) -> SourceSet {
    SourceSet::fake(
        cfg.sources
            .iter()
            .filter(|e| e.enabled)
            .filter_map(|e| {
                fakes
                    .iter()
                    .find(|(id, _)| *id == e.id)
                    .or_else(|| fakes.iter().find(|(_, p)| p.name() == e.kind.as_str()))
                    .map(|(_, p)| (e.id.as_str(), Arc::clone(p), e.location.as_str()))
            })
            .collect(),
    )
}

pub fn row(id: &str, kind: SourceType, location: &str) -> SourceEntry {
    SourceEntry {
        id: id.to_owned(),
        enabled: true,
        kind,
        location: location.to_owned(),
    }
}

/// Writes `bytes` to `path`, creating its folder.
pub fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    path.to_path_buf()
}

/// A 12 KiB JPEG-typed body at `/img/<name>` for each name (downloads want ≥ 10 KiB and an
/// image type).
pub async fn image_server(names: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    for name in names {
        Mock::given(method("GET"))
            .and(path(format!("/img/{name}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/jpeg")
                    .set_body_bytes(vec![0xAB; 12 * 1024]),
            )
            .mount(&server)
            .await;
    }
    server
}

/// A provider named `name` with one result at `url`.
pub fn provider(name: &'static str, url: &str) -> Arc<StaticProvider> {
    Arc::new(StaticProvider::new(
        name,
        vec![StaticProvider::result(name, url)],
    ))
}

/// Lets other tasks on this current-thread runtime run until they wait on something.
pub async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}
