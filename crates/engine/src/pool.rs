//! The local image pool and the pick a wallpaper change makes from it
//! (`docs/plans/variety-sources-impl.md` Task 12).
//!
//! [`Pool::collect`] lists, for each enabled `[[sources]]` row, the images already on disk:
//!
//! - online types: the files in `<download_dir>/<id>/` (filled by the downloader);
//! - `folder`: every image under `location`, recursively;
//! - `images`: the file `location` itself;
//! - `album-name` / `album-date`: the album folder `location`, resolved at pick time to the image
//!   after the source's cursor (see [`kakemono_sources::local::album_next`]);
//! - plus the implicit, always-enabled [`MANUAL_ID`] source for `[daemon] manual_dir`.
//!
//! Disabled rows, `unsupported` rows and sources without images are left out.
//!
//! [`pick`] chooses one image:
//!
//! 1. Choose a random non-empty source.
//! 2. Take up to [`MAX_CANDIDATES`] random files from it, without the current wallpaper and the
//!    last [`RECENT_HISTORY`] history entries.
//! 3. Return the first file that passes the filters. If none passes, try another source, up to
//!    [`MAX_CANDIDATES`] candidates in total.
//! 4. If nothing passes, return the candidate with the best [`Filters::score`] and log the
//!    filters it failed ([`PickOutcome::Fallback`]).
//!
//! The recent images are excluded only while something else is left: first the current
//! wallpaper and the recent history are skipped, then only the current wallpaper, then nothing.
//! Albums ignore the filters and the recent images, because their sequence is the user's intent.
//!
//! `pick` reads the [`State`] but doesn't change it, so a caller can pick from a copy without
//! holding a lock across the facts lookups. [`Picked::commit`] advances an album's cursor; call
//! it on the real state once the wallpaper is set, so a failed set doesn't skip an album image.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kakemono_config::schema::{Config, SourceType};
use kakemono_config::valid_id;
use kakemono_sources::is_image_file;
use kakemono_sources::local::{AlbumOrder, album_next, list_images};
use rand::seq::IndexedRandom;
use rand::{Rng, RngExt};

use crate::facts::FactsCache;
use crate::filters::{FilterCtx, Filters};
use crate::state::State;

/// Id of the implicit source for `[daemon] manual_dir`.
pub const MANUAL_ID: &str = "manual";
/// Most images one [`pick`] checks against the filters, over all sources.
pub const MAX_CANDIDATES: usize = 20;
/// How many of the newest history entries (besides the current wallpaper) [`pick`] avoids.
pub const RECENT_HISTORY: usize = 5;

/// Where a pool source comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolKind {
    /// A `[[sources]]` row of this type.
    Source(SourceType),
    /// The implicit `manual_dir` source.
    Manual,
}

impl PoolKind {
    /// The type's config string (`"wallhaven"`, `"folder"`, ...), or `"manual"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PoolKind::Source(kind) => kind.as_str(),
            PoolKind::Manual => MANUAL_ID,
        }
    }
}

/// The images of one pool source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolFiles {
    /// Images to choose from at random. Never empty in a collected [`Pool`].
    Files(Vec<PathBuf>),
    /// An album folder, shown in `order` from the source's cursor. Has at least one image when
    /// collected.
    Album {
        /// The album folder (its direct children are the album).
        dir: PathBuf,
        /// Name or date order.
        order: AlbumOrder,
    },
}

/// One source with images on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSource {
    /// The row id, or [`MANUAL_ID`].
    pub id: String,
    /// The row type, or [`PoolKind::Manual`].
    pub kind: PoolKind,
    /// Its images.
    pub files: PoolFiles,
}

/// Every enabled source that has images on disk, in config order (the manual source last).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pool {
    /// The sources; none is empty.
    pub sources: Vec<PoolSource>,
}

/// How a [`Picked`] image relates to the filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickOutcome {
    /// It passes every enabled filter (or no filter is enabled).
    Passed,
    /// No candidate passed. This one had the best score; `failed` lists the filters it fails
    /// (empty when no candidate's facts could be read).
    Fallback {
        /// Names of the failed filters, in [`Filters::check`] order.
        failed: Vec<&'static str>,
    },
    /// The next image of an album; filters don't apply.
    Album,
}

