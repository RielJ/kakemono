//! Wallpaper history and the persisted `state.json` (port of Go `internal/wallpaper/state.go`).
//!
//! The file format is byte-compatible with the Go daemon:
//!
//! - Keys in order `current` (nullable), `history` (`null` when empty), `history_index` (`-1`
//!   when empty), `paused`, `last_changed`.
//! - Written like Go's `json.MarshalIndent(s, "", "  ")`: two-space indent, no trailing
//!   newline, and `<`, `>`, `&`, U+2028, U+2029 escaped as `\u003c` and so on.
//! - `last_changed` is a Go `time.Time`: RFC 3339 with up to nine fractional digits (trailing
//!   zeros trimmed), `Z` for a zero offset, and `"0001-01-01T00:00:00Z"` for "never".
//!
//! - `album_cursors` (kakemono only, after `last_changed`) maps an album source id to the last
//!   album image shown. It is written only when non-empty, so a state without cursors keeps the
//!   Go bytes, and a file without the key (Go's, or an older kakemono's) loads with no cursors.
//!
//! Unlike Go, [`State::save`] is atomic (temp file in the same directory, then rename), and
//! [`State::load_or_migrate`] copies the Go app's `~/.config/wallpaperd/state.json` once into
//! the new location without touching the old file.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use kakemono_config::paths::Paths;
use kakemono_protocol::types::WallpaperInfo;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Maximum number of history entries kept. Older entries are dropped first.
pub const MAX_HISTORY: usize = 100;

/// Go's zero `time.Time` as `encoding/json` writes it.
const GO_ZERO_TIME: &str = "0001-01-01T00:00:00Z";

/// Errors from loading or saving `state.json`.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("state: read {}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("state: parse {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("state: marshal state: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("state: create dir for {}: {source}", path.display())]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("state: write {}: {source}", path.display())]
    Write { path: PathBuf, source: io::Error },
}

/// Where [`State::load_or_migrate`] got its state from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateOrigin {
    /// The kakemono state file existed and was read.
    Existing,
    /// The kakemono file was missing; the Go `wallpaperd` file was read and written to the new
    /// path. The old file is left untouched.
    Migrated,
    /// Neither file existed (or the legacy file was unreadable): a fresh, empty state.
    Fresh,
}

/// Result of [`State::load_or_migrate`].
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedState {
    pub state: State,
    pub origin: StateOrigin,
}

/// Persisted runtime state: the current wallpaper, the history ring (cap [`MAX_HISTORY`]), the
/// position in it, the pause flag and the time of the last `push`.
///
/// Invariants (enforced on load and by every method): `history_index` is `None` iff `history`
/// is empty, and otherwise is a valid index. `current` is stored separately, as in Go, and
/// is set to the entry at `history_index` by [`push`](Self::push), [`prev`](Self::prev) and
/// [`next`](Self::next).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    current: Option<WallpaperInfo>,
    history: Vec<WallpaperInfo>,
    history_index: Option<usize>,
    paused: bool,
    last_changed: Option<DateTime<FixedOffset>>,
    album_cursors: BTreeMap<String, PathBuf>,
}

