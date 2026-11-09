//! Task 18: `State` ports `internal/wallpaper/state_test.go` and checks byte-level `state.json`
//! compatibility with the Go daemon using the committed protocol fixtures.

use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, TimeZone, Timelike, Utc};
use kakemono_config::paths::{BaseDirs, Os, Paths};
use kakemono_engine::state::{MAX_HISTORY, State, StateError, StateOrigin, format_go_time};
use kakemono_protocol::types::WallpaperInfo;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../protocol/tests/fixtures")
        .join(name)
}

fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).expect("read fixture")
}

fn make_info(n: usize) -> WallpaperInfo {
    WallpaperInfo {
        path: format!("/tmp/wallpaper{n}.jpg"),
        source: "test".into(),
        title: format!("Wallpaper {n}"),
        ..WallpaperInfo::default()
    }
}

fn now() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(8 * 3600)
        .and_then(|tz| tz.with_ymd_and_hms(2024, 3, 15, 10, 30, 0).single())
        .expect("valid time")
}

fn titles(s: &State) -> Vec<&str> {
    s.history().iter().map(|w| w.title.as_str()).collect()
}

// --- Ports of state_test.go -------------------------------------------------------------------

#[test]
fn state_push_max_size() {
    let mut s = State::new();
    for i in 0..MAX_HISTORY + 10 {
        s.push(make_info(i), now());
    }
    assert_eq!(s.history().len(), MAX_HISTORY);
    assert_eq!(s.history_index(), i32::try_from(MAX_HISTORY).unwrap() - 1);
    // The oldest entries were dropped.
    assert_eq!(s.history()[0].title, "Wallpaper 10");
    assert_eq!(
        s.current().unwrap().title,
        format!("Wallpaper {}", MAX_HISTORY + 9)
    );
}

#[test]
fn state_history_navigation() {
    let mut s = State::new();
    s.push(make_info(1), now());
    s.push(make_info(2), now());
    s.push(make_info(3), now());

    assert_eq!(s.history_index(), 2);
    assert!(s.can_go_prev());
    assert!(!s.has_future_history());

    assert_eq!(s.prev().unwrap().title, "Wallpaper 2");
    assert_eq!(s.history_index(), 1);
    assert_eq!(s.current().unwrap().title, "Wallpaper 2");
    assert!(s.has_future_history());

    assert_eq!(s.prev().unwrap().title, "Wallpaper 1");
    assert!(!s.can_go_prev());

    assert!(s.prev().is_none(), "prev at beginning returns None");
    assert_eq!(s.history_index(), 0);

    assert_eq!(s.next().unwrap().title, "Wallpaper 2");
    assert_eq!(s.current().unwrap().title, "Wallpaper 2");
    assert_eq!(s.next().unwrap().title, "Wallpaper 3");
    assert!(s.next().is_none(), "next at end returns None");
    assert_eq!(s.history_index(), 2);
}

#[test]
fn state_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");

    let mut s = State::new();
    s.push(make_info(1), now());
    s.push(make_info(2), now());
    s.save(&path).unwrap();

    let loaded = State::load(&path).unwrap();
    assert_eq!(loaded.history().len(), 2);
    assert_eq!(loaded.history_index(), 1);
    assert_eq!(loaded.current().unwrap().title, "Wallpaper 2");
    assert_eq!(loaded, s);
}

#[test]
fn state_load_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let s = State::load(&dir.path().join("nonexistent/state.json")).unwrap();
    assert!(s.history().is_empty());
    assert_eq!(s.history_index(), -1);
    assert_eq!(s.history_position(), None);
    assert!(s.current().is_none());
    assert!(!s.paused());
    assert!(s.last_changed().is_none());
    assert!(!s.can_go_prev());
    assert!(!s.has_future_history());
}

#[test]
fn state_push_truncates_forward_history() {
    let mut s = State::new();
    s.push(make_info(1), now());
    s.push(make_info(2), now());
    s.push(make_info(3), now());
    s.prev();
    s.prev();

    s.push(make_info(99), now());

    assert_eq!(titles(&s), ["Wallpaper 1", "Wallpaper 99"]);
    assert_eq!(s.history_index(), 1);
    assert!(!s.has_future_history());
}

#[test]
fn state_save_creates_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("subdir").join("state.json");
    State::new().save(&path).unwrap();
    assert!(path.is_file());
}

// --- Go byte compatibility --------------------------------------------------------------------

