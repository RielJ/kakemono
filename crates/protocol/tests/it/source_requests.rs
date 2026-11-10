//! The source requests: `check_source`, `fetch`
//! and `download_now`, their response payloads, and `WallpaperInfo.source_id`.

use kakemono_protocol::types::{
    CheckSourceData, Command, DownloadNowData, Request, RequestError, Response, WallpaperInfo,
};

fn parse(line: &str) -> Result<Request, RequestError> {
    let cmd: Command = serde_json::from_str(line).expect("valid command JSON");
    Request::try_from(cmd)
}

fn wire(req: Request) -> String {
    serde_json::to_string(&Command::from(req)).expect("serialize")
}

// ─── check_source ─────────────────────────────────────────────────────────────────────────

#[test]
fn check_source_parses_type_and_location() {
    assert_eq!(
        parse(r#"{"cmd":"check_source","args":{"type":"wallhaven","location":"forest"}}"#),
        Ok(Request::CheckSource {
            kind: "wallhaven".into(),
            location: "forest".into(),
        })
    );
}

#[test]
fn check_source_missing_or_null_location_is_empty() {
    let bing = Ok(Request::CheckSource {
        kind: "bing".into(),
        location: String::new(),
    });
    assert_eq!(
        parse(r#"{"cmd":"check_source","args":{"type":"bing"}}"#),
        bing
    );
    assert_eq!(
        parse(r#"{"cmd":"check_source","args":{"type":"bing","location":null}}"#),
        bing
    );
}

#[test]
fn check_source_without_a_type_is_missing_type() {
    for line in [
        r#"{"cmd":"check_source"}"#,
        r#"{"cmd":"check_source","args":{"location":"forest"}}"#,
        r#"{"cmd":"check_source","args":{"type":""}}"#,
        r#"{"cmd":"check_source","args":{"type":3}}"#,
    ] {
        assert_eq!(parse(line), Err(RequestError::MissingType), "{line}");
    }
    assert_eq!(
        RequestError::MissingType.to_string(),
        "check_source: missing required argument 'type'"
    );
}

#[test]
fn check_source_non_string_location_is_an_error() {
    assert_eq!(
        parse(r#"{"cmd":"check_source","args":{"type":"folder","location":1}}"#),
        Err(RequestError::InvalidLocation)
    );
    assert_eq!(
        RequestError::InvalidLocation.to_string(),
        "check_source: argument 'location' must be a string"
    );
}

#[test]
fn check_source_wire_form() {
    let req = Request::CheckSource {
        kind: "wallhaven".into(),
        location: "forest".into(),
    };
    assert_eq!(req.name(), "check_source");
    assert_eq!(
        wire(req),
        r#"{"cmd":"check_source","args":{"location":"forest","type":"wallhaven"}}"#
    );
}

// ─── fetch ────────────────────────────────────────────────────────────────────────────────

#[test]
fn fetch_parses_url() {
    assert_eq!(
        parse(r#"{"cmd":"fetch","args":{"url":"https://x/y.jpg"}}"#),
        Ok(Request::Fetch {
            url: "https://x/y.jpg".into(),
        })
    );
    assert_eq!(
        parse(r#"{"cmd":"fetch","args":{"url":"/Users/me/a.png"}}"#),
        Ok(Request::Fetch {
            url: "/Users/me/a.png".into(),
        })
    );
}

#[test]
fn fetch_without_a_url_is_missing_url() {
    for line in [
        r#"{"cmd":"fetch"}"#,
        r#"{"cmd":"fetch","args":{"url":""}}"#,
        r#"{"cmd":"fetch","args":{"url":null}}"#,
        r#"{"cmd":"fetch","args":{"url":["a"]}}"#,
    ] {
        assert_eq!(parse(line), Err(RequestError::MissingUrl), "{line}");
    }
    assert_eq!(
        RequestError::MissingUrl.to_string(),
        "fetch: missing required argument 'url'"
    );
}

#[test]
fn fetch_wire_form() {
    let req = Request::Fetch {
        url: "https://x/y.jpg".into(),
    };
    assert_eq!(req.name(), "fetch");
    assert_eq!(
        wire(req),
        r#"{"cmd":"fetch","args":{"url":"https://x/y.jpg"}}"#
    );
}

// ─── download_now ─────────────────────────────────────────────────────────────────────────

#[test]
fn download_now_id_is_optional() {
    assert_eq!(
        parse(r#"{"cmd":"download_now","args":{"id":"wh-1"}}"#),
        Ok(Request::DownloadNow {
            id: Some("wh-1".into())
        })
    );
    for line in [
        r#"{"cmd":"download_now"}"#,
        r#"{"cmd":"download_now","args":{"id":null}}"#,
        r#"{"cmd":"download_now","args":{"id":""}}"#,
    ] {
        assert_eq!(parse(line), Ok(Request::DownloadNow { id: None }), "{line}");
    }
}

#[test]
fn download_now_non_string_id_is_an_error() {
    assert_eq!(
        parse(r#"{"cmd":"download_now","args":{"id":7}}"#),
        Err(RequestError::InvalidId)
    );
    assert_eq!(
        RequestError::InvalidId.to_string(),
        "download_now: argument 'id' must be a string"
    );
}

#[test]
fn download_now_wire_form() {
    assert_eq!(Request::DownloadNow { id: None }.name(), "download_now");
    assert_eq!(
        wire(Request::DownloadNow { id: None }),
        r#"{"cmd":"download_now"}"#
    );
    assert_eq!(
        wire(Request::DownloadNow {
            id: Some("wh-1".into())
        }),
        r#"{"cmd":"download_now","args":{"id":"wh-1"}}"#
    );
}

// ─── round trips ──────────────────────────────────────────────────────────────────────────

#[test]
fn new_requests_round_trip_through_command() {
    for req in [
        Request::CheckSource {
            kind: "folder".into(),
            location: "/pics".into(),
        },
        Request::CheckSource {
            kind: "bing".into(),
            location: String::new(),
        },
        Request::Fetch {
            url: "https://x/y.jpg".into(),
        },
        Request::DownloadNow { id: None },
        Request::DownloadNow {
            id: Some("wh-1".into()),
        },
    ] {
        let line = wire(req.clone());
        assert_eq!(parse(&line), Ok(req), "{line}");
    }
}

// ─── payloads ─────────────────────────────────────────────────────────────────────────────

#[test]
fn check_source_payload_is_count() {
    let r = Response::ok_data(&CheckSourceData { count: 3 }).unwrap();
    assert_eq!(
        serde_json::to_string(&r).unwrap(),
        r#"{"ok":true,"data":{"count":3}}"#
    );
    assert_eq!(
        r.data_as::<CheckSourceData>().unwrap(),
        Some(CheckSourceData { count: 3 })
    );
}

#[test]
fn download_now_payload_writes_null_path() {
    let none = Response::ok_data(&DownloadNowData { path: None }).unwrap();
    assert_eq!(
        serde_json::to_string(&none).unwrap(),
        r#"{"ok":true,"data":{"path":null}}"#
    );
    let some = Response::ok_data(&DownloadNowData {
        path: Some("/d/wh-1/a.jpg".into()),
    })
    .unwrap();
    assert_eq!(
        serde_json::to_string(&some).unwrap(),
        r#"{"ok":true,"data":{"path":"/d/wh-1/a.jpg"}}"#
    );
    assert_eq!(
        some.data_as::<DownloadNowData>().unwrap(),
        Some(DownloadNowData {
            path: Some("/d/wh-1/a.jpg".into())
        })
    );
}

// ─── WallpaperInfo.source_id ──────────────────────────────────────────────────────────────

#[test]
fn source_id_defaults_to_empty_for_old_json() {
    let info: WallpaperInfo = serde_json::from_str(r#"{"path":"/a.jpg","source":"bing"}"#).unwrap();
    assert_eq!(info.source_id, "");
}

#[test]
fn empty_source_id_is_not_written() {
    let v = serde_json::to_value(WallpaperInfo::default()).unwrap();
    assert!(v.get("source_id").is_none(), "{v}");
}

#[test]
fn source_id_is_written_after_source() {
    let info = WallpaperInfo {
        path: "/d/wh-1/a.jpg".into(),
        source: "wallhaven".into(),
        source_id: "wh-1".into(),
        ..WallpaperInfo::default()
    };
    let line = kakemono_protocol::codec::to_string_go(&info).unwrap();
    assert_eq!(
        line,
        r#"{"path":"/d/wh-1/a.jpg","source":"wallhaven","source_id":"wh-1","source_url":"","image_url":"","title":"","author":"","width":0,"height":0,"set_at":""}"#
    );
    let back: WallpaperInfo = serde_json::from_str(&line).unwrap();
    assert_eq!(back, info);
}
