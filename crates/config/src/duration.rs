//! Go `time.ParseDuration` port, so existing `interval` values ("1h30m0s", "1.5h") keep working.
//!
//! Grammar: `[-+]?([0-9]*(\.[0-9]*)?(ns|us|µs|μs|ms|s|m|h))+`, or the bare string `"0"`.
//! Negative results are rejected because `std::time::Duration` is unsigned.

use std::time::Duration;

/// Why a duration string was rejected. Messages follow Go's `time: …` wording.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DurationError {
    #[error("time: invalid duration {0:?}")]
    Invalid(String),
    #[error("time: missing unit in duration {0:?}")]
    MissingUnit(String),
    #[error("time: unknown unit {unit:?} in duration {input:?}")]
    UnknownUnit { unit: String, input: String },
    #[error("time: duration {0:?} is negative")]
    Negative(String),
}

const MAX_NANOS: u64 = 1 << 63;

fn unit_nanos(u: &str) -> Option<u64> {
    Some(match u {
        "ns" => 1,
        "us" | "\u{00b5}s" | "\u{03bc}s" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "m" => 60 * 1_000_000_000,
        "h" => 3_600 * 1_000_000_000,
        _ => return None,
    })
}

/// Consumes leading ASCII digits. `None` on overflow.
fn leading_int(s: &str) -> Option<(u64, &str)> {
    let end = s
        .bytes()
        .position(|c| !c.is_ascii_digit())
        .unwrap_or(s.len());
    let mut x: u64 = 0;
    for c in s[..end].bytes() {
        x = x.checked_mul(10)?.checked_add(u64::from(c - b'0'))?;
        if x > MAX_NANOS {
            return None;
        }
    }
    Some((x, &s[end..]))
}

/// Consumes leading digits of a fraction; digits past u64 precision are dropped, as in Go.
fn leading_fraction(s: &str) -> (u64, f64, &str) {
    let end = s
        .bytes()
        .position(|c| !c.is_ascii_digit())
        .unwrap_or(s.len());
    let mut x: u64 = 0;
    let mut scale = 1.0_f64;
    let mut overflow = false;
    for c in s[..end].bytes() {
        if overflow {
            continue;
        }
        match x
            .checked_mul(10)
            .and_then(|y| y.checked_add(u64::from(c - b'0')))
        {
            Some(y) if y <= MAX_NANOS => {
                x = y;
                scale *= 10.0;
            }
            _ => overflow = true,
        }
    }
    (x, scale, &s[end..])
}

