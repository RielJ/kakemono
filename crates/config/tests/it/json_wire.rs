//! The JSON the UI receives must match the ts-rs bindings. Every optional field is typed
//! `T | null` there, so an unset field must arrive as `null`, never as a missing key: the
//! frontend checks `=== null`, and a missing key reaches it as `undefined`.

use kakemono_config::Config;
use serde_json::Value;

#[test]
fn unset_optional_fields_serialise_as_null() {
    let json = serde_json::to_value(Config::default()).expect("serialise");
    for (table, key) in [
        ("daemon", "socket_path"),
        ("daemon", "manual_dir"),
        ("picker", "custom"),
        ("desktop", "set_command"),
        ("desktop", "detected_session"),
        ("desktop", "detected_at"),
        ("hooks", "on_change"),
    ] {
        assert_eq!(
            json[table].get(key),
            Some(&Value::Null),
            "{table}.{key} must be present as null"
        );
    }
}

#[test]
fn source_type_is_sent_under_the_key_type() {
    let json = serde_json::to_value(Config::default()).expect("serialise");
    let row = &json["sources"][0];
    assert_eq!(row["type"], Value::from("bing"));
    assert_eq!(row.get("kind"), None);
}
