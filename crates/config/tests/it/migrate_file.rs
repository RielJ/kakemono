//! Task 15: the legacy migration wired into loading ([`parse_str`]) and onto disk
//! ([`migrate_file`], [`migrate_config`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime};

use kakemono_config::paths::{BaseDirs, ConfigSource, Os, Paths};
use kakemono_config::{
    SourceType, load_with_home, migrate_config, migrate_document, migrate_file, parse_str, valid_id,
};
use toml_edit::DocumentMut;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/migrate")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The Go template config, which has `[light]`, `[dark]` and the provider `enabled` flags.
fn legacy_text() -> String {
    fixture("go_template.toml")
}

/// Every `config.toml.bak*` file next to `config`.
fn backups(config: &Path) -> Vec<PathBuf> {
    let name = config.file_name().unwrap().to_string_lossy().into_owned();
    let mut out: Vec<PathBuf> = std::fs::read_dir(config.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{name}.bak"))
        })
        .collect();
    out.sort();
    out
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

/// Moves `path`'s mtime an hour back, so a rewrite would show as a changed mtime.
fn age(path: &Path) -> SystemTime {
    let old = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    mtime(path)
}

fn doc(text: &str) -> DocumentMut {
    text.parse().unwrap()
}

fn paths_in(root: &Path) -> Paths {
    let base = BaseDirs {
        home: root.to_owned(),
        config_home: root.join("cfg"),
        cache_home: root.join("cache"),
        state_home: Some(root.join("state")),
        runtime_home: Some(root.join("run")),
        config_override: None,
    };
    Paths::from_base(&base, Os::Linux)
}

// ── parse_str ───────────────────────────────────────────────────────────────

#[test]
fn parse_str_migrates_legacy_text_in_memory() {
    let home = Path::new("/home/u");
    let text = legacy_text();
    let mut doc: DocumentMut = text.parse().unwrap();
    let report = migrate_document(&mut doc, home).expect("the Go template is legacy");
    assert!(!report.rows.is_empty());

    let cfg = parse_str(&text, home).unwrap();
    assert_eq!(cfg.sources, report.rows);
}

#[test]
fn parse_str_normalizes_source_ids() {
    let text = "[[sources]]\nid = \"Not Valid!\"\nenabled = true\ntype = \"bing\"\nlocation = \"\"\n\n\
                [[sources]]\nid = \"dup\"\nenabled = true\ntype = \"apod\"\nlocation = \"\"\n\n\
                [[sources]]\nid = \"dup\"\nenabled = true\ntype = \"bing\"\nlocation = \"\"\n";
    let cfg = parse_str(text, Path::new("/home/u")).unwrap();
    let ids: Vec<&str> = cfg.sources.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.iter().all(|id| valid_id(id)), "{ids:?}");
    assert_eq!(ids[1], "dup", "the first row keeps a duplicated id");
    assert_ne!(ids[2], "dup", "{ids:?}");
    assert_eq!(cfg.sources[0].kind, SourceType::Bing);
}

#[test]
fn parse_str_still_reports_invalid_toml() {
    assert!(parse_str("[daemon\n", Path::new("/home/u")).is_err());
}

// ── migrate_file (in place) ─────────────────────────────────────────────────

#[test]
fn migrate_file_keeps_a_byte_identical_bak_and_rewrites_the_config() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config = tmp.path().join("config.toml");
    let original = legacy_text().into_bytes();
    std::fs::write(&config, &original).unwrap();

    let done = migrate_file(&config, home)
        .unwrap()
        .expect("legacy file migrates");
    let bak = tmp.path().join("config.toml.bak");
    assert_eq!((&done.path, &done.backup), (&config, &bak));
    assert_eq!(
        std::fs::read(&bak).unwrap(),
        original,
        ".bak is byte-identical"
    );
    assert_eq!(backups(&config), vec![bak.clone()]);

    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("[[sources]]"), "{text}");
    assert!(!doc(&text).contains_key("light"), "{text}");
    let cfg = load_with_home(&config, home).unwrap();
    assert_eq!(cfg.sources.len(), done.report.rows.len());

    // A second call finds nothing to do and leaves the .bak alone.
    let bak_mtime = age(&bak);
    let config_mtime = age(&config);
    assert!(migrate_file(&config, home).unwrap().is_none());
    assert_eq!(std::fs::read(&bak).unwrap(), original);
    assert_eq!(mtime(&bak), bak_mtime);
    assert_eq!(mtime(&config), config_mtime);
    assert_eq!(backups(&config), vec![bak]);
}