/// The image [`pick`] chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// The image file.
    pub path: PathBuf,
    /// The source it came from (a row id or [`MANUAL_ID`]).
    pub source_id: String,
    /// The source's kind.
    pub kind: PoolKind,
    /// Whether it passed the filters.
    pub outcome: PickOutcome,
}

impl Picked {
    /// Records the pick in `state`: for an album, its cursor moves to this image. Other picks
    /// change nothing. The caller saves the state.
    pub fn commit(&self, state: &mut State) {
        if self.outcome == PickOutcome::Album {
            state.set_album_cursor(self.source_id.clone(), self.path.clone());
        }
    }
}

impl Pool {
    /// Lists the images of every enabled source of `cfg` (see the module docs). Blocking: it
    /// walks folders, so call it from `spawn_blocking` in async code.
    pub fn collect(cfg: &Config) -> Pool {
        let mut sources: Vec<PoolSource> = cfg
            .sources
            .iter()
            .filter(|entry| entry.enabled)
            .filter_map(|entry| {
                let files = source_files(cfg, &entry.id, entry.kind, &entry.location)?;
                Some(PoolSource {
                    id: entry.id.clone(),
                    kind: PoolKind::Source(entry.kind),
                    files,
                })
            })
            .collect();
        let manual = list_images(&cfg.daemon.manual_dir_resolved(), false);
        if !manual.is_empty() {
            sources.push(PoolSource {
                id: MANUAL_ID.to_owned(),
                kind: PoolKind::Manual,
                files: PoolFiles::Files(manual),
            });
        }
        Pool { sources }
    }

    /// Whether no source has images.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// The source with id `id`.
    pub fn get(&self, id: &str) -> Option<&PoolSource> {
        self.sources.iter().find(|s| s.id == id)
    }
}

