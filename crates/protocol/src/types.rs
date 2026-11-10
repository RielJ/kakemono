//! IPC wire types (plan §I). Serialisation matches the Go daemon byte for byte:
//! `Response.error`/`data` and `Command.args` are omitted when empty (Go `omitempty`),
//! every `WallpaperInfo` field is always written, and `StatusData.current` is written as
//! `null` when unset.
//!
//! New fields stay readable both ways: `WallpaperInfo.source_id` defaults to `""` when missing
//! and is not written when empty, so Go fixtures, old `state.json` files and old clients are
//! unaffected.
//!
//! # Source requests (`docs/plans/variety-sources-impl.md` Task 16)
//!
//! Errors use the usual `{"ok":false,"error":"<text>"}` line. A malformed request gets the
//! [`RequestError`] text; an engine failure gets the engine's text.
//!
//! **`check_source`**: validate a source row before adding it. Never writes a file.
//!
//! ```text
//! → {"cmd":"check_source","args":{"type":"wallhaven","location":"forest"}}
//! ← {"ok":true,"data":{"count":24}}                       // CheckSourceData
//! ← {"ok":false,"error":"no images found"}
//! ← {"ok":false,"error":"Internet access is off"}          // online type, [daemon] internet off
//! ← {"ok":false,"error":"check timed out after 15s"}
//! ← {"ok":false,"error":"<provider error text>"}          // e.g. "wallhaven: HTTP 401"
//! ← {"ok":false,"error":"check_source: missing required argument 'type'"}
//! ← {"ok":false,"error":"check_source: unknown source type \"nope\""}   // daemon-side
//! ← {"ok":false,"error":"unsupported source type"}         // engine, unreachable via the daemon
//! ```
//!
//! `type` is a `[[sources]] type` string (`SourceType` in `kakemono-config`); `location` is
//! optional (`""` for `bing`/`apod`). Online types run one provider query (15 s timeout) and
//! count its results. Local types count the images at `location` (`folder` recursively,
//! `album-*` the folder itself, `images` 1 when the file is an image).
//!
//! **`fetch`**: save an image (dropped URL or file) into `[daemon] manual_dir` and set it.
//!
//! ```text
//! → {"cmd":"fetch","args":{"url":"https://example.com/a.jpg"}}
//! → {"cmd":"fetch","args":{"url":"/Users/me/Pictures/a.png"}}   // or file:///...
//! ← {"ok":true,"data":{"path":"<manual_dir>/a.jpg","source":"manual","source_id":"manual",
//!    "source_url":"","image_url":"https://example.com/a.jpg","title":"a.jpg","author":"",
//!    "width":0,"height":0,"set_at":"2026-03-04T05:06:07+08:00"}}   // WallpaperInfo
//! ← {"ok":false,"error":"Internet access is off"}          // URL with [daemon] internet off
//! ← {"ok":false,"error":"not a JPEG, PNG or WebP image"}
//! ← {"ok":false,"error":"image is larger than 64 MB"}
//! ← {"ok":false,"error":"not an http(s) URL or absolute file path: \"a.jpg\""}
//! ← {"ok":false,"error":"download https://x/a.jpg: HTTP 404"}   // secrets in the URL redacted
//! ← {"ok":false,"error":"fetch: missing required argument 'url'"}
//! ```
//!
//! The file is named after the URL's last path segment or the file's name, with an extension
//! matching the detected type, made unique with `-2`, `-3`, ... A file already directly in
//! `manual_dir` is set in place. `image_url` is the URL, `""` for a local file. Local files
//! work with Internet access off.
//!
//! **`download_now`**: download one image now, ignoring the downloader's backoff.
//!
//! ```text
//! → {"cmd":"download_now","args":{"id":"wh-1"}}   // that enabled online source
//! → {"cmd":"download_now"}                       // every enabled online source with no image
//! ← {"ok":true,"data":{"path":"<download_dir>/wh-1/x.jpg"}}   // DownloadNowData
//! ← {"ok":true,"data":{"path":null}}           // no id, and no source needed an image
//! ← {"ok":false,"error":"no enabled online source \"wh-1\""}
//! ← {"ok":false,"error":"Internet access is off"}
//! ← {"ok":false,"error":"provider wallhaven fetch: ..."}   // failures joined with "; "
//! ← {"ok":false,"error":"download_now: argument 'id' must be a string"}
//! ```
//!
//! Without `id`, every enabled online source whose download folder has no image downloads one,
//! in config order; `path` is the first success, and the call fails only when all failed. The
//! image is not set.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use ts_rs::TS;

