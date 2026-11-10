//! The browse requests: `browse` and `browse_use`,
//! their argument parsing and errors, and their response payloads. The byte-exact fixtures
//! are in `golden.rs`.

use kakemono_protocol::types::{
    BrowseAction, BrowseSourceState, BrowseUseData, Command, Request, RequestError, Response,
};

fn parse(line: &str) -> Result<Request, RequestError> {
    let cmd: Command = serde_json::from_str(line).expect("valid command JSON");
    Request::try_from(cmd)
}

fn wire(req: Request) -> String {
    serde_json::to_string(&Command::from(req)).expect("serialize")
}

fn browse_use(action: BrowseAction) -> Request {
    Request::BrowseUse {
        source_id: "wh-59f2".into(),
        remote_id: "x8e1k2".into(),
        action,
    }
}

// ─── browse ───────────────────────────────────────────────────────────────────────────────

#[test]
fn browse_with_only_a_page_has_no_session_or_source() {
    assert_eq!(
        parse(r#"{"cmd":"browse","args":{"page":0}}"#),
        Ok(Request::Browse {
            session: None,
            page: 0,
            source: None,
        })
    );
}

#[test]
fn browse_parses_session_page_and_source() {
    assert_eq!(
        parse(r#"{"cmd":"browse","args":{"session":"9f2c","page":3,"source":"wh-59f2"}}"#),
        Ok(Request::Browse {
            session: Some("9f2c".into()),
            page: 3,
            source: Some("wh-59f2".into()),
        })
    );
}

#[test]
fn browse_missing_null_or_empty_optionals_read_as_none_and_page_zero() {
    let first = Ok(Request::Browse {
        session: None,
        page: 0,
        source: None,
    });
    for line in [
        r#"{"cmd":"browse"}"#,
        r#"{"cmd":"browse","args":null}"#,
        r#"{"cmd":"browse","args":{"page":null}}"#,
        r#"{"cmd":"browse","args":{"session":null,"page":0,"source":null}}"#,
        r#"{"cmd":"browse","args":{"session":"","page":0,"source":""}}"#,
    ] {
        assert_eq!(parse(line), first, "{line}");
    }
}

#[test]
fn browse_non_string_session_is_an_error() {
    assert_eq!(
        parse(r#"{"cmd":"browse","args":{"session":7,"page":0}}"#),
        Err(RequestError::InvalidSession)
    );
    assert_eq!(
        RequestError::InvalidSession.to_string(),
        "browse: argument 'session' must be a string"
    );
}

#[test]
fn browse_non_string_source_is_an_error() {
    assert_eq!(
        parse(r#"{"cmd":"browse","args":{"page":0,"source":["wh-1"]}}"#),
        Err(RequestError::InvalidSource)
    );
    assert_eq!(
        RequestError::InvalidSource.to_string(),
        "browse: argument 'source' must be a string"
    );
}

#[test]
fn browse_page_that_is_not_a_u32_is_an_error() {
    for line in [
        r#"{"cmd":"browse","args":{"page":-1}}"#,
        r#"{"cmd":"browse","args":{"page":1.5}}"#,
        r#"{"cmd":"browse","args":{"page":"3"}}"#,
        r#"{"cmd":"browse","args":{"page":4294967296}}"#,
    ] {
        assert_eq!(parse(line), Err(RequestError::InvalidPage), "{line}");
    }
    assert_eq!(
        RequestError::InvalidPage.to_string(),
        "browse: argument 'page' must be a non-negative integer"
    );
}

#[test]
fn browse_largest_page_parses() {
    assert_eq!(
        parse(r#"{"cmd":"browse","args":{"page":4294967295}}"#),
        Ok(Request::Browse {
            session: None,
            page: u32::MAX,
            source: None,
        })
    );
}

#[test]
fn browse_wire_form_always_writes_page_and_omits_none() {
    let first = Request::Browse {
        session: None,
        page: 0,
        source: None,
    };
    assert_eq!(first.name(), "browse");
    assert_eq!(wire(first), r#"{"cmd":"browse","args":{"page":0}}"#);
    assert_eq!(
        wire(Request::Browse {
            session: Some("9f2c".into()),
            page: 3,
            source: Some("wh-59f2".into()),
        }),
        r#"{"cmd":"browse","args":{"page":3,"session":"9f2c","source":"wh-59f2"}}"#
    );
}

// ─── browse_use ───────────────────────────────────────────────────────────────────────────

#[test]
fn browse_use_parses_each_action() {
    for (name, action) in [
        ("set", BrowseAction::Set),
        ("favorite", BrowseAction::Favorite),
        ("download", BrowseAction::Download),
    ] {
        let line = format!(
            r#"{{"cmd":"browse_use","args":{{"source_id":"wh-59f2","remote_id":"x8e1k2","action":"{name}"}}}}"#
        );
        assert_eq!(parse(&line), Ok(browse_use(action)), "{line}");
    }
}

#[test]
fn browse_use_without_a_source_id_is_missing_source_id() {
    for line in [
        r#"{"cmd":"browse_use","args":{"remote_id":"x8e1k2","action":"set"}}"#,
        r#"{"cmd":"browse_use","args":{"source_id":"","remote_id":"x8e1k2","action":"set"}}"#,
        r#"{"cmd":"browse_use","args":{"source_id":null,"remote_id":"x8e1k2","action":"set"}}"#,
        r#"{"cmd":"browse_use","args":{"source_id":1,"remote_id":"x8e1k2","action":"set"}}"#,
    ] {
        assert_eq!(parse(line), Err(RequestError::MissingSourceId), "{line}");
    }
    assert_eq!(
        RequestError::MissingSourceId.to_string(),
        "browse_use: missing required argument 'source_id'"
    );
}

#[test]
fn browse_use_without_a_remote_id_is_missing_remote_id() {
    for line in [
        r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","action":"set"}}"#,
        r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":"","action":"set"}}"#,
        r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":false,"action":"set"}}"#,
    ] {
        assert_eq!(parse(line), Err(RequestError::MissingRemoteId), "{line}");
    }
    assert_eq!(
        RequestError::MissingRemoteId.to_string(),
        "browse_use: missing required argument 'remote_id'"
    );
}

#[test]
fn browse_use_without_an_action_is_missing_action() {
    for line in [
        r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":"x8e1k2"}}"#,
        r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":"x8e1k2","action":""}}"#,
        r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":"x8e1k2","action":null}}"#,
        r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":"x8e1k2","action":2}}"#,
    ] {
        assert_eq!(parse(line), Err(RequestError::MissingAction), "{line}");
    }
    assert_eq!(
        RequestError::MissingAction.to_string(),
        "browse_use: missing required argument 'action'"
    );
}

#[test]
fn browse_use_unknown_action_is_an_error_naming_it() {
    assert_eq!(
        parse(
            r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":"x8e1k2","action":"nuke"}}"#
        ),
        Err(RequestError::UnknownAction("nuke".into()))
    );
    assert_eq!(
        RequestError::UnknownAction("nuke".into()).to_string(),
        r#"browse_use: unknown action "nuke""#
    );
}

#[test]
fn browse_use_action_is_case_sensitive() {
    assert_eq!(
        parse(
            r#"{"cmd":"browse_use","args":{"source_id":"wh-59f2","remote_id":"x8e1k2","action":"Set"}}"#
        ),
        Err(RequestError::UnknownAction("Set".into()))
    );
}

#[test]
fn browse_use_wire_form() {
    let req = browse_use(BrowseAction::Download);
    assert_eq!(req.name(), "browse_use");
    assert_eq!(
        wire(req),
        r#"{"cmd":"browse_use","args":{"action":"download","remote_id":"x8e1k2","source_id":"wh-59f2"}}"#
    );
}

#[test]
fn browse_action_wire_names_match_serde() {
    for action in [
        BrowseAction::Set,
        BrowseAction::Favorite,
        BrowseAction::Download,
    ] {
        assert_eq!(
            serde_json::to_value(action).expect("serialize"),
            serde_json::Value::String(action.as_str().into()),
            "{action:?}"
        );
    }
}

// ─── round trips ──────────────────────────────────────────────────────────────────────────

#[test]
fn browse_requests_round_trip_through_command() {
    for req in [
        Request::Browse {
            session: None,
            page: 0,
            source: None,
        },
        Request::Browse {
            session: Some("9f2c".into()),
            page: 3,
            source: None,
        },
        Request::Browse {
            session: None,
            page: 1,
            source: Some("wh-59f2".into()),
        },
        Request::Browse {
            session: Some("9f2c".into()),
            page: u32::MAX,
            source: Some("wh-59f2".into()),
        },
        browse_use(BrowseAction::Set),
        browse_use(BrowseAction::Favorite),
        browse_use(BrowseAction::Download),
    ] {
        let line = wire(req.clone());
        assert_eq!(parse(&line), Ok(req), "{line}");
    }
}

// ─── payloads ─────────────────────────────────────────────────────────────────────────────

#[test]
fn browse_source_state_is_tagged_with_state() {
    let limited = BrowseSourceState::RateLimited {
        message: "reddit: HTTP 429".into(),
        retry_after_secs: Some(60),
    };
    assert_eq!(
        serde_json::to_string(&limited).expect("serialize"),
        r#"{"state":"rate_limited","message":"reddit: HTTP 429","retry_after_secs":60}"#
    );
}

#[test]
fn browse_source_state_unit_variants_are_only_the_tag() {
    for (state, line) in [
        (BrowseSourceState::Active, r#"{"state":"active"}"#),
        (BrowseSourceState::Exhausted, r#"{"state":"exhausted"}"#),
        (BrowseSourceState::Unsupported, r#"{"state":"unsupported"}"#),
    ] {
        assert_eq!(serde_json::to_string(&state).expect("serialize"), line);
        assert_eq!(
            serde_json::from_str::<BrowseSourceState>(line).expect("parse"),
            state
        );
    }
}

#[test]
fn browse_source_state_message_variants_round_trip() {
    for (state, line) in [
        (
            BrowseSourceState::RateLimited {
                message: "m".into(),
                retry_after_secs: None,
            },
            r#"{"state":"rate_limited","message":"m","retry_after_secs":null}"#,
        ),
        (
            BrowseSourceState::NotConfigured {
                message: "m".into(),
            },
            r#"{"state":"not_configured","message":"m"}"#,
        ),
        (
            BrowseSourceState::Failed {
                message: "m".into(),
            },
            r#"{"state":"failed","message":"m"}"#,
        ),
    ] {
        assert_eq!(serde_json::to_string(&state).expect("serialize"), line);
        assert_eq!(
            serde_json::from_str::<BrowseSourceState>(line).expect("parse"),
            state
        );
    }
}

#[test]
fn browse_use_payload_writes_null_wallpaper() {
    let r = Response::ok_data(&BrowseUseData {
        path: "/d/favorites/x8e1k2.jpg".into(),
        wallpaper: None,
    })
    .expect("ok_data");
    assert_eq!(
        serde_json::to_string(&r).expect("serialize"),
        r#"{"ok":true,"data":{"path":"/d/favorites/x8e1k2.jpg","wallpaper":null}}"#
    );
}
