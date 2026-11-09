//! Task 17: `Registry::from_config` and `candidates` (Go `Registry.Random` semantics).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use kakemono_config::defaults_for_home;
use kakemono_sources::registry::Registry;
use kakemono_sources::{Provider, ProviderError, WallpaperResult, http};
use rand::SeedableRng;
use rand::rngs::StdRng;

/// A provider that only has a name and an enabled flag. (`fake::StaticProvider` is not
/// compiled for this crate's own integration tests: the `fake` feature is off here.)
struct Named {
    name: &'static str,
    enabled: bool,
}

#[async_trait]
impl Provider for Named {
    fn name(&self) -> &'static str {
        self.name
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    async fn fetch(&self, _query: &str) -> Result<Vec<WallpaperResult>, ProviderError> {
        Ok(vec![])
    }
}

fn names(list: &[Arc<dyn Provider>]) -> Vec<&'static str> {
    list.iter().map(|p| p.name()).collect()
}

fn sources(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn fake(name: &'static str, enabled: bool) -> Arc<Named> {
    Arc::new(Named { name, enabled })
}

fn registry(entries: &[(&'static str, bool)]) -> Registry {
    entries.iter().fold(Registry::new(), |r, (name, enabled)| {
        r.with(fake(name, *enabled))
    })
}

#[test]
fn from_config_registers_all_six_providers_in_go_order() {
    let cfg = defaults_for_home(Path::new("/home/test"));
    let reg = Registry::from_config(&cfg, http::client().unwrap());
    assert_eq!(
        reg.names(),
        ["bing", "wallhaven", "reddit", "unsplash", "apod", "local"]
    );
    // Defaults: only bing is enabled.
    assert_eq!(names(&reg.enabled()), ["bing"]);
    assert_eq!(names(&reg.candidates(&sources(&["bing"]))), ["bing"]);
}

#[test]
fn from_config_follows_enabled_flags() {
    let mut cfg = defaults_for_home(Path::new("/home/test"));
    cfg.bing.enabled = false;
    cfg.apod.enabled = true;
    cfg.local.enabled = true;
    cfg.reddit.enabled = true;
    cfg.reddit.subreddits.clear(); // reddit needs subreddits to count as enabled
    let reg = Registry::from_config(&cfg, http::client().unwrap());
    assert_eq!(names(&reg.enabled()), ["apod", "local"]);
    assert!(reg.get("reddit").is_some_and(|p| !p.enabled()));
}

#[test]
fn unknown_names_are_ignored() {
    let reg = registry(&[("bing", true), ("apod", true)]);
    let got = reg.candidates(&sources(&["nope", "bing", "flickr"]));
    assert_eq!(names(&got), ["bing"]);
}

#[test]
fn disabled_providers_are_excluded() {
    let reg = registry(&[("bing", true), ("apod", false), ("local", true)]);
    for _ in 0..50 {
        let got = reg.candidates(&sources(&["apod", "local"]));
        assert_eq!(names(&got), ["local"]);
    }
    assert!(names(&reg.enabled()).iter().all(|n| *n != "apod"));
}

#[test]
fn listed_enabled_providers_come_back_once_each_in_random_order() {
    let reg = registry(&[
        ("bing", true),
        ("apod", true),
        ("local", true),
        ("reddit", true),
    ]);
    let listed = sources(&["bing", "apod", "local", "apod"]);
    let mut orders = HashSet::new();
    let mut rng = StdRng::seed_from_u64(7);
    for _ in 0..200 {
        let got = names(&reg.candidates_with_rng(&listed, &mut rng));
        let set: HashSet<_> = got.iter().copied().collect();
        assert_eq!(got.len(), 3, "duplicates removed: {got:?}");
        assert_eq!(
            set,
            HashSet::from(["bing", "apod", "local"]),
            "reddit is not listed"
        );
        orders.insert(got);
    }
    assert!(orders.len() > 1, "order should vary: {orders:?}");
}

#[test]
fn falls_back_to_every_enabled_provider_when_none_listed_is_usable() {
    let reg = registry(&[("bing", true), ("apod", false), ("local", true)]);
    for listed in [sources(&[]), sources(&["apod"]), sources(&["unknown"])] {
        let got: HashSet<_> = names(&reg.candidates(&listed)).into_iter().collect();
        assert_eq!(got, HashSet::from(["bing", "local"]), "for {listed:?}");
    }
}

#[test]
fn fallback_order_is_shuffled() {
    let reg = registry(&[("bing", true), ("apod", true), ("local", true)]);
    let mut rng = StdRng::seed_from_u64(1);
    let firsts: HashSet<_> = (0..200)
        .map(|_| names(&reg.candidates_with_rng(&[], &mut rng))[0])
        .collect();
    assert_eq!(firsts, HashSet::from(["bing", "apod", "local"]));
}

#[test]
fn empty_when_nothing_is_enabled() {
    let reg = registry(&[("bing", false), ("apod", false)]);
    assert!(reg.candidates(&sources(&["bing"])).is_empty());
    assert!(reg.candidates(&[]).is_empty());
    assert!(reg.random(&sources(&["bing"])).is_none());
    assert!(Registry::new().candidates(&[]).is_empty());
}

#[test]
fn duplicate_names_weight_the_first_pick_like_go_random() {
    // Go shuffled the list and picked uniformly among enabled entries, so a name listed twice
    // was twice as likely. `candidates()[0]` keeps that distribution.
    let reg = registry(&[("bing", true), ("apod", true)]);
    let listed = sources(&["bing", "bing", "apod"]);
    let mut rng = StdRng::seed_from_u64(42);
    let n = 3000;
    let bing_first = (0..n)
        .filter(|_| names(&reg.candidates_with_rng(&listed, &mut rng))[0] == "bing")
        .count();
    let share = bing_first as f64 / f64::from(n);
    assert!((0.6..0.73).contains(&share), "bing first share {share}");
}

#[test]
fn register_replaces_a_provider_with_the_same_name() {
    let mut reg = registry(&[("bing", true), ("apod", true)]);
    reg.register(fake("bing", false));
    assert_eq!(reg.names(), ["bing", "apod"]);
    assert!(reg.get("bing").is_some_and(|p| !p.enabled()));
    assert_eq!(names(&reg.candidates(&sources(&["bing"]))), ["apod"]);
}

#[test]
fn debug_lists_names_and_enabled_flags() {
    let reg = registry(&[("bing", true), ("apod", false)]);
    assert_eq!(format!("{reg:?}"), r#"[("bing", true), ("apod", false)]"#);
}