/// Raw request line as sent on the wire: `{"cmd":"...","args":{...}}`.
///
/// Parsing is as lenient as Go's `json.Unmarshal` into `ipc.Command`: unknown keys are
/// ignored, a missing or `null` `cmd` becomes `""`, and a missing or `null` `args` becomes
/// an empty map. Convert to [`Request`] with `Request::try_from(cmd)`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
pub struct Command {
    pub cmd: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    #[ts(optional, as = "Option<std::collections::HashMap<String, Value>>")]
    pub args: Map<String, Value>,
}

impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Go's json.Unmarshal leaves a field untouched on `null`; mirror that as the default.
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            cmd: Option<String>,
            #[serde(default)]
            args: Option<Map<String, Value>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            cmd: raw.cmd.unwrap_or_default(),
            args: raw.args.unwrap_or_default(),
        })
    }
}

impl Command {
    /// A command with no arguments.
    pub fn new(cmd: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            args: Map::new(),
        }
    }
}

/// Typed request, the only form the daemon dispatches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Next,
    Prev,
    Set {
        path: String,
    },
    Favorite,
    /// Trash the current wallpaper and advance. New, optional: with `expected`, the daemon
    /// trashes only if the current path still equals it (checked under its op lock), else it
    /// answers `ok:false` and trashes nothing. Wire: `args.expected`, omitted when `None`.
    Trash {
        expected: Option<String>,
    },
    Pause,
    Resume,
    Status,
    /// New: re-read `config.toml` without restarting.
    Reload,
    /// New: the server keeps the connection open and streams [`Event`] lines.
    Subscribe,
    /// New: daemon version string.
    Version,
    /// New: diagnostics report. With `apply`, the daemon first re-sets the current wallpaper
    /// through its own setter and reports the outcome. Wire: `args.apply`, written only when
    /// `true`, so the plain form stays `{"cmd":"doctor"}`.
    Doctor {
        apply: bool,
    },
    /// New: count the images a source row would give, without saving anything. Wire:
    /// `args.type` (required, non-empty) and `args.location` (optional, `""` when missing).
    /// Answer: [`CheckSourceData`]. See the module docs.
    CheckSource {
        kind: String,
        location: String,
    },
    /// New: save the image at `url` (http(s) URL, `file://` URL or absolute path) into the
    /// manual folder and set it. Wire: `args.url` (required, non-empty). Answer:
    /// [`WallpaperInfo`], as `set` returns.
    Fetch {
        url: String,
    },
    /// New: download one image now for online source `id`, or with no `id` for every enabled
    /// online source with no image yet. Wire: `args.id`, omitted when `None`; `null` or `""`
    /// read as `None`. Answer: [`DownloadNowData`].
    DownloadNow {
        id: Option<String>,
    },
}

impl Request {
    /// The wire `cmd` string.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Set { .. } => "set",
            Self::Favorite => "favorite",
            Self::Trash { .. } => "trash",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Status => "status",
            Self::Reload => "reload",
            Self::Subscribe => "subscribe",
            Self::Version => "version",
            Self::Doctor { .. } => "doctor",
            Self::CheckSource { .. } => "check_source",
            Self::Fetch { .. } => "fetch",
            Self::DownloadNow { .. } => "download_now",
        }
    }
}

