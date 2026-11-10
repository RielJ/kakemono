//! Runs the Go-duration table that the settings UI also reads
//! (`crates/ui/frontend/src/settings/duration.test.ts`), so the TS mirror of
//! [`kakemono_config::parse_go`] can't drift from the Rust parser.

use std::time::Duration;

use kakemono_config::{DurationError, parse_go};
use serde::Deserialize;

#[derive(Deserialize)]
struct Cases {
    valid: Vec<Valid>,
    invalid: Vec<Invalid>,
}

#[derive(Deserialize)]
struct Valid {
    input: String,
    /// Decimal string: values reach 2^63 - 1, past JSON's safe integer range.
    nanos: String,
}

#[derive(Deserialize)]
struct Invalid {
    input: String,
    kind: String,
    unit: Option<String>,
}

fn cases() -> Cases {
    let text = include_str!("../fixtures/go_durations.json");
    serde_json::from_str(text).expect("go_durations.json parses")
}

#[test]
fn duration_shared_valid_cases() {
    let cases = cases();
    assert!(!cases.valid.is_empty());
    for case in cases.valid {
        let nanos: u64 = case.nanos.parse().expect("nanos is a u64");
        assert_eq!(
            parse_go(&case.input),
            Ok(Duration::from_nanos(nanos)),
            "input {:?}",
            case.input
        );
    }
}

#[test]
fn duration_shared_invalid_cases() {
    let cases = cases();
    assert!(!cases.invalid.is_empty());
    for case in cases.invalid {
        let err = parse_go(&case.input).expect_err(&case.input);
        let (kind, unit) = match &err {
            DurationError::Invalid(_) => ("invalid", None),
            DurationError::MissingUnit(_) => ("missing_unit", None),
            DurationError::UnknownUnit { unit, .. } => ("unknown_unit", Some(unit.as_str())),
            DurationError::Negative(_) => ("negative", None),
        };
        assert_eq!(kind, case.kind, "input {:?}: {err}", case.input);
        assert_eq!(unit, case.unit.as_deref(), "input {:?}", case.input);
    }
}