/// Chooses an image from `pool` (see the module docs). `facts` supplies the image facts for the
/// filters; it is not consulted when no filter is enabled. `state` gives the current wallpaper,
/// the recent history and the album cursors. Returns `None` only when the pool is empty.
pub async fn pick<R: Rng + ?Sized>(
    pool: &Pool,
    filters: &Filters,
    ctx: &FilterCtx,
    facts: &Arc<FactsCache>,
    state: &State,
    rng: &mut R,
) -> Option<Picked> {
    let mut candidates = exclusion_tiers(state)
        .iter()
        .map(|exclude| eligible(pool, exclude))
        .find(|c| !c.is_empty())?;
    let mut budget = MAX_CANDIDATES;
    // Best failing candidate so far: (score, source, path, failed filters).
    let mut best: Option<(usize, &PoolSource, PathBuf, Vec<&'static str>)> = None;
    // First candidate whose facts couldn't be read, the last resort.
    let mut unreadable: Option<(&PoolSource, PathBuf)> = None;
    while budget > 0 && !candidates.is_empty() {
        let (source, files) = candidates.swap_remove(rng.random_range(0..candidates.len()));
        let files = match files {
            Eligible::Album { dir, order } => {
                match album_next(dir, state.album_cursor(&source.id), order) {
                    Some(path) => return Some(picked(source, path, PickOutcome::Album)),
                    None => continue,
                }
            }
            Eligible::Files(files) => files,
        };
        if filters.is_empty() {
            let path = files.choose(rng)?;
            return Some(picked(source, path.to_path_buf(), PickOutcome::Passed));
        }
        let take = budget.min(files.len());
        let sample: Vec<PathBuf> = files.sample(rng, take).map(|p| p.to_path_buf()).collect();
        for path in sample {
            budget -= 1;
            let Some(image) = facts.facts(&path).await else {
                unreadable.get_or_insert((source, path));
                continue;
            };
            match filters.check(&path, &image, ctx) {
                Ok(()) => return Some(picked(source, path, PickOutcome::Passed)),
                Err(failed) => {
                    let score = filters.score(&path, &image, ctx);
                    if best.as_ref().is_none_or(|(s, ..)| score > *s) {
                        best = Some((score, source, path, failed));
                    }
                }
            }
        }
    }
    if let Some((_, source, path, failed)) = best {
        tracing::info!(
            path = %path.display(),
            source = %source.id,
            ?failed,
            "no image passed the filters; using the best one"
        );
        return Some(picked(source, path, PickOutcome::Fallback { failed }));
    }
    let (source, path) = unreadable?;
    tracing::info!(
        path = %path.display(),
        source = %source.id,
        "no image's facts could be read; using one unfiltered"
    );
    Some(picked(
        source,
        path,
        PickOutcome::Fallback { failed: Vec::new() },
    ))
}

/// The images of one enabled row, or `None` when it has none or isn't a usable type.
fn source_files(cfg: &Config, id: &str, kind: SourceType, location: &str) -> Option<PoolFiles> {
    let files = if kind.is_online() {
        // The id names a folder; an id that isn't `[a-z0-9-]` could escape the download dir.
        if !valid_id(id) {
            return None;
        }
        PoolFiles::Files(list_images(&cfg.daemon.download_dir.join(id), false))
    } else {
        let location = Path::new(location);
        match kind {
            SourceType::Folder => PoolFiles::Files(list_images(location, true)),
            SourceType::Images => {
                let ok = location.is_file() && is_image_file(location);
                PoolFiles::Files(if ok {
                    vec![location.to_path_buf()]
                } else {
                    Vec::new()
                })
            }
            SourceType::AlbumName | SourceType::AlbumDate => {
                if list_images(location, false).is_empty() {
                    return None;
                }
                let order = if kind == SourceType::AlbumName {
                    AlbumOrder::Name
                } else {
                    AlbumOrder::Date
                };
                PoolFiles::Album {
                    dir: location.to_path_buf(),
                    order,
                }
            }
            _ => return None,
        }
    };
    match &files {
        PoolFiles::Files(list) if list.is_empty() => None,
        _ => Some(files),
    }
}

/// The paths to avoid, from the strictest to none: the current wallpaper and the recent
/// history, then the current wallpaper only, then nothing.
fn exclusion_tiers(state: &State) -> [HashSet<PathBuf>; 3] {
    let current = state.current().map(|w| PathBuf::from(&w.path));
    let recent = state
        .history()
        .iter()
        .rev()
        .map(|w| PathBuf::from(&w.path))
        .filter(|p| Some(p) != current.as_ref())
        .take(RECENT_HISTORY);
    let strict = current.iter().cloned().chain(recent).collect();
    [strict, current.into_iter().collect(), HashSet::new()]
}

/// What a pool source offers once the excluded paths are removed.
enum Eligible<'a> {
    Files(Vec<&'a Path>),
    Album { dir: &'a Path, order: AlbumOrder },
}

/// Every source that still has an image outside `exclude`. Albums ignore `exclude`.
fn eligible<'a>(pool: &'a Pool, exclude: &HashSet<PathBuf>) -> Vec<(&'a PoolSource, Eligible<'a>)> {
    pool.sources
        .iter()
        .filter_map(|source| match &source.files {
            PoolFiles::Files(files) => {
                let left: Vec<&Path> = files
                    .iter()
                    .filter(|p| !exclude.contains(*p))
                    .map(PathBuf::as_path)
                    .collect();
                (!left.is_empty()).then_some((source, Eligible::Files(left)))
            }
            PoolFiles::Album { dir, order } => {
                Some((source, Eligible::Album { dir, order: *order }))
            }
        })
        .collect()
}

fn picked(source: &PoolSource, path: PathBuf, outcome: PickOutcome) -> Picked {
    Picked {
        path,
        source_id: source.id.clone(),
        kind: source.kind,
        outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use image::{ImageFormat, Rgb, RgbImage};
    use kakemono_config::schema::{BrightnessFilter, FiltersConfig, SourceEntry};
    use kakemono_platform::api::Theme;
    use kakemono_protocol::types::WallpaperInfo;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use tempfile::TempDir;

    const DARK: [u8; 3] = [10, 10, 12];
    const LIGHT: [u8; 3] = [245, 245, 240];

    fn write_png(path: &Path, color: [u8; 3]) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        RgbImage::from_pixel(32, 24, Rgb(color))
            .save_with_format(path, ImageFormat::Png)
            .unwrap();
        path.to_path_buf()
    }

    fn entry(id: &str, enabled: bool, kind: SourceType, location: &str) -> SourceEntry {
        SourceEntry {
            id: id.to_owned(),
            enabled,
            kind,
            location: location.to_owned(),
        }
    }

    /// Defaults rooted at `home`, with no sources and every filter off.
    fn config(home: &Path) -> Config {
        let mut cfg = kakemono_config::defaults_for_home(home);
        cfg.daemon.download_dir = home.join("downloads");
        cfg.daemon.manual_dir = Some(home.join("manual"));
        cfg.sources.clear();
        cfg.filters = FiltersConfig::default();
        cfg
    }

    fn dark_filter() -> Filters {
        let (filters, warnings) = Filters::compile(&FiltersConfig {
            brightness: BrightnessFilter::Dark,
            ..FiltersConfig::default()
        });
        assert!(warnings.is_empty());
        filters
    }

    fn ctx() -> FilterCtx {
        FilterCtx {
            screen: (0, 0),
            theme: Theme::Light,
        }
    }

    fn facts(home: &Path) -> Arc<FactsCache> {
        Arc::new(FactsCache::load(home.join("facts.json")))
    }

    fn files_source(id: &str, kind: PoolKind, files: Vec<PathBuf>) -> PoolSource {
        PoolSource {
            id: id.to_owned(),
            kind,
            files: PoolFiles::Files(files),
        }
    }

    fn with_current(path: &Path) -> State {
        let mut state = State::new();
        state.push(
            WallpaperInfo {
                path: path.to_string_lossy().into_owned(),
                ..WallpaperInfo::default()
            },
            chrono::Local::now().fixed_offset(),
        );
        state
    }

    #[test]
    fn pool_collect_includes_enabled_rows_and_manual() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let mut cfg = config(home);
        let dl = cfg.daemon.download_dir.clone();
        let wh = write_png(&dl.join("wh-1/a.png"), DARK);
        write_png(&dl.join("wh-1/.kakemono-seen.png"), DARK); // hidden: skipped
        write_png(&dl.join("off-1/b.png"), DARK);
        let nested = write_png(&home.join("pics/sub/c.png"), DARK);
        let top = write_png(&home.join("pics/d.png"), DARK);
        let single = write_png(&home.join("one/e.png"), DARK);
        write_png(&home.join("one/not-me.png"), DARK);
        let album = home.join("album");
        write_png(&album.join("01.png"), DARK);
        let manual = write_png(&home.join("manual/m.png"), DARK);
        write_png(&home.join("unsupported/u.png"), DARK);
        cfg.sources = vec![
            entry("wh-1", true, SourceType::Wallhaven, "forest"),
            entry("off-1", false, SourceType::Bing, ""),
            entry(
                "fld-1",
                true,
                SourceType::Folder,
                &home.join("pics").to_string_lossy(),
            ),
            entry("img-1", true, SourceType::Images, &single.to_string_lossy()),
            entry(
                "aln-1",
                true,
                SourceType::AlbumName,
                &album.to_string_lossy(),
            ),
            entry(
                "off-2",
                false,
                SourceType::Folder,
                &home.join("pics").to_string_lossy(),
            ),
            entry(
                "x-1",
                true,
                SourceType::Unsupported,
                &home.join("unsupported").to_string_lossy(),
            ),
            entry("empty-1", true, SourceType::Reddit, "wallpapers"),
        ];

        let pool = Pool::collect(&cfg);

        let expected = vec![
            files_source("wh-1", PoolKind::Source(SourceType::Wallhaven), vec![wh]),
            files_source(
                "fld-1",
                PoolKind::Source(SourceType::Folder),
                vec![top, nested],
            ),
            files_source("img-1", PoolKind::Source(SourceType::Images), vec![single]),
            PoolSource {
                id: "aln-1".to_owned(),
                kind: PoolKind::Source(SourceType::AlbumName),
                files: PoolFiles::Album {
                    dir: album,
                    order: AlbumOrder::Name,
                },
            },
            files_source(MANUAL_ID, PoolKind::Manual, vec![manual]),
        ];
        assert_eq!(pool.sources, expected);
    }

    #[test]
    fn pool_collect_manual_defaults_under_download_dir() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = config(tmp.path());
        cfg.daemon.manual_dir = None;
        let m = write_png(&cfg.daemon.download_dir.join("manual/m.png"), DARK);
        let pool = Pool::collect(&cfg);
        assert_eq!(
            pool.sources,
            vec![files_source(MANUAL_ID, PoolKind::Manual, vec![m])]
        );
    }

    #[tokio::test]
    async fn pool_pick_empty_pool_is_none() {
        let tmp = TempDir::new().unwrap();
        let pool = Pool::collect(&config(tmp.path()));
        assert!(pool.is_empty());
        let mut rng = StdRng::seed_from_u64(1);
        let picked = pick(
            &pool,
            &dark_filter(),
            &ctx(),
            &facts(tmp.path()),
            &State::new(),
            &mut rng,
        )
        .await;
        assert_eq!(picked, None);
    }

    #[tokio::test]
    async fn pool_pick_dark_filter_chooses_the_dark_image() {
        let tmp = TempDir::new().unwrap();
        let dark = write_png(&tmp.path().join("p/dark.png"), DARK);
        let light = write_png(&tmp.path().join("p/light.png"), LIGHT);
        let pool = Pool {
            sources: vec![files_source(
                "fld-1",
                PoolKind::Source(SourceType::Folder),
                vec![light, dark.clone()],
            )],
        };
        let cache = facts(tmp.path());
        for seed in 0..16 {
            let mut rng = StdRng::seed_from_u64(seed);
            let picked = pick(
                &pool,
                &dark_filter(),
                &ctx(),
                &cache,
                &State::new(),
                &mut rng,
            )
            .await
            .unwrap();
            assert_eq!(picked.path, dark, "seed {seed}");
            assert_eq!(picked.outcome, PickOutcome::Passed);
            assert_eq!(picked.source_id, "fld-1");
            assert_eq!(picked.kind, PoolKind::Source(SourceType::Folder));
        }
    }

    #[tokio::test]
    async fn pool_pick_dark_filter_searches_other_sources() {
        let tmp = TempDir::new().unwrap();
        let dark = write_png(&tmp.path().join("b/dark.png"), DARK);
        let lights: Vec<PathBuf> = (0..3)
            .map(|i| write_png(&tmp.path().join(format!("a/light{i}.png")), LIGHT))
            .collect();
        let pool = Pool {
            sources: vec![
                files_source("a", PoolKind::Source(SourceType::Folder), lights),
                files_source("b", PoolKind::Manual, vec![dark.clone()]),
            ],
        };
        let cache = facts(tmp.path());
        for seed in 0..16 {
            let mut rng = StdRng::seed_from_u64(seed);
            let picked = pick(
                &pool,
                &dark_filter(),
                &ctx(),
                &cache,
                &State::new(),
                &mut rng,
            )
            .await
            .unwrap();
            assert_eq!(picked.path, dark, "seed {seed}");
            assert_eq!(picked.source_id, "b");
        }
    }

    #[tokio::test]
    async fn pool_pick_falls_back_to_the_best_score() {
        let tmp = TempDir::new().unwrap();
        // Landscape + dark: the 32x24 light image passes landscape only (score 1); the 24x32
        // light image passes nothing (score 0).
        let wide = write_png(&tmp.path().join("p/wide.png"), LIGHT);
        let tall = tmp.path().join("p/tall.png");
        RgbImage::from_pixel(24, 32, Rgb(LIGHT))
            .save_with_format(&tall, ImageFormat::Png)
            .unwrap();
        let (filters, _) = Filters::compile(&FiltersConfig {
            landscape: true,
            brightness: BrightnessFilter::Dark,
            ..FiltersConfig::default()
        });
        let pool = Pool {
            sources: vec![files_source(
                "fld-1",
                PoolKind::Source(SourceType::Folder),
                vec![tall, wide.clone()],
            )],
        };
        let cache = facts(tmp.path());
        for seed in 0..16 {
            let mut rng = StdRng::seed_from_u64(seed);
            let picked = pick(&pool, &filters, &ctx(), &cache, &State::new(), &mut rng)
                .await
                .unwrap();
            assert_eq!(picked.path, wide, "seed {seed}");
            assert_eq!(
                picked.outcome,
                PickOutcome::Fallback {
                    failed: vec![crate::filters::BRIGHTNESS]
                }
            );
        }
    }

    #[tokio::test]
    async fn pool_pick_excludes_current_unless_it_is_the_only_image() {
        let tmp = TempDir::new().unwrap();
        let a = write_png(&tmp.path().join("p/a.png"), DARK);
        let b = write_png(&tmp.path().join("p/b.png"), DARK);
        let cache = facts(tmp.path());
        let state = with_current(&a);
        let both = Pool {
            sources: vec![files_source(
                "fld-1",
                PoolKind::Source(SourceType::Folder),
                vec![a.clone(), b.clone()],
            )],
        };
        let only = Pool {
            sources: vec![files_source(
                "fld-1",
                PoolKind::Source(SourceType::Folder),
                vec![a.clone()],
            )],
        };
        for seed in 0..16 {
            let mut rng = StdRng::seed_from_u64(seed);
            let picked = pick(&both, &Filters::default(), &ctx(), &cache, &state, &mut rng)
                .await
                .unwrap();
            assert_eq!(picked.path, b, "seed {seed}");
            let picked = pick(&only, &dark_filter(), &ctx(), &cache, &state, &mut rng)
                .await
                .unwrap();
            assert_eq!(picked.path, a, "seed {seed}");
        }
    }

    #[tokio::test]
    async fn pool_pick_avoids_recent_history() {
        let tmp = TempDir::new().unwrap();
        let files: Vec<PathBuf> = (0..8)
            .map(|i| write_png(&tmp.path().join(format!("p/{i}.png")), DARK))
            .collect();
        let mut state = State::new();
        // History 0..=6; current is 6, the recent five are 1..=5. Only 0 and 7 are left.
        for f in &files[..7] {
            state.push(
                WallpaperInfo {
                    path: f.to_string_lossy().into_owned(),
                    ..WallpaperInfo::default()
                },
                chrono::Local::now().fixed_offset(),
            );
        }
        let pool = Pool {
            sources: vec![files_source(
                "fld-1",
                PoolKind::Source(SourceType::Folder),
                files.clone(),
            )],
        };
        let cache = facts(tmp.path());
        for seed in 0..32 {
            let mut rng = StdRng::seed_from_u64(seed);
            let picked = pick(&pool, &Filters::default(), &ctx(), &cache, &state, &mut rng)
                .await
                .unwrap();
            assert!(
                picked.path == files[0] || picked.path == files[7],
                "seed {seed}: {}",
                picked.path.display()
            );
        }
    }

    #[tokio::test]
    async fn pool_pick_album_returns_next_and_commit_advances_cursor() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("album");
        // Light images: albums ignore the dark filter.
        let first = write_png(&album.join("01.png"), LIGHT);
        let second = write_png(&album.join("02.png"), LIGHT);
        let mut cfg = config(tmp.path());
        cfg.filters.brightness = BrightnessFilter::Dark;
        cfg.sources = vec![entry(
            "aln-1",
            true,
            SourceType::AlbumName,
            &album.to_string_lossy(),
        )];
        let pool = Pool::collect(&cfg);
        let cache = facts(tmp.path());
        let mut state = State::new();
        let mut rng = StdRng::seed_from_u64(7);
        let mut seen = Vec::new();
        for _ in 0..3 {
            let picked = pick(&pool, &dark_filter(), &ctx(), &cache, &state, &mut rng)
                .await
                .unwrap();
            assert_eq!(picked.outcome, PickOutcome::Album);
            assert_eq!(picked.source_id, "aln-1");
            picked.commit(&mut state);
            assert_eq!(state.album_cursor("aln-1"), Some(picked.path.as_path()));
            seen.push(picked.path);
        }
        assert_eq!(seen, vec![first.clone(), second, first]);
    }

    #[tokio::test]
    async fn pool_pick_non_album_commit_leaves_cursors_alone() {
        let tmp = TempDir::new().unwrap();
        let a = write_png(&tmp.path().join("p/a.png"), DARK);
        let pool = Pool {
            sources: vec![files_source(
                "fld-1",
                PoolKind::Source(SourceType::Folder),
                vec![a],
            )],
        };
        let mut state = State::new();
        let mut rng = StdRng::seed_from_u64(3);
        let picked = pick(
            &pool,
            &Filters::default(),
            &ctx(),
            &facts(tmp.path()),
            &state,
            &mut rng,
        )
        .await
        .unwrap();
        picked.commit(&mut state);
        assert!(state.album_cursors().is_empty());
    }

    #[test]
    fn pool_kind_names() {
        assert_eq!(PoolKind::Manual.as_str(), "manual");
        assert_eq!(
            PoolKind::Source(SourceType::AlbumDate).as_str(),
            "album-date"
        );
    }
}