/// Why a [`Command`] is not a valid [`Request`]. `Display` gives Go's exact error text,
/// which is what goes into `Response.error`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    /// `unknown command: "<name>"` (Go `%q` quoting).
    #[error("{}", unknown_command_error(.0))]
    UnknownCommand(String),
    /// `set` with no `path`, a non-string `path`, or `""`.
    #[error("set: missing required argument 'path'")]
    MissingPath,
    /// `trash` with an `expected` that is neither a string nor `null`.
    #[error("trash: argument 'expected' must be a string")]
    InvalidExpected,
    /// `doctor` with an `apply` that is neither a boolean nor `null`.
    #[error("doctor: argument 'apply' must be a boolean")]
    InvalidApply,
    /// `check_source` with no `type`, a non-string `type`, or `""`.
    #[error("check_source: missing required argument 'type'")]
    MissingType,
    /// `check_source` with a `location` that is neither a string nor `null`.
    #[error("check_source: argument 'location' must be a string")]
    InvalidLocation,
    /// `fetch` with no `url`, a non-string `url`, or `""`.
    #[error("fetch: missing required argument 'url'")]
    MissingUrl,
    /// `download_now` with an `id` that is neither a string nor `null`.
    #[error("download_now: argument 'id' must be a string")]
    InvalidId,
}

impl TryFrom<Command> for Request {
    type Error = RequestError;

    fn try_from(cmd: Command) -> Result<Self, Self::Error> {
        Ok(match cmd.cmd.as_str() {
            "next" => Self::Next,
            "prev" => Self::Prev,
            "set" => match cmd.args.get("path") {
                Some(Value::String(p)) if !p.is_empty() => Self::Set { path: p.clone() },
                _ => return Err(RequestError::MissingPath),
            },
            "favorite" => Self::Favorite,
            // Missing, `null` or `""` = no check, so Go clients and fixtures parse unchanged.
            "trash" => match cmd.args.get("expected") {
                None | Some(Value::Null) => Self::Trash { expected: None },
                Some(Value::String(p)) => Self::Trash {
                    expected: (!p.is_empty()).then(|| p.clone()),
                },
                Some(_) => return Err(RequestError::InvalidExpected),
            },
            "pause" => Self::Pause,
            "resume" => Self::Resume,
            "status" => Self::Status,
            "reload" => Self::Reload,
            "subscribe" => Self::Subscribe,
            "version" => Self::Version,
            // Missing, `null` or `false` = report only.
            "doctor" => match cmd.args.get("apply") {
                None | Some(Value::Null) => Self::Doctor { apply: false },
                Some(Value::Bool(apply)) => Self::Doctor { apply: *apply },
                Some(_) => return Err(RequestError::InvalidApply),
            },
            "check_source" => {
                let kind = match cmd.args.get("type") {
                    Some(Value::String(t)) if !t.is_empty() => t.clone(),
                    _ => return Err(RequestError::MissingType),
                };
                let location = match cmd.args.get("location") {
                    None | Some(Value::Null) => String::new(),
                    Some(Value::String(l)) => l.clone(),
                    Some(_) => return Err(RequestError::InvalidLocation),
                };
                Self::CheckSource { kind, location }
            }
            "fetch" => match cmd.args.get("url") {
                Some(Value::String(u)) if !u.is_empty() => Self::Fetch { url: u.clone() },
                _ => return Err(RequestError::MissingUrl),
            },
            "download_now" => match cmd.args.get("id") {
                None | Some(Value::Null) => Self::DownloadNow { id: None },
                Some(Value::String(id)) => Self::DownloadNow {
                    id: (!id.is_empty()).then(|| id.clone()),
                },
                Some(_) => return Err(RequestError::InvalidId),
            },
            _ => return Err(RequestError::UnknownCommand(cmd.cmd)),
        })
    }
}

impl From<Request> for Command {
    fn from(req: Request) -> Self {
        let mut out = Command::new(req.name());
        match req {
            Request::Set { path } => {
                out.args.insert("path".into(), Value::String(path));
            }
            Request::Trash {
                expected: Some(path),
            } => {
                out.args.insert("expected".into(), Value::String(path));
            }
            Request::Doctor { apply: true } => {
                out.args.insert("apply".into(), Value::Bool(true));
            }
            Request::CheckSource { kind, location } => {
                out.args.insert("type".into(), Value::String(kind));
                out.args.insert("location".into(), Value::String(location));
            }
            Request::Fetch { url } => {
                out.args.insert("url".into(), Value::String(url));
            }
            Request::DownloadNow { id: Some(id) } => {
                out.args.insert("id".into(), Value::String(id));
            }
            _ => {}
        }
        out
    }
}

