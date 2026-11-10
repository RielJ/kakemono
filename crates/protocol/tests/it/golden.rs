//! Golden compatibility tests against the Go fixtures (plan §I, §J, Task 3).
//!
//! Every test lives in this module, so `cargo nextest run -p kakemono-protocol -E 'test(golden::)'`
//! selects them all.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use kakemono_protocol::types::{
    Command, Event, Request, RequestError, Response, StatusData, WallpaperInfo,
    unknown_command_error,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(name: &str) -> String {
    let path = fixture_dir().join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Asserts both values have the same keys at every object level, then full equality.
fn assert_same_shape(context: &str, expected: &Value, actual: &Value) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            let ek: BTreeSet<_> = e.keys().collect();
            let ak: BTreeSet<_> = a.keys().collect();
            assert_eq!(ek, ak, "{context}: key sets differ");
            for (k, ev) in e {
                assert_same_shape(&format!("{context}.{k}"), ev, &a[k]);
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            assert_eq!(e.len(), a.len(), "{context}: array lengths differ");
            for (i, (ev, av)) in e.iter().zip(a).enumerate() {
                assert_same_shape(&format!("{context}[{i}]"), ev, av);
            }
        }
        _ => assert_eq!(expected, actual, "{context}: values differ"),
    }
}

/// Go JSON -> Rust type -> JSON must give back the same document.
fn round_trip<T: DeserializeOwned + Serialize>(context: &str, json: &str) -> T {
    let expected: Value = serde_json::from_str(json).expect("fixture is valid JSON");
    let typed: T =
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{context}: deserialize failed: {e}"));
    let actual = serde_json::to_value(&typed).expect("serialize");
    assert_same_shape(context, &expected, &actual);
    typed
}

fn round_trip_file<T: DeserializeOwned + Serialize>(name: &str) -> T {
    round_trip(name, &read(name))
}

// ---- marshalled Go values (tools/goldens/main.go, deleted at the cutover) ----

#[test]
fn golden_wallpaper_info_full() {
    let info: WallpaperInfo = round_trip_file("wallpaper_info_full.json");
    assert_eq!(info.source, "bing");
    assert_eq!(info.title, "Sunset <Golden Hour> & Co — café");
    assert_eq!((info.width, info.height), (3840, 2160));
    assert_eq!(info.set_at, "2024-03-15T10:30:00+08:00");
}

#[test]
fn golden_wallpaper_info_empty_keeps_every_key() {
    let info: WallpaperInfo = round_trip_file("wallpaper_info_empty.json");
    assert_eq!(info, WallpaperInfo::default());
    // A default value must serialise all nine keys, as Go has no omitempty there.
    let v = serde_json::to_value(WallpaperInfo::default()).expect("serialize");
    assert_eq!(v.as_object().map(|o| o.len()), Some(9));
}