#[test]
fn migrate_file_uses_a_timestamped_bak_when_a_different_bak_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.toml");
    let original = legacy_text().into_bytes();
    std::fs::write(&config, &original).unwrap();
    let bak = tmp.path().join("config.toml.bak");
    std::fs::write(&bak, b"someone else's backup").unwrap();

    let done = migrate_file(&config, tmp.path()).unwrap().unwrap();
    let backup = &done.backup;
    let name = backup.file_name().unwrap().to_string_lossy().into_owned();
    let ts = name.strip_prefix("config.toml.bak.").expect(&name);
    assert!(
        !ts.is_empty() && ts.chars().all(|c| c.is_ascii_digit()),
        "{name}"
    );
    assert_eq!(std::fs::read(backup).unwrap(), original);
    assert_eq!(std::fs::read(&bak).unwrap(), b"someone else's backup");
    assert_eq!(backups(&config).len(), 2);
}

#[test]
fn migrate_file_treats_an_identical_bak_as_done() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.toml");
    let original = legacy_text().into_bytes();
    std::fs::write(&config, &original).unwrap();
    let bak = tmp.path().join("config.toml.bak");
    std::fs::write(&bak, &original).unwrap();

    let done = migrate_file(&config, tmp.path()).unwrap().unwrap();
    assert_eq!((&done.path, &done.backup), (&config, &bak));
    assert_eq!(backups(&config), vec![bak]);
    assert!(
        std::fs::read_to_string(&config)
            .unwrap()
            .contains("[[sources]]")
    );
}

#[test]
fn migrate_file_never_rewrites_a_config_without_legacy_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.toml");
    let text = fixture("no_legacy.toml");
    std::fs::write(&config, &text).unwrap();
    let before = age(&config);

    assert!(migrate_file(&config, tmp.path()).unwrap().is_none());
    assert_eq!(mtime(&config), before, "mtime unchanged");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
    assert!(backups(&config).is_empty());
}

#[test]
fn migrate_file_ignores_missing_and_unparsable_files() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("config.toml");
    assert!(migrate_file(&missing, tmp.path()).unwrap().is_none());

    std::fs::write(&missing, "[light\n").unwrap();
    assert!(migrate_file(&missing, tmp.path()).unwrap().is_none());
    assert!(backups(&missing).is_empty());
}

#[cfg(unix)]
#[test]
fn migrate_file_in_a_read_only_dir_fails_but_loading_still_migrates() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("ro");
    std::fs::create_dir(&dir).unwrap();
    let config = dir.join("config.toml");
    let original = legacy_text();
    std::fs::write(&config, &original).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root ignores the mode bits; the test means nothing there.
    if std::fs::File::create(dir.join("probe")).is_ok() {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let result = migrate_file(&config, tmp.path());
    let cfg = load_with_home(&config, tmp.path());
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(result.is_err(), "{result:?}");
    assert_eq!(files.len(), 1, "no .bak or temp file left behind");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
    let cfg = cfg.unwrap();
    assert!(!cfg.sources.is_empty(), "the loader migrates in memory");
}

#[test]
fn concurrent_migrate_file_calls_write_one_bak() {
    for _ in 0..20 {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config.toml");
        let original = legacy_text().into_bytes();
        std::fs::write(&config, &original).unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (barrier, config, home) =
                    (barrier.clone(), config.clone(), tmp.path().to_owned());
                std::thread::spawn(move || {
                    barrier.wait();
                    migrate_file(&config, &home)
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().unwrap().unwrap())
            .collect();

        assert_eq!(
            results.iter().filter(|r| r.is_some()).count(),
            1,
            "exactly one call migrates"
        );
        let baks = backups(&config);
        assert_eq!(baks.len(), 1, "{baks:?}");
        assert_eq!(std::fs::read(&baks[0]).unwrap(), original);
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("[[sources]]"), "{text}");
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}

// ── migrate_config ──────────────────────────────────────────────────────────

#[test]
fn migrate_config_rewrites_the_primary_file_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(&paths.config_file, legacy_text()).unwrap();

    let source = paths.locate_config(None);
    assert_eq!(source, ConfigSource::Primary(paths.config_file.clone()));
    let done = migrate_config(&paths, &source)
        .unwrap()
        .expect("legacy file migrates");
    assert_eq!(done.path, paths.config_file);
    assert_eq!(backups(&paths.config_file), vec![done.backup.clone()]);

    // Nothing left to do on the next start.
    assert!(migrate_config(&paths, &source).unwrap().is_none());
}

#[test]
fn migrate_config_does_nothing_without_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    let source = paths.locate_config(None);
    assert_eq!(source, ConfigSource::Missing(paths.config_file.clone()));
    assert!(migrate_config(&paths, &source).unwrap().is_none());
    assert!(!paths.config_file.exists());
}