/// Response line: `{"ok":bool,"error"?:string,"data"?:any}`.
#[derive(Debug, Default, Serialize, Deserialize, TS)]
pub struct Response {
    #[serde(default)]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[ts(optional, as = "Option<String>")]
    pub error: String,
    /// Payload kept as raw JSON; decode with [`Response::data_as`].
    /// `null` on the wire reads back as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub data: Option<Box<RawValue>>,
}

impl Clone for Response {
    fn clone(&self) -> Self {
        Self {
            ok: self.ok,
            error: self.error.clone(),
            data: self.data.clone(),
        }
    }
}

impl PartialEq for Response {
    fn eq(&self, other: &Self) -> bool {
        self.ok == other.ok
            && self.error == other.error
            && self.data.as_ref().map(|d| d.get()) == other.data.as_ref().map(|d| d.get())
    }
}

impl Response {
    /// `{"ok":true}`.
    pub fn ok() -> Self {
        Self {
            ok: true,
            ..Self::default()
        }
    }

    /// `{"ok":true,"data":<value>}`. The payload is encoded like Go's `json.Marshal`
    /// ([`crate::codec::to_string_go`]), so the whole line matches Go byte for byte.
    pub fn ok_data<T: Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        let raw = RawValue::from_string(crate::codec::to_string_go(value)?)?;
        Ok(Self {
            ok: true,
            error: String::new(),
            data: Some(raw),
        })
    }

    /// `{"ok":false,"error":"<message>"}`.
    pub fn err(message: impl fmt::Display) -> Self {
        Self {
            ok: false,
            error: message.to_string(),
            data: None,
        }
    }

    /// Decodes `data` into `T`. `Ok(None)` when there is no payload.
    pub fn data_as<T: serde::de::DeserializeOwned>(&self) -> Result<Option<T>, serde_json::Error> {
        self.data
            .as_ref()
            .map(|raw| serde_json::from_str(raw.get()))
            .transpose()
    }
}

/// Wallpaper metadata. Every field is always serialised (Go has no `omitempty` here) and
/// every field defaults when missing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct WallpaperInfo {
    pub path: String,
    /// The source row's type (`"wallhaven"`, `"folder"`, `"album-name"`, ...), `"manual"` for
    /// the manual folder or `"local"` for `set`. Go wrote the provider name here.
    pub source: String,
    /// New: the `[[sources]]` row id the image came from, `"manual"` for the manual folder
    /// (including `fetch`), `""` for `set` and for entries written by older versions. Not
    /// written when empty, so the Go wire bytes are unchanged.
    #[serde(skip_serializing_if = "String::is_empty")]
    #[ts(optional, as = "Option<String>")]
    pub source_id: String,
    pub source_url: String,
    pub image_url: String,
    pub title: String,
    pub author: String,
    pub width: i32,
    pub height: i32,
    /// RFC 3339 with whole seconds in the daemon's local offset
    /// (Go `time.Now().Format(time.RFC3339)`), or `""` when unknown.
    pub set_at: String,
}

/// Payload of the `check_source` response: `{"count":n}`, the number of images found (always
/// at least 1; zero is the error `no images found`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CheckSourceData {
    pub count: u32,
}

/// Payload of the `download_now` response: `{"path":"..."}`, the downloaded file, or
/// `{"path":null}` when no `id` was given and no source needed an image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DownloadNowData {
    pub path: Option<String>,
}

/// Payload of the `status` response. `current` is serialised as `null` when unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct StatusData {
    pub current: Option<WallpaperInfo>,
    pub paused: bool,
    /// Go duration string as configured, e.g. `"30m"` or `"1h30m"`.
    pub interval: String,
    /// `"light"` or `"dark"`.
    pub theme: String,
    pub history_count: i32,
    /// `-1` when history is empty.
    pub history_index: i32,
}