#[test]
fn golden_wallpaper_info_missing_fields_default() {
    let info: WallpaperInfo = serde_json::from_str(r#"{"path":"/a.jpg"}"#).expect("parse");
    assert_eq!(info.path, "/a.jpg");
    assert_eq!(info.width, 0);
    assert_eq!(info.set_at, "");
}

#[test]
fn golden_status_current_null() {
    let status: StatusData = round_trip_file("status_current_null.json");
    assert!(status.current.is_none());
    assert_eq!(status.history_index, -1);
    let v = serde_json::to_value(&status).expect("serialize");
    assert_eq!(
        v.get("current"),
        Some(&Value::Null),
        "current must be present as null"
    );
}

#[test]
fn golden_status_current_set() {
    let status: StatusData = round_trip_file("status_current_set.json");
    assert!(status.paused);
    assert_eq!(status.interval, "1h30m");
    assert_eq!(status.theme, "dark");
    assert_eq!(status.history_count, 3);
    assert_eq!(status.history_index, 2);
    let current = status.current.expect("current set");
    assert_eq!(current.author, "© Photographer \"Quoted\"");
}

#[test]
fn golden_status_json_cli_output_is_semantically_status_data() {
    // `status --json` re-indents the payload with sorted keys; the CLI task
    // owns the byte layout, the type must still carry the same document.
    let status: StatusData = round_trip_file("status_json.txt");
    assert_eq!(status.history_count, 3);
}

#[test]
fn golden_response_ok_nodata() {
    let resp: Response = round_trip_file("response_ok_nodata.json");
    assert!(resp.ok);
    assert!(resp.data.is_none());
    assert_eq!(resp.error, "");
    let s = serde_json::to_string(&Response::ok()).expect("serialize");
    assert_eq!(s, r#"{"ok":true}"#);
}

#[test]
fn golden_response_ok_data() {
    let resp: Response = round_trip_file("response_ok_data.json");
    let info: WallpaperInfo = resp.data_as().expect("decode").expect("data present");
    let expected: WallpaperInfo = round_trip_file("wallpaper_info_full.json");
    assert_eq!(info, expected);
    let rebuilt = Response::ok_data(&info).expect("ok_data");
    assert_eq!(
        serde_json::to_value(&rebuilt).expect("serialize"),
        serde_json::from_str::<Value>(&read("response_ok_data.json")).expect("parse"),
    );
}

#[test]
fn golden_response_ok_status() {
    let resp: Response = round_trip_file("response_ok_status.json");
    let status: StatusData = resp.data_as().expect("decode").expect("data present");
    assert!(status.current.is_none());
    let rebuilt = Response::ok_data(&status).expect("ok_data");
    assert_eq!(
        serde_json::to_value(&rebuilt).expect("serialize"),
        serde_json::from_str::<Value>(&read("response_ok_status.json")).expect("parse"),
    );
}

#[test]
fn golden_response_err() {
    let resp: Response = round_trip_file("response_err.json");
    assert!(!resp.ok);
    assert_eq!(resp.error, unknown_command_error("bogus"));
    let rebuilt = Response::err(RequestError::UnknownCommand("bogus".into()));
    assert_eq!(
        serde_json::to_string(&rebuilt).expect("serialize"),
        read("response_err.json").trim_end()
    );
}

#[test]
fn golden_response_err_constructor() {
    let resp: Response = round_trip_file("response_err_constructor.json");
    assert_eq!(resp.error, "manager: no current wallpaper");
    let rebuilt = Response::err("manager: no current wallpaper");
    assert_eq!(
        serde_json::to_string(&rebuilt).expect("serialize"),
        read("response_err_constructor.json").trim_end()
    );
}

#[test]
fn golden_command_noargs() {
    let cmd: Command = round_trip_file("command_noargs.json");
    assert_eq!(Request::try_from(cmd.clone()), Ok(Request::Status));
    assert_eq!(Command::from(Request::Status), cmd);
}

#[test]
fn golden_command_set() {
    let cmd: Command = round_trip_file("command_set.json");
    let req = Request::try_from(cmd.clone()).expect("valid set");
    assert_eq!(
        req,
        Request::Set {
            path: "/Users/golden/Pictures/mountain & lake.jpg".into()
        }
    );
    assert_eq!(Command::from(req), cmd);
}

#[test]
fn golden_state_wallpaper_infos_round_trip() {
    // state.json belongs to the engine; its WallpaperInfo entries must still
    // round-trip through the protocol type.
    for name in ["state_empty.json", "state_populated.json"] {
        let state: Value = serde_json::from_str(&read(name)).expect("parse");
        let mut infos: Vec<&Value> = Vec::new();
        if !state["current"].is_null() {
            infos.push(&state["current"]);
        }
        if let Some(history) = state["history"].as_array() {
            infos.extend(history);
        }
        for (i, v) in infos.into_iter().enumerate() {
            let _: WallpaperInfo = round_trip(&format!("{name}#{i}"), &v.to_string());
        }
    }
}

// ---- request -> Request conversion ----

#[test]
fn golden_request_round_trips_every_variant() {
    let all = [
        Request::Next,
        Request::Prev,
        Request::Set {
            path: "/tmp/x.png".into(),
        },
        Request::Favorite,
        Request::Trash { expected: None },
        Request::Trash {
            expected: Some("/tmp/x.png".into()),
        },
        Request::Pause,
        Request::Resume,
        Request::Status,
        Request::Reload,
        Request::Subscribe,
        Request::Version,
        Request::Doctor { apply: false },
        Request::Doctor { apply: true },
    ];
    for req in all {
        let cmd = Command::from(req.clone());
        assert_eq!(cmd.cmd, req.name());
        let json = serde_json::to_string(&cmd).expect("serialize");
        let back: Command = serde_json::from_str(&json).expect("parse");
        assert_eq!(Request::try_from(back), Ok(req));
    }
}

/// New in Rust: `doctor` with `apply` (Task 37). Fixture written by hand, not by Go.
#[test]
fn golden_command_doctor_apply() {
    let cmd: Command = round_trip_file("command_doctor_apply.json");
    assert_eq!(
        Request::try_from(cmd.clone()),
        Ok(Request::Doctor { apply: true })
    );
    assert_go_bytes::<Command>(
        "command_doctor_apply.json",
        read("command_doctor_apply.json").trim_end(),
    );
    assert_eq!(Command::from(Request::Doctor { apply: true }), cmd);
}

#[test]
fn golden_doctor_apply_absent_null_or_false_means_report_only() {
    for json in [
        r#"{"cmd":"doctor"}"#,
        r#"{"cmd":"doctor","args":null}"#,
        r#"{"cmd":"doctor","args":{"apply":null}}"#,
        r#"{"cmd":"doctor","args":{"apply":false}}"#,
    ] {
        let cmd: Command = serde_json::from_str(json).expect("parse");
        assert_eq!(
            Request::try_from(cmd),
            Ok(Request::Doctor { apply: false }),
            "{json}"
        );
    }
    // Plain doctor writes no `args`: still `{"cmd":"doctor"}` byte for byte.
    let line = serde_json::to_string(&Command::from(Request::Doctor { apply: false })).unwrap();
    assert_eq!(line, r#"{"cmd":"doctor"}"#);
    for bad in [
        r#"{"cmd":"doctor","args":{"apply":"yes"}}"#,
        r#"{"cmd":"doctor","args":{"apply":1}}"#,
    ] {
        let bad: Command = serde_json::from_str(bad).unwrap();
        assert_eq!(
            Request::try_from(bad).unwrap_err().to_string(),
            "doctor: argument 'apply' must be a boolean"
        );
    }
}

/// New in Rust: `trash` with `expected` (Task 45b). Fixture written by hand, not by Go.
#[test]
fn golden_command_trash_expected() {
    let cmd: Command = round_trip_file("command_trash_expected.json");
    assert_eq!(
        Request::try_from(cmd.clone()),
        Ok(Request::Trash {
            expected: Some("/Users/golden/Pictures/mountain & lake.jpg".into())
        })
    );
    assert_go_bytes::<Command>(
        "command_trash_expected.json",
        read("command_trash_expected.json").trim_end(),
    );
    // Request -> Command writes the same args back.
    assert_eq!(Command::from(Request::try_from(cmd.clone()).unwrap()), cmd);
}

#[test]
fn golden_trash_expected_absent_null_or_empty_means_no_check() {
    for json in [
        r#"{"cmd":"trash"}"#,
        r#"{"cmd":"trash","args":null}"#,
        r#"{"cmd":"trash","args":{"expected":null}}"#,
        r#"{"cmd":"trash","args":{"expected":""}}"#,
    ] {
        let cmd: Command = serde_json::from_str(json).expect("parse");
        assert_eq!(
            Request::try_from(cmd),
            Ok(Request::Trash { expected: None }),
            "{json}"
        );
    }
    // No `args` written for the unchecked form: same bytes as Go's `{"cmd":"trash"}`.
    let line = serde_json::to_string(&Command::from(Request::Trash { expected: None })).unwrap();
    assert_eq!(line, r#"{"cmd":"trash"}"#);
    let bad: Command = serde_json::from_str(r#"{"cmd":"trash","args":{"expected":7}}"#).unwrap();
    assert_eq!(
        Request::try_from(bad).unwrap_err().to_string(),
        "trash: argument 'expected' must be a string"
    );
}

#[test]
fn golden_set_path_errors_match_go() {
    let missing = "set: missing required argument 'path'";
    for json in [
        r#"{"cmd":"set"}"#,
        r#"{"cmd":"set","args":{"path":42}}"#,
        r#"{"cmd":"set","args":{"path":""}}"#,
        r#"{"cmd":"set","args":null}"#,
    ] {
        let cmd: Command = serde_json::from_str(json).expect("parse");
        let err = Request::try_from(cmd).expect_err(json);
        assert_eq!(err.to_string(), missing, "{json}");
    }
}

#[test]
fn golden_unknown_command_quoting_matches_go_percent_q() {
    assert_eq!(
        unknown_command_error("bogus"),
        r#"unknown command: "bogus""#
    );
    assert_eq!(unknown_command_error(""), r#"unknown command: """#);
    assert_eq!(
        unknown_command_error("a\"b\\c"),
        r#"unknown command: "a\"b\\c""#
    );
    assert_eq!(
        unknown_command_error("tab\there\n"),
        r#"unknown command: "tab\there\n""#
    );
    assert_eq!(
        unknown_command_error("\u{7}\u{1b}"),
        r#"unknown command: "\a\x1b""#
    );
    assert_eq!(
        unknown_command_error("\u{7f}"),
        r#"unknown command: "\x7f""#
    );
    assert_eq!(
        unknown_command_error("café ☕"),
        r#"unknown command: "café ☕""#
    );
    assert_eq!(
        unknown_command_error("\u{a0}"),
        r#"unknown command: "\u00a0""#
    );
    assert_eq!(
        unknown_command_error("\u{feff}"),
        r#"unknown command: "\ufeff""#
    );
    assert_eq!(
        unknown_command_error("\u{e0001}"),
        r#"unknown command: "\U000e0001""#
    );
}

// ---- Event ----

#[test]
fn golden_event_changed_shape() {
    let info: WallpaperInfo = round_trip_file("wallpaper_info_full.json");
    let event = Event::Changed(info.clone());
    let v = serde_json::to_value(&event).expect("serialize");
    assert_eq!(v["event"], "changed");
    assert_eq!(v["data"], serde_json::to_value(&info).expect("serialize"));
    let back: Event = serde_json::from_value(v).expect("parse");
    assert_eq!(back, event);
}

#[test]
fn golden_event_unknown_is_tolerated() {
    let e: Event =
        serde_json::from_str(r#"{"event":"future","data":{"x":1}}"#).expect("parse unknown");
    assert_eq!(e, Event::Unknown);
    let e: Event = serde_json::from_str(r#"{"event":"future"}"#).expect("parse no data");
    assert_eq!(e, Event::Unknown);
}

// ---- wire captures from the running Go daemon ----

fn wire_cases() -> Vec<(String, Vec<String>, Vec<String>)> {
    let dir = fixture_dir().join("wire");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .expect("wire dir")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".request.jsonl").map(str::to_owned))
        .collect();
    names.sort();
    assert_eq!(names.len(), 22, "expected 22 wire cases");
    names
        .into_iter()
        .map(|name| {
            let req = read(&format!("wire/{name}.request.jsonl"));
            let resp = read(&format!("wire/{name}.response.jsonl"));
            let lines = |s: &str| s.lines().map(str::to_owned).collect::<Vec<_>>();
            (name, lines(&req), lines(&resp))
        })
        .collect()
}

#[test]
fn golden_wire_responses_round_trip() {
    for (name, _, responses) in wire_cases() {
        for (i, line) in responses.iter().enumerate() {
            let ctx = format!("{name}:{i}");
            let resp: Response = round_trip(&ctx, line);
            let Some(raw) = resp.data.as_ref() else {
                continue;
            };
            let data: Value = serde_json::from_str(raw.get()).expect("data json");
            if data.get("history_index").is_some() {
                let _: StatusData = round_trip(&ctx, raw.get());
            } else {
                let _: WallpaperInfo = round_trip(&ctx, raw.get());
            }
        }
    }
}

/// Every request line either fails to parse (Go: `invalid JSON: ...`), or
/// converts to a `Request` exactly when Go's dispatcher accepted it, with the
/// same error text for the dispatch-level failures.
#[test]
fn golden_wire_requests_match_go_dispatch() {
    let mut checked = 0;
    for (name, requests, responses) in wire_cases() {
        assert_eq!(
            requests.len(),
            responses.len(),
            "{name}: one response per line"
        );
        for (line, resp_line) in requests.iter().zip(&responses) {
            let resp: Response = serde_json::from_str(resp_line).expect("response");
            let parsed = serde_json::from_str::<Command>(line);
            if resp.error.starts_with("invalid JSON: ") {
                assert!(parsed.is_err(), "{name}: {line:?} should not parse");
                continue;
            }
            let cmd = parsed.unwrap_or_else(|e| panic!("{name}: {line:?}: {e}"));
            match Request::try_from(cmd) {
                Ok(_) => assert!(
                    resp.ok || resp.error.starts_with("manager: "),
                    "{name}: Go rejected {line:?} with {:?}",
                    resp.error
                ),
                Err(e) => assert_eq!(e.to_string(), resp.error, "{name}: {line:?}"),
            }
            checked += 1;
        }
    }
    assert!(checked >= 25, "checked only {checked} request lines");
}

#[test]
fn golden_wire_extra_fields_and_null_args_accepted() {
    let cmd: Command =
        serde_json::from_str(r#"{"cmd":"status","args":null,"extra":1}"#).expect("parse");
    assert!(cmd.args.is_empty());
    let cmd: Command = serde_json::from_str(r#"{"args":{}}"#).expect("parse");
    assert_eq!(cmd.cmd, "");
    assert_eq!(
        Request::try_from(cmd).map_err(|e| e.to_string()),
        Err(r#"unknown command: """#.to_owned())
    );
}

// ---- TypeScript bindings ----

#[test]
fn golden_ts_bindings_match_wire_shape() {
    use ts_rs::TS;
    let cfg = ts_rs::Config::default();
    let info = WallpaperInfo::decl(&cfg);
    assert!(info.contains("width: number"), "{info}");
    // Every Go field is always written. Only the new `source_id` (Task 16) is optional:
    // it is left out when empty so the Go bytes stay the same.
    assert!(info.contains("source_id?: string"), "{info}");
    assert!(
        !info.replace("source_id?: string", "").contains('?'),
        "no other optional WallpaperInfo fields: {info}"
    );
    let status = StatusData::decl(&cfg);
    assert!(status.contains("current: WallpaperInfo | null"), "{status}");
    assert!(status.contains("history_index: number"), "{status}");
    let resp = Response::decl(&cfg);
    assert!(resp.contains("error?: string"), "{resp}");
    assert!(resp.contains("data?: unknown"), "{resp}");
    let cmd = Command::decl(&cfg);
    assert!(cmd.contains("args?:"), "{cmd}");
    let event = Event::decl(&cfg);
    assert!(event.contains("\"changed\""), "{event}");
}

// ---- byte-exact re-encoding with the Go-compatible encoder ----

fn assert_go_bytes<T: DeserializeOwned + Serialize>(context: &str, line: &str) {
    let value: T = serde_json::from_str(line).expect(context);
    let bytes = kakemono_protocol::codec::to_string_go(&value).expect(context);
    assert_eq!(bytes, line, "{context}");
}

/// Re-encoding a parsed fixture reproduces Go's bytes exactly, including Go's
/// `\u003c`/`\u003e`/`\u0026` escapes and its key order.
#[test]
fn golden_go_encoder_reproduces_fixture_bytes() {
    for name in ["wallpaper_info_full.json", "wallpaper_info_empty.json"] {
        assert_go_bytes::<WallpaperInfo>(name, read(name).trim_end_matches('\n'));
    }
    for name in ["status_current_null.json", "status_current_set.json"] {
        assert_go_bytes::<StatusData>(name, read(name).trim_end_matches('\n'));
    }
    for name in ["command_noargs.json", "command_set.json"] {
        assert_go_bytes::<Command>(name, read(name).trim_end_matches('\n'));
    }
    for name in [
        "response_ok_nodata.json",
        "response_ok_data.json",
        "response_ok_status.json",
        "response_err.json",
        "response_err_constructor.json",
    ] {
        assert_go_bytes::<Response>(name, read(name).trim_end_matches('\n'));
    }
    for (name, _, responses) in wire_cases() {
        for (i, line) in responses.iter().enumerate() {
            assert_go_bytes::<Response>(&format!("{name}:{i}"), line);
        }
    }
}

/// `Response::ok_data` builds the same line Go's daemon sends for the payload.
#[test]
fn golden_ok_data_matches_go_bytes() {
    for name in ["response_ok_data.json", "response_ok_status.json"] {
        let line = read(name);
        let line = line.trim_end_matches('\n');
        let parsed: Response = serde_json::from_str(line).expect("fixture");
        let data: Value = parsed.data_as().expect("data").expect("some");
        let typed = if name.contains("status") {
            let s: StatusData = serde_json::from_value(data).expect("status");
            Response::ok_data(&s)
        } else {
            let w: WallpaperInfo = serde_json::from_value(data).expect("info");
            Response::ok_data(&w)
        }
        .expect("ok_data");
        let out = kakemono_protocol::codec::to_string_go(&typed).expect("ser");
        assert_eq!(out, line, "{name}");
    }
}