/// Parse the Go bytes, save them, and compare the bytes exactly.
fn assert_round_trip(name: &str) -> State {
    let go = read_fixture(name);
    let s = State::from_json_slice(&go).unwrap();
    let ours = s.to_json_bytes().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&ours),
        String::from_utf8_lossy(&go),
        "{name} must round-trip byte for byte"
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    s.save(&path).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        go,
        "{name}: save writes Go bytes"
    );
    assert_eq!(State::load(&path).unwrap(), s);
    s
}

#[test]
fn state_golden_empty_round_trip() {
    let s = assert_round_trip("state_empty.json");
    assert_eq!(s, State::new());
    assert!(s.last_changed().is_none(), "Go zero time loads as None");
    // And a fresh state writes exactly Go's fresh-state bytes.
    assert_eq!(
        State::new().to_json_bytes().unwrap(),
        read_fixture("state_empty.json")
    );
}

#[test]
fn state_golden_populated_round_trip() {
    let s = assert_round_trip("state_populated.json");
    assert_eq!(s.history().len(), 3);
    assert_eq!(s.history_index(), 1);
    assert!(s.paused());
    let cur = s.current().unwrap();
    assert_eq!(cur, &s.history()[1]);
    assert_eq!(cur.title, "Sunset <Golden Hour> & Co — café");
    assert_eq!(cur.author, "© Photographer \"Quoted\"");
    assert_eq!((cur.width, cur.height), (3840, 2160));
    assert!(s.can_go_prev());
    assert!(s.has_future_history());

    let lc = s.last_changed().unwrap();
    assert_eq!(lc.offset().local_minus_utc(), 8 * 3600);
    assert_eq!(lc.nanosecond(), 123_456_789);
    assert_eq!(
        format_go_time(&lc).unwrap(),
        "2024-03-15T10:30:00.123456789+08:00"
    );
}

#[test]
fn state_golden_wire_replay_round_trip() {
    let s = assert_round_trip("wire/state_after_replay.json");
    assert_eq!(s.history().len(), 3);
    assert_eq!(s.history_index(), 2);
    assert!(!s.paused());
    assert_eq!(
        s.current().unwrap().path,
        "/tmp/wpd-goldens-replay/pics/alpha.png"
    );
    assert_eq!(s.history()[1].title, "beta.png");
    assert_eq!(s.last_changed().unwrap().nanosecond(), 344_332_000);
}

#[test]
fn state_populated_then_push_matches_go_semantics() {
    // Index 1 of 3: pushing drops the forward entry, as Go's Push does.
    let mut s = State::from_json_slice(&read_fixture("state_populated.json")).unwrap();
    s.push(make_info(7), now());
    assert_eq!(s.history().len(), 3);
    assert_eq!(s.history()[2].title, "Wallpaper 7");
    assert_eq!(s.last_changed(), Some(now()));
}

#[test]
fn state_go_time_format() {
    let utc = Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap();
    assert_eq!(format_go_time(&utc).unwrap(), "2024-01-02T03:04:05Z");
    let frac = utc.with_nanosecond(120_000_000).unwrap();
    assert_eq!(format_go_time(&frac).unwrap(), "2024-01-02T03:04:05.12Z");
    let west = FixedOffset::west_opt(5 * 3600 + 30 * 60)
        .unwrap()
        .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
        .unwrap();
    assert_eq!(format_go_time(&west).unwrap(), "2024-01-02T03:04:05-05:30");
    let far = Utc.with_ymd_and_hms(10000, 1, 1, 0, 0, 0).unwrap();
    assert!(format_go_time(&far).is_err());
}