impl State {
    /// An empty state: no current wallpaper, empty history, index `-1`, not paused.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads `path`. A missing file yields [`State::new`] (Go `LoadState`).
    ///
    /// # Errors
    ///
    /// [`StateError`] when the file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        match std::fs::read(path) {
            Ok(bytes) => Self::from_json_slice(&bytes).map_err(|source| StateError::Parse {
                path: path.to_owned(),
                source,
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::new()),
            Err(source) => Err(StateError::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// Loads `state_file`. If it does not exist and `legacy_state_file` (the Go app's file)
    /// does, reads that once and saves it to `state_file`; the legacy file is never modified.
    ///
    /// An unreadable or corrupt legacy file is logged and ignored ([`StateOrigin::Fresh`]):
    /// the user cannot fix it through kakemono. An unreadable or corrupt `state_file`, or a
    /// failure to write the migrated copy, is an error.
    ///
    /// # Errors
    ///
    /// [`StateError`] for an unreadable or corrupt `state_file`, or a failed migration write.
    pub fn load_or_migrate(
        state_file: &Path,
        legacy_state_file: &Path,
    ) -> Result<LoadedState, StateError> {
        let exists = |p: &Path| {
            p.try_exists().map_err(|source| StateError::Read {
                path: p.to_owned(),
                source,
            })
        };
        if exists(state_file)? || state_file == legacy_state_file {
            return Ok(LoadedState {
                state: Self::load(state_file)?,
                origin: StateOrigin::Existing,
            });
        }
        if !legacy_state_file.exists() {
            return Ok(LoadedState {
                state: Self::new(),
                origin: StateOrigin::Fresh,
            });
        }
        let state = match Self::load(legacy_state_file) {
            Ok(state) => state,
            Err(e) => {
                tracing::warn!(error = %e, "ignoring unreadable legacy state file");
                return Ok(LoadedState {
                    state: Self::new(),
                    origin: StateOrigin::Fresh,
                });
            }
        };
        state.save(state_file)?;
        tracing::info!(
            from = %legacy_state_file.display(),
            to = %state_file.display(),
            "migrated wallpaperd state"
        );
        Ok(LoadedState {
            state,
            origin: StateOrigin::Migrated,
        })
    }

    /// [`load_or_migrate`](Self::load_or_migrate) with `paths.state_file` and
    /// `paths.legacy_state_file`.
    ///
    /// # Errors
    ///
    /// Those of [`load_or_migrate`](Self::load_or_migrate).
    pub fn load_from_paths(paths: &Paths) -> Result<LoadedState, StateError> {
        Self::load_or_migrate(&paths.state_file, &paths.legacy_state_file)
    }

    /// Writes the state to `path` atomically (temp file in the same directory, then rename),
    /// creating parent directories. The bytes are those of Go's `json.MarshalIndent`.
    ///
    /// # Errors
    ///
    /// [`StateError`] when the folder cannot be created or the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let bytes = self.to_json_bytes().map_err(StateError::Serialize)?;
        write_atomic(path, &bytes)
    }

    /// Parses `state.json` bytes as Go's `json.Unmarshal` would (missing keys default, unknown
    /// keys ignored), then repairs an out-of-range `history_index` (see [`State`]).
    ///
    /// # Errors
    ///
    /// The bytes are not valid JSON or don't match the state's shape.
    pub fn from_json_slice(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    /// Serialises like Go's `json.MarshalIndent(s, "", "  ")`.
    ///
    /// # Errors
    ///
    /// Serialisation failed (a time outside Go's range).
    pub fn to_json_bytes(&self) -> serde_json::Result<Vec<u8>> {
        kakemono_protocol::codec::to_vec_go_pretty(self)
    }

    /// Appends `info` and makes it current, stamping `last_changed` with `now`.
    ///
    /// Forward history (left after [`prev`](Self::prev)) is discarded first, like a browser.
    /// The oldest entries are dropped beyond [`MAX_HISTORY`].
    pub fn push<Tz: TimeZone>(&mut self, info: WallpaperInfo, now: DateTime<Tz>) {
        if let Some(i) = self.history_index {
            self.history.truncate(i + 1);
        }
        self.history.push(info);
        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(..excess);
        }
        let last = self.history.len() - 1;
        self.history_index = Some(last);
        self.current = self.history.last().cloned();
        self.last_changed = Some(now.fixed_offset());
    }

    /// Steps one entry back and makes it current. `None` (and no change) at the beginning.
    pub fn prev(&mut self) -> Option<&WallpaperInfo> {
        if !self.can_go_prev() {
            return None;
        }
        let i = self.history_index? - 1;
        self.move_to(i)
    }

    /// Steps one entry forward within existing history and makes it current. `None` (and no
    /// change) at the end; fetching a new wallpaper is the engine's job.
    #[allow(
        clippy::should_implement_trait,
        reason = "history navigation named after Go's State.Next, not an iterator"
    )]
    pub fn next(&mut self) -> Option<&WallpaperInfo> {
        if !self.has_future_history() {
            return None;
        }
        let i = self.history_index.map_or(0, |i| i + 1);
        self.move_to(i)
    }

    /// Whether an older history entry exists.
    #[must_use]
    pub fn can_go_prev(&self) -> bool {
        self.history_index.is_some_and(|i| i > 0)
    }

    /// Whether a newer entry exists that the user stepped back from.
    #[must_use]
    pub fn has_future_history(&self) -> bool {
        match self.history_index {
            Some(i) => i + 1 < self.history.len(),
            None => !self.history.is_empty(),
        }
    }

    /// The current wallpaper, if any.
    #[must_use]
    pub fn current(&self) -> Option<&WallpaperInfo> {
        self.current.as_ref()
    }

    /// History, oldest first.
    #[must_use]
    pub fn history(&self) -> &[WallpaperInfo] {
        &self.history
    }

    /// Position in [`history`](Self::history); `None` when history is empty.
    #[must_use]
    pub fn history_position(&self) -> Option<usize> {
        self.history_index
    }

    /// The position as Go and `StatusData.history_index` report it: `-1` when empty.
    #[must_use]
    pub fn history_index(&self) -> i32 {
        self.history_index
            .map_or(-1, |i| i32::try_from(i).unwrap_or(i32::MAX))
    }

    /// `history().len()` as `StatusData.history_count`.
    #[must_use]
    pub fn history_count(&self) -> i32 {
        i32::try_from(self.history.len()).unwrap_or(i32::MAX)
    }

    /// Whether auto-rotation is paused.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Sets the pause flag. Callers persist with [`save`](Self::save).
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Time of the last [`push`](Self::push); `None` if never (Go's zero time).
    #[must_use]
    pub fn last_changed(&self) -> Option<DateTime<FixedOffset>> {
        self.last_changed
    }

    /// The last image shown from the album source `id`, if any.
    pub fn album_cursor(&self, id: &str) -> Option<&Path> {
        self.album_cursors.get(id).map(PathBuf::as_path)
    }

    /// Records `path` as the last image shown from the album source `id`. Callers persist with
    /// [`save`](Self::save).
    pub fn set_album_cursor(&mut self, id: impl Into<String>, path: impl Into<PathBuf>) {
        self.album_cursors.insert(id.into(), path.into());
    }

    /// Every album cursor, by source id.
    #[must_use]
    pub fn album_cursors(&self) -> &BTreeMap<String, PathBuf> {
        &self.album_cursors
    }

    fn move_to(&mut self, i: usize) -> Option<&WallpaperInfo> {
        let item = self.history.get(i)?.clone();
        self.history_index = Some(i);
        self.current = Some(item);
        self.current.as_ref()
    }
}