impl Default for StatusData {
    fn default() -> Self {
        Self {
            current: None,
            paused: false,
            interval: String::new(),
            theme: String::new(),
            history_count: 0,
            history_index: -1,
        }
    }
}

/// Line streamed on a `subscribe` connection: `{"event":"<name>","data":...}`.
///
/// Unknown event names deserialise to [`Event::Unknown`] so old clients survive new
/// daemons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    /// The wallpaper changed: `{"event":"changed","data":WallpaperInfo}`.
    Changed(WallpaperInfo),
    /// Rotation paused (`true`) or resumed (`false`).
    Paused(bool),
    /// System theme changed: `"light"` or `"dark"`.
    ThemeChanged(String),
    /// An event this version does not know. Never sent.
    #[serde(skip_serializing)]
    #[ts(skip)]
    Unknown,
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            event: String,
            #[serde(default)]
            data: Value,
        }
        fn data<T: serde::de::DeserializeOwned, E: serde::de::Error>(v: Value) -> Result<T, E> {
            serde_json::from_value(v).map_err(E::custom)
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(match raw.event.as_str() {
            "changed" => Self::Changed(data(raw.data)?),
            "paused" => Self::Paused(data(raw.data)?),
            "theme_changed" => Self::ThemeChanged(data(raw.data)?),
            _ => Self::Unknown,
        })
    }
}

/// Go's dispatcher error for an unknown `cmd`: `unknown command: "x"`, with the name
/// quoted as Go's `%q` (`strconv.Quote`) does.
pub fn unknown_command_error(name: &str) -> String {
    format!("unknown command: {}", go_quote(name))
}

/// Reimplements Go's `strconv.Quote` for valid UTF-8.
///
/// Go keeps a rune as-is when `unicode.IsPrint` is true. Without Unicode tables this
/// treats as non-printable: control characters, whitespace other than U+0020, the
/// common format (Cf) characters, private-use code points and U+FFFE/U+FFFF. Unassigned
/// code points are kept as-is, where Go would escape them.
fn go_quote(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if go_is_print(c) => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c if (c as u32) < 0x10000 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => {
                let _ = write!(out, "\\U{:08x}", c as u32);
            }
        }
    }
    out.push('"');
    out
}

fn go_is_print(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    let u = c as u32;
    let format_or_private = matches!(
        u,
        0xAD | 0x600..=0x605
            | 0x61C
            | 0x6DD
            | 0x70F
            | 0x890..=0x891
            | 0x8E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xE000..=0xF8FF
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0xFFFE..=0xFFFF
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
            | 0xF0000..
    );
    !format_or_private
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_null_data_reads_as_none() {
        let r: Response = serde_json::from_str(r#"{"ok":true,"data":null}"#).expect("parse");
        assert!(r.data.is_none());
        assert_eq!(serde_json::to_string(&r).expect("ser"), r#"{"ok":true}"#);
    }

    #[test]
    fn command_null_cmd_is_empty() {
        let c: Command = serde_json::from_str(r#"{"cmd":null}"#).expect("parse");
        assert_eq!(c.cmd, "");
    }

    #[test]
    fn command_non_string_cmd_is_invalid_json() {
        assert!(serde_json::from_str::<Command>(r#"{"cmd":1}"#).is_err());
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"x","args":[]}"#).is_err());
    }

    #[test]
    fn event_wire_names() {
        let s = serde_json::to_string(&Event::Paused(true)).expect("ser");
        assert_eq!(s, r#"{"event":"paused","data":true}"#);
        let s = serde_json::to_string(&Event::ThemeChanged("dark".into())).expect("ser");
        assert_eq!(s, r#"{"event":"theme_changed","data":"dark"}"#);
        let back: Event = serde_json::from_str(&s).expect("parse");
        assert_eq!(back, Event::ThemeChanged("dark".into()));
    }
}