#[test]
fn state_push_stamps_now_in_its_offset() {
    let mut s = State::new();
    s.push(
        make_info(1),
        Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap(),
    );
    let json = String::from_utf8(s.to_json_bytes().unwrap()).unwrap();
    assert!(
        json.contains(r#""last_changed": "2024-01-02T03:04:05Z""#),
        "{json}"
    );
    assert!(json.contains(r#""history_index": 0"#), "{json}");

    // prev/next do not touch last_changed (Go).
    s.push(make_info(2), now());
    s.prev();
    s.next();
    assert_eq!(s.last_changed(), Some(now()));
}

#[test]
fn state_go_escaping_in_saved_file() {
    let mut s = State::new();
    s.push(
        WallpaperInfo {
            title: "a<b>&c\u{2028}\u{2029}é\"\\\n".into(),
            ..make_info(1)
        },
        now(),
    );
    let json = String::from_utf8(s.to_json_bytes().unwrap()).unwrap();
    assert!(
        json.contains(r#""title": "a\u003cb\u003e\u0026c\u2028\u2029é\"\\\n""#),
        "{json}"
    );
    assert_eq!(State::from_json_slice(json.as_bytes()).unwrap(), s);
}

// --- Lenient parsing and repair ---------------------------------------------------------------

#[test]
fn state_parse_is_lenient_like_go() {
    // `{}`: Go gives index 0 with nil history; we normalise to -1.
    let s = State::from_json_slice(b"{}").unwrap();
    assert_eq!(s, State::new());

    let s = State::from_json_slice(
        br#"{"history":null,"last_changed":null,"unknown":[1,2],"paused":true}"#,
    )
    .unwrap();
    assert!(s.paused());
    assert!(s.history().is_empty());
    assert!(s.last_changed().is_none());

    // Zero time in another offset is still Go's zero.
    let s = State::from_json_slice(br#"{"last_changed":"0001-01-01T08:00:00+08:00"}"#).unwrap();
    assert!(s.last_changed().is_none());

    assert!(State::from_json_slice(br#"{"last_changed":"yesterday"}"#).is_err());
    assert!(State::from_json_slice(br#"{"history_index":"1"}"#).is_err());
    assert!(State::from_json_slice(b"not json").is_err());
}

#[test]
fn state_out_of_range_index_is_repaired() {
    let two = r#"[{"path":"/a","title":"A"},{"path":"/b","title":"B"}]"#;
    for idx in ["5", "-1", "-7"] {
        let json = format!(r#"{{"history":{two},"history_index":{idx}}}"#);
        let mut s = State::from_json_slice(json.as_bytes()).unwrap();
        assert_eq!(s.history_index(), 1, "index {idx}");
        assert_eq!(s.current().unwrap().title, "B", "index {idx}");
        assert!(!s.has_future_history());
        assert_eq!(s.prev().unwrap().title, "A");
    }

    // In range: kept as is, current from the file.
    let json = format!(r#"{{"history":{two},"history_index":0,"current":{{"path":"/a"}}}}"#);
    let s = State::from_json_slice(json.as_bytes()).unwrap();
    assert_eq!(s.history_index(), 0);
    assert!(s.has_future_history());
    assert_eq!(s.current().unwrap().path, "/a");

    // Empty history with a stale index.
    let s = State::from_json_slice(br#"{"history":[],"history_index":3}"#).unwrap();
    assert_eq!(s.history_index(), -1);
}

#[test]
fn state_load_corrupt_file_is_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, b"{").unwrap();
    let err = State::load(&path).unwrap_err();
    assert!(matches!(err, StateError::Parse { .. }), "{err:?}");
    assert!(err.to_string().starts_with("state: parse "), "{err}");
}

#[test]
fn state_paused_flag() {
    let mut s = State::new();
    s.set_paused(true);
    assert!(s.paused());
    let json = String::from_utf8(s.to_json_bytes().unwrap()).unwrap();
    assert!(json.contains(r#""paused": true"#));
    s.set_paused(false);
    assert!(!s.paused());
}

// --- Atomic save ------------------------------------------------------------------------------

fn dir_entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn state_save_is_atomic_and_leaves_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, b"old contents").unwrap();

    let mut s = State::new();
    s.push(make_info(1), now());
    s.save(&path).unwrap();
    s.push(make_info(2), now());
    s.save(&path).unwrap();

    assert_eq!(State::load(&path).unwrap(), s);
    assert_eq!(dir_entries(dir.path()), ["state.json"]);
}

#[test]
fn state_failed_save_keeps_old_file_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    // The target is a non-empty directory, so the final rename fails.
    let path = dir.path().join("state.json");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep"), b"x").unwrap();

    let err = State::new().save(&path).unwrap_err();
    assert!(matches!(err, StateError::Write { .. }), "{err:?}");
    assert_eq!(dir_entries(dir.path()), ["state.json"]);
    assert_eq!(std::fs::read(path.join("keep")).unwrap(), b"x");
}

#[cfg(unix)]
#[test]
fn state_saved_file_mode_is_0644() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    State::new().save(&path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    // The umask may clear bits but never adds any.
    assert_eq!(mode & !0o644, 0, "mode {mode:o}");
}

// --- Migration from the Go app ----------------------------------------------------------------

struct Layout {
    _dir: tempfile::TempDir,
    new: PathBuf,
    legacy: PathBuf,
}

fn layout() -> Layout {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join(".config/kakemono/state.json");
    let legacy = dir.path().join(".config/wallpaperd/state.json");
    Layout {
        _dir: dir,
        new,
        legacy,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn state_migrates_legacy_file_once_without_touching_it() {
    let l = layout();
    let go = read_fixture("state_populated.json");
    write(&l.legacy, &go);
    let legacy_mtime = std::fs::metadata(&l.legacy).unwrap().modified().unwrap();

    let loaded = State::load_or_migrate(&l.new, &l.legacy).unwrap();
    assert_eq!(loaded.origin, StateOrigin::Migrated);
    assert_eq!(loaded.state.history().len(), 3);
    assert_eq!(
        std::fs::read(&l.new).unwrap(),
        go,
        "new file is the Go bytes"
    );
    assert_eq!(std::fs::read(&l.legacy).unwrap(), go, "legacy unchanged");
    assert_eq!(
        std::fs::metadata(&l.legacy).unwrap().modified().unwrap(),
        legacy_mtime
    );
    assert_eq!(dir_entries(l.legacy.parent().unwrap()), ["state.json"]);

    // Changes go to the new file only; the next load reads the new file.
    let mut s = loaded.state;
    s.set_paused(false);
    s.save(&l.new).unwrap();
    let again = State::load_or_migrate(&l.new, &l.legacy).unwrap();
    assert_eq!(again.origin, StateOrigin::Existing);
    assert!(!again.state.paused());
    assert_eq!(std::fs::read(&l.legacy).unwrap(), go);
}

#[test]
fn state_existing_file_wins_over_legacy() {
    let l = layout();
    write(&l.legacy, &read_fixture("state_populated.json"));
    write(&l.new, &read_fixture("state_empty.json"));

    let loaded = State::load_or_migrate(&l.new, &l.legacy).unwrap();
    assert_eq!(loaded.origin, StateOrigin::Existing);
    assert_eq!(loaded.state, State::new());
}

#[test]
fn state_fresh_when_neither_exists_and_nothing_is_written() {
    let l = layout();
    let loaded = State::load_or_migrate(&l.new, &l.legacy).unwrap();
    assert_eq!(loaded.origin, StateOrigin::Fresh);
    assert_eq!(loaded.state, State::new());
    assert!(!l.new.exists());
    assert!(!l.legacy.exists());
}

#[test]
fn state_corrupt_legacy_is_ignored_and_left_alone() {
    let l = layout();
    write(&l.legacy, b"{ broken");
    let loaded = State::load_or_migrate(&l.new, &l.legacy).unwrap();
    assert_eq!(loaded.origin, StateOrigin::Fresh);
    assert!(!l.new.exists());
    assert_eq!(std::fs::read(&l.legacy).unwrap(), b"{ broken");
}

#[test]
fn state_corrupt_new_file_is_an_error() {
    let l = layout();
    write(&l.new, b"{ broken");
    write(&l.legacy, &read_fixture("state_populated.json"));
    let err = State::load_or_migrate(&l.new, &l.legacy).unwrap_err();
    assert!(matches!(err, StateError::Parse { .. }), "{err:?}");
}

#[test]
fn state_same_path_for_new_and_legacy_just_loads() {
    let l = layout();
    write(&l.legacy, &read_fixture("state_populated.json"));
    let loaded = State::load_or_migrate(&l.legacy, &l.legacy).unwrap();
    assert_eq!(loaded.origin, StateOrigin::Existing);
    assert_eq!(loaded.state.history().len(), 3);
}

#[test]
fn state_load_from_paths_uses_config_paths() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let base = BaseDirs {
        home: home.clone(),
        config_home: home.join(".config"),
        cache_home: home.join(".cache"),
        state_home: Some(home.join(".local/state")),
        runtime_home: None,
        config_override: None,
    };
    let paths = Paths::from_base(&base, Os::MacOs);
    let go = read_fixture("wire/state_after_replay.json");
    write(&paths.legacy_state_file, &go);

    let loaded = State::load_from_paths(&paths).unwrap();
    assert_eq!(loaded.origin, StateOrigin::Migrated);
    assert_eq!(paths.state_file, home.join(".config/kakemono/state.json"));
    assert_eq!(std::fs::read(&paths.state_file).unwrap(), go);
}