// ---------------------------------------------------------------------------------------------
// Serde: the on-disk shape.

#[derive(Serialize)]
struct ReprRef<'a> {
    current: Option<&'a WallpaperInfo>,
    #[serde(serialize_with = "ser_history")]
    history: &'a [WallpaperInfo],
    history_index: i64,
    paused: bool,
    #[serde(serialize_with = "ser_go_time")]
    last_changed: Option<DateTime<FixedOffset>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    album_cursors: &'a BTreeMap<String, PathBuf>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Repr {
    current: Option<WallpaperInfo>,
    #[serde(deserialize_with = "de_history")]
    history: Vec<WallpaperInfo>,
    /// Go `int`; a missing key is `0`, as in Go.
    history_index: i64,
    paused: bool,
    #[serde(deserialize_with = "de_go_time")]
    last_changed: Option<DateTime<FixedOffset>>,
    /// Missing in Go's file and in older kakemono files.
    album_cursors: BTreeMap<String, PathBuf>,
}

impl Serialize for State {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ReprRef {
            current: self.current.as_ref(),
            history: &self.history,
            history_index: self
                .history_index
                .map_or(-1, |i| i64::try_from(i).unwrap_or(i64::MAX)),
            paused: self.paused,
            last_changed: self.last_changed,
            album_cursors: &self.album_cursors,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_repr(Repr::deserialize(deserializer)?))
    }
}

impl State {
    /// Go never validated `history_index`; a bad value would panic or misbehave there. Here an
    /// empty history means index `None`, and an out-of-range index points at the newest entry.
    fn from_repr(r: Repr) -> Self {
        let len = r.history.len();
        let mut current = r.current;
        let history_index = if len == 0 {
            None
        } else {
            match usize::try_from(r.history_index) {
                Ok(i) if i < len => Some(i),
                _ => {
                    tracing::warn!(
                        history_index = r.history_index,
                        history_len = len,
                        "state.json history_index out of range; using the newest entry"
                    );
                    current = r.history.last().cloned();
                    Some(len - 1)
                }
            }
        };
        Self {
            current,
            history: r.history,
            history_index,
            paused: r.paused,
            last_changed: r.last_changed,
            album_cursors: r.album_cursors,
        }
    }
}

/// A Go nil slice marshals as `null`; a fresh Go state has a nil history.
fn ser_history<S: Serializer>(h: &&[WallpaperInfo], s: S) -> Result<S::Ok, S::Error> {
    if h.is_empty() {
        s.serialize_none()
    } else {
        h.serialize(s)
    }
}

fn de_history<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<WallpaperInfo>, D::Error> {
    Ok(Option::<Vec<WallpaperInfo>>::deserialize(d)?.unwrap_or_default())
}

fn ser_go_time<S: Serializer>(t: &Option<DateTime<FixedOffset>>, s: S) -> Result<S::Ok, S::Error> {
    match t {
        None => s.serialize_str(GO_ZERO_TIME),
        Some(t) => s.serialize_str(&format_go_time(t).map_err(serde::ser::Error::custom)?),
    }
}