/// Parses a Go duration string: `"30m"`, `"1h30m"`, `"1h30m0s"`, `"1.5h"`, `"90s"`, `"300ms"`, `"0"`.
pub fn parse_go(input: &str) -> Result<Duration, DurationError> {
    let invalid = || DurationError::Invalid(input.to_owned());
    let mut s = input;
    let mut neg = false;
    if let Some(rest) = s.strip_prefix('-') {
        neg = true;
        s = rest;
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest;
    }
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    if s.is_empty() {
        return Err(invalid());
    }

    let mut total: u64 = 0;
    while !s.is_empty() {
        let first = s.as_bytes()[0];
        if !(first == b'.' || first.is_ascii_digit()) {
            return Err(invalid());
        }

        let before = s.len();
        let (whole, rest) = leading_int(s).ok_or_else(invalid)?;
        s = rest;
        let has_int = before != s.len();

        let mut frac = 0_u64;
        let mut scale = 1.0_f64;
        let mut has_frac = false;
        if let Some(rest) = s.strip_prefix('.') {
            let before = rest.len();
            let (f, sc, rest) = leading_fraction(rest);
            frac = f;
            scale = sc;
            has_frac = before != rest.len();
            s = rest;
        }
        if !has_int && !has_frac {
            return Err(invalid());
        }

        let unit_end = s
            .bytes()
            .position(|c| c == b'.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        if unit_end == 0 {
            return Err(DurationError::MissingUnit(input.to_owned()));
        }
        let unit_str = &s[..unit_end];
        s = &s[unit_end..];
        let unit = unit_nanos(unit_str).ok_or_else(|| DurationError::UnknownUnit {
            unit: unit_str.to_owned(),
            input: input.to_owned(),
        })?;

        if whole > MAX_NANOS / unit {
            return Err(invalid());
        }
        let mut v = whole * unit;
        if frac > 0 {
            // Same float arithmetic as Go; truncation is intended.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let extra = (frac as f64 * (unit as f64 / scale)) as u64;
            v = v.checked_add(extra).ok_or_else(invalid)?;
            if v > MAX_NANOS {
                return Err(invalid());
            }
        }
        total = total.checked_add(v).ok_or_else(invalid)?;
        if total > MAX_NANOS {
            return Err(invalid());
        }
    }

    if neg {
        if total == 0 {
            return Ok(Duration::ZERO);
        }
        return Err(DurationError::Negative(input.to_owned()));
    }
    if total > MAX_NANOS - 1 {
        return Err(invalid());
    }
    Ok(Duration::from_nanos(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: u64 = 1;
    const US: u64 = 1_000;
    const MS: u64 = 1_000_000;
    const S: u64 = 1_000_000_000;
    const M: u64 = 60 * S;
    const H: u64 = 60 * M;

    #[test]
    fn valid_table() {
        // Cases from Go's src/time/time_test.go parseDurationTests (non-negative ones).
        let cases: &[(&str, u64)] = &[
            ("0", 0),
            ("5s", 5 * S),
            ("30s", 30 * S),
            ("1478s", 1478 * S),
            ("+5s", 5 * S),
            ("-0", 0),
            ("+0", 0),
            ("5.0s", 5 * S),
            ("5.6s", 5 * S + 600 * MS),
            ("5.s", 5 * S),
            (".5s", 500 * MS),
            ("1.0s", S),
            ("1.00s", S),
            ("1.004s", S + 4 * MS),
            ("1.0040s", S + 4 * MS),
            ("100.00100s", 100 * S + MS),
            ("10ns", 10 * NS),
            ("11us", 11 * US),
            ("12µs", 12 * US),
            ("12μs", 12 * US),
            ("13ms", 13 * MS),
            ("14s", 14 * S),
            ("15m", 15 * M),
            ("16h", 16 * H),
            ("3h30m", 3 * H + 30 * M),
            ("10.5s4m", 4 * M + 10 * S + 500 * MS),
            (
                "1h2m3s4ms5us6ns",
                H + 2 * M + 3 * S + 4 * MS + 5 * US + 6 * NS,
            ),
            ("39h9m14.425s", 39 * H + 9 * M + 14 * S + 425 * MS),
            ("52763797000ns", 52_763_797_000 * NS),
            ("0.3333333333333333333h", 20 * M),
            ("9007199254740993ns", (1 << 53) + 1),
            ("9223372036854775807ns", (1 << 63) - 1),
            ("9223372036854775.807us", (1 << 63) - 1),
            ("9223372036s854ms775us807ns", (1 << 63) - 1),
            ("0.100000000000000000000h", 6 * M),
            (
                "0.830103483285477580700h",
                49 * M + 48 * S + 372_539_827 * NS,
            ),
            // Plan examples.
            ("30m", 30 * M),
            ("1h30m", H + 30 * M),
            ("1h30m0s", H + 30 * M),
            ("1.5h", H + 30 * M),
            ("90s", 90 * S),
            ("300ms", 300 * MS),
        ];
        for &(input, nanos) in cases {
            assert_eq!(
                parse_go(input),
                Ok(Duration::from_nanos(nanos)),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn invalid_table() {
        let cases: &[(&str, DurationError)] = &[
            ("", DurationError::Invalid(String::new())),
            ("3", DurationError::MissingUnit("3".into())),
            ("-", DurationError::Invalid("-".into())),
            ("s", DurationError::Invalid("s".into())),
            (".", DurationError::Invalid(".".into())),
            ("-.", DurationError::Invalid("-.".into())),
            (".s", DurationError::Invalid(".s".into())),
            ("+.s", DurationError::Invalid("+.s".into())),
            (
                "1d",
                DurationError::UnknownUnit {
                    unit: "d".into(),
                    input: "1d".into(),
                },
            ),
            (
                "30 m",
                DurationError::UnknownUnit {
                    unit: " m".into(),
                    input: "30 m".into(),
                },
            ),
            (
                "\u{85}\u{85}",
                DurationError::Invalid("\u{85}\u{85}".into()),
            ),
            (
                "9223372036854775808ns",
                DurationError::Invalid("9223372036854775808ns".into()),
            ),
            (
                "9223372036854775.808us",
                DurationError::Invalid("9223372036854775.808us".into()),
            ),
            (
                "9223372036854ms775us808ns",
                DurationError::Invalid("9223372036854ms775us808ns".into()),
            ),
            ("-5s", DurationError::Negative("-5s".into())),
        ];
        for (input, err) in cases {
            assert_eq!(parse_go(input).as_ref(), Err(err), "input {input:?}");
        }
    }

    #[test]
    fn error_messages_match_go() {
        assert_eq!(
            parse_go("1d").unwrap_err().to_string(),
            r#"time: unknown unit "d" in duration "1d""#
        );
        assert_eq!(
            parse_go("x").unwrap_err().to_string(),
            r#"time: invalid duration "x""#
        );
        assert_eq!(
            parse_go("3").unwrap_err().to_string(),
            r#"time: missing unit in duration "3""#
        );
    }
}