fn de_go_time<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<FixedOffset>>, D::Error> {
    // Go: `null` leaves the zero time; anything else must be an RFC 3339 string.
    let Some(s) = Option::<String>::deserialize(d)? else {
        return Ok(None);
    };
    let t = DateTime::parse_from_rfc3339(&s)
        .map_err(|e| serde::de::Error::custom(format!("last_changed {s:?}: {e}")))?;
    // Go's `IsZero` compares the instant, whatever the offset.
    Ok((Some(t.naive_utc()) != go_zero_naive()).then_some(t))
}

/// `0001-01-01T00:00:00` UTC, Go's zero `time.Time`.
fn go_zero_naive() -> Option<NaiveDateTime> {
    NaiveDate::from_ymd_opt(1, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0))
}

/// Formats like Go's `time.Time.MarshalJSON` (RFC 3339 with nanoseconds, trailing zeros
/// trimmed, `Z` for a zero offset). Errors for years outside `0..=9999`, as Go does.
///
/// # Errors
///
/// A year outside `0..=9999`.
pub fn format_go_time<Tz: TimeZone>(t: &DateTime<Tz>) -> Result<String, String> {
    let t = t.fixed_offset();
    let year = chrono::Datelike::year(&t);
    if !(0..=9999).contains(&year) {
        return Err(format!("year {year} outside of range [0,9999]"));
    }
    let mut out = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    // chrono represents a leap second as nanosecond >= 1e9; Go has no leap seconds.
    let nanos = t.nanosecond().min(999_999_999);
    if nanos > 0 {
        let frac = format!("{nanos:09}");
        out.push('.');
        out.push_str(frac.trim_end_matches('0'));
    }
    let offset = t.offset().local_minus_utc();
    if offset == 0 {
        out.push('Z');
    } else {
        let sign = if offset < 0 { '-' } else { '+' };
        let mins = offset.unsigned_abs() / 60;
        out.push_str(&format!("{sign}{:02}:{:02}", mins / 60, mins % 60));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Atomic write.

/// Creates the parent folder, then writes with [`kakemono_fs::atomic_write`] (unique temp file,
/// fsync, rename).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|source| StateError::CreateDir {
        path: dir.to_owned(),
        source,
    })?;
    kakemono_fs::atomic_write(path, bytes).map_err(|source| StateError::Write {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `state.json` written before album cursors existed (the Go daemon's shape).
    const OLD_STATE: &str = r#"{
  "current": {"path": "/w/a.jpg", "source": "local", "source_url": "", "image_url": "", "title": "", "author": "", "width": 0, "height": 0, "set_at": ""},
  "history": [{"path": "/w/a.jpg", "source": "local", "source_url": "", "image_url": "", "title": "", "author": "", "width": 0, "height": 0, "set_at": ""}],
  "history_index": 0,
  "paused": true,
  "last_changed": "0001-01-01T00:00:00Z"
}"#;

    #[test]
    fn state_album_cursors_round_trip() {
        let mut s = State::new();
        assert_eq!(s.album_cursor("aln-1"), None);
        s.set_album_cursor("aln-1", "/albums/one/02.jpg");
        s.set_album_cursor("ald-2", "/albums/two/b.png");
        s.set_album_cursor("aln-1", "/albums/one/03.jpg");
        assert_eq!(
            s.album_cursor("aln-1"),
            Some(Path::new("/albums/one/03.jpg"))
        );

        let bytes = s.to_json_bytes().unwrap();
        let back = State::from_json_slice(&bytes).unwrap();
        assert_eq!(back, s);
        assert_eq!(
            back.album_cursor("ald-2"),
            Some(Path::new("/albums/two/b.png"))
        );
        assert_eq!(back.album_cursors().len(), 2);
    }

    #[test]
    fn state_without_album_cursors_still_loads() {
        let s = State::from_json_slice(OLD_STATE.as_bytes()).unwrap();
        assert!(s.paused());
        assert_eq!(s.current().unwrap().path, "/w/a.jpg");
        assert!(s.album_cursors().is_empty());
    }

    #[test]
    fn state_without_album_cursors_keeps_the_go_shape() {
        // No cursors: the bytes stay byte-compatible with the Go daemon (no extra key).
        let json = String::from_utf8(State::new().to_json_bytes().unwrap()).unwrap();
        assert!(!json.contains("album_cursors"), "{json}");
        assert!(
            json.ends_with("\"last_changed\": \"0001-01-01T00:00:00Z\"\n}"),
            "{json}"
        );
    }
}
